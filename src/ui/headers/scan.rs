use blip_core::image::{self, Header};
use eframe::egui;
use std::collections::{HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Reading a header is mostly waiting for the drive, so several run at once.
const READERS: usize = 8;
const READERS_VAR: &str = "BLIP_HEADER_READERS";
const MAX_READERS: usize = 64;
const BATCH_INTERVAL: Duration = Duration::from_millis(250);

pub struct Batch {
    /// `None` for files that couldn't be read.
    pub headers: Vec<(PathBuf, Option<Header>)>,
    pub progress: Progress,
}

pub enum Progress {
    Reading {
        done: usize,
        total: usize,
    },
    Finished {
        done: usize,
        failed: usize,
        elapsed: Duration,
    },
}

pub struct HeaderScanner(Arc<Shared>);

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    queue: VecDeque<PathBuf>,
    reading: HashSet<PathBuf>,
    results: Vec<(PathBuf, Option<Header>)>,
    /// Bumped by `cancel`, so reads still running for a cancelled scan are dropped.
    run: u64,
    done: usize,
    failed: usize,
    started: Option<Instant>,
    last_take: Option<Instant>,
    ctx: Option<egui::Context>,
}

impl State {
    fn finished(&self) -> bool {
        self.queue.is_empty() && self.reading.is_empty()
    }
}

impl HeaderScanner {
    pub fn new(
        ctx: Option<egui::Context>,
        wait_for_image: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let readers = (std::env::var(READERS_VAR).ok())
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|n| (1..=MAX_READERS).contains(n))
            .unwrap_or(READERS);
        Self::with_readers(ctx, readers, wait_for_image)
    }

    pub(in crate::ui) fn with_readers(
        ctx: Option<egui::Context>,
        readers: usize,
        wait_for_image: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                ctx,
                ..State::default()
            }),
            wake: Condvar::new(),
        });
        let wait_for_image = Arc::new(wait_for_image);
        for _ in 0..readers {
            let (shared, wait_for_image) = (shared.clone(), wait_for_image.clone());
            std::thread::spawn(move || read_headers(&shared, &*wait_for_image));
        }
        Self(shared)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Replaces what is still waiting to be read.
    pub fn request(&self, paths: Vec<PathBuf>) {
        let mut s = self.lock();
        let s = &mut *s;
        if s.started.is_none() {
            if paths.is_empty() {
                return;
            }
            s.started = Some(Instant::now());
            s.done = 0;
            s.failed = 0;
            s.last_take = None;
        }
        let read: HashSet<&PathBuf> = s.results.iter().map(|(p, _)| p).collect();
        s.queue = (paths.into_iter())
            .filter(|p| !read.contains(p) && !s.reading.contains(p))
            .collect();
        self.0.wake.notify_all();
    }

    pub fn cancel(&self) {
        let mut s = self.lock();
        s.run += 1;
        s.queue.clear();
        s.reading.clear();
        s.results.clear();
        s.started = None;
    }

    pub fn running(&self) -> bool {
        self.lock().started.is_some()
    }

    /// At most every [`BATCH_INTERVAL`] until the scan finishes.
    pub fn take(&self) -> Option<Batch> {
        let mut s = self.lock();
        let started = s.started?;
        let finished = s.finished();
        if !finished && s.last_take.is_some_and(|t| t.elapsed() < BATCH_INTERVAL) {
            return None;
        }
        s.last_take = Some(Instant::now());
        let headers = std::mem::take(&mut s.results);
        let progress = if finished {
            s.started = None;
            Progress::Finished {
                done: s.done,
                failed: s.failed,
                elapsed: started.elapsed(),
            }
        } else {
            Progress::Reading {
                done: s.done,
                total: s.done + s.reading.len() + s.queue.len(),
            }
        };
        Some(Batch { headers, progress })
    }
}

