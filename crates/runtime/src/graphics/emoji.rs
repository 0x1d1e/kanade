/*
 * a letter as the picture its color font holds. Color emoji fonts like Noto Color Emoji have no
 * outlines, only a png per emoji, so a letter drawn from its outline would be blank space
 */

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use ttf_parser::{Face, GlyphId, RasterImageFormat};

use crate::graphics::image::{Bitmap, decode_png};

// the size asked of the font, which gives its nearest strike
const PIXELS: u16 = 256;

// the picture and where it sits, in the strike's own pixels
pub struct Picture {
    pub image: Arc<Bitmap>,

    // from the pen, and the picture's bottom edge up from the baseline
    pub left: f32,
    pub bottom: f32,

    // the strike's pixels to an em, which scales it to the text
    pub pixels_per_em: f32,
}

// the face's place in memory and the glyph, which name a letter
type Key = (usize, u16);

thread_local! {
    /*
     * each emoji's picture, decoded from the font once; fonts stay loaded for the whole run, so where
     * a face lives in memory names it, and the same Arc lets the gpu keep one texture
     */
    static PICTURES: RefCell<HashMap<Key, Option<Arc<Picture>>>> =
        RefCell::new(HashMap::new());
}

// none for a letter the font holds no png of, like any letter of a plain font
pub fn picture(face: &Face, id: GlyphId) -> Option<Arc<Picture>> {
    let key: Key = (std::ptr::from_ref(face) as usize, id.0);

    PICTURES.with_borrow_mut(|pictures| {
        pictures
            .entry(key)
            .or_insert_with(|| read(face, id).map(Arc::new))
            .clone()
    })
}

fn read(face: &Face, id: GlyphId) -> Option<Picture> {
    let raster = face.glyph_raster_image(id, PIXELS)?;

    if raster.format != RasterImageFormat::PNG {
        return None;
    }

    let (width, height, pixels) = decode_png(Cursor::new(raster.data))?;

    Some(Picture {
        image: Arc::new(Bitmap::from_rgba(width, height, pixels)),
        left: f32::from(raster.x),
        bottom: f32::from(raster.y),
        pixels_per_em: f32::from(raster.pixels_per_em),
    })
}
