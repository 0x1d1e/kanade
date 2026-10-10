//! Text on the lock screen: shaped by rustybuzz and filled from the font's own outlines, so it
//! needs no text stack of its own; a variable font is set to the weight asked.

use std::path::PathBuf;
use std::sync::Arc;

use rustybuzz::ttf_parser::{GlyphId, OutlineBuilder, Tag};
use rustybuzz::{Face, Feature, UnicodeBuffer, Variation};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

use crate::Color;

/*
 * a font file, the face in it, and its weight: a variable font is set to it, a static one is the
 * weight already
 */
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FontFile {
    pub path: PathBuf,

    // as fontconfig says it: a variable font's named instance above the face's own index
    pub index: u32,
    pub weight: f32,
}

impl FontFile {
    // the face in the file, without the named instance, which `weight` stands for
    fn face(&self) -> u32 {
        self.index & 0xFFFF
    }
}

// a face of a font file read, which others of the same file share
#[derive(Clone)]
pub(crate) struct Font {
    file: FontFile,
    data: Arc<[u8]>,
}

impl Font {
    // none if the face does not parse, so one kept always does
    pub(crate) fn new(file: &FontFile, data: Arc<[u8]>) -> Option<Font> {
        Face::from_slice(&data, file.face())?;

        Some(Font {
            file: file.clone(),
            data,
        })
    }

    // the same face at `weight`, as a fallback is set to the face it stands in for
    pub(crate) fn weighted(&self, weight: f32) -> Font {
        let mut font = self.clone();
        font.file.weight = weight;
        font
    }

    pub(crate) fn weight(&self) -> f32 {
        self.file.weight
    }

    // whether it has a glyph for every character of `text` but spaces and controls
    pub(crate) fn covers(&self, text: &str) -> bool {
        let face = self.face();
        text.chars()
            .filter(|character| !character.is_whitespace() && !character.is_control())
            .all(|character| face.glyph_index(character).is_some())
    }

    fn face(&self) -> Face<'_> {
        // parsed when read, so it parses
        let mut face = Face::from_slice(&self.data, self.file.face())
            .unwrap_or_else(|| unreachable!("a font read parses"));

        face.set_variations(&[Variation {
            tag: Tag::from_bytes(b"wght"),
            value: self.file.weight,
        }]);

        face
    }

    /*
     * `text` laid out at `size` pixels, its figures all as wide if `tabular`, so a clock does not
     * shift as it ticks
     */
    pub(crate) fn shape(&self, text: &str, size: f32, tabular: bool) -> Line {
        let face = self.face();
        let scale = size / face.units_per_em() as f32;

        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);

        let features = if tabular {
            vec![Feature::new(Tag::from_bytes(b"tnum"), 1, ..)]
        } else {
            Vec::new()
        };
        let shaped = rustybuzz::shape(&face, &features, buffer);

        let mut x = 0.0;
        let mut glyphs = Vec::new();
        for (info, position) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
            glyphs.push(Glyph {
                id: info.glyph_id as u16,
                x: x + position.x_offset as f32 * scale,
                y: position.y_offset as f32 * scale,
            });
            x += position.x_advance as f32 * scale;
        }

        Line {
            glyphs,
            width: x,
            size,
            ascender: f32::from(face.ascender()) * scale,
            descender: -f32::from(face.descender()) * scale,
            cap: face
                .capital_height()
                .map_or(size * 0.7, |cap| f32::from(cap) * scale),
        }
    }

    /*
     * `line` filled in `color` with its baseline's left end at `x`, `baseline`, faded by
     * `opacity`
     */
    pub(crate) fn draw(
        &self,
        pixmap: &mut Pixmap,
        line: &Line,
        (x, baseline): (f32, f32),
        color: Color,
        opacity: f32,
    ) {
        let face = self.face();
        let scale = line.size / face.units_per_em() as f32;
        let mut outline = Outline {
            path: PathBuilder::new(),
            scale,
            x: 0.0,
            y: 0.0,
        };

        for glyph in &line.glyphs {
            outline.x = x + glyph.x;
            outline.y = baseline - glyph.y;
            face.outline_glyph(GlyphId(glyph.id), &mut outline);
        }

        let Some(path) = outline.path.finish() else {
            return;
        };

        let mut paint = Paint::default();
        paint.set_color(color.faded(opacity).skia());
        paint.anti_alias = true;
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

#[derive(Debug, Clone, Copy)]
struct Glyph {
    id: u16,
    x: f32,
    y: f32,
}

// a run of text laid out, in pixels
pub(crate) struct Line {
    glyphs: Vec<Glyph>,
    pub width: f32,
    size: f32,

    // how far above and below the baseline the font reaches, and how tall its capitals are
    pub ascender: f32,
    pub descender: f32,
    pub cap: f32,
}

// the outlines of glyphs, from font units up, into one path at a pen position
struct Outline {
    path: PathBuilder,
    scale: f32,
    x: f32,
    y: f32,
}

impl Outline {
    fn at(&self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x * self.scale, self.y - y * self.scale)
    }
}

impl OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.path.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.path.line_to(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let ((x1, y1), (x, y)) = (self.at(x1, y1), self.at(x, y));
        self.path.quad_to(x1, y1, x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let ((x1, y1), (x2, y2), (x, y)) = (self.at(x1, y1), self.at(x2, y2), self.at(x, y));
        self.path.cubic_to(x1, y1, x2, y2, x, y);
    }

    fn close(&mut self) {
        self.path.close();
    }
}
