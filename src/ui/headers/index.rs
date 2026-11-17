use blip_core::image::Header;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

const MAX_VALUE_SUGGESTIONS: usize = 50;

#[derive(Default)]
pub struct HeaderIndex {
    headers: HashMap<PathBuf, Header>,
    failed: HashSet<PathBuf>,
    files_with_keyword: BTreeMap<String, usize>,
    version: u64,
    memo: Option<(String, u64, Vec<Completion>)>,
    /// Kept while its value is typed.
    keyword_values: Option<KeywordValues>,
}

struct KeywordValues {
    keyword: String,
    version: u64,
    values: Vec<Value>,
}

struct Value {
    text: String,
    lowercase: String,
    file_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    pub insert: String,
    pub label: String,
    pub file_count: usize,
}

impl HeaderIndex {
    pub fn get(&self, path: &Path) -> Option<&Header> {
        self.headers.get(path)
    }

    pub fn len(&self) -> usize {
        self.headers.len()
    }

    /// Returns whether it is new.
    pub fn insert_scanned(&mut self, path: PathBuf, header: Option<Header>) -> bool {
        let Some(header) = header else {
            self.failed.insert(path);
            return false;
        };
        if self.headers.contains_key(&path) {
            return false;
        }
        count_keywords(&mut self.files_with_keyword, &header, true);
        self.headers.insert(path, header);
        self.version += 1;
        true
    }

    /// Replaces a scanned header, which lacks the keywords of FITS extension images.
    /// Returns whether it changed.
    pub fn insert_loaded(&mut self, path: PathBuf, header: Header) -> bool {
        self.failed.remove(&path);
        if self.headers.get(&path) == Some(&header) {
            return false;
        }
        self.remove(&path);
        count_keywords(&mut self.files_with_keyword, &header, true);
        self.headers.insert(path, header);
        self.version += 1;
        true
    }

    pub fn remove(&mut self, path: &Path) {
        self.failed.remove(path);
        if let Some(old) = self.headers.remove(path) {
            count_keywords(&mut self.files_with_keyword, &old, false);
            self.version += 1;
        }
    }

    pub fn clear(&mut self) {
        self.headers.clear();
        self.failed.clear();
        self.files_with_keyword.clear();
        self.version += 1;
    }

    pub fn forget_failures(&mut self) {
        self.failed.clear();
    }

    pub fn retain(&mut self, keep: impl Fn(&Path) -> bool) {
        self.failed.retain(|p| keep(p));
        let gone: Vec<PathBuf> = (self.headers.keys())
            .filter(|p| !keep(p))
            .cloned()
            .collect();
        for path in gone {
            self.remove(&path);
        }
    }

    pub fn missing<'a>(&self, paths: impl IntoIterator<Item = &'a PathBuf>) -> Vec<PathBuf> {
        (paths.into_iter())
            .filter(|p| !self.headers.contains_key(*p) && !self.failed.contains(*p))
            .cloned()
            .collect()
    }

    /// Keywords starting with `word`, or after `KEYWORD:` that keyword's values containing
    /// the rest, most common first. A leading `-` is kept.
    pub fn complete(&mut self, word: &str) -> &[Completion] {
        let fresh = |(w, v, _): &(String, u64, _)| w == word && *v == self.version;
        if !self.memo.as_ref().is_some_and(fresh) {
            let completions = self.completions(word);
            self.memo = Some((word.to_string(), self.version, completions));
        }
        self.memo.as_ref().map_or(&[], |(_, _, c)| c)
    }

    fn completions(&mut self, word: &str) -> Vec<Completion> {
        let (sign, word) = match word.strip_prefix('-') {
            Some(w) => ("-", w),
            None => ("", word),
        };
        if word.starts_with('"') {
            return Vec::new();
        }
        let Some((key, value)) = word.split_once(':') else {
            if word.chars().count() < 2 {
                return Vec::new();
            }
            let prefix = word.to_lowercase();
            let mut keywords: Vec<Completion> = (self.files_with_keyword.iter())
                .filter(|(k, _)| k.to_lowercase().starts_with(&prefix))
                .map(|(k, &file_count)| Completion {
                    insert: format!("{sign}{k}:"),
                    label: k.clone(),
                    file_count,
                })
                .collect();
            keywords.sort_by_key(|c| std::cmp::Reverse(c.file_count));
            return keywords;
        };
        let wanted = value.to_lowercase();
        (self.values_of(key).iter())
            .filter(|v| v.lowercase.contains(&wanted))
            .take(MAX_VALUE_SUGGESTIONS)
            .map(|v| {
                let quoted = if v.text.contains(char::is_whitespace) {
                    format!("\"{}\"", v.text)
                } else {
                    v.text.clone()
                };
                Completion {
                    insert: format!("{sign}{key}:{quoted} "),
                    label: v.text.clone(),
                    file_count: v.file_count,
                }
            })
            .collect()
    }

    fn values_of(&mut self, keyword: &str) -> &[Value] {
        let fresh = |c: &KeywordValues| {
            c.version == self.version && c.keyword.eq_ignore_ascii_case(keyword)
        };
        if !self.keyword_values.as_ref().is_some_and(fresh) {
            let mut file_counts: HashMap<&str, usize> = HashMap::new();
            for header in self.headers.values() {
                let mut seen: Vec<&str> = Vec::new();
                for (_, v) in header
                    .iter()
                    .filter(|(k, _)| k.eq_ignore_ascii_case(keyword))
                {
                    if !v.is_empty() && !seen.contains(&v.as_str()) {
                        seen.push(v);
                        *file_counts.entry(v).or_default() += 1;
                    }
                }
            }
            let mut values: Vec<Value> = (file_counts.into_iter())
                .map(|(text, file_count)| Value {
                    text: text.to_string(),
                    lowercase: text.to_lowercase(),
                    file_count,
                })
                .collect();
            values.sort_by(|a, b| {
                b.file_count
                    .cmp(&a.file_count)
                    .then_with(|| a.text.cmp(&b.text))
            });
            self.keyword_values = Some(KeywordValues {
                keyword: keyword.to_string(),
                version: self.version,
                values,
            });
        }
        self.keyword_values.as_ref().map_or(&[], |c| &c.values)
    }
}

