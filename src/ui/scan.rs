//! Listing the opened folder in the background, which can take seconds on network shares.

use eframe::egui;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

const EXTENSIONS: [&str; 4] = ["fit", "fits", "fts", "xisf"];

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Images of the subfolders come first, folder by folder, then the folder's own.
/// Unreadable subfolders are skipped.
pub fn list_images(dir: &Path, subfolder_depth: usize) -> io::Result<Vec<PathBuf>> {
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

pub struct Listing {
    pub dir: PathBuf,
    pub depth: usize,
    pub files: io::Result<Vec<PathBuf>>,
}

/// Only the latest listing is kept; older ones are dropped when they finish.
#[derive(Clone, Default)]
pub struct Scanner(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    id: u64,
    dir: PathBuf,
    depth: usize,
    files: Option<io::Result<Vec<PathBuf>>>,
    taken: bool,
    ctx: Option<egui::Context>,
}

impl Scanner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn attach(&self, ctx: egui::Context) {
        self.lock().ctx = Some(ctx);
    }

    /// Lists `dir` on a background thread.
    pub fn scan(&self, dir: &Path, depth: usize) {
        if let Some(id) = self.begin(dir, depth) {
            let (scanner, dir) = (self.clone(), dir.to_path_buf());
            std::thread::spawn(move || scanner.finish(id, list_images(&dir, depth)));
        }
    }

    /// Starts a listing to be passed to `finish`, unless one of the same folder is already
    /// under way or waiting to be taken.
    pub fn begin(&self, dir: &Path, depth: usize) -> Option<u64> {
        let mut state = self.lock();
        if state.id > 0 && !state.taken && state.dir == dir && state.depth == depth {
            return None;
        }
        state.id += 1;
        state.dir = dir.to_path_buf();
        state.depth = depth;
        state.files = None;
        state.taken = false;
        Some(state.id)
    }

    pub fn finish(&self, id: u64, files: io::Result<Vec<PathBuf>>) {
        let mut state = self.lock();
        if state.id == id {
            state.files = Some(files);
            if let Some(ctx) = &state.ctx {
                ctx.request_repaint();
            }
        }
    }

    pub fn busy(&self) -> bool {
        let state = self.lock();
        state.id > 0 && !state.taken && state.files.is_none()
    }

    pub fn take(&self) -> Option<Listing> {
        let mut state = self.lock();
        let files = state.files.take()?;
        state.taken = true;
        Some(Listing {
            dir: state.dir.clone(),
            depth: state.depth,
            files,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuses_a_pending_listing_and_drops_superseded_ones() {
        let scanner = Scanner::default();
        let a = scanner.begin(Path::new("a"), 0).unwrap();
        assert_eq!(scanner.begin(Path::new("a"), 0), None);
        let b = scanner.begin(Path::new("b"), 0).unwrap();
        scanner.finish(a, Ok(Vec::new()));
        assert!(scanner.take().is_none());
        assert!(scanner.busy());
        scanner.finish(b, Ok(vec![PathBuf::from("b/1.fits")]));
        let listing = scanner.take().unwrap();
        assert_eq!(listing.dir, Path::new("b"));
        assert!(!scanner.busy());
        assert!(scanner.begin(Path::new("b"), 0).is_some());
    }
}
