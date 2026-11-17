//! Rendered images kept on the GPU, and the files that failed to load.

use super::gpu::{MemoryBudget, MemoryQuery};
use super::loader::Loaded;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const CACHED_IMAGES: usize = 10;
const CACHED_FULL_RES_WITHOUT_BUDGET: usize = 4;
const MIN_CACHED_IMAGES: usize = 2;

struct CacheEntry {
    loaded: Loaded,
    last_used: u64,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum EvictionOrder {
    OutsideWindow { least_recently_used: u64 },
    InWindow { lowest_priority: Reverse<usize> },
}

pub struct ImageCache {
    entries: HashMap<PathBuf, CacheEntry>,
    errors: HashMap<PathBuf, String>,
    /// The shown image while a settings change re-renders it (avoids a black flash).
    stale: Option<(PathBuf, CacheEntry)>,
    tick: u64,
    gpu_memory: Option<MemoryQuery>,
    memory_budget: Option<MemoryBudget>,
    /// Largest image since the last settings change.
    image_bytes: usize,
    /// Set once an upload ran out of GPU memory.
    memory_cap: Option<usize>,
}

impl ImageCache {
    pub fn new(gpu_memory: Option<MemoryQuery>) -> Self {
        Self {
            entries: HashMap::new(),
            errors: HashMap::new(),
            stale: None,
            tick: 0,
            memory_budget: gpu_memory.as_ref().and_then(MemoryQuery::budget),
            gpu_memory,
            image_bytes: 0,
            memory_cap: None,
        }
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.entries.contains_key(path)
    }

    pub fn get(&self, path: &Path) -> Option<&Loaded> {
        self.entries.get(path).map(|e| &e.loaded)
    }

    /// The image, or while it re-renders, the stale one.
    pub fn rendered(&self, path: &Path) -> Option<&Loaded> {
        let stale = (self.stale.as_ref()).filter(|(p, _)| p == path);
        (self.entries.get(path).or(stale.map(|(_, e)| e))).map(|e| &e.loaded)
    }

    /// Marks the rendered image as just used; false if there is none.
    pub fn touch(&mut self, path: &Path) -> bool {
        let stale = (self.stale.as_mut()).filter(|(p, _)| p == path);
        let Some(entry) = self.entries.get_mut(path).or(stale.map(|(_, e)| e)) else {
            return false;
        };
        self.tick += 1;
        entry.last_used = self.tick;
        true
    }

    pub fn error(&self, path: &Path) -> Option<&String> {
        self.errors.get(path)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn gpu_bytes(&self) -> usize {
        (self.entries.values())
            .chain(self.stale.as_ref().map(|(_, e)| e))
            .map(|e| e.loaded.gpu_bytes())
            .sum()
    }

    pub fn memory_budget(&self) -> Option<MemoryBudget> {
        self.memory_budget
    }

    pub fn update_memory_budget(&mut self) {
        self.memory_budget = self.gpu_memory.as_ref().and_then(MemoryQuery::budget);
    }

    pub fn insert(&mut self, path: PathBuf, loaded: Loaded) {
        self.tick += 1;
        self.image_bytes = self.image_bytes.max(loaded.gpu_bytes());
        let last_used = self.tick;
        self.entries.insert(path, CacheEntry { loaded, last_used });
    }

    pub fn insert_error(&mut self, path: PathBuf, error: String) {
        self.errors.insert(path, error);
    }

    pub fn remove(&mut self, path: &Path) {
        self.entries.remove(path);
    }

    pub fn forget(&mut self, path: &Path) {
        self.entries.remove(path);
        self.errors.remove(path);
    }

    pub fn discard_stale(&mut self) {
        self.stale = None;
    }

    /// Drops the image, but keeps it as the stale one until it's rendered again.
    pub fn rerender(&mut self, path: &Path) {
        self.stale = self.entries.remove_entry(path).or(self.stale.take());
    }

    /// Like `clear`, but keeps the shown image as the stale one.
    pub fn rerender_all(&mut self, shown: Option<&Path>) {
        if let Some(shown) = shown {
            self.rerender(shown);
        }
        let stale = self.stale.take();
        self.clear();
        self.stale = stale;
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.errors.clear();
        self.stale = None;
        self.image_bytes = 0;
        self.memory_cap = None;
    }

    /// Half the GPU memory budget; the rest is left to other processes and decoding.
    pub fn limit(&self, ahead: usize, behind: usize, full_res: bool) -> usize {
        let wanted = CACHED_IMAGES.max(1 + ahead + behind);
        let fits = match self.memory_budget {
            Some(m) if self.image_bytes > 0 => {
                let n = m.budget / 2 / self.image_bytes as u64;
                usize::try_from(n)
                    .unwrap_or(usize::MAX)
                    .max(MIN_CACHED_IMAGES)
            }
            _ if full_res => CACHED_FULL_RES_WITHOUT_BUDGET,
            _ => wanted,
        };
        wanted.min(fits).min(self.memory_cap.unwrap_or(usize::MAX))
    }

    pub fn eviction_victim(
        &self,
        window: &[&PathBuf],
        keep: [Option<&PathBuf>; 2],
    ) -> Option<PathBuf> {
        let last_used = (self.entries.iter()).map(|(p, e)| (p, e.last_used));
        eviction_victim(last_used, window, keep).cloned()
    }

    /// Drops `victim` and caps the cache at the images that fit. Returns how many that
    /// is; `None` if not even `path`, the current image, fits.
    pub fn out_of_memory(
        &mut self,
        path: PathBuf,
        victim: Option<PathBuf>,
        is_current: bool,
    ) -> Option<usize> {
        self.stale = None;
        let fits = match victim {
            Some(victim) => {
                let fits = self.entries.len();
                self.entries.remove(&victim);
                fits
            }
            None if is_current => {
                self.errors.insert(path, "not enough GPU memory".into());
                return None;
            }
            // Only the current image fits.
            None => 1,
        };
        self.memory_cap = Some(fits);
        Some(fits)
    }
}

fn eviction_victim<'a>(
    last_used: impl Iterator<Item = (&'a PathBuf, u64)>,
    window: &[&PathBuf],
    keep: [Option<&PathBuf>; 2],
) -> Option<&'a PathBuf> {
    let eviction_order =
        |path: &PathBuf, last_used: u64| match window.iter().position(|w| *w == path) {
            Some(priority) => EvictionOrder::InWindow {
                lowest_priority: Reverse(priority),
            },
            None => EvictionOrder::OutsideWindow {
                least_recently_used: last_used,
            },
        };
    last_used
        .filter(|(p, _)| !keep.contains(&Some(*p)))
        .min_by_key(|&(p, used)| eviction_order(p, used))
        .map(|(p, _)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_outside_the_window_first_then_the_farthest_in_it() {
        let [current, shown, near, far, old, recent] =
            ["current", "shown", "near", "far", "old", "recent"].map(PathBuf::from);
        let window = [&current, &near, &far];
        let keep = [Some(&current), Some(&shown)];
        let mut cached = vec![
            (&current, 1),
            (&shown, 0),
            (&near, 2),
            (&far, 3),
            (&recent, 5),
            (&old, 4),
        ];
        let mut evicted = Vec::new();
        while let Some(victim) = eviction_victim(cached.iter().copied(), &window, keep) {
            evicted.push(victim.clone());
            cached.retain(|(p, _)| *p != victim);
        }
        assert_eq!(evicted, [old, recent, far, near]);
    }
}
