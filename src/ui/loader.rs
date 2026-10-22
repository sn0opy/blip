//! Background loading: worker threads read, render and upload images, always taking
//! the most recently requested one first.
//!
//! Star measurement and plate solving are a separate, lower-priority job: workers only
//! measure stars when no image is waiting, so it never delays an image. The exception is
//! an urgent star job, which someone is waiting for: it goes before any image. It needs the
//! raw data, so a loaded image whose stars are still wanted keeps it in a small store
//! (at most [`KEEP_RAW`] images; the least important goes first, and anything no longer
//! wanted goes on every queue change). A star job whose data is gone reads the file
//! again.
//!
//! Workers start before the window exists and only wait for [`Loader::attach`] to upload.
//!
//! [`Loader::set_queue`] replaces all pending work and bumps a generation counter.
//! Running jobs check between stages whether they're still wanted and stop early if
//! not. A panic inside a job is caught and reported as that file's error (or as "no
//! stars"), so a bad file can't take a worker down.

use super::texture::{self, Filter, Level, Overlay, Texture, Uploader};
use super::{MeasuredStars, panic_message};
use blip_core::solve::{self, Hint};
use blip_core::tilt::Tilt;
use blip_core::{image, process, stars};
use eframe::egui::{self, Pos2, Rect};
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

/// One worker for the image the user is waiting on, one prefetching. More mostly
/// just compete for network bandwidth.
const WORKERS: usize = 2;
/// Loaded images whose raw data is kept for a pending star measurement.
const KEEP_RAW: usize = 3;

/// Everything that affects how a file is rendered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub stretch: process::StretchParams,
    pub binning: Binning,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Binning {
    MaxSide(usize),
    Factor(usize),
    Full,
}

impl Binning {
    fn max_side(self, img: &image::Image) -> Option<usize> {
        match self {
            Binning::MaxSide(side) => Some(side),
            Binning::Factor(factor) => Some(process::side_for_bin(img, factor)),
            Binning::Full => None,
        }
    }
}

pub struct Loaded {
    pub texture: Texture,
    /// Clipped pixels, `None` if there are none.
    pub clip_overlay: Option<ClipOverlay>,
    pub size: [usize; 2],
    pub src_size: [usize; 2],
    pub bin: usize,
    pub file_bytes: usize,
    pub read_ms: f64,
    pub decode_ms: f64,
    pub process_ms: f64,
    /// Building mipmaps and uploading.
    pub texture_ms: f64,
    pub info: String,
    pub clip: process::ClipStats,
    pub stretch: process::StretchInfo,
    pub bayer: Option<image::Bayer>,
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
    OutOfMemory(PathBuf, Settings),
    /// Star sizes, the tilt they show and the plate solve.
    Stars(PathBuf, Box<MeasuredStars>),
}

/// What a worker is busy with.
#[derive(Clone)]
pub enum Work {
    Image(Job),
    Stars(PathBuf),
}

/// What the workers are doing, for the debug panel.
pub struct Status {
    pub workers: [Option<Work>; WORKERS],
    /// Jobs waiting for a worker.
    pub queued_images: usize,
    pub queued_stars: usize,
    pub raw_images: usize,
    pub raw_bytes: usize,
}

#[derive(Clone, PartialEq)]
pub struct Job {
    pub path: PathBuf,
    pub settings: Settings,
}

struct Raw {
    path: PathBuf,
    img: image::Image,
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
    waiting_for: Option<Job>,
    wanted_stars: Vec<PathBuf>,
    stars: VecDeque<PathBuf>,
    urgent_stars: Option<PathBuf>,
    raw: Vec<Raw>,
    /// What each worker is running.
    working: [Option<Work>; WORKERS],
}

impl Queue {
    fn running_images(&self) -> impl Iterator<Item = &Job> {
        self.working.iter().filter_map(|w| match w {
            Some(Work::Image(job)) => Some(job),
            _ => None,
        })
    }

    fn running_stars(&self, path: &Path) -> bool {
        (self.working.iter()).any(|w| matches!(w, Some(Work::Stars(p)) if p == path))
    }

    /// Not while its image loads, which keeps the raw data for it.
    fn urgent_star_job(&mut self) -> Option<PathBuf> {
        let path = self.urgent_stars.as_ref()?;
        if self.running_images().any(|j| j.path == *path) {
            return None;
        }
        self.urgent_stars.take()
    }

