//! Background decoding thread pool. Full-size image requests always take
//! priority over thumbnails; thumbnails are served most-recently-requested first,
//! so whatever is currently scrolled into view loads first.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use egui::ColorImage;

use crate::decode::{self, Decoded};
use crate::source::Page;

pub const THUMB_SIDE: u32 = 256;

pub enum Output {
    Full {
        index: usize,
        result: Result<Decoded, String>,
    },
    Thumb {
        index: usize,
        result: Result<ColorImage, String>,
        /// The page is an animated GIF, PNG or WebP (shown with an "A" badge).
        animated: bool,
    },
}

#[derive(Default)]
struct Queue {
    full: VecDeque<usize>,
    thumbs: Vec<usize>,
    shutdown: bool,
}

pub struct Workers {
    shared: Arc<(Mutex<Queue>, Condvar)>,
    pub results: Receiver<Output>,
}

impl Workers {
    pub fn new(pages: Arc<Vec<Page>>, ctx: egui::Context) -> Self {
        let shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let (tx, results) = std::sync::mpsc::channel();
        let n = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(2, 8);
        for i in 0..n {
            let shared = shared.clone();
            let pages = pages.clone();
            let tx = tx.clone();
            let ctx = ctx.clone();
            std::thread::Builder::new()
                .name(format!("decoder-{i}"))
                .spawn(move || worker_loop(shared, pages, tx, ctx))
                .expect("failed to spawn decoder thread");
        }
        Workers { shared, results }
    }

    /// Replace pending full-image requests (most important first).
    pub fn request_full(&self, indices: &[usize]) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.full.clear();
        q.full.extend(indices.iter().copied());
        cv.notify_all();
    }

    pub fn request_thumb(&self, index: usize) {
        let (lock, cv) = &*self.shared;
        lock.lock()
            .unwrap_or_else(|e| e.into_inner())
            .thumbs
            .push(index);
        cv.notify_one();
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).shutdown = true;
        cv.notify_all();
    }
}

enum Job {
    Full(usize),
    Thumb(usize),
}

fn worker_loop(
    shared: Arc<(Mutex<Queue>, Condvar)>,
    pages: Arc<Vec<Page>>,
    tx: Sender<Output>,
    ctx: egui::Context,
) {
    let (lock, cv) = &*shared;
    loop {
        let job = {
            let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if q.shutdown {
                    return;
                }
                if let Some(i) = q.full.pop_front() {
                    break Job::Full(i);
                }
                if let Some(i) = q.thumbs.pop() {
                    break Job::Thumb(i);
                }
                q = cv.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        let out = match job {
            Job::Full(index) => {
                let page = &pages[index];
                let result = guarded(|| {
                    page.read_bytes()
                        .and_then(|b| decode::decode_full(&b, page.file_name()))
                });
                Output::Full { index, result }
            }
            Job::Thumb(index) => {
                let page = &pages[index];
                let mut animated = false;
                let result = guarded(|| match page.video_path() {
                    Some(path) => crate::video::thumbnail(path, THUMB_SIDE),
                    None => page.read_bytes().and_then(|b| {
                        animated = decode::is_animated(&b);
                        decode::decode_thumbnail(&b, page.file_name(), THUMB_SIDE)
                    }),
                });
                Output::Thumb {
                    index,
                    result,
                    animated,
                }
            }
        };
        if tx.send(out).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// Run a decoding job, turning a panic inside a decoder library into an error for
/// that one file, so a bad file can't take a decoder thread (or the app) down.
fn guarded<T>(job: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
        .unwrap_or_else(|panic| Err(format!("decoder failed: {}", crate::panic_message(&*panic))))
}
