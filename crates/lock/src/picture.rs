//! The pictures behind the lock screen: the wallpaper scaled to cover an output, blurred or not and
//! shaded, and the frost the glass shows of it, worked out once per output and size, off the
//! client's thread; and the face, cut round.

use tiny_skia::{
    FillRule, FilterQuality, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Transform,
};

use crate::{Backdrop, Color};

// a decoded image: rows of straight-alpha RGBA, 8 bits each
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Picture {
    // premultiplied, as tiny-skia draws; none if its size and pixels disagree
    pub(crate) fn pixmap(&self) -> Option<Pixmap> {
        let mut pixmap = Pixmap::new(self.width, self.height)?;

        if self.rgba.len() != pixmap.data().len() {
            return None;
        }

        let (from, _) = self.rgba.as_chunks::<4>();
        for (to, from) in pixmap
            .data_mut()
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(from)
        {
            let alpha = u16::from(from[3]);
            let premultiply = |channel: u8| ((u16::from(channel) * alpha + 127) / 255) as u8;

            *to = [
                premultiply(from[0]),
                premultiply(from[1]),
                premultiply(from[2]),
                from[3],
            ];
        }

        Some(pixmap)
    }
}

// what the glass of the lock screen shows of the backdrop, as blurred as the shell's glass
const FROST: f32 = 24.0;

// how much smaller `lock.backdrop` blurred is worked out a logical pixel, and blurred there, as the
// shell's was
const BLURRED_BY: f32 = 8.0;
const BLURRED: u32 = 4;

// an output's backdrop, as shown, and its frost, a quarter the size, stretched as it is drawn
pub(crate) struct Prepared {
    pub shown: Pixmap,
    pub frost: Pixmap,
}

/*
 * the backdrop of an output `width` by `height` pixels at `scale`: the wallpaper, if it decoded,
 * covering it, else its color
 */
pub(crate) fn prepare(
    backdrop: &Backdrop,
    wallpaper: Option<&Pixmap>,
    (width, height): (u32, u32),
    scale: f32,
) -> Option<Prepared> {
    let mut shown = Pixmap::new(width, height)?;

    match (backdrop, wallpaper) {
        (
            Backdrop::Wallpaper {
                blurred,
                shade,
                otherwise,
                ..
            },
            Some(wallpaper),
        ) => {
            let covered = cover(wallpaper, width, height)?;

            // over its color, so a wallpaper with see-through parts never shows what is under
            // the lock
            fill(&mut shown, *otherwise);
            if *blurred {
                let by = (BLURRED_BY * scale).round().max(1.0) as u32;
                let mut small = shrink(&covered, by)?;
                blur(&mut small, BLURRED);
                stretch(&small, &mut shown);
            } else {
                stretch(&covered, &mut shown);
            }

            fill(&mut shown, *shade);
        }
        (Backdrop::Wallpaper { otherwise, .. }, None) | (Backdrop::Solid(otherwise), _) => {
            fill(&mut shown, *otherwise);
        }
    }

    // a quarter the size, blurred so as much as the shell's glass blurs at this scale
    let by = 4;
    let mut small = shrink(&shown, by)?;
    blur(
        &mut small,
        ((FROST * scale / by as f32) / 3.0).round().max(1.0) as u32,
    );

    Some(Prepared {
        shown,
        frost: small,
    })
}

// `picture` scaled to cover `width` by `height`, its middle kept
pub(crate) fn cover(picture: &Pixmap, width: u32, height: u32) -> Option<Pixmap> {
    let ratio =
        (width as f32 / picture.width() as f32).max(height as f32 / picture.height() as f32);

    // averaged down first, as a filter skips pixels when shrinking far
    let by = (1.0 / ratio).floor().max(1.0) as u32;
    let shrunk;
    let picture = if by > 1 {
        shrunk = shrink(picture, by)?;
        &shrunk
    } else {
        picture
    };

    let ratio =
        (width as f32 / picture.width() as f32).max(height as f32 / picture.height() as f32);
    let x = (width as f32 - picture.width() as f32 * ratio) / 2.0;
    let y = (height as f32 - picture.height() as f32 * ratio) / 2.0;

    let mut covered = Pixmap::new(width, height)?;
    covered.draw_pixmap(
        0,
        0,
        picture.as_ref(),
        &PixmapPaint {
            quality: FilterQuality::Bicubic,
            ..PixmapPaint::default()
        },
        Transform::from_row(ratio, 0.0, 0.0, ratio, x, y),
        None,
    );

    Some(covered)
}

// `pixmap` drawn over all of `onto`, stretched
fn stretch(pixmap: &Pixmap, onto: &mut Pixmap) {
    let (x, y) = (
        onto.width() as f32 / pixmap.width() as f32,
        onto.height() as f32 / pixmap.height() as f32,
    );

    onto.draw_pixmap(
        0,
        0,
        pixmap.as_ref(),
        &PixmapPaint {
            quality: FilterQuality::Bilinear,
            ..PixmapPaint::default()
        },
        Transform::from_scale(x, y),
        None,
    );
}

