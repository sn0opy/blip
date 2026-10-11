//! Background loading: worker threads read, render and upload images, always taking
//! the most recently requested one first.
//!
//! Star measurement is a separate, lower-priority job: workers only measure stars
//! when no image is waiting, so it never delays an image. It needs the raw data, so a
//! loaded image whose stars are still wanted keeps it in a small store (at most
//! [`KEEP_RAW`] images; the least important goes first, and anything no longer wanted
//! goes on every queue change). A star job whose data is gone reads the file again.
//!
//! [`Loader::set_queue`] replaces all pending work and bumps a generation counter.
//! Running jobs check between stages whether they're still wanted and stop early if
//! not. A panic inside a job is caught and reported as that file's error (or as "no
//! stars"), so a bad file can't take a worker down.

use super::panic_message;
use super::texture::{self, Filter, Texture, Uploader};
use blip::stars::StarStats;
use blip::{fits, process, stars};
use eframe::egui::{self, Pos2, Rect};
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;

/// One worker for the image the user is waiting on, one prefetching. More mostly
/// just compete for network bandwidth.
const WORKERS: usize = 2;
const MAX_CLIP_POINTS: usize = 4_000_000;
/// Loaded images whose raw data is kept for a pending star measurement.
const KEEP_RAW: usize = 3;

/// Everything that affects how a file is rendered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub stretch: process::StretchParams,
    /// `None` = full resolution, otherwise bin to this long side.
    pub max_side: Option<usize>,
}

pub struct Loaded {
    pub texture: Texture,
    /// Clipped pixels, `None` if there are none.
    pub clip_overlay: Option<ClipOverlay>,
    pub size: [usize; 2],
    pub src_size: [usize; 2],
    pub bin: usize,
    pub read_ms: f64,
    pub process_ms: f64,
    /// Building mipmaps and uploading.
    pub texture_ms: f64,
    pub info: String,
    pub clip: process::ClipStats,
    pub stretch: process::StretchInfo,
    pub bayer: Option<fits::Bayer>,
}

impl Loaded {
    pub fn gpu_bytes(&self) -> usize {
        self.texture.bytes() + self.clip_overlay.as_ref().map_or(0, |o| o.texture.bytes())
    }
}

pub struct ClipOverlay {
    pub texture: Texture,
    /// The part of the texture covering the image (its last cells may stick out).
    pub uv: Rect,
}

pub enum Message {
    Image(PathBuf, Settings, Result<Box<Loaded>, String>),
    /// Star sizes and how long measuring took (ms).
    Stars(PathBuf, StarStats, f64),
}

#[derive(Clone, PartialEq)]
struct Job {
    path: PathBuf,
    settings: Settings,
}

struct Raw {
    path: PathBuf,
    img: fits::FitsImage,
    clip: process::ClipStats,
}

#[derive(Default)]
struct Queue {
    /// Bumped by every `set_queue`.
    generation: u64,
    /// Everything requested by the last `set_queue`, most important first, and the
    /// part of it not running yet.
    wanted_images: Vec<Job>,
    images: VecDeque<Job>,
    running_images: Vec<Job>,
    wanted_stars: Vec<PathBuf>,
    stars: VecDeque<PathBuf>,
    running_stars: Vec<PathBuf>,
    raw: Vec<Raw>,
}

impl Queue {
    /// The most important star job that doesn't wait for its image to load.
    fn next_star_job(&mut self) -> Option<PathBuf> {
        let loading = |p: &PathBuf| {
            (self.images.iter())
                .chain(&self.running_images)
                .any(|j| j.path == *p)
        };
        let i = self.stars.iter().position(|p| !loading(p))?;
        self.stars.remove(i)
    }

    fn keep_raw(&mut self, raw: Raw) {
        if !self.wanted_stars.contains(&raw.path) {
            return;
        }
        self.raw.push(raw);
        while self.raw.len() > KEEP_RAW {
            let rank = |r: &Raw| self.wanted_stars.iter().position(|p| *p == r.path);
            let (i, _) = (self.raw.iter().enumerate())
                .max_by_key(|(_, r)| rank(r).unwrap_or(usize::MAX))
                .expect("raw store is not empty");
            self.raw.swap_remove(i);
        }
    }

