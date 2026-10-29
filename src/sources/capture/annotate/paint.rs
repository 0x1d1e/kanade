//! Rasterizing the editor's shapes onto the screenshot's pixels, on the save worker, and the png a
//! saved annotation is written as. The preview draws the same geometry as vectors; nothing here
//! runs on the draw thread.

use kanade_runtime::Color;

use super::{Shape, Tool};

/*
 * the stroke's width in image pixels, the same for every shape: a fraction of the image's longest
 * side, so a mark reads the same on a small window and on a large one, but never a hairline
 */
pub fn stroke(width: u32, height: u32) -> f32 {
    (width.max(height) as f32 / 400.0).clamp(2.0, 12.0)
}

/*
 * the arrowhead's three points, its tip at `end` whichever way the arrow was drawn: the tip, then
 * the two base corners behind it. None for a shaft of no length
 */
pub fn arrow_head(start: (f32, f32), end: (f32, f32), thickness: f32) -> Option<[(f32, f32); 3]> {
    let (dx, dy) = (end.0 - start.0, end.1 - start.1);
    let length = (dx * dx + dy * dy).sqrt();

    if length < f32::EPSILON {
        return None;
    }

    let (unit_x, unit_y) = (dx / length, dy / length);
    let reach = (length * 0.3)
        .clamp(thickness * 3.0, thickness * 7.0)
        .min(length);
    let base = (end.0 - unit_x * reach, end.1 - unit_y * reach);
    let spread = reach * 0.45;

    // the base corners stand off the shaft's line, a half width each way
    Some([
        (end.0, end.1),
        (base.0 - unit_y * spread, base.1 + unit_x * spread),
        (base.0 + unit_y * spread, base.1 - unit_x * spread),
    ])
}

// draws `shapes` onto premultiplied-free RGBA `pixels`, width * height of them
pub fn paint(pixels: &mut [u8], width: u32, height: u32, shapes: &[Shape], colour: Color) {
    let thickness = stroke(width, height);

    for shape in shapes {
        match shape.tool {
            Tool::Rectangle => {
                let (left, top) = (shape.min_x(), shape.min_y());
                let (right, bottom) = (shape.max_x(), shape.max_y());

                segment(
                    pixels,
                    width,
                    height,
                    (left, top),
                    (right, top),
                    thickness,
                    colour,
                );
                segment(
                    pixels,
                    width,
                    height,
                    (right, top),
                    (right, bottom),
                    thickness,
                    colour,
                );
                segment(
                    pixels,
                    width,
                    height,
                    (right, bottom),
                    (left, bottom),
                    thickness,
                    colour,
                );
                segment(
                    pixels,
                    width,
                    height,
                    (left, bottom),
                    (left, top),
                    thickness,
                    colour,
                );
            }
            Tool::Arrow => {
                segment(
                    pixels,
                    width,
                    height,
                    shape.start,
                    shape.end,
                    thickness,
                    colour,
                );

                if let Some(head) = arrow_head(shape.start, shape.end, thickness) {
                    triangle(pixels, width, height, head, colour);
                }
            }
        }
    }
}

fn segment(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    start: (f32, f32),
    end: (f32, f32),
    thickness: f32,
    colour: Color,
) {
    let half = thickness / 2.0;
    let left = (start.0.min(end.0) - half).floor() as i64;
    let right = (start.0.max(end.0) + half).ceil() as i64;
    let top = (start.1.min(end.1) - half).floor() as i64;
    let bottom = (start.1.max(end.1) + half).ceil() as i64;

    for y in top.max(0)..=bottom.min(i64::from(height) - 1) {
        for x in left.max(0)..=right.min(i64::from(width) - 1) {
            let distance = distance(x as f32 + 0.5, y as f32 + 0.5, start, end);
            let coverage = (half + 0.5 - distance).clamp(0.0, 1.0);

            if coverage > 0.0 {
                blend(pixels, width, x as u32, y as u32, colour, coverage);
            }
        }
    }
}

fn triangle(pixels: &mut [u8], width: u32, height: u32, points: [(f32, f32); 3], colour: Color) {
    let [a, b, c] = points;
    let left = a.0.min(b.0).min(c.0).floor() as i64;
    let right = a.0.max(b.0).max(c.0).ceil() as i64;
    let top = a.1.min(b.1).min(c.1).floor() as i64;
    let bottom = a.1.max(b.1).max(c.1).ceil() as i64;

    for y in top.max(0)..=bottom.min(i64::from(height) - 1) {
        for x in left.max(0)..=right.min(i64::from(width) - 1) {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let first = edge(a, b, px, py);
            let second = edge(b, c, px, py);
            let third = edge(c, a, px, py);
            let inside = (first >= 0.0 && second >= 0.0 && third >= 0.0)
                || (first <= 0.0 && second <= 0.0 && third <= 0.0);
            let margin = first.abs().min(second.abs()).min(third.abs());
            let coverage = (if inside { margin } else { -margin } + 0.5).clamp(0.0, 1.0);

            if coverage > 0.0 {
                blend(pixels, width, x as u32, y as u32, colour, coverage);
            }
        }
    }
}

