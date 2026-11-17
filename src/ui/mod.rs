//! The egui application: state, file handling and background jobs. Input handling,
//! views and dialogs live in the submodules.

mod actions;
mod dialogs;
mod draw;
mod filter;
mod gpu;
mod header;
mod headers;
mod list;
mod loader;
mod menu;
mod prefs;
mod rejects;
mod scan;
mod texture;
mod toggles;
mod toolbar;
mod trash;
mod tree;
mod view;

pub use gpu::wgpu_options;
pub use loader::{Loader, Preloader};
pub use scan::Scanner;

use blip_core::process::StretchParams;
use blip_core::solve::Solution;
use blip_core::stars::StarStats;
use blip_core::tilt::Tilt;
use dialogs::Deletion;
use eframe::egui::{self, Vec2};
use headers::{HeaderScanner, Progress};
use list::FileList;
use loader::{Binning, Loaded, Message, Settings};
use prefs::{BinningPref, Preferences};
use scan::{Listing, is_image, list_images};
use std::any::Any;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

pub const APP_ID: &str = "sh.sascha.blip";
/// Previews are binned so their long side is at most this many pixels.
const MAX_SIDE: usize = 3200;
/// Number of rendered images kept on the GPU (binned / full resolution without a memory
/// budget).
const CACHE_SIZE: usize = 10;
const CACHE_SIZE_FULL: usize = 4;
const MIN_CACHE: usize = 2;
const STATUS_FADE: f32 = 0.8;

/// A message in the bottom-left corner. Transient ones fade out after `ttl` seconds.
struct Status {
    kind: StatusKind,
    text: String,
    since: Instant,
    ttl: Option<f32>,
}

#[derive(Clone, Copy, PartialEq)]
enum StatusKind {
    Message,
    FolderError,
    ReadingHeaders,
}

pub struct MeasuredStars {
    stats: StarStats,
    tilt: Option<Tilt>,
    detect_ms: f64,
    solution: Result<Solution, String>,
    focal_length_mm: Option<f64>,
}

impl MeasuredStars {
    fn failed(reason: &str) -> Self {
        Self {
            stats: StarStats::default(),
            tilt: None,
            detect_ms: 0.0,
            solution: Err(reason.to_string()),
            focal_length_mm: None,
        }
    }
}

/// On for every image if `default`, except for the one it was last toggled on.
struct ImageToggle {
    default: bool,
    flipped: Option<PathBuf>,
}

impl ImageToggle {
    fn new(default: bool) -> Self {
        Self {
            default,
            flipped: None,
        }
    }

    fn is_on(&self, path: &Path) -> bool {
        self.default != (self.flipped.as_deref() == Some(path))
    }

    fn toggle(&mut self, path: &Path) -> bool {
        self.flip(path);
        self.is_on(path)
    }

    /// Like `toggle`, but returns the image this reverts to the default, if any.
    fn flip(&mut self, path: &Path) -> Option<PathBuf> {
        match self.flipped.replace(path.to_path_buf()) {
            Some(p) if p == path => {
                self.flipped = None;
                None
            }
            other => other,
        }
    }
}

struct CacheEntry {
    loaded: Loaded,
    last_used: u64,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum EvictionOrder {
    OutsideWindow { least_recently_used: u64 },
    InWindow { lowest_priority: Reverse<usize> },
}

pub struct App {
    loader: Loader,
    scanner: Scanner,
    /// Last folder of a previous session; only the Open dialog's start location.
    last_dir: Option<PathBuf>,
    list: FileList,
    /// Direction of the last navigation step, used to prefetch ahead.
    direction: isize,
    cache: HashMap<PathBuf, CacheEntry>,
    errors: HashMap<PathBuf, String>,
    /// The image on screen; lags behind `selected` while the next one loads.
    shown: Option<PathBuf>,
    /// The shown image while a settings change re-renders it (avoids a black flash).
    stale: Option<(PathBuf, CacheEntry)>,
    tick: u64,
    gpu_memory: Option<gpu::MemoryQuery>,
    memory_budget: Option<gpu::MemoryBudget>,
    /// Largest image since the last settings change.
    image_bytes: usize,
    /// Set once an upload ran out of GPU memory.
    memory_cap: Option<usize>,

