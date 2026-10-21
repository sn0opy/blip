//! The sidebar's folder tree, derived from the list of images below the opened folder.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Folder { path: PathBuf, depth: usize },
    Image { index: usize, depth: usize },
}

impl Row {
    pub fn depth(&self) -> usize {
        match self {
            Row::Folder { depth, .. } | Row::Image { depth, .. } => *depth,
        }
    }
}

/// Folders start expanded and stay collapsed across rescans of the same root.
#[derive(Default)]
pub struct FolderTree {
    root: PathBuf,
    rows: Vec<Row>,
    collapsed: HashSet<PathBuf>,
}

impl FolderTree {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            ..Self::default()
        }
    }

    /// The images of each folder must be listed together, as `list_images` does.
    pub fn rebuild(&mut self, images: &[PathBuf]) {
        self.rows.clear();
        let mut previous_folders: Vec<&Path> = Vec::new();
        for (index, image) in images.iter().enumerate() {
            let folders = self.folders_between_root_and(image);
            let shared = (previous_folders.iter().zip(&folders))
                .take_while(|(a, b)| a == b)
                .count();
            for (depth, folder) in folders.iter().enumerate().skip(shared) {
                let path = folder.to_path_buf();
                self.rows.push(Row::Folder { path, depth });
            }
            let depth = folders.len();
            self.rows.push(Row::Image { index, depth });
            previous_folders = folders;
        }
    }

    /// Outermost first, without the root itself.
    fn folders_between_root_and<'a>(&self, image: &'a Path) -> Vec<&'a Path> {
        let mut folders: Vec<&Path> = (image.ancestors().skip(1))
            .take_while(|folder| *folder != self.root && folder.starts_with(&self.root))
            .collect();
        folders.reverse();
        folders
    }

    pub fn visible_rows(&self) -> Vec<&Row> {
        let mut collapsed_folder_depth = None;
        (self.rows.iter())
            .filter(|row| {
                if collapsed_folder_depth.is_some_and(|depth| row.depth() > depth) {
                    return false;
                }
                collapsed_folder_depth = match row {
                    Row::Folder { path, depth } if self.collapsed.contains(path) => Some(*depth),
                    _ => None,
                };
                true
            })
            .collect()
    }

    pub fn is_collapsed(&self, folder: &Path) -> bool {
        self.collapsed.contains(folder)
    }

    pub fn toggle(&mut self, folder: &Path) {
        if !self.collapsed.remove(folder) {
            self.collapsed.insert(folder.to_path_buf());
        }
    }

    pub fn reveal(&mut self, image: &Path) {
        self.collapsed.retain(|folder| !image.starts_with(folder));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(images: &[&str]) -> (FolderTree, Vec<PathBuf>) {
        let images: Vec<PathBuf> = images.iter().map(PathBuf::from).collect();
        let mut tree = FolderTree::new(Path::new("/root"));
        tree.rebuild(&images);
        (tree, images)
    }

    fn folder(path: &str, depth: usize) -> Row {
        Row::Folder {
            path: PathBuf::from(path),
            depth,
        }
    }

    fn image(index: usize, depth: usize) -> Row {
        Row::Image { index, depth }
    }

    #[test]
    fn folders_appear_once_before_their_images() {
        let (tree, _) = tree(&[
            "/root/a/b/1.fits",
            "/root/a/b/2.fits",
            "/root/a/3.fits",
            "/root/c/4.fits",
            "/root/5.fits",
        ]);
        let expected = [
            folder("/root/a", 0),
            folder("/root/a/b", 1),
            image(0, 2),
            image(1, 2),
            image(2, 1),
            folder("/root/c", 0),
            image(3, 1),
            image(4, 0),
        ];
        assert_eq!(tree.visible_rows(), expected.iter().collect::<Vec<_>>());
    }

    #[test]
    fn collapsed_folders_hide_their_contents_until_revealed() {
        let (mut tree, images) = tree(&["/root/a/b/1.fits", "/root/a/2.fits", "/root/3.fits"]);
        tree.toggle(Path::new("/root/a/b"));
        tree.toggle(Path::new("/root/a"));
        let expected = [folder("/root/a", 0), image(2, 0)];
        assert_eq!(tree.visible_rows(), expected.iter().collect::<Vec<_>>());

        tree.reveal(&images[0]);
        assert_eq!(tree.visible_rows().len(), 5);
    }
}
