//! Exporting images as PNG or JPEG in the background.

use super::loader::panic_message;
use super::{App, file_name};
use blip_core::process::StretchParams;
use eframe::egui;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

/// The status messages of finished exports: `Err` if one failed.
pub struct Exports {
    tx: Sender<Result<String, String>>,
    rx: Receiver<Result<String, String>>,
}

impl Default for Exports {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self { tx, rx }
    }
}

impl App {
    pub(super) fn export_current(&mut self, ctx: &egui::Context) {
        if let Some(src) = self.current().cloned() {
            self.export(ctx, src);
        }
    }

    /// Ask for a target file, then export at full resolution in the background.
    pub(super) fn export(&mut self, ctx: &egui::Context, src: PathBuf) {
        let Some(dst) = ask_for_target(&src) else {
            return;
        };
        let params = self.settings(&src).stretch;
        let (tx, ctx) = (self.exports.tx.clone(), ctx.clone());
        let text = format!("Exporting {}…", file_name(&dst));
        self.status.set(text, None);
        std::thread::spawn(move || {
            let _ = tx.send(export(&src, &dst, params));
            ctx.request_repaint();
        });
    }

    pub(super) fn poll_exports(&mut self) {
        while let Ok(msg) = self.exports.rx.try_recv() {
            match msg {
                Ok(text) => self.status.set(text, Some(2.5)),
                Err(text) => self.status.set(text, Some(8.0)),
            }
        }
    }
}

fn ask_for_target(src: &Path) -> Option<PathBuf> {
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
    dialog.save_file()
}

fn export(src: &Path, dst: &Path, params: StretchParams) -> Result<String, String> {
    let result = catch_unwind(AssertUnwindSafe(|| {
        blip_core::export::export(src, dst, params)
    }))
    .unwrap_or_else(|panic| Err(format!("internal error: {}", panic_message(&*panic))));
    match result {
        Ok(path) => Ok(format!("Exported {}", path.display())),
        Err(e) => Err(format!("Export failed: {e}")),
    }
}
