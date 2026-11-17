//! The egui application: state, file handling and background jobs. Input handling,
//! views and dialogs live in the submodules.

mod actions;
mod cache;
mod dialogs;
mod draw;
mod export;
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
mod status;
mod texture;
mod toggles;
mod toolbar;
mod trash;
mod tree;
mod view;

pub use gpu::wgpu_options;
pub use loader::{Loader, Preloader};
pub use scan::Scanner;

use blip_core::dso::Dso;
use blip_core::process::StretchParams;
use cache::ImageCache;
use dialogs::Deletion;
use eframe::egui::{self, Vec2};
use export::Exports;
use headers::{HeaderScanner, Progress};
use list::FileList;
use loader::{Binning, Loaded, MeasuredStars, Message, Settings};
use prefs::{BinningPref, Preferences};
use scan::{Listing, is_image, list_images};
use status::{StatusKind, StatusLine};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Duration;

pub const APP_ID: &str = "sh.sascha.blip";
/// Previews are binned so their long side is at most this many pixels.
const MAX_SIDE: usize = 3200;

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

pub struct App {
    loader: Loader,
    scanner: Scanner,
    /// Last folder of a previous session; only the Open dialog's start location.
    last_dir: Option<PathBuf>,
    list: FileList,
    /// Direction of the last navigation step, used to prefetch ahead.
    direction: isize,
    cache: ImageCache,
    /// The image on screen; lags behind `selected` while the next one loads.
    shown: Option<PathBuf>,

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
    overlay: draw::CachedOverlay<view::OverlayState>,
    header_table: header::HeaderTable,
    /// The settings dialog's values while a slider is dragged, applied on release.
    prefs_draft: Option<Preferences>,
    status: StatusLine,
    title: String,

    exports: Exports,
    disposal: Option<Receiver<rejects::Disposal>>,
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
        cc.egui_ctx.all_styles_mut(round_widgets);
        scanner.attach(cc.egui_ctx.clone());
        loader.attach(
            cc.egui_ctx.clone(),
            texture::Uploader::new(&cc.egui_ctx, cc.wgpu_render_state.as_ref()),
        );
        let prefs = Preferences::load(cc.storage);
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
            cache: ImageCache::new(gpu_memory),
            shown: None,
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
            overlay: draw::CachedOverlay::default(),
            header_table: header::HeaderTable::default(),
            prefs_draft: None,
            status: StatusLine::default(),
            title: String::new(),
            exports: Exports::default(),
            disposal: None,
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
        self.status.clear(StatusKind::ReadingHeaders);
        self.direction = 1;
        self.cache.clear();
        self.stars.clear();
        self.shown = None;
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
                self.status.clear(StatusKind::FolderError);
                files
            }
            Err(e) => {
                let text = format!("Cannot read folder: {e}");
                self.status.show(StatusKind::FolderError, text, None);
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
            self.status.clear(StatusKind::ReadingHeaders);
        }
        let Some((progress, changed)) = self.list.poll_headers() else {
            return;
        };
        match progress {
            Progress::Reading { done, total } => {
                let text = format!("Reading headers {done}/{total}");
                self.status.show(StatusKind::ReadingHeaders, text, None);
            }
            Progress::Finished {
                done,
                failed,
                elapsed,
            } => {
                let text = headers_read(done, failed, elapsed);
                self.status.set(text, Some(4.0));
            }
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
        self.cache.update_memory_budget();
        let window: Vec<&PathBuf> = (self.prefetch_window().into_iter())
            .map(|(_, p)| p)
            .filter(|p| self.cache.error(p).is_none())
            .collect();
        let missing = |have: &dyn Fn(&PathBuf) -> bool| -> Vec<PathBuf> {
            window
                .iter()
                .filter(|p| !have(p))
                .map(|p| (*p).clone())
                .collect()
        };
        let images = missing(&|p| self.cache.contains(p));
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
        self.cache.rerender(&cur);
        let state = if on { "on" } else { "off" };
        let key = actions::key_hint(actions::Action::AutoStretch);
        let text = format!("Auto stretch {state} ({key} to toggle)");
        self.status.set(text, Some(2.0));
        self.request_loads();
    }

    fn reload_all(&mut self) {
        self.cache.rerender_all(self.shown.as_deref());
        self.request_loads();
    }

    fn poll_background(&mut self) {
        if let Some(listing) = self.scanner.take() {
            self.apply_listing(listing);
        }
        self.poll_headers();
        self.poll_disposal();
        let limit = self.cache_limit();
        let mut loaded_any = false;
        let mut files_changed = false;
        while let Some(msg) = self.loader.try_recv() {
            match msg {
                Message::Image(path, settings, result)
                    if settings == self.settings(&path) && self.list.is_listed(&path) =>
                {
                    loaded_any = true;
                    match result {
                        Ok(loaded) => files_changed |= self.receive_image(path, *loaded),
                        Err(e) => self.cache.insert_error(path, e),
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
            self.cache.update_memory_budget();
            self.evict();
            if self.cache_limit() != limit {
                self.request_loads();
            }
        }
        self.poll_exports();
    }

    /// Returns whether the file list changed.
    fn receive_image(&mut self, path: PathBuf, mut loaded: Loaded) -> bool {
        let header = std::mem::take(&mut loaded.header);
        let files_changed = self.list.add_loaded_header(path.clone(), header);
        self.cache.insert(path, loaded);
        files_changed
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

    fn cache_limit(&self) -> usize {
        let (ahead, behind) = (self.prefs.ahead, self.prefs.behind);
        self.cache.limit(ahead, behind, self.loads_full_res())
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
        let shown = self.shown.as_ref().filter(|_| !drop_shown);
        let window: Vec<&PathBuf> = (self.prefetch_window().into_iter())
            .map(|(_, p)| p)
            .collect();
        self.cache.eviction_victim(&window, [self.current(), shown])
    }

    fn out_of_memory(&mut self, path: PathBuf) {
        let is_current = self.current() == Some(&path);
        let victim = self.eviction_victim(is_current);
        if let Some(fits) = self.cache.out_of_memory(path, victim, is_current) {
            let s = if fits == 1 { "" } else { "s" };
            self.status.set(
                format!("GPU memory full: keeping {fits} image{s}"),
                Some(4.0),
            );
        }
        self.request_loads();
    }

    fn delete(&mut self, path: &Path) {
        if let Err(e) = std::fs::remove_file(path) {
            self.status.set(format!("Delete failed: {e}"), Some(8.0));
            return;
        }
        self.forget(&[path.to_path_buf()]);
        let text = format!("Deleted {}", file_name(path));
        self.status.set(text, Some(2.5));
        self.request_loads();
    }

    /// Drops files that are no longer in the folder.
    fn forget(&mut self, files: &[PathBuf]) {
        for file in files {
            self.cache.forget(file);
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

    fn pick_folder(&mut self) {
        let mut dialog = rfd::FileDialog::new();
        if let Some(dir) = self.list.dir().or(self.last_dir.as_deref()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(dir) = dialog.pick_folder() {
            self.open(&dir);
        }
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

fn subject_name(d: &Dso, separator: &str) -> String {
    if d.common_name.is_empty() {
        d.label.to_string()
    } else {
        format!("{}{separator}{}", d.label, d.common_name)
    }
}

fn round_widgets(style: &mut egui::Style) {
    let widgets = &mut style.visuals.widgets;
    for state in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.corner_radius = egui::CornerRadius::same(draw::CORNER_RADIUS);
    }
}

fn file_count(n: usize) -> String {
    if n == 1 {
        "1 file".into()
    } else {
        format!("{n} files")
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