fn fill(pixmap: &mut Pixmap, color: Color) {
    let Some(rect) = Rect::from_xywh(0.0, 0.0, pixmap.width() as f32, pixmap.height() as f32)
    else {
        return;
    };

    let mut paint = Paint::default();
    paint.set_color(color.skia());
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

// `by` times smaller each way, each pixel the mean of those it covers
fn shrink(pixmap: &Pixmap, by: u32) -> Option<Pixmap> {
    let (width, height) = (
        pixmap.width().div_ceil(by).max(1),
        pixmap.height().div_ceil(by).max(1),
    );
    let mut small = Pixmap::new(width, height)?;
    let source = pixmap.data();
    let stride = pixmap.width() as usize * 4;

    for (row, line) in small
        .data_mut()
        .chunks_exact_mut(width as usize * 4)
        .enumerate()
    {
        let top = row as u32 * by;
        let bottom = (top + by).min(pixmap.height());

        for (column, pixel) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let left = column as u32 * by;
            let right = (left + by).min(pixmap.width());
            let mut sum = [0u32; 4];

            for y in top..bottom {
                let start = y as usize * stride + left as usize * 4;
                let end = y as usize * stride + right as usize * 4;

                for from in source[start..end].as_chunks::<4>().0 {
                    for (sum, channel) in sum.iter_mut().zip(from) {
                        *sum += u32::from(*channel);
                    }
                }
            }

            let count = (bottom - top) * (right - left);
            for (to, sum) in pixel.iter_mut().zip(sum) {
                *to = ((sum + count / 2) / count) as u8;
            }
        }
    }

    Some(small)
}

/*
 * three box blurs of `radius` each way, near enough a gaussian; on premultiplied pixels, so it
 * stays premultiplied
 */
pub(crate) fn blur(pixmap: &mut Pixmap, radius: u32) {
    let (width, height) = (pixmap.width() as usize, pixmap.height() as usize);
    let radius = radius as usize;
    let mut line = Vec::new();

    for _ in 0..3 {
        let data = pixmap.data_mut();

        for row in 0..height {
            box_blur(data, row * width * 4, 4, width, radius, &mut line);
        }

        for column in 0..width {
            box_blur(data, column * 4, width * 4, height, radius, &mut line);
        }
    }
}

// one pass along `count` pixels from `start`, `step` bytes apart, the edges held
fn box_blur(
    data: &mut [u8],
    start: usize,
    step: usize,
    count: usize,
    radius: usize,
    line: &mut Vec<[u8; 4]>,
) {
    line.clear();
    line.extend((0..count).map(|index| {
        let at = start + index * step;
        [data[at], data[at + 1], data[at + 2], data[at + 3]]
    }));

    let width = (2 * radius + 1) as u32;
    let pixel = |index: isize| line[index.clamp(0, count as isize - 1) as usize];
    let mut sum = [0u32; 4];

    for index in -(radius as isize)..=radius as isize {
        for (sum, channel) in sum.iter_mut().zip(pixel(index)) {
            *sum += u32::from(channel);
        }
    }

    for index in 0..count {
        let at = start + index * step;
        for (channel, sum) in sum.iter().enumerate() {
            data[at + channel] = ((sum + width / 2) / width) as u8;
        }

        let (gone, coming) = (
            pixel(index as isize - radius as isize),
            pixel(index as isize + radius as isize + 1),
        );
        for channel in 0..4 {
            sum[channel] = sum[channel] + u32::from(coming[channel]) - u32::from(gone[channel]);
        }
    }
}

// `picture` covering a circle `size` pixels across, edged smoothly
pub(crate) fn round(picture: &Pixmap, size: u32) -> Option<Pixmap> {
    let covered = cover(picture, size, size)?;
    let mut round = Pixmap::new(size, size)?;
    let half = size as f32 / 2.0;
    let circle = PathBuilder::from_circle(half, half, half)?;

    let paint = Paint {
        shader: tiny_skia::Pattern::new(
            covered.as_ref(),
            tiny_skia::SpreadMode::Pad,
            FilterQuality::Nearest,
            1.0,
            Transform::identity(),
        ),
        anti_alias: true,
        ..Paint::default()
    };
    round.fill_path(
        &circle,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    Some(round)
}

#[cfg(test)]
mod tests {
    use crate::Image;
    use std::path::PathBuf;

    use super::*;

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Pixmap {
        Picture {
            width,
            height,
            rgba: rgba.repeat((width * height) as usize),
        }
        .pixmap()
        .unwrap()
    }

    // nothing under the lock surface shows through a wallpaper's see-through parts
    #[test]
    fn a_see_through_wallpaper_shows_none_of_what_is_under_the_lock() {
        let wallpaper = solid(8, 8, [10, 20, 30, 0]);
        for blurred in [false, true] {
            let backdrop = Backdrop::Wallpaper {
                image: Image {
                    path: PathBuf::from("/wallpaper.png"),
                    modified: None,
                },
                blurred,
                shade: Color::rgba(0, 0, 0, 31),
                otherwise: Color::rgba(40, 40, 40, 255),
            };

            let prepared = prepare(&backdrop, Some(&wallpaper), (16, 16), 1.0).unwrap();
            assert!(
                prepared
                    .shown
                    .pixels()
                    .iter()
                    .all(|pixel| pixel.alpha() == 255)
            );
        }
    }
}