    fn take_raw(&mut self, path: &Path) -> Option<Raw> {
        let i = self.raw.iter().position(|r| r.path == path)?;
        Some(self.raw.swap_remove(i))
    }
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        // Jobs run outside the lock, so it can't be poisoned by one of them.
        self.queue.lock().unwrap()
    }
}

enum Task {
    Image(Job),
    Stars(PathBuf, Option<Raw>),
}

pub struct Loader {
    shared: Arc<Shared>,
    rx: Receiver<Message>,
}

impl Loader {
    pub fn new(ctx: egui::Context, uploader: Uploader) -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            wake: Condvar::new(),
        });
        let (tx, rx) = channel();
        for _ in 0..WORKERS {
            let worker = Worker {
                shared: shared.clone(),
                tx: tx.clone(),
                ctx: ctx.clone(),
                uploader: uploader.clone(),
            };
            std::thread::spawn(move || worker.run());
        }
        Self { shared, rx }
    }

    /// Replace all pending work: `images` to load and `stars` to measure, each in
    /// priority order. Running jobs that are still wanted continue.
    pub fn set_queue(&self, images: Vec<PathBuf>, stars: Vec<PathBuf>, settings: Settings) {
        let mut q = self.shared.lock();
        q.generation += 1;
        q.wanted_images = (images.into_iter())
            .map(|path| Job { path, settings })
            .collect();
        q.images = (q.wanted_images.iter())
            .filter(|j| !q.running_images.contains(j))
            .cloned()
            .collect();
        q.stars = (stars.iter())
            .filter(|p| !q.running_stars.contains(p))
            .cloned()
            .collect();
        q.wanted_stars = stars;
        let Queue {
            raw, wanted_stars, ..
        } = &mut *q;
        raw.retain(|r| wanted_stars.contains(&r.path));
        self.shared.wake.notify_all();
    }

    pub fn try_recv(&self) -> Option<Message> {
        self.rx.try_recv().ok()
    }
}

struct Worker {
    shared: Arc<Shared>,
    tx: Sender<Message>,
    ctx: egui::Context,
    uploader: Uploader,
}

impl Worker {
    fn run(&self) {
        loop {
            let (task, generation) = {
                let mut q = self.shared.lock();
                loop {
                    if let Some(job) = q.images.pop_front() {
                        q.running_images.push(job.clone());
                        break (Task::Image(job), q.generation);
                    }
                    if let Some(path) = q.next_star_job() {
                        q.running_stars.push(path.clone());
                        let raw = q.take_raw(&path);
                        break (Task::Stars(path, raw), q.generation);
                    }
                    q = self.shared.wake.wait(q).unwrap();
                }
            };
            let message = match task {
                Task::Image(job) => self.image(job, generation),
                Task::Stars(path, raw) => self.stars(path, raw, generation),
            };
            if let Some(message) = message {
                if self.tx.send(message).is_err() {
                    return; // The app is gone.
                }
                self.ctx.request_repaint();
            }
        }
    }

    /// Load an image; `None` if it stopped being wanted.
    fn image(&self, job: Job, generation: u64) -> Option<Message> {
        let wanted = || {
            let q = self.shared.lock();
            q.generation == generation || q.wanted_images.contains(&job)
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            load(&self.uploader, &job.path, job.settings, &wanted)
        }))
        .unwrap_or_else(|panic| Some(Err(format!("internal error: {}", panic_message(&*panic)))));

        let mut q = self.shared.lock();
        if let Some(i) = q.running_images.iter().position(|j| *j == job) {
            q.running_images.swap_remove(i);
        }
        let result = result.map(|result| match result {
            Ok((loaded, raw)) => {
                q.keep_raw(raw);
                Ok(Box::new(loaded))
            }
            Err(e) => {
                // Nothing to measure in a file that can't be read.
                q.wanted_stars.retain(|p| *p != job.path);
                q.stars.retain(|p| *p != job.path);
                Err(e)
            }
        });
        drop(q);
        // Its star job may have been waiting for it.
        self.shared.wake.notify_all();
        Some(Message::Image(job.path, job.settings, result?))
    }

    /// Measure star sizes; `None` if they stopped being wanted.
    fn stars(&self, path: PathBuf, raw: Option<Raw>, generation: u64) -> Option<Message> {
        let wanted = || {
            let q = self.shared.lock();
            q.generation == generation || q.wanted_stars.contains(&path)
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            let (img, clip) = match raw {
                Some(raw) => (raw.img, raw.clip),
                None => {
                    let Ok(img) = fits::read_file(&path) else {
                        return Some((StarStats::default(), 0.0));
                    };
                    let clip = process::clip_stats(&img);
                    (img, clip)
                }
            };
            if !wanted() {
                return None;
            }
            let t = Instant::now();
            let stars = stars::detect(&img, &clip);
            Some((stars, t.elapsed().as_secs_f64() * 1000.0))
        }))
        .unwrap_or_else(|_| Some((StarStats::default(), 0.0)));

        let mut q = self.shared.lock();
        if let Some(i) = q.running_stars.iter().position(|p| *p == path) {
            q.running_stars.swap_remove(i);
        }
        drop(q);
        let (stats, ms) = result?;
        Some(Message::Stars(path, stats, ms))
    }
}

