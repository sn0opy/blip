//! The file list's filter: space-separated terms that must all appear in an image's path
//! below the opened folder, ignoring case. `-term` excludes, and quotes keep spaces.
//! With header search, `KEYWORD:value` terms look for the value in that header entry.

use super::headers::HeaderIndex;
use blip_core::image::Header;
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
    key: Option<String>,
    text: String,
    excluded: bool,
}

impl Filter {
    pub fn parse(query: &str, header_search: bool) -> Self {
        let mut terms = Vec::new();
        let mut chars = query.chars().peekable();
        loop {
            while chars.next_if(|c| c.is_whitespace()).is_some() {}
            if chars.peek().is_none() {
                break;
            }
            let excluded = chars.next_if_eq(&'-').is_some();
            let mut key = None;
            let mut text = String::new();
            let mut quoted = false;
            while let Some(c) = chars.next_if(|c| quoted || !c.is_whitespace()) {
                match c {
                    '"' => quoted = !quoted,
                    ':' if header_search && !quoted && key.is_none() && !text.is_empty() => {
                        key = Some(std::mem::take(&mut text));
                    }
                    c => text.push(c),
                }
            }
            if !text.is_empty() || key.is_some() {
                let text = text.to_lowercase();
                terms.push(Term {
                    key,
                    text,
                    excluded,
                });
            }
        }
        Self { terms }
    }

    pub fn needs_headers(&self) -> bool {
        self.terms.iter().any(|t| t.key.is_some())
    }

    fn matches(&self, search_path: &str, header: Option<&Header>) -> bool {
        self.terms.iter().all(|t| {
            let found = match (&t.key, header) {
                (None, _) => search_path.contains(&t.text),
                (Some(_), None) => return false,
                (Some(key), Some(header)) => (header.iter()).any(|(k, v)| {
                    k.eq_ignore_ascii_case(key) && v.to_lowercase().contains(&t.text)
                }),
            };
            found != t.excluded
        })
    }

    /// Also returns the index of `anchor`, or of the next match if it is filtered out.
    pub fn apply(
        &self,
        listed: &[ListedImage],
        anchor: Option<&Path>,
        headers: &HeaderIndex,
    ) -> (Vec<PathBuf>, usize) {
        let needs_headers = self.needs_headers();
        let mut files = Vec::new();
        let mut selected = None;
        for image in listed {
            if Some(image.path.as_path()) == anchor {
                selected = Some(files.len());
            }
            let header = needs_headers.then(|| headers.get(&image.path)).flatten();
            if self.matches(&image.search_path, header) {
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
            key: None,
            text: text.into(),
            excluded,
        }
    }

    fn header_term(key: &str, text: &str, excluded: bool) -> Term {
        Term {
            key: Some(key.into()),
            ..term(text, excluded)
        }
    }

    fn paths(filter: &str) -> Filter {
        Filter::parse(filter, false)
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
    fn parses_header_terms_only_with_header_search() {
        let query = r#"Filter:Ha -object:"M 31" imagetyp: "a:b" :x light"#;
        let parsed = Filter::parse(query, true);
        let expected = [
            header_term("Filter", "ha", false),
            header_term("object", "m 31", true),
            header_term("imagetyp", "", false),
            term("a:b", false),
            term(":x", false),
            term("light", false),
        ];
        assert_eq!(parsed.terms, expected);
        assert!(parsed.needs_headers());
        assert!(!paths(query).needs_headers());
        assert_eq!(paths("filter:ha").terms, [term("filter:ha", false)]);
    }

    #[test]
    fn every_term_must_match_folders_too() {
        let filter = paths("light -flat");
        let matches =
            |path: &str| filter.matches(&search_path(Path::new(path), Path::new("")), None);
        assert!(matches("2026-11-17/LIGHT_Ha_001.fits"));
        assert!(matches("lights/m31_001.fits"));
        assert!(!matches("lights/flat_001.fits"));
        assert!(!matches("flats/light_001.fits"));
        assert!(!matches("darks/dark_001.fits"));
    }

    #[test]
    fn header_terms_look_into_the_keyword_only() {
        let header: Header = [("FILTER", "Ha"), ("OBJECT", "M 31"), ("IMAGETYP", "LIGHT")]
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .into();
        let matches = |query: &str| Filter::parse(query, true).matches("1.fits", Some(&header));
        assert!(matches("filter:ha object:31 imagetyp:"));
        assert!(matches("-filter:oiii -bayerpat:"));
        assert!(!matches("filter:oiii"));
        assert!(!matches("object:ha"));
        assert!(!matches("bayerpat:"));
        assert!(!matches("-imagetyp:light"));
        let unknown = Filter::parse("-filter:oiii", true);
        assert!(!unknown.matches("1.fits", None));
    }

    #[test]
    fn matches_paths_below_the_root() {
        let root = Path::new("/data/M31");
        let path = search_path(&root.join("Lights").join("1.fits"), root);
        assert_eq!(path, "lights/1.fits");
        assert!(paths("LIGHTS/").matches(&path, None));
        assert!(!paths("m31").matches(&path, None));
    }

    fn listing(paths: &[&str]) -> Vec<ListedImage> {
        (paths.iter())
            .map(|p| ListedImage::new(PathBuf::from(p), Path::new("/r")))
            .collect()
    }

    #[test]
    fn keeps_the_anchor_or_moves_to_the_next_match() {
        let listed = listing(&["/r/a1", "/r/b1", "/r/a2", "/r/b2"]);
        let headers = HeaderIndex::default();
        let a = paths("a");
        let at = |filter: &Filter, anchor: &str| {
            let anchor = Some(Path::new(anchor));
            let (files, selected) = filter.apply(&listed, anchor, &headers);
            (files[selected].clone(), files.len())
        };
        assert_eq!(at(&a, "/r/a2"), (PathBuf::from("/r/a2"), 2));
        assert_eq!(at(&a, "/r/b1"), (PathBuf::from("/r/a2"), 2));
        assert_eq!(at(&a, "/r/b2"), (PathBuf::from("/r/a2"), 2));
        assert_eq!(at(&Filter::default(), "/r/b2"), (PathBuf::from("/r/b2"), 4));
        assert_eq!(a.apply(&listed, None, &headers).1, 0);
        assert_eq!(paths("x").apply(&listed, None, &headers), (Vec::new(), 0));
    }

    #[test]
    fn header_terms_skip_files_whose_header_is_unknown() {
        let listed = listing(&["/r/1", "/r/2", "/r/3"]);
        let path = |i: usize| listed[i].path.clone();
        let mut headers = HeaderIndex::default();
        let ha = vec![("FILTER".to_string(), "Ha".to_string())];
        headers.insert_scanned(path(2), Some(ha));
        headers.insert_scanned(path(1), Some(Header::new()));
        let filter = Filter::parse("filter:ha", true);
        let anchor = Some(listed[0].path.as_path());
        assert_eq!(filter.apply(&listed, anchor, &headers), (vec![path(2)], 0));
        let unknown = Filter::parse("-filter:oiii", true);
        assert_eq!(
            unknown.apply(&listed, anchor, &headers).0,
            [path(1), path(2)]
        );
    }
}