    /// The most important star job that doesn't wait for its image to load.
    fn next_star_job(&mut self) -> Option<PathBuf> {
        let loading = |p: &PathBuf| {
            (self.images.iter())
                .chain(self.running_images())
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

struct Gpu {
    ctx: egui::Context,
    uploader: Uploader,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    gpu: OnceLock<Gpu>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        // Jobs run outside the lock, so it can't be poisoned by one of them.
        self.queue.lock().unwrap()
    }

    fn set_queue(
        &self,
        images: Vec<(PathBuf, Settings)>,
        mut stars: Vec<PathBuf>,
        urgent_stars: Option<PathBuf>,
        waiting: bool,
    ) {
        if let Some(p) = &urgent_stars {
            stars.retain(|s| s != p);
            stars.insert(0, p.clone());
        }
        let mut q = self.lock();
        q.generation += 1;
        q.wanted_images = (images.into_iter())
            .map(|(path, settings)| Job { path, settings })
            .collect();
        q.waiting_for = q.wanted_images.first().filter(|_| waiting).cloned();
        q.images = (q.wanted_images.iter())
            .filter(|j| !q.running_images().any(|r| r == *j))
            .cloned()
            .collect();
        q.urgent_stars = urgent_stars.filter(|p| !q.running_stars(p));
        q.stars = (stars.iter())
            .filter(|p| !q.running_stars(p) && q.urgent_stars.as_ref() != Some(*p))
            .cloned()
            .collect();
        q.wanted_stars = stars;
        let Queue {
            raw, wanted_stars, ..
        } = &mut *q;
        raw.retain(|r| wanted_stars.contains(&r.path));
        self.wake.notify_all();
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
    pub fn new() -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            wake: Condvar::new(),
            gpu: OnceLock::new(),
        });
        let (tx, rx) = channel();
        for id in 0..WORKERS {
            let worker = Worker {
                id,
                shared: shared.clone(),
                tx: tx.clone(),
            };
            std::thread::spawn(move || worker.run());
        }
        Self { shared, rx }
    }

    pub fn attach(&self, ctx: egui::Context, uploader: Uploader) {
        let _ = self.shared.gpu.set(Gpu { ctx, uploader });
    }

    /// Replace all pending work: `images` to load and `stars` to measure, each in
    /// priority order, and `urgent_stars` to measure before anything else. Running jobs
    /// that are still wanted continue. `waiting`: the first image is the one on screen;
    /// nothing but the urgent star job starts while it loads.
    pub fn set_queue(
        &self,
        images: Vec<(PathBuf, Settings)>,
        stars: Vec<PathBuf>,
        urgent_stars: Option<PathBuf>,
        waiting: bool,
    ) {
        self.shared.set_queue(images, stars, urgent_stars, waiting);
    }

    /// The texture size limit images are rendered for, once attached.
    pub fn max_texture_side(&self) -> Option<usize> {
        (self.shared.gpu.get()).map(|g| g.uploader.max_texture_side())
    }

    pub fn preloader(&self) -> Preloader {
        Preloader(self.shared.clone())
    }

    pub fn try_recv(&self) -> Option<Message> {
        self.rx.try_recv().ok()
    }

    pub fn status(&self) -> Status {
        let q = self.shared.lock();
        Status {
            workers: q.working.clone(),
            queued_images: q.images.len(),
            queued_stars: q.stars.len() + usize::from(q.urgent_stars.is_some()),
            raw_images: q.raw.len(),
            raw_bytes: q.raw.iter().map(|r| size_of_val(&r.img.data[..])).sum(),
        }
    }
}

#[derive(Clone)]
pub struct Preloader(Arc<Shared>);

impl Preloader {
    /// Does nothing once anything was queued.
    pub fn preload(&self, path: PathBuf, settings: Settings) {
        if self.0.lock().generation == 0 {
            self.0
                .set_queue(vec![(path, settings)], Vec::new(), None, true);
        }
    }
}

struct Worker {
    id: usize,
    shared: Arc<Shared>,
    tx: Sender<Message>,
}

impl Worker {
    fn run(&self) {
        loop {
            let (task, generation) = {
                let mut q = self.shared.lock();
                loop {
                    if let Some(path) = q.urgent_star_job() {
                        q.working[self.id] = Some(Work::Stars(path.clone()));
                        let raw = q.take_raw(&path);
                        break (Task::Stars(path, raw), q.generation);
                    }
                    let busy = (q.waiting_for.as_ref())
                        .is_some_and(|j| q.running_images().any(|r| r == j));
                    if busy {
                        q = self.shared.wake.wait(q).unwrap();
                        continue;
                    }
                    if let Some(job) = q.images.pop_front() {
                        q.working[self.id] = Some(Work::Image(job.clone()));
                        break (Task::Image(job), q.generation);
                    }
                    if let Some(path) = q.next_star_job() {
                        q.working[self.id] = Some(Work::Stars(path.clone()));
                        let raw = q.take_raw(&path);
                        break (Task::Stars(path, raw), q.generation);
                    }
                    q = self.shared.wake.wait(q).unwrap();
                }
            };
            self.repaint();
            let message = match task {
                Task::Image(job) => self.image(job, generation),
                Task::Stars(path, raw) => self.stars(path, raw, generation),
            };
            if let Some(message) = message
                && self.tx.send(message).is_err()
            {
                return; // The app is gone.
            }
            self.repaint();
        }
    }

