//! The images below the opened folder, narrowed down by the filter.

use super::filter::{Filter, ListedImage};
use super::headers::{HeaderIndex, HeaderScanner, Progress};
use super::prefs::Preferences;
use super::tree::{FolderTree, Row};
use blip_core::image::Header;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct FileList {
    dir: Option<PathBuf>,
    listed: Vec<ListedImage>,
    files: Vec<PathBuf>,
    rejected: HashSet<PathBuf>,
    selected: usize,
    /// The image last selected, kept while the filter hides it.
    anchor: Option<PathBuf>,
    filter: String,
    query: Filter,
    header_search: bool,
    scan_headers: bool,
    headers: HeaderIndex,
    scanner: HeaderScanner,
    grouped: bool,
    tree: FolderTree,
    top_row: Option<PathBuf>,
    scroll_rows: isize,
    scroll_to_selected: bool,
}

impl FileList {
    pub fn new(scanner: HeaderScanner, prefs: &Preferences) -> Self {
        Self {
            dir: None,
            listed: Vec::new(),
            files: Vec::new(),
            rejected: HashSet::new(),
            selected: 0,
            anchor: None,
            filter: String::new(),
            query: Filter::default(),
            header_search: prefs.header_search,
            scan_headers: prefs.scan_headers,
            headers: HeaderIndex::default(),
            scanner,
            grouped: prefs.group_by_folder,
            tree: FolderTree::default(),
            top_row: None,
            scroll_rows: 0,
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
        self.scanner.cancel();
        self.headers.clear();
        self.rejected.clear();
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
        let listed: HashSet<&Path> = (self.listed.iter())
            .map(|image| image.path.as_path())
            .collect();
        self.headers.retain(|p| listed.contains(p));
        self.rejected.retain(|p| listed.contains(p.as_path()));
        self.headers.forget_failures();
        self.apply_filter();
        self.update_header_scan(true);
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
        if self.scanner.running() {
            self.update_header_scan(true);
        }
        self.selected
    }

    /// The selection moves on to the next remaining image.
    pub fn remove(&mut self, paths: &[PathBuf]) {
        let gone: HashSet<&Path> = paths.iter().map(PathBuf::as_path).collect();
        self.listed
            .retain(|image| !gone.contains(image.path.as_path()));
        for path in &gone {
            self.headers.remove(path);
            self.rejected.remove(*path);
        }
        let shown = self.files.len();
        let kept_before_selected = (self.files.iter().take(self.selected))
            .filter(|p| !gone.contains(p.as_path()))
            .count();
        self.files.retain(|p| !gone.contains(p.as_path()));
        if self.files.len() != shown {
            self.selected = kept_before_selected.min(self.files.len().saturating_sub(1));
            self.anchor = self.current().cloned();
            self.tree.rebuild(&self.files, self.grouped);
        }
        self.scroll_to_selected = true;
    }

    pub fn is_rejected(&self, path: &Path) -> bool {
        self.rejected.contains(path)
    }

    /// Returns whether the image is rejected now.
    pub fn toggle_rejected(&mut self, path: &Path) -> bool {
        let rejected = !self.rejected.remove(path);
        if rejected {
            self.rejected.insert(path.to_path_buf());
        }
        rejected
    }

    pub fn clear_rejected(&mut self) {
        self.rejected.clear();
    }

    /// Including the ones the filter hides.
    pub fn rejected_count(&self) -> usize {
        self.rejected.len()
    }

    pub fn rejected_in(&self, folder: &Path) -> usize {
        (self.rejected.iter())
            .filter(|p| p.starts_with(folder))
            .count()
    }

    /// In listing order, including the ones the filter hides.
    pub fn rejected(&self) -> Vec<PathBuf> {
        (self.listed.iter())
            .map(|image| &image.path)
            .filter(|p| self.rejected.contains(*p))
            .cloned()
            .collect()
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Call [`Self::filter_changed`] after editing.
    pub fn filter_editor(&mut self) -> (&mut String, &mut HeaderIndex) {
        (&mut self.filter, &mut self.headers)
    }

    pub fn filter_changed(&mut self) {
        self.query = Filter::parse(&self.filter, self.header_search);
        self.apply_filter();
        self.update_header_scan(false);
    }

    pub fn header_search(&self) -> bool {
        self.header_search
    }

    /// Returns whether the files changed.
    pub fn apply_prefs(&mut self, prefs: &Preferences, old: &Preferences) -> bool {
        if prefs.group_by_folder != old.group_by_folder {
            self.set_grouped(prefs.group_by_folder);
        }
        let header_search = (prefs.header_search, prefs.scan_headers);
        if header_search == (old.header_search, old.scan_headers) {
            return false;
        }
        (self.header_search, self.scan_headers) = header_search;
        self.filter_changed();
        true
    }

    pub fn header_count(&self) -> usize {
        self.headers.len()
    }

    pub fn scanning(&self) -> bool {
        self.scanner.running()
    }

    /// With `requeue`, this replaces what a running scan still has to read.
    fn update_header_scan(&mut self, requeue: bool) {
        let wanted = self.header_search && (self.scan_headers || self.query.needs_headers());
        if !wanted {
            self.scanner.cancel();
        } else if requeue || !self.scanner.running() {
            let order = self.listed_from_anchor();
            self.scanner.request(self.headers.missing(order));
        }
    }

    fn listed_from_anchor(&self) -> impl Iterator<Item = &PathBuf> {
        let anchor = self.anchor.as_ref();
        let start = (self.listed.iter()).position(|image| Some(&image.path) == anchor);
        let (before, after) = self.listed.split_at(start.unwrap_or(0));
        after.iter().chain(before).map(|image| &image.path)
    }

    /// Also returns whether the files changed.
    pub fn poll_headers(&mut self) -> Option<(Progress, bool)> {
        let batch = self.scanner.take()?;
        let changed = self.add_headers(batch.headers);
        Some((batch.progress, changed))
    }

    fn add_headers(&mut self, headers: Vec<(PathBuf, Option<Header>)>) -> bool {
        let mut added = false;
        for (path, header) in headers {
            added |= self.headers.insert_scanned(path, header);
        }
        added && self.refilter_for_headers()
    }

    /// Returns whether the files changed.
    pub fn add_loaded_header(&mut self, path: PathBuf, header: Header) -> bool {
        self.headers.insert_loaded(path, header) && self.refilter_for_headers()
    }

    fn refilter_for_headers(&mut self) -> bool {
        if !self.query.needs_headers() {
            return false;
        }
        let top = self.top_row.take();
        let before = top.as_deref().and_then(|p| self.visible_row(p));
        self.refilter();
        let after = top.as_deref().and_then(|p| self.visible_row(p));
        if let (Some(before), Some(after)) = (before, after) {
            self.scroll_rows += after as isize - before as isize;
        }
        self.top_row = top;
        true
    }

    fn apply_filter(&mut self) {
        self.refilter();
        self.scroll_to_selected = true;
    }

    fn refilter(&mut self) {
        let anchor = self.anchor.as_deref();
        (self.files, self.selected) = (self.query).apply(&self.listed, anchor, &self.headers);
        if self.anchor.is_none() {
            self.anchor = self.current().cloned();
        }
        self.tree.rebuild(&self.files, self.grouped);
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

    pub fn row_path<'a>(&'a self, row: &'a Row) -> &'a Path {
        match row {
            Row::Folder { path, .. } => path,
            Row::Image { index, .. } => &self.files[*index],
        }
    }

    fn visible_row(&self, path: &Path) -> Option<usize> {
        (self.tree.visible_rows().iter()).position(|row| self.row_path(row) == path)
    }

    pub fn set_top_row(&mut self, path: Option<PathBuf>) {
        self.top_row = path;
    }

    pub fn take_scroll_rows(&mut self) -> isize {
        std::mem::take(&mut self.scroll_rows)
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
        let scanner = HeaderScanner::with_readers(None, 0, || {});
        let prefs = Preferences {
            header_search: true,
            ..Preferences::default()
        };
        let mut list = FileList::new(scanner, &prefs);
        list.open(PathBuf::from("/r"), None);
        list.set_listing(paths.iter().map(PathBuf::from).collect());
        list
    }

    fn filter(list: &mut FileList, text: &str) {
        *list.filter_editor().0 = text.to_string();
        list.filter_changed();
    }

    fn current(list: &FileList) -> &str {
        list.current().and_then(|p| p.to_str()).unwrap_or_default()
    }

    fn filter_header(value: &str) -> Option<Header> {
        Some(vec![("FILTER".to_string(), value.to_string())])
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
        filter(&mut list, "b filter:x");
        list.add_headers(vec![(PathBuf::from("/r/a"), filter_header("x"))]);
        assert_eq!(list.header_count(), 1);
        list.open(PathBuf::from("/s"), Some(PathBuf::from("/s/c")));
        assert_eq!((list.filter(), list.header_count()), ("", 0));
        assert_eq!(current(&list), "/s/c");
        list.set_listing(vec![PathBuf::from("/s/b"), PathBuf::from("/s/c")]);
        assert_eq!(list.selected(), 1);
    }

    #[test]
    fn headers_coming_in_keep_the_top_row_in_place() {
        let mut list = list(&["/r/1", "/r/2", "/r/3", "/r/4"]);
        list.select(3);
        filter(&mut list, "filter:ha");
        list.add_headers(vec![(PathBuf::from("/r/4"), filter_header("Ha"))]);
        list.take_scroll_rows();
        list.set_top_row(Some(PathBuf::from("/r/4")));

        let ha = ["/r/1", "/r/3"].map(|p| (PathBuf::from(p), filter_header("Ha")));
        assert!(list.add_headers(ha.into()));
        assert_eq!(list.files().len(), 3);
        assert_eq!(list.take_scroll_rows(), 2);
        assert!(!list.add_headers(vec![(PathBuf::from("/r/2"), None)]));
        assert_eq!(current(&list), "/r/4");
        assert!(list.take_scroll_to_selected());
        list.add_headers(vec![(PathBuf::from("/r/2"), filter_header("Ha"))]);
        assert!(!list.take_scroll_to_selected());
    }

    #[test]
    fn loaded_headers_count_even_when_a_read_failed() {
        let mut list = list(&["/r/1"]);
        filter(&mut list, "filter:ha");
        list.add_headers(vec![(PathBuf::from("/r/1"), None)]);
        assert!(list.files().is_empty());
        let loaded = filter_header("Ha").unwrap_or_default();
        assert!(list.add_loaded_header(PathBuf::from("/r/1"), loaded));
        assert_eq!(current(&list), "/r/1");
    }

    #[test]
    fn headers_are_read_from_the_selected_image_on() {
        let mut list = list(&["/r/a", "/r/b", "/r/c"]);
        let order =
            |list: &FileList| -> Vec<PathBuf> { list.listed_from_anchor().cloned().collect() };
        list.select(1);
        assert_eq!(order(&list), ["/r/b", "/r/c", "/r/a"].map(PathBuf::from));
        list.open(PathBuf::from("/r"), Some(PathBuf::from("/r/gone")));
        list.set_listing(["/r/a", "/r/b"].map(PathBuf::from).into());
        assert_eq!(order(&list), ["/r/a", "/r/b"].map(PathBuf::from));
    }

    #[test]
    fn removing_the_selected_image_selects_the_next_one() {
        let mut list = list(&["/r/1", "/r/2", "/r/3"]);
        list.select(1);
        list.remove(&[PathBuf::from("/r/2")]);
        assert_eq!(current(&list), "/r/3");
        list.remove(&[PathBuf::from("/r/3")]);
        assert_eq!(current(&list), "/r/1");
        assert!(!list.is_listed(Path::new("/r/3")));
    }

    #[test]
    fn removing_several_images_selects_the_next_remaining_one() {
        let mut list = list(&["/r/1", "/r/2", "/r/3", "/r/4", "/r/5"]);
        list.select(2);
        list.remove(&["/r/1", "/r/3", "/r/4"].map(PathBuf::from));
        assert_eq!(current(&list), "/r/5");
        assert_eq!(list.files(), ["/r/2", "/r/5"].map(PathBuf::from));
    }

    #[test]
    fn removing_a_hidden_image_keeps_the_selection() {
        let mut list = list(&["/r/a1", "/r/b1", "/r/a2"]);
        list.select(2);
        filter(&mut list, "a");
        list.remove(&[PathBuf::from("/r/b1")]);
        assert_eq!(current(&list), "/r/a2");
        filter(&mut list, "");
        assert_eq!(list.files(), ["/r/a1", "/r/a2"].map(PathBuf::from));
    }

    #[test]
    fn rejections_last_until_another_folder_is_opened() {
        let mut list = list(&["/r/a/1", "/r/a/2", "/r/b/3"]);
        assert!(list.toggle_rejected(Path::new("/r/b/3")));
        assert!(list.toggle_rejected(Path::new("/r/a/1")));
        assert!(list.toggle_rejected(Path::new("/r/a/2")));
        assert!(!list.toggle_rejected(Path::new("/r/a/2")));
        filter(&mut list, "b");
        assert_eq!(list.rejected(), ["/r/a/1", "/r/b/3"].map(PathBuf::from));
        assert_eq!(list.rejected_in(Path::new("/r/a")), 1);

        list.set_listing(["/r/a/1", "/r/a/2"].map(PathBuf::from).into());
        assert_eq!(list.rejected(), [PathBuf::from("/r/a/1")]);
        list.remove(&[PathBuf::from("/r/a/1")]);
        assert_eq!(list.rejected_count(), 0);

        list.toggle_rejected(Path::new("/r/a/2"));
        list.open(PathBuf::from("/r"), None);
        assert_eq!(list.rejected_count(), 0);
    }
}