/// Read, render and upload. `None` once `wanted` says the result isn't needed.
fn load(
    uploader: &Uploader,
    path: &Path,
    settings: Settings,
    wanted: &dyn Fn() -> bool,
) -> Option<Result<(Loaded, Raw), String>> {
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let img = match fits::read_file(path) {
        Ok(img) => img,
        Err(e) => return Some(Err(e)),
    };
    let read_ms = ms(t);
    if !wanted() {
        return None;
    }

    let t = Instant::now();
    // "Full resolution" falls back to the least binning that fits the GPU's
    // texture size limit (typically 8192 or 16384 px).
    let max_texture = uploader.max_texture_side();
    let r = match settings.max_side {
        None if img.width.max(img.height) <= max_texture => {
            process::render_full::<process::Rgba8>(&img, settings.stretch)
        }
        None => process::render(&img, max_texture, settings.stretch),
        Some(side) => process::render(&img, side.min(max_texture), settings.stretch),
    };
    let clip = process::clip_stats(&img);
    let clip_points = process::clip_points(&img, &clip, r.bin, r.width, r.height, MAX_CLIP_POINTS);
    let process_ms = ms(t);
    if !wanted() {
        return None;
    }

    let t = Instant::now();
    let name = path.to_string_lossy();
    let mipmaps = uploader.mipmaps();
    let overlay = texture::clip_overlay(&clip_points, r.width, r.height, mipmaps);
    drop(clip_points);
    let clip_overlay = overlay.map(|o| {
        let covered = [r.width, r.height].map(|s| s.div_ceil(o.cell) * o.cell);
        let uv = Rect::from_min_max(
            Pos2::ZERO,
            Pos2::new(
                r.width as f32 / covered[0] as f32,
                r.height as f32 / covered[1] as f32,
            ),
        );
        let texture = uploader.upload(&format!("{name} clipped"), o.levels, Filter::Nearest);
        ClipOverlay { texture, uv }
    });
    let base = texture::Level {
        width: r.width,
        height: r.height,
        pixels: r.pixels,
    };
    let levels = if mipmaps {
        texture::smooth_mips(base)
    } else {
        vec![base]
    };
    let texture = uploader.upload(&name, levels, Filter::Smooth);
    let texture_ms = ms(t);

    let loaded = Loaded {
        texture,
        clip_overlay,
        size: [r.width, r.height],
        src_size: [img.width, img.height],
        bin: r.bin,
        read_ms,
        process_ms,
        texture_ms,
        info: describe(&img),
        clip,
        stretch: r.stretch,
        bayer: img.bayer,
    };
    let raw = Raw {
        path: path.to_path_buf(),
        img,
        clip,
    };
    Some(Ok((loaded, raw)))
}

/// One-line summary of the capture from the header.
fn describe(img: &fits::FitsImage) -> String {
    let mut parts = Vec::new();
    if let Some(v) = img.get("OBJECT") {
        parts.push(v.to_string());
    }
    if let Some(v) = img.get_f64("EXPTIME").or(img.get_f64("EXPOSURE")) {
        parts.push(format!("{v}s"));
    }
    if let Some(v) = img.get("FILTER") {
        parts.push(v.to_string());
    }
    if let Some(v) = img.get("GAIN") {
        parts.push(format!("gain {v}"));
    }
    if let Some(v) = img.get_f64("CCD-TEMP") {
        parts.push(format!("{v:.1}°C"));
    }
    if let Some(v) = img.get("DATE-OBS") {
        let without_fraction = v.split('.').next().unwrap_or(v);
        parts.push(without_fraction.replace('T', " "));
    }
    parts.join(" · ")
}