    fn repaint(&self) {
        if let Some(gpu) = self.shared.gpu.get() {
            gpu.ctx.request_repaint();
        }
    }

    /// Load an image; `None` if it stopped being wanted.
    fn image(&self, job: Job, generation: u64) -> Option<Message> {
        let wanted = || {
            let q = self.shared.lock();
            q.generation == generation || q.wanted_images.contains(&job)
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            load(&self.shared.gpu, &job.path, job.settings, &wanted)
        }))
        .unwrap_or_else(|panic| {
            let e = format!("internal error: {}", panic_message(&*panic));
            Some(Err(LoadError::File(e)))
        });

        let mut q = self.shared.lock();
        q.working[self.id] = None;
        let message = result.map(|result| match result {
            Ok((loaded, raw)) => {
                q.keep_raw(raw);
                Message::Image(job.path, job.settings, Ok(Box::new(loaded)))
            }
            Err(LoadError::File(e)) => {
                // Nothing to measure in a file that can't be read.
                q.wanted_stars.retain(|p| *p != job.path);
                q.stars.retain(|p| *p != job.path);
                Message::Image(job.path, job.settings, Err(e))
            }
            Err(LoadError::OutOfMemory) => Message::OutOfMemory(job.path, job.settings),
        });
        drop(q);
        // Its star job may have been waiting for it.
        self.shared.wake.notify_all();
        message
    }

    /// Measure star sizes and plate solve; `None` if they stopped being wanted.
    fn stars(&self, path: PathBuf, raw: Option<Raw>, generation: u64) -> Option<Message> {
        let wanted = || {
            let q = self.shared.lock();
            q.generation == generation || q.wanted_stars.contains(&path)
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            let (img, clip) = match raw {
                Some(raw) => (raw.img, raw.clip),
                None => {
                    let Ok(img) = image::read_file(&path) else {
                        return Some(MeasuredStars::failed("cannot read file"));
                    };
                    let clip = process::clip_stats(&img);
                    (img, clip)
                }
            };
            if !wanted() {
                return None;
            }
            let t = Instant::now();
            let stats = stars::detect(&img, &clip);
            let tilt = Tilt::measure(&stats.stars, img.width, img.height);
            let detect_ms = t.elapsed().as_secs_f64() * 1000.0;
            let solution = solve::solve(
                &stats.points_brightest_first,
                img.width,
                img.height,
                &Hint::from_header(&img),
            );
            let focal_length_mm =
                (solution.as_ref().ok()).and_then(|s| solve::focal_length_mm(&img, &s.wcs));
            Some(MeasuredStars {
                stats,
                tilt,
                detect_ms,
                solution,
                focal_length_mm,
            })
        }))
        .unwrap_or_else(|panic| {
            Some(MeasuredStars::failed(&format!(
                "internal error: {}",
                panic_message(&*panic)
            )))
        });

        self.shared.lock().working[self.id] = None;
        Some(Message::Stars(path, Box::new(result?)))
    }
}

enum LoadError {
    File(String),
    OutOfMemory,
}