    params: StretchParams,
    /// Steps by which the stretch's target background is raised (`StretchParams::boosted`).
    boost: u32,
    full_res: bool,
    /// Crops of the corners, edges and center instead of the whole image.
    mosaic: bool,
    show_info: bool,
    show_debug: bool,
    /// Graphics backend and adapter, for the debug panel.
    gpu_info: String,
    zoom: f32,
    pan: Vec2,
    /// Zoom at one sensor pixel per screen pixel.
    actual_size: bool,
    flash_clip: ImageToggle,
    star_view: ImageToggle,
    tilt_view: ImageToggle,
    sky_view: ImageToggle,
    /// Shown without the auto stretch.
    auto_stretch: ImageToggle,
    /// Star measurements, computed by the loader for every image.
    stars: HashMap<PathBuf, MeasuredStars>,
    /// The file whose context menu is open; its stars are measured first.
    menu_file: Option<PathBuf>,

    prefs: Preferences,
    confirm_delete: Option<Deletion>,
    show_help: bool,
    show_settings: bool,
    show_header: bool,
    header_sort: Option<header::Sort>,
    /// The settings dialog's values while a slider is dragged, applied on release.
    prefs_draft: Option<Preferences>,
    status: Option<Status>,
    title: String,

    export_tx: Sender<Result<String, String>>,
    export_rx: Receiver<Result<String, String>>,
}

/// What `App::settings` gives every image right after start.
fn initial_settings(prefs: &Preferences) -> Settings {
    Settings {
        stretch: StretchParams {
            neutralize: prefs.neutralize,
            auto_stretch: prefs.auto_stretch,
            ..StretchParams::default()
        }
        .boosted(prefs.boost),
        binning: prefs.binning.binning(),
    }
}

pub fn preload(preloader: Preloader, scanner: Scanner, path: PathBuf) {
    std::thread::spawn(move || {
        let prefs = Preferences::load_saved(APP_ID);
        let depth = prefs.subfolder_depth;
        if !path.is_dir() {
            if is_image(&path) {
                preloader.preload(path.clone(), initial_settings(&prefs));
            }
            scanner.scan(path.parent().unwrap_or(Path::new("")), depth);
            return;
        }
        let Some(id) = scanner.begin(&path, depth) else {
            return;
        };
        let files = list_images(&path, depth);
        if let Some(first) = files.as_ref().ok().and_then(|f| f.first()) {
            preloader.preload(first.clone(), initial_settings(&prefs));
        }
        scanner.finish(id, files);
    });
}

/// 0, then two images ahead for each one behind: 0, d, 2d, -d, 3d, 4d, -2d, …
fn prefetch_offsets(d: isize, ahead: usize, behind: usize) -> impl Iterator<Item = isize> {
    let mut ahead = (1..=ahead as isize).map(move |i| i * d);
    let mut behind = (1..=behind as isize).map(move |i| -i * d);
    let mut n = 0;
    std::iter::once(0).chain(std::iter::from_fn(move || {
        n += 1;
        if n % 3 == 0 {
            behind.next().or_else(|| ahead.next())
        } else {
            ahead.next().or_else(|| behind.next())
        }
    }))
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext,
        start: Option<PathBuf>,
        loader: Loader,
        scanner: Scanner,
    ) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        scanner.attach(cc.egui_ctx.clone());
        loader.attach(
            cc.egui_ctx.clone(),
            texture::Uploader::new(&cc.egui_ctx, cc.wgpu_render_state.as_ref()),
        );
        let prefs = Preferences::load(cc.storage);
        let (export_tx, export_rx) = channel();
        let gpu_memory = cc.wgpu_render_state.as_ref().map(gpu::MemoryQuery::new);
        let activity = loader.activity();
        let header_scanner =
            HeaderScanner::new(Some(cc.egui_ctx.clone()), move || activity.wait_for_image());
        let mut app = Self {
            loader,
            scanner,
            last_dir: cc
                .storage
                .and_then(|s| s.get_string("last_dir"))
                .map(PathBuf::from),
            list: FileList::new(header_scanner, &prefs),
            direction: 1,
            cache: HashMap::new(),
            errors: HashMap::new(),
            shown: None,
            stale: None,
            tick: 0,
            memory_budget: gpu_memory.as_ref().and_then(gpu::MemoryQuery::budget),
            gpu_memory,
            image_bytes: 0,
            memory_cap: None,
            params: StretchParams {
                neutralize: prefs.neutralize,
                ..StretchParams::default()
            },
            boost: prefs.boost,
            full_res: false,
            mosaic: false,
            show_info: prefs.show_info,
            show_debug: prefs.show_debug,
            gpu_info: cc
                .wgpu_render_state
                .as_ref()
                .map(|rs| {
                    let info = rs.adapter.get_info();
                    format!("{:?} · {}", info.backend, info.name)
                })
                .unwrap_or_default(),
            zoom: 1.0,
            pan: Vec2::ZERO,
            actual_size: prefs.actual_size,
            flash_clip: ImageToggle::new(prefs.flash_clip),
            star_view: ImageToggle::new(prefs.star_view),
            tilt_view: ImageToggle::new(prefs.tilt_view),
            sky_view: ImageToggle::new(prefs.sky_view),
            auto_stretch: ImageToggle::new(prefs.auto_stretch),
            stars: HashMap::new(),
            menu_file: None,
            prefs,
            confirm_delete: None,
            show_help: false,
            show_settings: false,
            show_header: false,
            header_sort: None,
            prefs_draft: None,
            status: None,
            title: String::new(),
            export_tx,
            export_rx,
        };
        if let Some(path) = start {
            app.open(&path);
        }
        app
    }

    fn current(&self) -> Option<&PathBuf> {
        self.list.current()
    }

    /// Open a folder, or a file (shown right away, then selected in its folder's listing).
    fn open(&mut self, path: &Path) {
        let (dir, file) = if path.is_dir() {
            (path.to_path_buf(), None)
        } else {
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            (dir, Some(path.to_path_buf()))
        };
        self.list.open(dir, file.filter(|f| is_image(f)));
        self.clear_status(StatusKind::ReadingHeaders);
        self.direction = 1;
        self.cache.clear();
        self.errors.clear();
        self.stars.clear();
        self.shown = None;
        self.stale = None;
        self.image_bytes = 0;
        self.memory_cap = None;
        self.files_changed();
        self.refresh();
    }

    fn refresh(&mut self) {
        if let Some(dir) = self.list.dir() {
            self.scanner.scan(dir, self.prefs.subfolder_depth);
        }
    }

    fn apply_listing(&mut self, listing: Listing) {
        if self.list.dir() != Some(listing.dir.as_path())
            || listing.depth != self.prefs.subfolder_depth
        {
            return;
        }
        let files = match listing.files {
            Ok(files) => {
                self.clear_status(StatusKind::FolderError);
                files
            }
            Err(e) => {
                let text = format!("Cannot read folder: {e}");
                self.show_status(StatusKind::FolderError, text, None);
                Vec::new()
            }
        };
        self.list.set_listing(files);
        self.files_changed();
    }

    fn files_changed(&mut self) {
        if self.list.files().is_empty() {
            self.shown = None;
        }
        self.request_loads();
    }

    fn poll_headers(&mut self) {
        if !self.list.scanning() {
            self.clear_status(StatusKind::ReadingHeaders);
        }
        let Some((progress, changed)) = self.list.poll_headers() else {
            return;
        };
        match progress {
            Progress::Reading { done, total } => {
                let text = format!("Reading headers {done}/{total}");
                self.show_status(StatusKind::ReadingHeaders, text, None);
            }
            Progress::Finished {
                done,
                failed,
                elapsed,
            } => self.set_status(headers_read(done, failed, elapsed), Some(4.0)),
        }
        if changed {
            self.files_changed();
        }
    }

    fn select(&mut self, index: usize) {
        if self.list.files().is_empty() {
            return;
        }
        let old = self.list.selected();
        let index = self.list.select(index);
        if index != old {
            self.direction = if index > old { 1 } else { -1 };
        }
        self.request_loads();
    }

    fn step(&mut self, delta: isize) {
        let Some(last) = self.list.files().len().checked_sub(1) else {
            return;
        };
        let i = (self.list.selected() as isize + delta).clamp(0, last as isize);
        self.select(i as usize);
    }

    /// The current image, then its neighbors (mostly ahead), with their offsets from the
    /// current one.
    fn prefetch_window(&self) -> Vec<(isize, &PathBuf)> {
        let (files, s) = (self.list.files(), self.list.selected() as isize);
        prefetch_offsets(self.direction, self.prefs.ahead, self.prefs.behind)
            .filter_map(|o| Some((o, files.get(usize::try_from(s + o).ok()?)?)))
            .take(self.cache_limit())
            .collect()
    }

    /// Queue the current image first, then prefetch neighbors, and measure stars of
    /// the same files where still missing. The file with the open menu is measured
    /// before anything else.
    fn request_loads(&mut self) {
        self.memory_budget = self.gpu_memory.as_ref().and_then(gpu::MemoryQuery::budget);
        let window: Vec<&PathBuf> = (self.prefetch_window().into_iter())
            .map(|(_, p)| p)
            .filter(|p| !self.errors.contains_key(*p))
            .collect();
        let missing = |have: &dyn Fn(&PathBuf) -> bool| -> Vec<PathBuf> {
            window
                .iter()
                .filter(|p| !have(p))
                .map(|p| (*p).clone())
                .collect()
        };
        let images = missing(&|p| self.cache.contains_key(p));
        let stars = missing(&|p| self.stars.contains_key(p));
        let urgent_stars = (self.menu_file.clone()).filter(|p| !self.stars.contains_key(p));
        let waiting = self.current().is_some_and(|c| images.first() == Some(c));
        let images = (images.into_iter())
            .map(|p| {
                let settings = self.settings(&p);
                (p, settings)
            })
            .collect();
        self.loader.set_queue(images, stars, urgent_stars, waiting);
    }

    fn settings(&self, path: &Path) -> Settings {
        Settings {
            stretch: StretchParams {
                auto_stretch: self.auto_stretch.is_on(path),
                ..self.params.boosted(self.boost)
            },
            binning: if self.loads_full_res() {
                Binning::Full
            } else {
                self.prefs.binning.binning()
            },
        }
    }

    fn toggle_stretch(&mut self) {
        let Some(cur) = self.current().cloned() else {
            return;
        };
        if let Some(reverted) = self.auto_stretch.flip(&cur) {
            self.cache.remove(&reverted);
        }
        let on = self.auto_stretch.is_on(&cur);
        self.stale = self.cache.remove_entry(&cur).or(self.stale.take());
        let state = if on { "on" } else { "off" };
        let key = actions::key_hint(actions::Action::AutoStretch);
        self.set_status(format!("Auto stretch {state} ({key} to toggle)"), Some(2.0));
        self.request_loads();
    }

    fn reload_all(&mut self) {
        self.stale = self
            .shown
            .as_ref()
            .and_then(|p| self.cache.remove_entry(p))
            .or(self.stale.take());
        self.cache.clear();
        self.errors.clear();
        self.image_bytes = 0;
        self.memory_cap = None;
        self.request_loads();
    }

    /// Collect finished background work: folder listings, previews, star measurements,
    /// exports.
    fn poll_background(&mut self) {
        if let Some(listing) = self.scanner.take() {
            self.apply_listing(listing);
        }
        self.poll_headers();
        let limit = self.cache_limit();
        let mut loaded_any = false;
        let mut files_changed = false;
        while let Some(msg) = self.loader.try_recv() {
            match msg {
                // Ignore stale settings and files that are no longer listed.
                Message::Image(path, settings, result)
                    if settings == self.settings(&path) && self.list.is_listed(&path) =>
                {
                    loaded_any = true;
                    match result {
                        Ok(loaded) => {
                            self.tick += 1;
                            let last_used = self.tick;
                            let mut loaded = *loaded;
                            let header = std::mem::take(&mut loaded.header);
                            files_changed |= self.list.add_loaded_header(path.clone(), header);
                            self.image_bytes = self.image_bytes.max(loaded.gpu_bytes());
                            self.cache.insert(path, CacheEntry { loaded, last_used });
                        }
                        Err(e) => {
                            self.errors.insert(path, e);
                        }
                    }
                }
                Message::OutOfMemory(path, settings)
                    if settings == self.settings(&path) && self.list.is_listed(&path) =>
                {
                    self.out_of_memory(path);
                }
                Message::Stars(path, measured) if self.list.is_listed(&path) => {
                    self.stars.insert(path, *measured);
                }
                _ => {}
            }
        }
        if files_changed {
            self.files_changed();
        }
        if loaded_any {
            self.memory_budget = self.gpu_memory.as_ref().and_then(gpu::MemoryQuery::budget);
            self.evict();
            if self.cache_limit() != limit {
                self.request_loads();
            }
        }

        while let Ok(msg) = self.export_rx.try_recv() {
            match msg {
                Ok(text) => self.set_status(text, Some(2.5)),
                Err(text) => self.set_status(text, Some(8.0)),
            }
        }
    }

    fn loads_full_res(&self) -> bool {
        self.full_res || self.mosaic || self.prefs.binning == BinningPref::Bin1
    }

    fn apply_prefs(&mut self, prefs: Preferences) {
        let old = std::mem::replace(&mut self.prefs, prefs);
        if prefs.show_info != old.show_info {
            self.show_info = prefs.show_info;
        }
        if prefs.show_debug != old.show_debug {
            self.show_debug = prefs.show_debug;
        }
        if prefs.actual_size != old.actual_size {
            self.set_actual_size(prefs.actual_size);
        }
        let reload = prefs.neutralize != old.neutralize
            || prefs.auto_stretch != old.auto_stretch
            || prefs.binning != old.binning
            || prefs.boost != old.boost;
        if prefs.neutralize != old.neutralize {
            self.params.neutralize = prefs.neutralize;
        }
        for (toggle, on, was) in [
            (&mut self.auto_stretch, prefs.auto_stretch, old.auto_stretch),
            (&mut self.flash_clip, prefs.flash_clip, old.flash_clip),
            (&mut self.star_view, prefs.star_view, old.star_view),
            (&mut self.tilt_view, prefs.tilt_view, old.tilt_view),
            (&mut self.sky_view, prefs.sky_view, old.sky_view),
        ] {
            if on != was {
                *toggle = ImageToggle::new(on);
            }
        }
        if prefs.boost != old.boost {
            self.boost = prefs.boost;
        }
        if prefs.binning != old.binning {
            self.full_res = false;
        }
        if self.list.apply_prefs(&prefs, &old) {
            self.files_changed();
        }
        if prefs.subfolder_depth != old.subfolder_depth {
            self.refresh();
        }
        if reload {
            self.reload_all();
        } else if (prefs.ahead, prefs.behind) != (old.ahead, old.behind) {
            self.evict();
            self.request_loads();
        }
    }

    /// Half the GPU memory budget; the rest is left to other processes and decoding.
    fn cache_limit(&self) -> usize {
        let wanted = CACHE_SIZE.max(1 + self.prefs.ahead + self.prefs.behind);
        let fits = match self.memory_budget {
            Some(m) if self.image_bytes > 0 => {
                let n = m.budget / 2 / self.image_bytes as u64;
                usize::try_from(n).unwrap_or(usize::MAX).max(MIN_CACHE)
            }
            _ if self.loads_full_res() => CACHE_SIZE_FULL,
            _ => wanted,
        };
        wanted.min(fits).min(self.memory_cap.unwrap_or(usize::MAX))
    }

    fn evict(&mut self) {
        while self.cache.len() > self.cache_limit() {
            let Some(victim) = self.eviction_victim(false) else {
                break;
            };
            self.cache.remove(&victim);
        }
    }

    fn eviction_victim(&self, drop_shown: bool) -> Option<PathBuf> {
        let current = self.current();
        let shown = self.shown.as_ref().filter(|_| !drop_shown);
        let window: Vec<&PathBuf> = (self.prefetch_window().into_iter())
            .map(|(_, p)| p)
            .collect();
        let eviction_order =
            |path: &PathBuf, entry: &CacheEntry| match window.iter().position(|w| *w == path) {
                Some(priority) => EvictionOrder::InWindow {
                    lowest_priority: Reverse(priority),
                },
                None => EvictionOrder::OutsideWindow {
                    least_recently_used: entry.last_used,
                },
            };
        (self.cache.iter())
            .filter(|(p, _)| Some(*p) != current && Some(*p) != shown)
            .min_by_key(|(p, e)| eviction_order(p, e))
            .map(|(p, _)| p.clone())
    }

    fn out_of_memory(&mut self, path: PathBuf) {
        self.stale = None;
        let is_current = self.current() == Some(&path);
        let fits = match self.eviction_victim(is_current) {
            Some(victim) => {
                let fits = self.cache.len();
                self.cache.remove(&victim);
                fits
            }
            None if is_current => {
                self.errors.insert(path, "not enough GPU memory".into());
                self.request_loads();
                return;
            }
            // Only the current image fits.
            None => 1,
        };
        self.memory_cap = Some(fits);
        let s = if fits == 1 { "" } else { "s" };
        self.set_status(
            format!("GPU memory full: keeping {fits} image{s}"),
            Some(4.0),
        );
        self.request_loads();
    }

    fn delete(&mut self, path: &Path) {
        if let Err(e) = std::fs::remove_file(path) {
            self.set_status(format!("Delete failed: {e}"), Some(8.0));
            return;
        }
        self.forget(&[path.to_path_buf()]);
        self.set_status(format!("Deleted {}", file_name(path)), Some(2.5));
        self.request_loads();
    }

    /// Drops files that are no longer in the folder.
    fn forget(&mut self, files: &[PathBuf]) {
        for file in files {
            self.cache.remove(file);
            self.errors.remove(file);
            self.stars.remove(file);
        }
        if self
            .shown
            .as_ref()
            .is_some_and(|shown| files.contains(shown))
        {
            self.shown = None;
        }
        self.list.remove(files);
    }

    fn set_status(&mut self, text: String, ttl: Option<f32>) {
        self.show_status(StatusKind::Message, text, ttl);
    }

    fn show_status(&mut self, kind: StatusKind, text: String, ttl: Option<f32>) {
        self.status = Some(Status {
            kind,
            text,
            since: Instant::now(),
            ttl,
        });
    }

    fn clear_status(&mut self, kind: StatusKind) {
        if self.status.as_ref().is_some_and(|s| s.kind == kind) {
            self.status = None;
        }
    }

    fn pick_folder(&mut self) {
        let mut dialog = rfd::FileDialog::new();
        if let Some(dir) = self.list.dir().or(self.last_dir.as_deref()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(dir) = dialog.pick_folder() {
            self.open(&dir);
        }
    }

    fn export_current(&mut self, ctx: &egui::Context) {
        if let Some(src) = self.current().cloned() {
            self.export(ctx, src);
        }
    }

    /// Ask for a target file, then export at full resolution in the background.
    fn export(&mut self, ctx: &egui::Context, src: PathBuf) {
        let stem = src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut dialog = rfd::FileDialog::new()
            .set_file_name(format!("{stem}.png"))
            .add_filter("PNG", &["png"])
            .add_filter("JPEG", &["jpg", "jpeg"]);
        if let Some(dir) = src.parent() {
            dialog = dialog.set_directory(dir);
        }
        let Some(dst) = dialog.save_file() else {
            return;
        };
        let params = self.settings(&src).stretch;
        let (tx, ctx) = (self.export_tx.clone(), ctx.clone());
        self.set_status(format!("Exporting {}…", file_name(&dst)), None);
        std::thread::spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| {
                blip_core::export::export(&src, &dst, params)
            }))
            .unwrap_or_else(|panic| Err(format!("internal error: {}", panic_message(&*panic))));
            let msg = match result {
                Ok(path) => Ok(format!("Exported {}", path.display())),
                Err(e) => Err(format!("Export failed: {e}")),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match self.current() {
            Some(p) => format!(
                "blip · {} · {}/{}",
                file_name(p),
                self.list.selected() + 1,
                self.list.files().len()
            ),
            None => "blip".into(),
        };
        // Setting the native title every frame costs window-manager work and repaints.
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_background();

        let dropped = ctx.input(|i| {
            i.raw
                .dropped_files
                .first()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
        });
        #[cfg(target_os = "macos")]
        let dropped = dropped.or_else(crate::macos::take_opened);
        if let Some(path) = dropped {
            self.open(&path);
        }

        self.handle_keys(&ctx);
        self.delete_dialog(&ctx);
        self.help_dialog(&ctx);
        self.settings_dialog(&ctx);
        self.header_dialog(&ctx);

        egui::Panel::left("files")
            .resizable(true)
            .default_size(280.0)
            .min_size(140.0)
            .show(ui, |ui| self.file_list(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| self.image_view(ui));

        self.update_title(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.prefs.save(storage);
        if let Some(dir) = self.list.dir() {
            storage.set_string("last_dir", dir.display().to_string());
        }
    }
}

fn headers_read(done: usize, failed: usize, elapsed: Duration) -> String {
    let elapsed = if elapsed < Duration::from_secs(1) {
        format!("{} ms", elapsed.as_millis())
    } else {
        format!("{:.1} s", elapsed.as_secs_f64())
    };
    let s = if done == 1 { "" } else { "s" };
    let mut text = format!("Read {done} header{s} in {elapsed}");
    if failed > 0 {
        text += &format!(", {failed} unreadable");
    }
    text
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn file_count(n: usize) -> String {
    if n == 1 {
        "1 file".into()
    } else {
        format!("{n} files")
    }
}

/// The message of a caught panic.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefetch_takes_two_ahead_per_one_behind() {
        let offsets = |d, ahead, behind| prefetch_offsets(d, ahead, behind).collect::<Vec<_>>();
        assert_eq!(offsets(1, 3, 1), [0, 1, 2, -1, 3]);
        assert_eq!(offsets(-1, 4, 2), [0, -1, -2, 1, -3, -4, 2]);
        assert_eq!(offsets(1, 1, 3), [0, 1, -1, -2, -3]);
        assert_eq!(offsets(1, 0, 0), [0]);
    }

    #[test]
    fn toggling_another_image_reverts_the_previous_one() {
        let (a, b) = (Path::new("a"), Path::new("b"));
        let mut t = ImageToggle::new(true);
        assert_eq!(t.flip(a), None);
        assert!(!t.is_on(a));
        assert_eq!(t.flip(b), Some(a.to_path_buf()));
        assert!(t.is_on(a) && !t.is_on(b));
        assert_eq!(t.flip(b), None);
        assert!(t.is_on(b));
    }
}