// the signed distance from the point to the line through `a` and `b`, positive on one side
fn edge(a: (f32, f32), b: (f32, f32), x: f32, y: f32) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = (dx * dx + dy * dy).sqrt();

    if length < f32::EPSILON {
        return 0.0;
    }

    ((x - a.0) * dy - (y - a.1) * dx) / length
}

// the distance from the point to the segment `start`-`end`
fn distance(x: f32, y: f32, start: (f32, f32), end: (f32, f32)) -> f32 {
    let (dx, dy) = (end.0 - start.0, end.1 - start.1);
    let length_squared = dx * dx + dy * dy;

    if length_squared < f32::EPSILON {
        return ((x - start.0).powi(2) + (y - start.1).powi(2)).sqrt();
    }

    let t = (((x - start.0) * dx + (y - start.1) * dy) / length_squared).clamp(0.0, 1.0);
    let (near_x, near_y) = (start.0 + t * dx, start.1 + t * dy);

    ((x - near_x).powi(2) + (y - near_y).powi(2)).sqrt()
}

fn blend(pixels: &mut [u8], width: u32, x: u32, y: u32, colour: Color, coverage: f32) {
    let index = (y as usize * width as usize + x as usize) * 4;
    let alpha = (colour.alpha() as f32 / 255.0) * coverage.clamp(0.0, 1.0);

    let Some(pixel) = pixels.get_mut(index..index + 4) else {
        return;
    };

    for (channel, source) in pixel
        .iter_mut()
        .zip([colour.red(), colour.green(), colour.blue()])
    {
        *channel = (source as f32 * alpha + *channel as f32 * (1.0 - alpha)).round() as u8;
    }

    // a screenshot is opaque, so a mark neither cuts a hole nor fades it
    pixel[3] = 255;
}

// an 8-bit RGBA png of the pixels
pub fn encode(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();

    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);

    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(pixels))
        .map_err(|error| format!("cannot encode the picture: {error}"))?;

    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arrowhead_sits_at_its_end_whichever_way_it_was_drawn() {
        let starts = [(0.0, 0.0), (100.0, 100.0), (100.0, 0.0), (0.0, 100.0)];
        let ends = [(100.0, 100.0), (0.0, 0.0), (0.0, 100.0), (100.0, 0.0)];

        for (start, end) in starts.into_iter().zip(ends) {
            let [tip, left, right] = arrow_head(start, end, 3.0).expect("a shaft has a head");

            assert_eq!(tip, end);

            // both corners stand behind the tip, toward where the arrow came from
            for corner in [left, right] {
                let behind =
                    (corner.0 - tip.0) * (start.0 - end.0) + (corner.1 - tip.1) * (start.1 - end.1);

                assert!(behind > 0.0, "{corner:?} is not behind {tip:?}");
            }

            // and they are a head's width apart, mirrored across the shaft
            let length = ((start.0 - end.0).powi(2) + (start.1 - end.1).powi(2)).sqrt();
            let head = ((left.0 - tip.0).powi(2) + (left.1 - tip.1).powi(2)).sqrt();

            assert!(head < length, "the head is longer than its arrow");
        }
    }

    #[test]
    fn a_mark_draws_on_the_pixels_without_a_hole() {
        let mut pixels = vec![0u8; 20 * 20 * 4];

        paint(
            &mut pixels,
            20,
            20,
            &[Shape {
                tool: Tool::Rectangle,
                start: (4.0, 4.0),
                end: (16.0, 16.0),
            }],
            Color::WHITE,
        );

        // the outline is marked and its middle is left alone
        let at = |x: usize, y: usize| pixels[(y * 20 + x) * 4];
        assert!(at(4, 10) > 0, "the left edge was not drawn");
        assert!(at(10, 4) > 0, "the top edge was not drawn");
        assert_eq!(at(10, 10), 0, "the rectangle's middle was drawn");
        // a mark never fades the pixels it covers
        assert_eq!(pixels[(10 * 20 + 4) * 4 + 3], 255);
    }
}
