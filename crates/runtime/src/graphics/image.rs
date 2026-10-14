mod decode;
mod shrink;
mod svg;

use std::any::TypeId;
use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex, PoisonError};
use std::thread;

use crate::changes;
use crate::unpoisoned::MutexExt;

pub use decode::{Format, MOST_PIXELS, decode, decode_png, format};

// one size for every svg; keying the cache by the shown size would sharpen a big svg that looks soft
const ICON_SIDE: f32 = 256.0;

// how many images decode at once: a grid of thumbnails fills in parallel, and the rest of the
// machine keeps its cores
const WORKERS: usize = 3;

/*
 * the most small copies kept, and their most bytes, a grid of a few hundred wallpapers' worth;
 * past either, those no window holds go, the longest unused first
 */
const THUMBNAILS: usize = 512;
const THUMBNAIL_BYTES: usize = 128 << 20;

// an svg's bytes drawn with the longest side `longest` pixels across: width, height, plain RGBA
pub fn draw_svg(bytes: &[u8], longest: f32) -> Option<(u32, u32, Vec<u8>)> {
    svg::rasterize(bytes, longest)
}

// rgba pixels with plain, not premultiplied, alpha
pub struct Bitmap {
    width: u32,
    height: u32,

    // shared with the gpu, which draws from the same pixels instead of a copy
    pub pixels: Arc<Vec<u8>>,
}

impl Bitmap {
    // one clear pixel, for a texture slot that has to hold something
    pub fn empty() -> Self {
        Self {
            width: 1,
            height: 1,
            pixels: Arc::new(vec![0; 4]),
        }
    }

    // plain RGBA pixels, width * height * 4 of them
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            pixels: Arc::new(pixels),
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    fn bytes(&self) -> usize {
        self.pixels.len()
    }
}

// how an image asked for stands
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageState {
    // waiting for a worker, or decoding
    Loading,
    Ready,

    // unreadable, or not an image the runtime decodes
    Failed,
}

/*
 * what a window waiting on an image counts as having read, so a finished
 * decode only draws the windows still missing a picture, not every window
 */
struct Decoded;

// a file and the size it is shrunk to cover (none for its own size)
type Key = (PathBuf, Option<(u32, u32)>);

enum Slot {
    Loading,
    Ready(Arc<Bitmap>),
    Failed,
}

struct Entry {
    slot: Slot,

    // the tick it was last asked for at
    used: u64,
}

// every image asked for, and those waiting for a worker
#[derive(Default)]
struct Loader {
    entries: HashMap<Key, Entry>,

    // the last asked decodes first, so the pictures in view fill in before ones scrolled past
    waiting: Vec<Key>,

    // counts every ask, for which small copy went unused longest
    tick: u64,

    workers: usize,
}

static LOADER: LazyLock<Mutex<Loader>> = LazyLock::new(Mutex::default);

// wakes a worker for a key waiting
static WAITING: Condvar = Condvar::new();

/*
 * decoding a large image takes long enough to drop frames, so it happens
 * on a worker; until it is done this gives none and the image is
 * left out, then the window is woken to draw it
 */
pub fn load(path: &Path, cover: Option<(u32, u32)>) -> Option<Arc<Bitmap>> {
    ask(path, cover).1
}

// how the image stands, asking for it if nothing did yet
pub fn state(path: &Path, cover: Option<(u32, u32)>) -> ImageState {
    ask(path, cover).0
}

fn ask(path: &Path, cover: Option<(u32, u32)>) -> (ImageState, Option<Arc<Bitmap>>) {
    let mut loader = LOADER.locked();

    loader.tick += 1;
    let tick = loader.tick;
    let key = (path.to_path_buf(), cover);

    if let Some(entry) = loader.entries.get_mut(&key) {
        entry.used = tick;

        return match &entry.slot {
            Slot::Loading => {
                changes::note_read(TypeId::of::<Decoded>());
                (ImageState::Loading, None)
            }
            Slot::Ready(image) => (ImageState::Ready, Some(Arc::clone(image))),
            Slot::Failed => (ImageState::Failed, None),
        };
    }

    loader.entries.insert(
        key.clone(),
        Entry {
            slot: Slot::Loading,
            used: tick,
        },
    );
    loader.waiting.push(key);

    if loader.workers < WORKERS {
        loader.workers += 1;
        thread::spawn(work);
    }

    drop(loader);
    WAITING.notify_one();

    changes::note_read(TypeId::of::<Decoded>());

    (ImageState::Loading, None)
}

