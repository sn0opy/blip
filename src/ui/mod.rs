//! The egui application: state, file handling and background jobs. Input handling,
//! views and dialogs live in the submodules.

mod dialogs;
mod draw;
mod gpu;
mod keys;
mod loader;
mod prefs;
mod texture;
mod tree;
mod view;

pub use gpu::wgpu_options;
pub use loader::{Loader, Preloader};

use blip_core::process::StretchParams;
use blip_core::solve::Solution;
use blip_core::stars::StarStats;
use blip_core::tilt::Tilt;
use eframe::egui::{self, Vec2};
use loader::{Binning, Loaded, Message, Settings};
use prefs::{BinningPref, Preferences};
use std::any::Any;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;
use tree::FolderTree;

pub const APP_ID: &str = "sh.sascha.blip";
/// Previews are binned so their long side is at most this many pixels.
const MAX_SIDE: usize = 3200;
/// Number of rendered images kept on the GPU (binned / full resolution).
const CACHE_SIZE: usize = 10;
const CACHE_SIZE_FULL: usize = 4;
const EXTENSIONS: [&str; 4] = ["fit", "fits", "fts", "xisf"];
const STATUS_FADE: f32 = 0.8;

/// A message in the bottom-left corner. Transient ones fade out after `ttl` seconds.
struct Status {
    text: String,
    since: Instant,
    ttl: Option<f32>,
}

pub struct MeasuredStars {
    stats: StarStats,
    tilt: Option<Tilt>,
    detect_ms: f64,
    solution: Result<Solution, String>,
}

