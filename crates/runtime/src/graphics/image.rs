mod decode;
mod shrink;
mod svg;

use std::any::TypeId;
use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex, PoisonError};
use std::thread;
use std::time::SystemTime;

use crate::changes;
use crate::unpoisoned::MutexExt;

pub use decode::{Format, MOST_PIXELS, decode, decode_png, format, side_fits};

// one size for every svg; keying the cache by the shown size would sharpen a big svg that looks soft
const ICON_SIDE: f32 = 256.0;

// how many images decode at once: a grid of thumbnails fills in parallel, and the rest of the
// machine keeps its cores
const WORKERS: usize = 3;

/*
 * the most small copies kept, and their most bytes; past either, those no window holds go, the
 * longest unused first. Every caller asks for an icon or a tile, never a picture: the largest is a
 * Wallpapers tile at `SCALE` 2, 288x180 px, 202 KiB. One grid draws at most 30 of them, 5.9 MiB:
 * six rows built and clipped between rows, four showing whole. Next a Dock icon at its most
 * magnified, 144 KiB. So the bounds hold several gridfuls scrolled away and back, beside every
 * Dock icon and other Surface's
 */
const THUMBNAILS: usize = 128;
const THUMBNAIL_BYTES: usize = 24 << 20;

// an svg's bytes drawn with the longest side `longest` pixels across: width, height, plain RGBA
pub fn draw_svg(bytes: &[u8], longest: f32) -> Option<(u32, u32, Vec<u8>)> {
    // the side bounds the square the drawing fits in, so a request past `MOST_PIXELS` is refused
    side_fits(longest).then(|| svg::rasterize(bytes, longest))?
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

// a file, the size it is shrunk to cover (none for its own size), and the file it was asked for
//
// the stamp is the file's modification time and length: a file replaced in place, an icon
// overwritten by an update or a wallpaper by another, keeps its path, so the stamp is what tells
// the new one from the old, instead of serving the old pixels forever
type Key = (PathBuf, Option<(u32, u32)>, Stamp);

// what a file read when it was asked for; one gone or unreadable stamps as none, and reading it
// fails, so every such ask of that path shares one
fn stamp(path: &Path) -> Stamp {
    let metadata = path.metadata().ok()?;

    Some((metadata.modified().ok()?, metadata.len()))
}

type Stamp = Option<(SystemTime, u64)>;

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
    let key = (path.to_path_buf(), cover, stamp(path));

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
    let (path, cover, _) = key;

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

    loader.entries.retain(|(_, cover, _), entry| {
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::time::Duration;

    // a png of one clear pixel at `path`, of `extra` bytes past its end
    fn write_png(path: &Path, extra: usize) {
        write_png_of(path, [0, 0, 0, 0]);

        if extra > 0 {
            let mut bytes = fs::read(path).unwrap();
            bytes.extend(std::iter::repeat_n(0, extra));
            fs::write(path, bytes).unwrap();
        }
    }

    // a png of one pixel of `pixels` at `path`
    fn write_png_of(path: &Path, pixels: [u8; 4]) {
        let file = fs::File::create(path).unwrap();

        let mut encoder = png::Encoder::new(file, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);

        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&pixels).unwrap();
    }

    /*
     * an image replaced in place keeps its path, so the cache tells the new file from the old one
     * by its stamp instead of serving the old pixels; an icon overwritten by an update did that
     * forever
     */
    #[test]
    fn a_replaced_file_is_asked_for_again() {
        let path = std::env::temp_dir().join(format!("kanade-replaced-{}.png", std::process::id()));

        write_png(&path, 0);
        let first = stamp(&path).expect("a file written has a stamp");

        // the same path, written again and a byte longer, so both tell it apart
        thread::sleep(Duration::from_millis(10));
        write_png(&path, 1);
        let second = stamp(&path).expect("a file written again has a stamp");

        assert_ne!(
            first, second,
            "a replaced file stamped the same, so its old pixels were served"
        );

        fs::remove_file(&path).unwrap();
    }

    /*
     * the whole path a window takes: a file replaced in place is read again, not answered from the
     * cache with its old pixels, which is what a stamp-less key did forever
     */
    #[test]
    fn a_replaced_file_is_drawn_from_its_new_pixels() {
        let path = std::env::temp_dir().join(format!("kanade-newer-{}.png", std::process::id()));

        // a clear pixel, then an opaque one, at the same path
        write_png(&path, 0);
        let before = wait_for(&path, [0, 0, 0, 0]);

        thread::sleep(Duration::from_millis(10));
        write_png_of(&path, [9, 8, 7, 255]);

        let after = wait_for(&path, [9, 8, 7, 255]);

        assert!(
            !Arc::ptr_eq(&before, &after),
            "a file replaced in place was still drawn from its old pixels"
        );

        fs::remove_file(&path).unwrap();
    }

    // the bitmap a file is drawn from with `want` as its one pixel, waiting while it is read
    fn wait_for(path: &Path, want: [u8; 4]) -> Arc<Bitmap> {
        let started = std::time::Instant::now();

        loop {
            if let Some(image) = load(path, None)
                && image.pixels[..] == want
            {
                return image;
            }

            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the image was never drawn with the pixels it was written with"
            );

            thread::sleep(Duration::from_millis(10));
        }
    }

    // a file gone or unreadable stamps as none, one stamp for every such ask of that path
    #[test]
    fn a_missing_file_stamps_none() {
        let path = std::env::temp_dir().join("kanade-nowhere-at-all.png");

        assert_eq!(stamp(&path), None);
        assert_eq!(stamp(&path), None);
    }
}
