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
 * bound cached thumbnails by count and bytes, evicting the least recently used unheld copies; the
 * limits retain several scrolled Wallpapers grids alongside magnified Dock icons and other Surfaces.
 */
const THUMBNAILS: usize = 128;
const THUMBNAIL_BYTES: usize = 24 << 20;

// the most thumbnails waiting for a worker; past this, the longest unused go so scrolling
// past many pictures doesn't keep them waiting or decode them after they're gone
const QUEUED: usize = 64;

// full-size preview decodes stay tightly bounded while waiting and when no longer displayed
const FULL_QUEUED: usize = 4;
const FULL_UNHELD: usize = 3;

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

    // the same pixels one picture hands to several windows, drawn without a copy each time
    pub fn shared(width: u32, height: u32, pixels: Arc<Vec<u8>>) -> Self {
        Self {
            width,
            height,
            pixels,
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
 * decode only draws the windows still missing a picture, not every window;
 * thumbnails use a separate marker so grid decodes never wake full-size previews
 */
struct Decoded;
struct DecodedThumbnail;

fn decoded_type(cover: Option<(u32, u32)>) -> TypeId {
    if cover.is_some() {
        TypeId::of::<DecodedThumbnail>()
    } else {
        TypeId::of::<Decoded>()
    }
}

// file path, optional cover size, and modification time plus length to detect in-place replacement
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
                loader.prioritize(&key);
                changes::note_read(decoded_type(cover));
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
    loader.waiting.push(key.clone());
    loader.trim_waiting();

    if loader.workers < WORKERS {
        loader.workers += 1;
        thread::spawn(work);
    }

    drop(loader);
    WAITING.notify_one();

    changes::note_read(decoded_type(cover));

    (ImageState::Loading, None)
}

// decodes what waits, for good
fn work() {
    loop {
        let key = {
            let mut loader = LOADER.locked();

            loop {
                if let Some(key) = loader.pop_waiting() {
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

    let Some(entry) = loader.entries.get_mut(&key) else {
        return;
    };

    entry.slot = match image {
        Some(image) => Slot::Ready(Arc::new(image)),
        None => Slot::Failed,
    };

    if key.1.is_some() {
        loader.trim();
    } else {
        loader.trim_full();
    }

    drop(loader);

    changes::mark(decoded_type(key.1));
}

impl Loader {
    // moves an image asked again to the back of the queue, so it decodes before older ones
    fn prioritize(&mut self, key: &Key) {
        if let Some(pos) = self.waiting.iter().position(|k| k == key) {
            let key = self.waiting.remove(pos);
            self.waiting.push(key);
        }
    }

    // pops the most recently requested full-size decode ahead of any queued thumbnails
    fn pop_waiting(&mut self) -> Option<Key> {
        if let Some(pos) = self
            .waiting
            .iter()
            .rposition(|(_, cover, _)| cover.is_none())
        {
            return Some(self.waiting.remove(pos));
        }

        self.waiting.pop()
    }

    // queued requests past their limit go, the longest unused first
    fn trim_waiting(&mut self) {
        self.trim_waiting_kind(true, QUEUED);
        self.trim_waiting_kind(false, FULL_QUEUED);
    }

    fn trim_waiting_kind(&mut self, thumbnail: bool, limit: usize) {
        while self
            .waiting
            .iter()
            .filter(|k| k.1.is_some() == thumbnail)
            .count()
            > limit
        {
            let oldest = self
                .waiting
                .iter()
                .enumerate()
                .filter(|(_, k)| k.1.is_some() == thumbnail)
                .min_by_key(|(_, k)| self.entries.get(*k).map(|e| e.used).unwrap_or(0))
                .map(|(i, _)| i);

            let Some(pos) = oldest else {
                break;
            };

            let evicted = self.waiting.remove(pos);
            self.entries.remove(&evicted);
        }
    }

    /*
     * keeps at most FULL_UNHELD unheld full-size decodes warm, evicting the longest unused first;
     * held bitmaps and decodes in flight stay
     */
    fn trim_full(&mut self) {
        let mut unheld: Vec<(u64, Key)> = self
            .entries
            .iter()
            .filter(|(key, _)| key.1.is_none())
            .filter_map(|(key, entry)| {
                let unheld = match &entry.slot {
                    Slot::Ready(image) => Arc::strong_count(image) == 1,
                    Slot::Failed => true,
                    Slot::Loading => false,
                };

                unheld.then(|| (entry.used, key.clone()))
            })
            .collect();

        if unheld.len() <= FULL_UNHELD {
            return;
        }

        unheld.sort_unstable_by_key(|(used, _)| *used);

        let remove = unheld.len() - FULL_UNHELD;
        for (_, key) in unheld.into_iter().take(remove) {
            self.entries.remove(&key);
        }
    }

    // drops every unheld full-size entry while preserving held bitmaps, loads in flight, and thumbnails
    fn forget_unheld_full(&mut self) {
        self.entries.retain(|(_, cover, _), entry| {
            if cover.is_some() {
                return true;
            }

            match &entry.slot {
                Slot::Ready(kept) => Arc::strong_count(kept) > 1,
                Slot::Failed => false,
                Slot::Loading => true,
            }
        });
    }

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
            key.1?;

            let bytes = match &entry.slot {
                Slot::Ready(image) => image.bytes(),
                Slot::Failed => 0,
                Slot::Loading => return None,
            };

            Some((key, entry, bytes))
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

// trims unheld full-size bitmaps down to FULL_UNHELD after a window stops drawing old frames
pub(crate) fn trim_full() {
    LOADER.locked().trim_full();
}

// drops all unheld full-size bitmaps when a layer hides or a Surface visit finishes
pub(crate) fn forget_unheld_full() {
    LOADER.locked().forget_unheld_full();
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

// tests touching global LOADER take turns with memory::trim_if_due tests
#[cfg(test)]
pub(crate) static TEST_TURN: Mutex<()> = Mutex::new(());

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
        let _turn = TEST_TURN.locked();
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

    /*
     * thumbnails waiting for a worker are bounded so a fast scroll past many pictures doesn't keep
     * them queued or decode them after they are gone; one asked for again after eviction re-queues
     * and decodes correctly
     */
    #[test]
    fn queued_thumbnails_are_bounded_and_reload_after_eviction() {
        let _turn = TEST_TURN.locked();
        let path = std::env::temp_dir().join(format!("kanade-queue-{}.png", std::process::id()));
        write_png(&path, 0);

        // overflow the queue with thumbnail requests so older ones are evicted
        for i in 1..=300 {
            let cover = Some((32, i as u32));
            state(&path, cover);
        }

        let evicted_cover = {
            let loader = LOADER.locked();
            let waiting_thumbnails = loader.waiting.iter().filter(|k| k.1.is_some()).count();
            assert!(
                waiting_thumbnails <= QUEUED,
                "waiting thumbnails exceeded the bound: {waiting_thumbnails} > {QUEUED}"
            );

            (1..=300)
                .map(|i| Some((32, i as u32)))
                .find(|cover| {
                    let key = (path.clone(), *cover, stamp(&path));
                    !loader.entries.contains_key(&key)
                })
                .expect("overflowing the queue must evict older thumbnails")
        };

        // the evicted thumbnail was removed from entries; asking for it again re-queues and loads it
        let started = std::time::Instant::now();
        loop {
            if let Some(image) = load(&path, evicted_cover) {
                assert_eq!(image.pixels[..], [0, 0, 0, 0]);
                break;
            }

            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the evicted thumbnail was never loaded when asked for again"
            );

            thread::sleep(Duration::from_millis(10));
        }

        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn full_size_loads_take_priority_and_bound_waiting_and_unheld_entries() {
        let _turn = TEST_TURN.locked();

        assert_ne!(decoded_type(Some((32, 32))), decoded_type(None));
        assert_eq!(decoded_type(None), TypeId::of::<Decoded>());

        let key =
            |name: &str, cover: Option<(u32, u32)>| -> Key { (PathBuf::from(name), cover, None) };

        let mut loader = Loader::default();
        let total_full = FULL_QUEUED + 3;
        for i in 1..=total_full {
            let k = key(&format!("full-{i}.png"), None);
            loader.entries.insert(
                k.clone(),
                Entry {
                    slot: Slot::Loading,
                    used: i as u64,
                },
            );
            loader.waiting.push(k);
        }

        for i in 1..=2 {
            let k = key(&format!("thumb-{i}.png"), Some((32, 32)));
            loader.entries.insert(
                k.clone(),
                Entry {
                    slot: Slot::Loading,
                    used: (total_full + i) as u64,
                },
            );
            loader.waiting.push(k);
        }

        loader.trim_waiting();

        // stale full-size requests are evicted beyond FULL_QUEUED while newer ones and thumbnails stay
        assert_eq!(
            loader.waiting.iter().filter(|k| k.1.is_none()).count(),
            FULL_QUEUED
        );
        for i in 1..=3 {
            assert!(
                !loader
                    .entries
                    .contains_key(&key(&format!("full-{i}.png"), None))
            );
        }
        for i in 4..=total_full {
            assert!(
                loader
                    .entries
                    .contains_key(&key(&format!("full-{i}.png"), None))
            );
        }
        assert!(
            loader
                .entries
                .contains_key(&key("thumb-1.png", Some((32, 32))))
        );
        assert!(
            loader
                .entries
                .contains_key(&key("thumb-2.png", Some((32, 32))))
        );

        // workers pop the most recent full-size request ahead of queued thumbnails
        assert_eq!(
            loader.pop_waiting(),
            Some(key(&format!("full-{total_full}.png"), None))
        );

        // trim_full bounds unheld completed full-size entries while keeping held, loading, and thumbnails
        let mut cache = Loader::default();
        let held_bitmap = Arc::new(Bitmap::empty());
        let held_key = key("held.png", None);
        cache.entries.insert(
            held_key.clone(),
            Entry {
                slot: Slot::Ready(Arc::clone(&held_bitmap)),
                used: 1,
            },
        );
        let loading_key = key("loading.png", None);
        cache.entries.insert(
            loading_key.clone(),
            Entry {
                slot: Slot::Loading,
                used: 2,
            },
        );
        let thumb_key = key("thumb.png", Some((64, 64)));
        cache.entries.insert(
            thumb_key.clone(),
            Entry {
                slot: Slot::Ready(Arc::new(Bitmap::empty())),
                used: 3,
            },
        );

        // 5 unheld completed full-size entries: 2 oldest (tick 10, 11) should be evicted
        cache.entries.insert(
            key("unheld-1.png", None),
            Entry {
                slot: Slot::Ready(Arc::new(Bitmap::empty())),
                used: 10,
            },
        );
        cache.entries.insert(
            key("unheld-2-failed.png", None),
            Entry {
                slot: Slot::Failed,
                used: 11,
            },
        );
        for (i, used) in [(3, 12), (4, 13), (5, 14)] {
            cache.entries.insert(
                key(&format!("unheld-{i}.png"), None),
                Entry {
                    slot: Slot::Ready(Arc::new(Bitmap::empty())),
                    used,
                },
            );
        }

        cache.trim_full();

        assert!(!cache.entries.contains_key(&key("unheld-1.png", None)));
        assert!(
            !cache
                .entries
                .contains_key(&key("unheld-2-failed.png", None))
        );
        for i in 3..=5 {
            assert!(
                cache
                    .entries
                    .contains_key(&key(&format!("unheld-{i}.png"), None))
            );
        }
        assert!(cache.entries.contains_key(&held_key));
        assert!(cache.entries.contains_key(&loading_key));
        assert!(cache.entries.contains_key(&thumb_key));

        // forget_unheld_full drops all remaining unheld full-size entries, keeping held, loading, and thumbnails
        cache.forget_unheld_full();

        for i in 3..=5 {
            assert!(
                !cache
                    .entries
                    .contains_key(&key(&format!("unheld-{i}.png"), None))
            );
        }
        assert!(cache.entries.contains_key(&held_key));
        assert!(cache.entries.contains_key(&loading_key));
        assert!(cache.entries.contains_key(&thumb_key));

        // finish ignores keys already evicted from LOADER without inserting them back
        let missing = key("evicted-before-finish.png", None);
        finish(missing.clone(), Some(Bitmap::empty()));
        assert!(!LOADER.locked().entries.contains_key(&missing));

        // global forget_unheld_full drops unheld full-size bitmaps and keeps held ones
        let global_held = Arc::new(Bitmap::empty());
        let global_held_key = key("global-held.png", None);
        let global_unheld_key = key("global-unheld.png", None);
        {
            let mut global = LOADER.locked();
            global.entries.insert(
                global_held_key.clone(),
                Entry {
                    slot: Slot::Ready(Arc::clone(&global_held)),
                    used: 100,
                },
            );
            global.entries.insert(
                global_unheld_key.clone(),
                Entry {
                    slot: Slot::Ready(Arc::new(Bitmap::empty())),
                    used: 101,
                },
            );
        }
        forget_unheld_full();
        {
            let mut global = LOADER.locked();
            assert!(global.entries.contains_key(&global_held_key));
            assert!(!global.entries.contains_key(&global_unheld_key));
            global.entries.remove(&global_held_key);
        }
        drop(global_held);
    }
}