// decodes what waits, for good
fn work() {
    loop {
        let key = {
            let mut loader = LOADER.locked();

            loop {
                if let Some(key) = loader.waiting.pop() {
                    break key;
                }

                loader = WAITING.wait(loader).unwrap_or_else(PoisonError::into_inner);
            }
        };

        // a decoder that panics on a broken file fails that image, not the worker
        let image = panic::catch_unwind(AssertUnwindSafe(|| read_key(&key)))
            .ok()
            .flatten();

        finish(key, image);
    }
}

fn read_key(key: &Key) -> Option<Bitmap> {
    let (path, cover) = key;

    let image = read(path)?;

    // only the small copy is kept, the full size one is dropped here
    Some(match cover {
        Some((width, height)) => shrink::to_cover(image, *width, *height),
        None => image,
    })
}

fn finish(key: Key, image: Option<Bitmap>) {
    let mut loader = LOADER.locked();

    if let Some(entry) = loader.entries.get_mut(&key) {
        entry.slot = match image {
            Some(image) => Slot::Ready(Arc::new(image)),
            None => Slot::Failed,
        };
    }

    if key.1.is_some() {
        loader.trim();
    }

    drop(loader);

    changes::mark(TypeId::of::<Decoded>());
}

impl Loader {
    /*
     * small copies past THUMBNAILS or THUMBNAIL_BYTES go, the longest unused first; one a window
     * holds stays, and so does one still decoding. A failed one goes too, so a file being written
     * then is read again later
     */
    fn trim(&mut self) {
        // a small copy, with its bytes
        fn thumbnail<'a>(
            (key, entry): (&'a Key, &'a Entry),
        ) -> Option<(&'a Key, &'a Entry, usize)> {
            let bytes = match &entry.slot {
                Slot::Ready(image) => image.bytes(),
                _ => 0,
            };

            key.1.is_some().then_some((key, entry, bytes))
        }

        let (mut count, mut bytes) = self
            .entries
            .iter()
            .filter_map(thumbnail)
            .fold((0, 0), |(count, total), (_, _, bytes)| {
                (count + 1, total + bytes)
            });

        if count <= THUMBNAILS && bytes <= THUMBNAIL_BYTES {
            return;
        }

        let mut unheld: Vec<(u64, Key, usize)> = self
            .entries
            .iter()
            .filter_map(thumbnail)
            .filter(|(_, entry, _)| match &entry.slot {
                Slot::Ready(image) => Arc::strong_count(image) == 1,
                Slot::Failed => true,
                Slot::Loading => false,
            })
            .map(|(key, entry, bytes)| (entry.used, key.clone(), bytes))
            .collect();

        unheld.sort_unstable_by_key(|(used, ..)| *used);

        for (_, key, size) in unheld {
            if count <= THUMBNAILS && bytes <= THUMBNAIL_BYTES {
                break;
            }

            self.entries.remove(&key);
            count -= 1;
            bytes -= size;
        }
    }
}

/*
 * frees a full size image no window shows anymore; small copies stay
 * until there are too many, as decoding them again is slow
 */
pub fn forget(image: &Arc<Bitmap>) {
    let mut loader = LOADER.locked();

    loader.entries.retain(|(_, cover), entry| {
        let same = matches!(&entry.slot, Slot::Ready(kept) if Arc::ptr_eq(kept, image));

        cover.is_some() || !same
    });
}

// none for a file that can't be read or decoded, like one still being written
pub fn read(path: &Path) -> Option<Bitmap> {
    let (width, height, pixels) = decode(path, ICON_SIDE)?;

    Some(Bitmap {
        width,
        height,
        pixels: Arc::new(pixels),
    })
}
