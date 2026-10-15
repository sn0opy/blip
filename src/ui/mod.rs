//! The egui application: state, file handling and background jobs. Input handling,
//! views and dialogs live in the submodules.

mod dialogs;
mod draw;
mod gpu;
mod keys;
mod loader;
mod texture;
mod view;

pub use gpu::wgpu_options;

use blip::process::StretchParams;
use blip::stars::StarStats;
use eframe::egui::{self, Vec2};
use loader::{Loaded, Loader, Message, Settings};
use std::any::Any;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

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

struct MeasuredStars {
    stats: StarStats,
    ms: f64,
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
    full_res: bool,
    show_info: bool,
    show_debug: bool,
    /// File list shows the end of long names instead of the start.
    show_name_end: bool,
    /// Graphics backend and adapter, for the debug panel.
    gpu_info: String,
    zoom: f32,
    pan: Vec2,
    /// Per-image overlays: switching images turns them off.
    flash_clip: Option<PathBuf>,
    star_view: Option<PathBuf>,
    /// Star measurements, computed by the loader for every image.
    stars: HashMap<PathBuf, MeasuredStars>,

    confirm_delete: Option<PathBuf>,
    show_help: bool,
    scroll_to_selected: bool,
    status: Option<Status>,
    title: String,

    export_tx: Sender<Result<String, String>>,
    export_rx: Receiver<Result<String, String>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, start: Option<PathBuf>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (export_tx, export_rx) = channel();
        let mut app = Self {
            loader: Loader::new(
                cc.egui_ctx.clone(),
                texture::Uploader::new(&cc.egui_ctx, cc.wgpu_render_state.as_ref()),
            ),
            dir: None,
            last_dir: cc
                .storage
                .and_then(|s| s.get_string("last_dir"))
                .map(PathBuf::from),
            files: Vec::new(),
            selected: 0,
            direction: 1,
            cache: HashMap::new(),
            errors: HashMap::new(),
            shown: None,
            stale: None,
            tick: 0,
            params: StretchParams::default(),
            full_res: false,
            show_info: true,
            show_debug: false,
            show_name_end: false,
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
            flash_clip: None,
            star_view: None,
            stars: HashMap::new(),
            confirm_delete: None,
            show_help: false,
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
        self.files = match std::fs::read_dir(dir) {
            Ok(rd) => {
                let mut files: Vec<PathBuf> = rd
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| {
                        p.extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
                    })
                    .collect();
                files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
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

    /// Queue the current image first, then prefetch neighbors (mostly ahead), and
    /// measure stars of the same files where still missing.
    fn request_loads(&mut self) {
        let d = self.direction;
        let s = self.selected as isize;
        let window: Vec<&PathBuf> = [s, s + d, s + 2 * d, s - d, s + 3 * d]
            .into_iter()
            .filter(|&i| i >= 0 && (i as usize) < self.files.len())
            .map(|i| &self.files[i as usize])
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
        self.loader.set_queue(images, stars, self.settings());
    }

    fn settings(&self) -> Settings {
        Settings {
            stretch: self.params,
            max_side: (!self.full_res).then_some(MAX_SIDE),
        }
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
                    if settings == self.settings() && self.files.contains(&path) =>
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
                Message::Stars(path, stats, ms) if self.files.contains(&path) => {
                    self.stars.insert(path, MeasuredStars { stats, ms });
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

    /// Number of rendered images kept on the GPU.
    fn cache_limit(&self) -> usize {
        if self.full_res {
            CACHE_SIZE_FULL
        } else {
            CACHE_SIZE
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
        let (tx, ctx, params) = (self.export_tx.clone(), ctx.clone(), self.params);
        self.set_status(format!("Exporting {}…", file_name(&dst)), None);
        std::thread::spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| {
                blip::export::export(&src, &dst, params)
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

        egui::Panel::left("files")
            .resizable(true)
            .default_size(280.0)
            .min_size(140.0)
            .show(ui, |ui| self.file_list(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| self.image_view(ui));

        self.update_title(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
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
