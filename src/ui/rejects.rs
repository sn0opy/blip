//! Rejecting images while blinking, and moving, trashing or deleting the rejected ones.

use super::actions::{self, Action};
use super::dialogs::Deletion;
use super::prefs::RejectHandling;
use super::{App, draw, file_count, file_name, trash};
use eframe::egui::{self, Vec2};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{TryRecvError, channel};

pub const REJECTED_FOLDER: &str = "_rejected";

pub(super) struct Disposal {
    handling: RejectHandling,
    done: Vec<PathBuf>,
    errors: Vec<String>,
}

impl App {
    pub(super) fn toggle_rejected(&mut self, path: &Path) {
        let verb = if self.list.toggle_rejected(path) {
            "Rejected"
        } else {
            "Unrejected"
        };
        let text = format!("{verb} {}", file_name(path));
        self.status.set(text, Some(2.5));
    }

    fn disposing(&self) -> bool {
        self.disposal.is_some()
    }

    pub(super) fn handle_rejected(&mut self, ctx: &egui::Context) {
        if self.list.rejected_count() == 0 || self.disposing() {
            return;
        }
        match self.prefs.reject_handling {
            RejectHandling::Delete => self.confirm_delete = Some(Deletion::Rejected),
            handling => self.dispose_rejected(ctx, handling),
        }
    }

    pub(super) fn dispose_rejected(&mut self, ctx: &egui::Context, handling: RejectHandling) {
        let Some(root) = self.list.dir().map(Path::to_path_buf) else {
            return;
        };
        let files = self.list.rejected();
        let text = disposing_message(handling, files.len());
        self.status.set(text, None);
        let (tx, rx) = channel();
        self.disposal = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(dispose(handling, &root, files));
            ctx.request_repaint();
        });
    }

    pub(super) fn poll_disposal(&mut self) {
        let Some(rx) = &self.disposal else {
            return;
        };
        let disposal = match rx.try_recv() {
            Ok(disposal) => disposal,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.disposal = None;
                let text = "Handling the rejected files stopped unexpectedly".to_string();
                return self.status.set(text, Some(8.0));
            }
        };
        self.disposal = None;
        self.forget(&disposal.done);
        let message = disposed_message(disposal.handling, disposal.done.len());
        match disposal.errors.first() {
            None => self.status.set(message, Some(2.5)),
            Some(e) => {
                let failed = format!("{message}, {} failed: {e}", disposal.errors.len());
                self.status.set(failed, Some(8.0));
            }
        }
        self.request_loads();
    }

    /// Below the file list while any file is rejected.
    pub(super) fn rejected_panel(&mut self, ui: &mut egui::Ui) {
        let count = self.list.rejected_count();
        if count == 0 {
            return;
        }
        let frame = egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, 6));
        egui::Panel::bottom("rejected").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(draw::RED, format!("{count} rejected"));
                let right = egui::Layout::right_to_left(egui::Align::Center);
                ui.with_layout(right, |ui| {
                    let clear = ui.button("Clear").on_hover_text("Unreject all files");
                    if clear.clicked() {
                        self.list.clear_rejected();
                    }
                });
            });
            let label = match self.prefs.reject_handling {
                RejectHandling::MoveToFolder => format!("Move to {REJECTED_FOLDER}/"),
                RejectHandling::MoveToTrash => "Move to trash".into(),
                RejectHandling::Delete => "Delete…".into(),
            };
            let centered = (egui::Atom::grow(), label, egui::Atom::grow());
            let button = egui::Button::new(centered).min_size(Vec2::new(ui.available_width(), 0.0));
            let button = ui.add_enabled(!self.disposing(), button);
            if actions::tooltip(button, Action::HandleRejected).clicked() {
                self.handle_rejected(ui.ctx());
            }
        });
    }
}

fn dispose(handling: RejectHandling, root: &Path, files: Vec<PathBuf>) -> Disposal {
    let results = match handling {
        RejectHandling::MoveToFolder => each_file(&files, |f| move_to_rejected_folder(root, f)),
        RejectHandling::MoveToTrash => trash::move_to_trash(&files),
        RejectHandling::Delete => each_file(&files, |f| std::fs::remove_file(f)),
    };
    let mut done = Vec::new();
    let mut errors = Vec::new();
    for (file, result) in files.into_iter().zip(results) {
        match result {
            Ok(()) => done.push(file),
            Err(e) => errors.push(e),
        }
    }
    Disposal {
        handling,
        done,
        errors,
    }
}

fn disposing_message(handling: RejectHandling, files: usize) -> String {
    let files = file_count(files);
    match handling {
        RejectHandling::MoveToFolder => format!("Moving {files} to {REJECTED_FOLDER}/…"),
        RejectHandling::MoveToTrash => format!("Moving {files} to the trash…"),
        RejectHandling::Delete => format!("Deleting {files}…"),
    }
}

fn disposed_message(handling: RejectHandling, done: usize) -> String {
    let done = file_count(done);
    match handling {
        RejectHandling::MoveToFolder => format!("Moved {done} to {REJECTED_FOLDER}/"),
        RejectHandling::MoveToTrash => format!("Moved {done} to the trash"),
        RejectHandling::Delete => format!("Deleted {done}"),
    }
}

fn each_file(
    files: &[PathBuf],
    dispose: impl Fn(&Path) -> io::Result<()>,
) -> Vec<Result<(), String>> {
    (files.iter())
        .map(|f| dispose(f).map_err(|e| e.to_string()))
        .collect()
}

pub fn is_rejected_folder(path: &Path) -> bool {
    path.file_name() == Some(REJECTED_FOLDER.as_ref())
}

/// Into the rejected folder in `root`, in the same subfolders. Never replaces a file.
fn move_to_rejected_folder(root: &Path, file: &Path) -> io::Result<()> {
    let relative = file.strip_prefix(root).map_err(io::Error::other)?;
    let target = unused_path(root.join(REJECTED_FOLDER).join(relative));
    if let Some(folder) = target.parent() {
        std::fs::create_dir_all(folder)?;
    }
    std::fs::rename(file, target)
}

/// `path`, or `name (2).ext`, `name (3).ext`, … if it's taken.
fn unused_path(path: PathBuf) -> PathBuf {
    let taken = |p: &Path| p.symlink_metadata().is_ok();
    if !taken(&path) {
        return path;
    }
    let stem = path.file_stem().unwrap_or_default().to_owned();
    let extension = path.extension().map(OsString::from);
    let mut n = 2;
    loop {
        let mut name = stem.clone();
        name.push(format!(" ({n})"));
        if let Some(extension) = &extension {
            name.push(".");
            name.push(extension);
        }
        let numbered = path.with_file_name(name);
        if !taken(&numbered) {
            return numbered;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::scan::list_images;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("blip-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, relative).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rejected_files_keep_their_subfolders_and_never_replace_one() {
        let root = TempDir::new("reject");
        let light = root.file("night 1/light.fits");
        root.file("_rejected/night 1/light.fits");
        root.file("_rejected/night 1/light (2).fits");

        move_to_rejected_folder(&root.0, &light).unwrap();
        assert!(!light.exists());
        let moved = root.0.join("_rejected/night 1/light (3).fits");
        assert_eq!(
            std::fs::read_to_string(moved).unwrap(),
            "night 1/light.fits"
        );
        assert!(list_images(&root.0, 2).unwrap().is_empty());
    }

    #[test]
    fn numbers_names_without_an_extension() {
        let root = TempDir::new("unused");
        let taken = root.file("light");
        assert_eq!(unused_path(taken), root.0.join("light (2)"));
    }
}
