/*
 * color emoji fonts provide PNGs, not outlines; write each requested image as an icon file. Memoize
 * visible tiles only, and let content-addressed `raster::write` reduce repeats to an `exists()` check.
 */

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{LazyLock, Mutex};

use fontconfig::Fontconfig;
use kanade_runtime::{MutexExt, request_frame};
use rustybuzz::ttf_parser::{GlyphId, RasterImageFormat};
use rustybuzz::{Face, UnicodeBuffer};

use crate::raster;

// the size asked of the font, which gives its nearest; the tile scales it to its cell
const PIXELS: u16 = 128;

// what each emoji's font holds, by emoji: none for one the font has no png of, and for all when
// there is no emoji font, so they show as letters
static PICTURES: LazyLock<Mutex<HashMap<&'static str, Option<PathBuf>>>> =
    LazyLock::new(Mutex::default);

// one worker owns rasterization; visible requests wait in a bounded queue
const QUEUE: usize = 32;

static PENDING: LazyLock<Mutex<HashSet<&'static str>>> = LazyLock::new(Mutex::default);
static REQUESTS: LazyLock<Option<SyncSender<&'static str>>> = LazyLock::new(|| {
    let (send, receive) = mpsc::sync_channel::<&'static str>(QUEUE);

    std::thread::Builder::new()
        .name("emoji-pictures".into())
        .spawn(move || {
            while let Ok(key) = receive.recv() {
                let picture = std::panic::catch_unwind(|| drawn(key)).unwrap_or(None);

                PICTURES.locked().insert(key, picture);
                PENDING.locked().remove(key);
                request_frame();
            }
        })
        .ok()?;

    Some(send)
});

// the font, read and parsed once: a tile drawn is a glyph looked up in it, so parsing it again for
// each would cost more than the drawing
static FACE: LazyLock<Option<Face<'static>>> = LazyLock::new(|| {
    let (bytes, index) = font()?;

    Face::from_slice(Box::leak(bytes.into_boxed_slice()), index)
});

// `emoji`'s picture, rasterized the first time it is asked for
pub fn picture(emoji: &str) -> Option<PathBuf> {
    // a string not of the table's own is not one this memo can hold
    let key = emojis::get(emoji).map(|emoji| emoji.as_str())?;

    if let Some(known) = PICTURES.locked().get(key) {
        return known.clone();
    }

    if !PENDING.locked().insert(key) {
        return None;
    }

    let Some(requests) = REQUESTS.as_ref() else {
        PENDING.locked().remove(key);
        PICTURES.locked().insert(key, None);
        return None;
    };

    if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) = requests.try_send(key) {
        PENDING.locked().remove(key);
    }

    None
}

// what the emoji font holds for `emoji`, shaped as the Surface would draw it
fn drawn(emoji: &str) -> Option<PathBuf> {
    let face = FACE.as_ref()?;

    // shaped, since a flag or a family is one glyph made of several characters
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(emoji);

    let shaped = rustybuzz::shape(face, &[], buffer);

    let [glyph] = shaped.glyph_infos() else {
        return None;
    };

    let id = GlyphId(u16::try_from(glyph.glyph_id).ok()?);

    let image = face
        .glyph_raster_image(id, PIXELS)
        .filter(|image| image.format == RasterImageFormat::PNG)?;

    Some(raster::write("emoji", "png", image.data))
}

// the font fontconfig picks for emoji, and which of the file's fonts it is
fn font() -> Option<(Vec<u8>, u32)> {
    let found = Fontconfig::new()?.find("emoji", None).ok()?;

    // fontconfig keeps a variable font's named style in the upper half, the file only knows the lower
    let index = found.index.unwrap_or(0) as u32 & 0xffff;

    match fs::read(&found.path) {
        Ok(bytes) => Some((bytes, index)),
        Err(error) => {
            eprintln!("kanade: failed to read {}: {error}", found.path.display());
            None
        }
    }
}
