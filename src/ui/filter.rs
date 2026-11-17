//! The file list's filter: space-separated terms that must all appear in an image's path
//! below the opened folder, ignoring case. `-term` excludes, and quotes keep spaces.

use std::path::{Path, PathBuf};

#[derive(Debug, Default, PartialEq)]
pub struct Filter {
    terms: Vec<Term>,
}

pub struct ListedImage {
    pub path: PathBuf,
    search_path: String,
}

impl ListedImage {
    pub fn new(path: PathBuf, root: &Path) -> Self {
        let search_path = search_path(&path, root);
        Self { path, search_path }
    }
}

#[derive(Debug, PartialEq)]
struct Term {
    text: String,
    excluded: bool,
}

impl Filter {
    pub fn parse(query: &str) -> Self {
        let mut terms = Vec::new();
        let mut chars = query.chars().peekable();
        loop {
            while chars.next_if(|c| c.is_whitespace()).is_some() {}
            if chars.peek().is_none() {
                break;
            }
            let excluded = chars.next_if_eq(&'-').is_some();
            let mut text = String::new();
            let mut quoted = false;
            while let Some(c) = chars.next_if(|c| quoted || !c.is_whitespace()) {
                match c {
                    '"' => quoted = !quoted,
                    c => text.push(c),
                }
            }
            if !text.is_empty() {
                let text = text.to_lowercase();
                terms.push(Term { text, excluded });
            }
        }
        Self { terms }
    }

    fn matches(&self, search_path: &str) -> bool {
        (self.terms.iter()).all(|t| search_path.contains(&t.text) != t.excluded)
    }

    /// Also returns the index of `anchor`, or of the next match if it is filtered out.
    pub fn apply(&self, listed: &[ListedImage], anchor: Option<&Path>) -> (Vec<PathBuf>, usize) {
        let mut files = Vec::new();
        let mut selected = None;
        for image in listed {
            if Some(image.path.as_path()) == anchor {
                selected = Some(files.len());
            }
            if self.matches(&image.search_path) {
                files.push(image.path.clone());
            }
        }
        let last = files.len().saturating_sub(1);
        (files, selected.unwrap_or(0).min(last))
    }
}

fn search_path(path: &Path, root: &Path) -> String {
    let parts = path.strip_prefix(root).unwrap_or(path).components();
    let parts: Vec<_> = parts.map(|c| c.as_os_str().to_string_lossy()).collect();
    parts.join("/").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(text: &str, excluded: bool) -> Term {
        Term {
            text: text.into(),
            excluded,
        }
    }

    fn paths(filter: &str) -> Filter {
        Filter::parse(filter)
    }

    #[test]
    fn parses_terms_exclusions_and_quotes() {
        let parsed = paths(r#"  M31 -flat "dark frames" -"bias 1"  Ha"#);
        let expected = [
            term("m31", false),
            term("flat", true),
            term("dark frames", false),
            term("bias 1", true),
            term("ha", false),
        ];
        assert_eq!(parsed.terms, expected);
    }

    #[test]
    fn quotes_make_a_leading_dash_literal_and_may_stay_open() {
        let parsed = paths(r#""-1" a"b c"d "open end"#);
        let expected = [
            term("-1", false),
            term("ab cd", false),
            term("open end", false),
        ];
        assert_eq!(parsed.terms, expected);
    }

    #[test]
    fn empty_terms_are_ignored() {
        assert!(paths(r#" - "" -"" "#).terms.is_empty());
    }

    #[test]
    fn every_term_must_match_folders_too() {
        let filter = paths("light -flat");
        let matches = |path: &str| filter.matches(&search_path(Path::new(path), Path::new("")));
        assert!(matches("2026-11-17/LIGHT_Ha_001.fits"));
        assert!(matches("lights/m31_001.fits"));
        assert!(!matches("lights/flat_001.fits"));
        assert!(!matches("flats/light_001.fits"));
        assert!(!matches("darks/dark_001.fits"));
    }

    #[test]
    fn matches_paths_below_the_root() {
        let root = Path::new("/data/M31");
        let path = search_path(&root.join("Lights").join("1.fits"), root);
        assert_eq!(path, "lights/1.fits");
        assert!(paths("LIGHTS/").matches(&path));
        assert!(!paths("m31").matches(&path));
    }

    fn listing(paths: &[&str]) -> Vec<ListedImage> {
        (paths.iter())
            .map(|p| ListedImage::new(PathBuf::from(p), Path::new("/r")))
            .collect()
    }

    #[test]
    fn keeps_the_anchor_or_moves_to_the_next_match() {
        let listed = listing(&["/r/a1", "/r/b1", "/r/a2", "/r/b2"]);
        let a = paths("a");
        let at = |filter: &Filter, anchor: &str| {
            let anchor = Some(Path::new(anchor));
            let (files, selected) = filter.apply(&listed, anchor);
            (files[selected].clone(), files.len())
        };
        assert_eq!(at(&a, "/r/a2"), (PathBuf::from("/r/a2"), 2));
        assert_eq!(at(&a, "/r/b1"), (PathBuf::from("/r/a2"), 2));
        assert_eq!(at(&a, "/r/b2"), (PathBuf::from("/r/a2"), 2));
        assert_eq!(at(&Filter::default(), "/r/b2"), (PathBuf::from("/r/b2"), 4));
        assert_eq!(a.apply(&listed, None).1, 0);
        assert_eq!(paths("x").apply(&listed, None), (Vec::new(), 0));
    }
}
