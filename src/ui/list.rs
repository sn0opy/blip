//! The images below the opened folder, narrowed down by the filter.

use super::filter::{Filter, ListedImage};
use super::prefs::Preferences;
use super::tree::FolderTree;
use std::path::{Path, PathBuf};

pub struct FileList {
    dir: Option<PathBuf>,
    listed: Vec<ListedImage>,
    files: Vec<PathBuf>,
    selected: usize,
    /// The image last selected, kept while the filter hides it.
    anchor: Option<PathBuf>,
    filter: String,
    query: Filter,
    grouped: bool,
    tree: FolderTree,
    scroll_to_selected: bool,
}

impl FileList {
    pub fn new(prefs: &Preferences) -> Self {
        Self {
            dir: None,
            listed: Vec::new(),
            files: Vec::new(),
            selected: 0,
            anchor: None,
            filter: String::new(),
            query: Filter::default(),
            grouped: prefs.group_by_folder,
            tree: FolderTree::default(),
            scroll_to_selected: false,
        }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn current(&self) -> Option<&PathBuf> {
        self.files.get(self.selected)
    }

    pub fn is_listed(&self, path: &Path) -> bool {
        self.listed.iter().any(|image| image.path == path)
    }

    pub fn has_images(&self) -> bool {
        !self.listed.is_empty()
    }

    pub fn open(&mut self, dir: PathBuf, file: Option<PathBuf>) {
        self.filter.clear();
        self.query = Filter::default();
        self.tree = FolderTree::new(&dir);
        self.dir = Some(dir);
        self.anchor = file.clone();
        self.set_listed(file.into_iter().collect());
        self.apply_filter();
    }

    pub fn set_listing(&mut self, paths: Vec<PathBuf>) {
        self.set_listed(paths);
        self.apply_filter();
    }

    fn set_listed(&mut self, paths: Vec<PathBuf>) {
        let dir = self.dir.as_deref().unwrap_or(Path::new(""));
        self.listed = (paths.into_iter())
            .map(|path| ListedImage::new(path, dir))
            .collect();
    }

    pub fn select(&mut self, index: usize) -> usize {
        let Some(last) = self.files.len().checked_sub(1) else {
            return 0;
        };
        self.selected = index.min(last);
        self.anchor = self.current().cloned();
        self.scroll_to_selected = true;
        self.selected
    }

    pub fn remove(&mut self, path: &Path) {
        self.listed.retain(|image| image.path != path);
        if let Some(i) = self.files.iter().position(|p| p == path) {
            self.files.remove(i);
            self.tree.rebuild(&self.files, self.grouped);
            if self.selected > i || self.selected >= self.files.len() {
                self.selected = self.selected.saturating_sub(1);
            }
            self.anchor = self.current().cloned();
        }
        self.scroll_to_selected = true;
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Call [`Self::filter_changed`] after editing.
    pub fn filter_editor(&mut self) -> &mut String {
        &mut self.filter
    }

    pub fn filter_changed(&mut self) {
        self.query = Filter::parse(&self.filter);
        self.apply_filter();
    }

    pub fn apply_prefs(&mut self, prefs: &Preferences, old: &Preferences) {
        if prefs.group_by_folder != old.group_by_folder {
            self.set_grouped(prefs.group_by_folder);
        }
    }

    fn apply_filter(&mut self) {
        let anchor = self.anchor.as_deref();
        (self.files, self.selected) = (self.query).apply(&self.listed, anchor);
        if self.anchor.is_none() {
            self.anchor = self.current().cloned();
        }
        self.tree.rebuild(&self.files, self.grouped);
        self.scroll_to_selected = true;
    }

    pub fn grouped(&self) -> bool {
        self.grouped
    }

    pub fn set_grouped(&mut self, grouped: bool) {
        self.grouped = grouped;
        self.tree.rebuild(&self.files, grouped);
        self.scroll_to_selected = true;
    }

    pub fn tree(&self) -> &FolderTree {
        &self.tree
    }

    pub fn toggle_folder(&mut self, folder: &Path) {
        self.tree.toggle(folder);
    }

    /// Also opens the folders of the selected image.
    pub fn take_scroll_to_selected(&mut self) -> bool {
        let scroll = std::mem::take(&mut self.scroll_to_selected);
        if scroll && let Some(current) = self.files.get(self.selected) {
            self.tree.reveal(current);
        }
        scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(paths: &[&str]) -> FileList {
        let mut list = FileList::new(&Preferences::default());
        list.open(PathBuf::from("/r"), None);
        list.set_listing(paths.iter().map(PathBuf::from).collect());
        list
    }

    fn filter(list: &mut FileList, text: &str) {
        *list.filter_editor() = text.to_string();
        list.filter_changed();
    }

    fn current(list: &FileList) -> &str {
        list.current().and_then(|p| p.to_str()).unwrap_or_default()
    }

    #[test]
    fn the_selection_comes_back_when_the_filter_shows_it_again() {
        let mut list = list(&["/r/a1", "/r/b1", "/r/a2"]);
        list.select(1);
        filter(&mut list, "a");
        assert_eq!(current(&list), "/r/a2");
        filter(&mut list, "");
        assert_eq!(current(&list), "/r/b1");
        assert_eq!(list.selected(), 1);
    }

    #[test]
    fn opening_starts_over() {
        let mut list = list(&["/r/a", "/r/b"]);
        filter(&mut list, "b");
        list.open(PathBuf::from("/s"), Some(PathBuf::from("/s/c")));
        assert_eq!(list.filter(), "");
        assert_eq!(current(&list), "/s/c");
        list.set_listing(vec![PathBuf::from("/s/b"), PathBuf::from("/s/c")]);
        assert_eq!(list.selected(), 1);
    }

    #[test]
    fn removing_the_selected_image_selects_the_next_one() {
        let mut list = list(&["/r/1", "/r/2", "/r/3"]);
        list.select(1);
        list.remove(Path::new("/r/2"));
        assert_eq!(current(&list), "/r/3");
        list.remove(Path::new("/r/3"));
        assert_eq!(current(&list), "/r/1");
        assert!(!list.is_listed(Path::new("/r/3")));
    }
}
