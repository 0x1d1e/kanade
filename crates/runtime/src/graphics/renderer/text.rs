use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use ttf_parser::{Face, GlyphId};

use crate::graphics::emoji::{self, Picture};
use crate::graphics::{Area, BezierPath, Color, Outline, Renderer, Transform, font};

use super::Command;

thread_local! {
    /*
     * each letter's outline, read from the font once; fonts stay loaded
     * for the whole run, so where a face lives in memory names it
     */
    static OUTLINES: RefCell<HashMap<(usize, u16), Option<BezierPath>>> =
        RefCell::new(HashMap::new());
}

impl Renderer {
    /*
     * letters are drawn as small cached pictures, which the gpu handles far
     * faster than outlines; only rotated or stretched text needs the outlines
     */
    pub fn text(
        &mut self,
        content: &str,
        font: &'static Face<'static>,
        size: f32,
        color: Color,
        area: Area,
    ) {
        match self.transform.even_scale() {
            Some(scale) => self.text_pictures(content, font, size * scale, color, area),
            None => self.text_outlines(content, font, size, color, area),
        }
    }

    /*
     * the line starts on a whole pixel and every letter keeps its rounded
     * distance from that start, so a moving line never shimmers apart
     */
    fn text_pictures(
        &mut self,
        content: &str,
        font: &'static Face<'static>,
        pixel_size: f32,
        color: Color,
        area: Area,
    ) {
        let units = pixel_size / f32::from(font.units_per_em());

        let start = self.transform.map(area.x, area.y);

        let line_x = start.0.round();
        let baseline = (start.1 + f32::from(font.ascender()) * units).round();

        let mut pen = 0.0;

        for letter in content.chars() {
            let measured = font::measure_letter(font, letter, pixel_size);

            let letter_x = line_x + f32::round(pen);

            pen += measured.advance;

            if let Some(picture) = emoji::picture(measured.face, measured.id) {
                self.picture(
                    &picture,
                    letter_x,
                    baseline,
                    pixel_size,
                    Transform::IDENTITY,
                );

                continue;
            }

            self.commands.push(Command::Glyph {
                face: measured.face,
                id: measured.id.0,
                size: pixel_size,
                color,
                x: letter_x,
                y: baseline,
            });
        }
    }

    fn text_outlines(&mut self, content: &str, font: &Face, size: f32, color: Color, area: Area) {
        // fonts measure in their own units, this turns them into pixels
        let units = size / f32::from(font.units_per_em());

        let baseline = area.y + f32::from(font.ascender()) * units;

        let mut pen_x = area.x;

        for letter in content.chars() {
            let measured = font::measure_letter(font, letter, size);

            let letter_x = pen_x;

            pen_x += measured.advance;

            if let Some(picture) = emoji::picture(measured.face, measured.id) {
                self.picture(&picture, letter_x, baseline, size, self.transform);

                continue;
            }

            // a space has no outline but still moves the pen
            let Some(path) = outline(measured.face, measured.id) else {
                continue;
            };

            let face_units = measured.units;

            // fonts point y upward, the screen points it downward
            let letter_transform =
                Transform::from_row(face_units, 0.0, 0.0, -face_units, letter_x, baseline);
            let transform = letter_transform.post_concat(self.transform);

            self.commands.push(Command::Fill {
                path,
                transform,
                color,
            });
        }
    }
}

impl Renderer {
    /*
     * a letter that is a picture, with its pen at `x` on the `baseline` and the text `size` tall,
     * both in the space `to_device` takes to the canvas
     */
    fn picture(
        &mut self,
        picture: &Picture,
        x: f32,
        baseline: f32,
        size: f32,
        to_device: Transform,
    ) {
        let scale = size / picture.pixels_per_em;

        let width = picture.image.width() as f32 * scale;
        let height = picture.image.height() as f32 * scale;

        let left = (x + picture.left * scale).round();
        let top = (baseline - picture.bottom * scale - height).round();

        self.commands.push(Command::Image {
            image: Arc::clone(&picture.image),
            transform: Transform::from_row(scale, 0.0, 0.0, scale, left, top)
                .post_concat(to_device),
            area: Area::new(left, top, width, height),
            radius: 0.0,
            clip_transform: to_device,
        });
    }
}

fn outline(font: &Face, id: GlyphId) -> Option<BezierPath> {
    let key = (std::ptr::from_ref(font) as usize, id.0);

    OUTLINES.with_borrow_mut(|outlines| {
        let path = outlines
            .entry(key)
            .or_insert_with(|| read_outline(font, id));

        path.clone()
    })
}

fn read_outline(font: &Face, id: GlyphId) -> Option<BezierPath> {
    let mut outline = Outline::new();

    font.outline_glyph(id, &mut outline)?;

    outline.finish()
}