impl MeasuredStars {
    fn failed(reason: &str) -> Self {
        Self {
            stats: StarStats::default(),
            tilt: None,
            detect_ms: 0.0,
            solution: Err(reason.to_string()),
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

pub struct App {
    loader: Loader,
    dir: Option<PathBuf>,
    /// Last folder of a previous session; only the Open dialog's start location.
    last_dir: Option<PathBuf>,
    files: Vec<PathBuf>,
    tree: FolderTree,
    selected: usize,
    /// Direction of the last navigation step, used to prefetch ahead.
    direction: isize,
    cache: HashMap<PathBuf, CacheEntry>,
    errors: HashMap<PathBuf, String>,
    /// The image on screen; lags behind `selected` while the next one loads.
    shown: Option<PathBuf>,
    /// The shown image while a settings change re-renders it (avoids a black flash).
    stale: Option<(PathBuf, CacheEntry)>,
    tick: u64,

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

    prefs: Preferences,
    confirm_delete: Option<PathBuf>,
    show_help: bool,
    show_settings: bool,
    /// The settings dialog's values while a slider is dragged, applied on release.
    prefs_draft: Option<Preferences>,
    scroll_to_selected: bool,
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

pub fn preload(preloader: Preloader, path: PathBuf) {
    std::thread::spawn(move || {
        let prefs = Preferences::load_saved(APP_ID);
        let first = if path.is_dir() {
            list_images(&path, prefs.subfolder_depth)
                .ok()
                .and_then(|files| files.into_iter().next())
        } else {
            is_image(&path).then_some(path)
        };
        if let Some(first) = first {
            preloader.preload(first, initial_settings(&prefs));
        }
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

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Images of the subfolders come first, folder by folder, then the folder's own.
/// Unreadable subfolders are skipped.
fn list_images(dir: &Path, subfolder_depth: usize) -> std::io::Result<Vec<PathBuf>> {
    let mut subfolders = Vec::new();
    let mut images = Vec::new();
    for path in std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
    {
        if is_image(&path) {
            images.push(path);
        } else if subfolder_depth > 0 && path.is_dir() {
            subfolders.push(path);
        }
    }
    let by_name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
    subfolders.sort_by_key(by_name);
    images.sort_by_key(by_name);
    let mut listed: Vec<PathBuf> = (subfolders.iter())
        .flat_map(|folder| list_images(folder, subfolder_depth - 1).unwrap_or_default())
        .collect();
    listed.extend(images);
    Ok(listed)
}

impl App {
    pub fn new(cc: &eframe::CreationContext, start: Option<PathBuf>, loader: Loader) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        loader.attach(
            cc.egui_ctx.clone(),
            texture::Uploader::new(&cc.egui_ctx, cc.wgpu_render_state.as_ref()),
        );
        let prefs = Preferences::load(cc.storage);
        let (export_tx, export_rx) = channel();
        let mut app = Self {
            loader,
            dir: None,
            last_dir: cc
                .storage
                .and_then(|s| s.get_string("last_dir"))
                .map(PathBuf::from),
            files: Vec::new(),
            tree: FolderTree::default(),
            selected: 0,
            direction: 1,
            cache: HashMap::new(),
            errors: HashMap::new(),
            shown: None,
            stale: None,
            tick: 0,
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
            prefs,
            confirm_delete: None,
            show_help: false,
            show_settings: false,
            prefs_draft: None,
            scroll_to_selected: false,
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
        self.files.get(self.selected)
    }

    /// Open a folder, or a file (opens its folder and selects it).
    fn open(&mut self, path: &Path) {
        let (dir, file) = if path.is_dir() {
            (path.to_path_buf(), None)
        } else {
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            (dir, Some(path.to_path_buf()))
        };
        self.tree = FolderTree::new(&dir);
        self.dir = Some(dir);
        self.cache.clear();
        self.errors.clear();
        self.stars.clear();
        self.shown = None;
        self.refresh();
        if let Some(f) = file
            && let Some(i) = self.files.iter().position(|p| *p == f)
        {
            self.select(i);
        }
    }

    fn refresh(&mut self) {
        let Some(dir) = &self.dir else { return };
        let current = self.current().cloned();
        self.files = match list_images(dir, self.prefs.subfolder_depth) {
            Ok(files) => {
                if self
                    .status
                    .as_ref()
                    .is_some_and(|st| st.text.starts_with("Cannot read folder"))
                {
                    self.status = None;
                }
                files
            }
            Err(e) => {
                self.set_status(format!("Cannot read folder: {e}"), None);
                Vec::new()
            }
        };
        self.tree.rebuild(&self.files);
        self.selected = current
            .and_then(|c| self.files.iter().position(|p| *p == c))
            .unwrap_or(0);
        self.scroll_to_selected = true;
        self.request_loads();
    }

    fn select(&mut self, index: usize) {
        if self.files.is_empty() {
            return;
        }
        let index = index.min(self.files.len() - 1);
        if index != self.selected {
            self.direction = if index > self.selected { 1 } else { -1 };
        }
        self.selected = index;
        self.scroll_to_selected = true;
        self.request_loads();
    }

    fn step(&mut self, delta: isize) {
        if self.files.is_empty() {
            return;
        }
        let i = (self.selected as isize + delta).clamp(0, self.files.len() as isize - 1);
        self.select(i as usize);
    }

    /// The current image, then its neighbors (mostly ahead), with their offsets from the
    /// current one.
    fn prefetch_window(&self) -> Vec<(isize, &PathBuf)> {
        let s = self.selected as isize;
        prefetch_offsets(self.direction, self.prefs.ahead, self.prefs.behind)
            .filter_map(|o| Some((o, self.files.get(usize::try_from(s + o).ok()?)?)))
            .take(self.cache_limit())
            .collect()
    }

    /// Queue the current image first, then prefetch neighbors, and measure stars of
    /// the same files where still missing.
    fn request_loads(&mut self) {
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
        let waiting = self.current().is_some_and(|c| images.first() == Some(c));
        let images = (images.into_iter())
            .map(|p| {
                let settings = self.settings(&p);
                (p, settings)
            })
            .collect();
        self.loader.set_queue(images, stars, waiting);
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
        self.set_status(format!("Auto stretch {state} (A to toggle)"), Some(2.0));
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
        self.request_loads();
    }

    /// Collect finished background work: previews, star measurements, exports.
    fn poll_background(&mut self) {
        let mut loaded_any = false;
        while let Some(msg) = self.loader.try_recv() {
            match msg {
                // Ignore stale settings and files that are no longer listed.
                Message::Image(path, settings, result)
                    if settings == self.settings(&path) && self.files.contains(&path) =>
                {
                    loaded_any = true;
                    match result {
                        Ok(loaded) => {
                            self.tick += 1;
                            let last_used = self.tick;
                            let loaded = *loaded;
                            self.cache.insert(path, CacheEntry { loaded, last_used });
                        }
                        Err(e) => {
                            self.errors.insert(path, e);
                        }
                    }
                }
                Message::Stars(path, measured) if self.files.contains(&path) => {
                    self.stars.insert(path, *measured);
                }
                _ => {}
            }
        }
        if loaded_any {
            self.evict();
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

    /// Number of rendered images kept on the GPU.
    fn cache_limit(&self) -> usize {
        if self.loads_full_res() {
            CACHE_SIZE_FULL
        } else {
            CACHE_SIZE.max(1 + self.prefs.ahead + self.prefs.behind)
        }
    }

    fn evict(&mut self) {
        while self.cache.len() > self.cache_limit() {
            let current = self.current();
            let victim = self
                .cache
                .iter()
                .filter(|(p, _)| Some(*p) != current && Some(*p) != self.shown.as_ref())
                .min_by_key(|(_, e)| e.last_used)
                .map(|(p, _)| p.clone());
            let Some(victim) = victim else { break };
            self.cache.remove(&victim);
        }
    }

    fn delete(&mut self, path: &Path) {
        if let Err(e) = std::fs::remove_file(path) {
            self.set_status(format!("Delete failed: {e}"), Some(8.0));
            return;
        }
        self.cache.remove(path);
        self.errors.remove(path);
        self.stars.remove(path);
        if self.shown.as_deref() == Some(path) {
            self.shown = None;
        }
        if let Some(i) = self.files.iter().position(|p| p == path) {
            self.files.remove(i);
            self.tree.rebuild(&self.files);
            if self.selected > i || self.selected >= self.files.len() {
                self.selected = self.selected.saturating_sub(1);
            }
        }
        self.set_status(format!("Deleted {}", file_name(path)), Some(2.5));
        self.scroll_to_selected = true;
        self.request_loads();
    }

    fn set_status(&mut self, text: String, ttl: Option<f32>) {
        self.status = Some(Status {
            text,
            since: Instant::now(),
            ttl,
        });
    }

    fn pick_folder(&mut self) {
        let mut dialog = rfd::FileDialog::new();
        if let Some(dir) = self.dir.as_ref().or(self.last_dir.as_ref()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(dir) = dialog.pick_folder() {
            self.open(&dir);
        }
    }

    /// Ask for a target file, then export at full resolution in the background.
    fn export(&mut self, ctx: &egui::Context) {
        let Some(src) = self.current().cloned() else {
            return;
        };
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
                self.selected + 1,
                self.files.len()
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
        if let Some(dir) = &self.dir {
            storage.set_string("last_dir", dir.display().to_string());
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
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