fn read_headers(shared: &Shared, wait_for_image: &dyn Fn()) {
    let lock = || shared.state.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let (path, run) = {
            let mut s = lock();
            loop {
                if let Some(path) = s.queue.pop_front() {
                    s.reading.insert(path.clone());
                    break (path, s.run);
                }
                s = shared.wake.wait(s).unwrap_or_else(|e| e.into_inner());
            }
        };
        wait_for_image();
        let header = catch_unwind(AssertUnwindSafe(|| image::read_header(&path)))
            .ok()
            .and_then(Result::ok);
        let mut s = lock();
        if s.run != run {
            continue;
        }
        s.reading.remove(&path);
        s.done += 1;
        s.failed += usize::from(header.is_none());
        s.results.push((path, header));
        if let Some(ctx) = &s.ctx {
            if s.finished() {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(BATCH_INTERVAL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn header(cards: &[(&str, &str)]) -> Header {
        (cards.iter())
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn finish(scanner: &HeaderScanner) -> (Vec<(PathBuf, Option<Header>)>, usize, usize) {
        let mut read = Vec::new();
        loop {
            match scanner.take() {
                Some(Batch {
                    headers,
                    progress: Progress::Finished { done, failed, .. },
                }) => {
                    read.extend(headers);
                    read.sort();
                    return (read, done, failed);
                }
                Some(batch) => read.extend(batch.headers),
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    }

    #[test]
    fn requeuing_skips_headers_waiting_to_be_taken() {
        let scanner = HeaderScanner::with_readers(None, 1, || {});
        let [a, b, c] = ["/nonexistent/a", "/nonexistent/b", "/nonexistent/c"].map(PathBuf::from);
        scanner.request(vec![a.clone(), b.clone()]);
        while scanner.lock().done < 2 {
            std::thread::sleep(Duration::from_millis(1));
        }
        scanner.request(vec![a.clone(), b.clone(), c.clone()]);
        let (read, done, failed) = finish(&scanner);
        assert_eq!((done, failed), (3, 3));
        assert_eq!(read, [(a, None), (b, None), (c, None)]);
    }

    #[test]
    fn reads_of_a_cancelled_scan_are_dropped() {
        let released = Arc::new(AtomicBool::new(false));
        let gate = released.clone();
        let scanner = HeaderScanner::with_readers(None, 1, move || {
            while !gate.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let [a, b] = ["/nonexistent/a", "/nonexistent/b"].map(PathBuf::from);
        scanner.request(vec![a.clone()]);
        while scanner.lock().reading.is_empty() {
            std::thread::sleep(Duration::from_millis(1));
        }
        scanner.cancel();
        assert!(!scanner.running());
        scanner.request(vec![a.clone(), b.clone()]);
        released.store(true, Ordering::Relaxed);
        let (read, done, _) = finish(&scanner);
        assert_eq!(done, 2);
        assert_eq!(read, [(a, None), (b, None)]);
    }

    #[test]
    fn scans_every_requested_file() {
        let dir = std::env::temp_dir().join(format!("blip-headers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let card = |s: &str| format!("{s:<80}");
        let fits = [card("SIMPLE  = T"), card("FILTER  = 'Ha'"), card("END")].concat();
        let paths: Vec<PathBuf> = (0..10).map(|i| dir.join(format!("{i}.fits"))).collect();
        for path in &paths {
            std::fs::write(path, format!("{fits:<2880}")).unwrap();
        }
        let missing = dir.join("missing.fits");

        let scanner = HeaderScanner::new(None, || {});
        assert!(scanner.take().is_none());
        scanner.request([paths.clone(), vec![missing.clone()]].concat());
        let (read, done, failed) = finish(&scanner);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!((done, failed), (11, 1));
        assert!(!scanner.running());
        let ha = header(&[("SIMPLE", "T"), ("FILTER", "Ha")]);
        let mut expected: Vec<(PathBuf, Option<Header>)> = (paths.into_iter())
            .map(|p| (p, Some(ha.clone())))
            .chain([(missing, None)])
            .collect();
        expected.sort();
        assert_eq!(read, expected);
    }
}