fn count_keywords(keywords: &mut BTreeMap<String, usize>, header: &Header, add: bool) {
    let mut seen: Vec<&str> = Vec::new();
    for (k, _) in header {
        if seen.contains(&k.as_str()) {
            continue;
        }
        seen.push(k);
        if add {
            *keywords.entry(k.clone()).or_default() += 1;
        } else if let Some(n) = keywords.get_mut(k) {
            *n -= 1;
            if *n == 0 {
                keywords.remove(k);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(cards: &[(&str, &str)]) -> Header {
        (cards.iter())
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn index() -> HeaderIndex {
        let mut index = HeaderIndex::default();
        let files = [
            [("FILTER", "Ha"), ("OBJECT", "M 31"), ("FOCPOS", "100")],
            [("FILTER", "Ha"), ("OBJECT", "M 31"), ("FOCPOS", "101")],
            [("FILTER", "OIII"), ("OBJECT", "M 33"), ("FOCPOS", "102")],
        ];
        for (i, cards) in files.iter().enumerate() {
            index.insert_scanned(PathBuf::from(format!("{i}.fits")), Some(header(cards)));
        }
        index
    }

    fn inserts(completions: &[Completion]) -> Vec<&str> {
        completions.iter().map(|c| c.insert.as_str()).collect()
    }

    #[test]
    fn completes_keywords_and_their_values() {
        let mut index = index();
        assert_eq!(inserts(index.complete("f")), Vec::<&str>::new());
        assert_eq!(inserts(index.complete("fi")), ["FILTER:"]);
        assert_eq!(inserts(index.complete("-fo")), ["-FOCPOS:"]);
        assert_eq!(
            inserts(index.complete("filter:")),
            ["filter:Ha ", "filter:OIII "]
        );
        assert_eq!(index.complete("filter:")[0].file_count, 2);
        assert_eq!(inserts(index.complete("-filter:i")), ["-filter:OIII "]);
        assert_eq!(inserts(index.complete("object:31")), ["object:\"M 31\" "]);
        assert!(index.complete("\"fi").is_empty());
    }

    #[test]
    fn typing_a_value_reuses_the_keywords_values() {
        let mut index = index();
        index.complete("filter:");
        index.headers.clear();
        assert_eq!(inserts(index.complete("FILTER:o")), ["FILTER:OIII "]);
        assert!(index.complete("object:").is_empty());
    }

    #[test]
    fn completions_follow_changes() {
        let mut index = index();
        assert_eq!(index.complete("filter:s").len(), 0);
        let sii = Some(header(&[("FILTER", "SII")]));
        index.insert_scanned(PathBuf::from("3.fits"), sii);
        assert_eq!(inserts(index.complete("filter:s")), ["filter:SII "]);
        index.retain(|p| p != Path::new("3.fits"));
        assert_eq!(index.complete("filter:s").len(), 0);
        assert_eq!(index.files_with_keyword["FILTER"], 3);
    }

    #[test]
    fn loaded_headers_replace_scanned_ones() {
        let mut index = index();
        let path = PathBuf::from("0.fits");
        assert!(!index.insert_scanned(path.clone(), Some(Header::new())));
        assert_eq!(index.get(&path).map(Vec::len), Some(3));
        let loaded = header(&[("FILTER", "SII"), ("BAYERPAT", "RGGB")]);
        assert!(index.insert_loaded(path.clone(), loaded.clone()));
        assert!(!index.insert_loaded(path.clone(), loaded));
        assert_eq!(index.files_with_keyword["FILTER"], 3);
        assert_eq!(index.files_with_keyword["FOCPOS"], 2);
        assert_eq!(index.files_with_keyword["BAYERPAT"], 1);
        assert_eq!(inserts(index.complete("filter:s")), ["filter:SII "]);
    }

    #[test]
    fn failed_reads_wait_until_forgotten_or_loaded() {
        let mut index = index();
        let [known, broken, new] = ["0.fits", "5.fits", "6.fits"].map(PathBuf::from);
        assert!(!index.insert_scanned(broken.clone(), None));
        assert_eq!(index.get(&broken), None);
        let listed = [known, broken.clone(), new.clone()];
        assert_eq!(index.missing(&listed), std::slice::from_ref(&new));
        index.forget_failures();
        assert_eq!(index.missing(&listed), [broken.clone(), new.clone()]);
        index.insert_scanned(broken.clone(), None);
        index.insert_loaded(broken, Header::new());
        assert_eq!(index.missing(&listed), [new]);
    }
}