/// Read, render and upload. `None` once `wanted` says the result isn't needed.
fn load(
    gpu: &OnceLock<Gpu>,
    path: &Path,
    settings: Settings,
    wanted: &dyn Fn() -> bool,
) -> Option<Result<(Loaded, Raw), LoadError>> {
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let (img, read) = match image::read_file_timed(path) {
        Ok(r) => r,
        Err(e) => return Some(Err(LoadError::File(e))),
    };
    let read_ms = read.io.as_secs_f64() * 1000.0;
    let decode_ms = ms(t) - read_ms;
    if !wanted() {
        return None;
    }

    // Before the GPU is ready, assume no texture size limit and redo the render if needed.
    let clip = process::clip_stats(&img);
    let limits = |u: &Uploader| (u.max_texture_side(), u.mipmaps());
    let (max_texture, mipmaps) = gpu
        .get()
        .map_or((usize::MAX, true), |g| limits(&g.uploader));
    let mut prepared = prepare(&img, &clip, settings, max_texture, mipmaps);
    let uploader = &gpu.wait().uploader;
    if !wanted() {
        return None;
    }
    let (max_texture, mipmaps) = limits(uploader);
    if prepared.side != process::preview_side(&img, settings.binning.max_side(&img), max_texture) {
        prepared = prepare(&img, &clip, settings, max_texture, mipmaps);
        if !wanted() {
            return None;
        }
    }

    let t = Instant::now();
    let Prepared {
        r,
        levels,
        overlay,
        process_ms,
        mips_ms,
        ..
    } = prepared;
    let name = path.to_string_lossy();
    let clip_overlay = overlay.map(|(o, uv)| {
        let texture = uploader.upload(&format!("{name} clipped"), o.levels, Filter::Nearest);
        texture.map(|texture| ClipOverlay { texture, uv })
    });
    let Ok(clip_overlay) = clip_overlay.transpose() else {
        return Some(Err(LoadError::OutOfMemory));
    };
    let Ok(texture) = uploader.upload(&name, levels, Filter::Smooth) else {
        return Some(Err(LoadError::OutOfMemory));
    };
    let texture_ms = mips_ms + ms(t);

    let loaded = Loaded {
        texture,
        clip_overlay,
        size: [r.width, r.height],
        src_size: [img.width, img.height],
        bin: r.bin,
        file_bytes: read.bytes,
        read_ms,
        decode_ms,
        process_ms,
        texture_ms,
        info: img.describe(),
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

struct Prepared {
    side: Option<usize>,
    r: process::Rendered<process::Rgba8>,
    levels: Vec<Level>,
    overlay: Option<(Overlay, Rect)>,
    process_ms: f64,
    mips_ms: f64,
}

fn prepare(
    img: &image::Image,
    clip: &process::ClipStats,
    settings: Settings,
    max_texture: usize,
    mipmaps: bool,
) -> Prepared {
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    // "Full resolution" falls back to the least binning that fits the GPU's texture size
    // limit (typically 8192 or 16384 px).
    let side = process::preview_side(img, settings.binning.max_side(img), max_texture);
    let (mut r, clip_points) =
        process::render_preview::<process::Rgba8>(img, clip, side, settings.stretch);
    let process_ms = ms(t);

    let t = Instant::now();
    let overlay = texture::clip_overlay(&clip_points, r.width, r.height, mipmaps).map(|o| {
        let covered = [r.width, r.height].map(|s| s.div_ceil(o.cell) * o.cell);
        let uv = Rect::from_min_max(
            Pos2::ZERO,
            Pos2::new(
                r.width as f32 / covered[0] as f32,
                r.height as f32 / covered[1] as f32,
            ),
        );
        (o, uv)
    });
    drop(clip_points);
    let base = Level {
        width: r.width,
        height: r.height,
        pixels: std::mem::take(&mut r.pixels),
    };
    let levels = if mipmaps {
        texture::smooth_mips(base)
    } else {
        vec![base]
    };
    Prepared {
        side,
        r,
        levels,
        overlay,
        process_ms,
        mips_ms: ms(t),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Shared {
        Shared {
            queue: Mutex::new(Queue::default()),
            wake: Condvar::new(),
            gpu: OnceLock::new(),
        }
    }

    fn settings() -> Settings {
        Settings {
            stretch: process::StretchParams::default(),
            binning: Binning::Full,
        }
    }

    #[test]
    fn urgent_stars_go_first_but_wait_for_their_running_image() {
        let shared = shared();
        let [a, b, c] = ["a", "b", "c"].map(PathBuf::from);
        let images = vec![(a.clone(), settings()), (c.clone(), settings())];
        let stars = vec![a.clone(), b.clone(), c.clone()];
        shared.set_queue(images, stars, Some(c.clone()), true);

        let mut q = shared.lock();
        assert_eq!(q.wanted_stars, [c.clone(), a.clone(), b.clone()]);
        assert_eq!(q.stars, [a, b]);
        q.working[0] = Some(Work::Image(Job {
            path: c.clone(),
            settings: settings(),
        }));
        assert_eq!(q.urgent_star_job(), None);
        q.working[0] = None;
        assert_eq!(q.urgent_star_job(), Some(c));
        assert_eq!(q.urgent_star_job(), None);
    }

    #[test]
    fn a_running_star_job_is_not_queued_again_as_urgent() {
        let shared = shared();
        let a = PathBuf::from("a");
        shared.lock().working[1] = Some(Work::Stars(a.clone()));
        shared.set_queue(Vec::new(), Vec::new(), Some(a.clone()), false);
        let q = shared.lock();
        assert_eq!(q.urgent_stars, None);
        assert!(q.stars.is_empty());
        assert_eq!(q.wanted_stars, [a]);
    }
}
