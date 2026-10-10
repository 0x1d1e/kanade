/*
 * an emoji as the picture its color font holds. The runtime draws a letter from its outline, and color
 * emoji fonts like Noto Color Emoji have none, only a png per emoji; so the png is written as a
 * file and drawn like an app's icon
 */

use std::fs;
use std::path::PathBuf;

use fontconfig::Fontconfig;
use rustybuzz::ttf_parser::{GlyphId, RasterImageFormat};
use rustybuzz::{Face, UnicodeBuffer};

use crate::raster;

// the size asked of the font, which gives its nearest; the row scales it to its icon
const PIXELS: u16 = 128;

/*
 * each emoji's picture in order; none for one the font has no png of, and for all when there is
 * no emoji font, so they show as letters
 */
pub fn pictures(emoji: &[&str]) -> Vec<Option<PathBuf>> {
    let Some((bytes, index)) = font() else {
        return vec![None; emoji.len()];
    };

    let Some(face) = Face::from_slice(&bytes, index) else {
        return vec![None; emoji.len()];
    };

    emoji.iter().map(|emoji| picture(&face, emoji)).collect()
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

fn picture(face: &Face, emoji: &str) -> Option<PathBuf> {
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
