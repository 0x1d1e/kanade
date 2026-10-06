/*
 * the body's drop shadow, blurred once per corner radius and drawn as eight images around the
 * body: four corners at their own size and four edges stretched along it. Amane's own shadow
 * goes through Vello, which on every morphing frame cost enough gpu time to drop frames (#45);
 * images only move textures. The body covers the middle, so it has no piece there. Each piece is
 * a draw of its own, which on an integrated gpu costs more than the pixels it fills, so the
 * corners stay whole rather than cut down to what the body leaves showing
 */

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};
use std::thread;

use amane::{Color, Image, Rectangle, Widget};

use crate::island::geometry::{self, Rect};
use crate::raster;
use crate::theme;

// texels per logical pixel, like the icons; the smaller copies Amane prepares fit scale 1
const SCALE: u32 = 2;

// the body's coverage at a corner's curve is the share of these points per texel inside it
const SAMPLES: u32 = 4;

// how far, in logical pixels, the edges reach under the body, so no line shows between them
// where its edge falls between pixels
const OVERLAP: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShadowStyle {
    // the gaussian's spread, in logical pixels
    pub blur: u32,

    // how far below the body it falls, at most the smallest radius so the body hides its middle
    pub drop: u32,

    // its alpha is the opacity under the body's middle
    pub color: Color,
}

impl ShadowStyle {
    // subtle (plan 7)
    pub(crate) const fn island() -> Self {
        Self {
            blur: 6,
            drop: 4,
            color: theme::SHADOW,
        }
    }

    // how far past the body's edge the blur reaches, in logical pixels
    pub(crate) fn reach(self) -> u32 {
        (self.kernel() + 1).div_ceil(SCALE)
    }

    // the blur's half width in texels, three spreads, past which it adds nothing visible
    fn kernel(self) -> u32 {
        3 * self.blur * SCALE
    }
}

// a radius and style blurred into the same pieces every time: radius, blur, drop and color
type Key = (u32, u32, u32, [u8; 4]);

// the corners top left, top right, bottom left, bottom right, then the top, bottom, left, right edges
type Pieces = [PathBuf; 8];

static DRAWN: LazyLock<Mutex<HashMap<Key, Pieces>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/*
 * blurs and starts decoding the pieces for every radius a body rests at, the only place that
 * blurs: the resting body's right away, so the first frames have a shadow to draw, the rest on
 * their own thread. Amane lets go of an image no window draws anymore, so later morphs decode
 * them again, the nearest ready radius standing in for the frame or two
 */
pub(crate) fn prepare(style: ShadowStyle) {
    prepare_radius(geometry::REST.radius as u32, style);

    thread::spawn(move || {
        for radius in radii() {
            prepare_radius(radius, style);
        }
    });
}

fn prepare_radius(radius: u32, style: ShadowStyle) {
    let key = key(radius, style);

    if lock().contains_key(&key) {
        return;
    }

    // blurred outside the lock, so drawing a frame never waits on this
    let pieces = pieces(radius, style);

    for piece in &pieces {
        Image::loaded(piece);
    }

    lock().entry(key).or_insert(pieces);
}

fn lock() -> MutexGuard<'static, HashMap<Key, Pieces>> {
    DRAWN.lock().unwrap_or_else(PoisonError::into_inner)
}

/*
 * the shadow of a body with this corner radius, to go under it. Mid-morph the nearest resting
 * radius stands in, a few pixels off under a blur in motion: every radius drawn is a set of
 * textures Amane decodes and uploads again, which mid-morph costs more than the pixels (#45).
 * Pieces still decoding are stood in for by the nearest radius that is ready, so the shadow
 * never blinks out
 */
pub(crate) fn draw(layers: &mut Vec<Box<dyn Widget>>, body: Rect, radius: f32, style: ShadowStyle) {
    let fits = (body.width.min(body.height) / 2.0).floor().max(0.0) as u32;

    let Some((radius, pieces)) = ready(radius, fits, style) else {
        return;
    };

    let placed = place(body, radius, style);

    for (piece, at) in pieces.into_iter().zip(placed) {
        if at.width <= 0.0 || at.height <= 0.0 {
            continue;
        }

        let image = Image::stretch(piece);

        layers.push(Box::new(
            Rectangle::new()
                .width(at.width)
                .height(at.height)
                .fill(image)
                .translate(at.x, at.y),
        ));
    }
}

// every corner radius a body rests at, smallest first
fn radii() -> Vec<u32> {
    let mut radii = geometry::SHAPES.map(|shape| shape.radius as u32).to_vec();

    radii.sort_unstable();
    radii.dedup();
    radii
}

fn key(radius: u32, style: ShadowStyle) -> Key {
    let color = style.color;

    (
        radius,
        style.blur,
        style.drop,
        [color.red(), color.green(), color.blue(), color.alpha()],
    )
}

/*
 * the pieces of the prepared radius nearest the wanted one that are all decoded and still fit
 * the body, else none. Only looks: asking starts a prepared radius decoding, and Amane draws
 * the window again when it is done
 */
fn ready(wanted: f32, fits: u32, style: ShadowStyle) -> Option<(u32, Pieces)> {
    let drawn = lock();

    let mut prepared: Vec<(u32, &Pieces)> = radii()
        .into_iter()
        // a larger one would overlap its corners on a pill
        .filter(|radius| *radius <= fits)
        .filter_map(|radius| Some((radius, drawn.get(&key(radius, style))?)))
        .collect();

    // the nearest the wanted radius first
    prepared.sort_by(|(one, _), (other, _)| {
        (*one as f32 - wanted)
            .abs()
            .total_cmp(&(*other as f32 - wanted).abs())
    });

    prepared
        .into_iter()
        // every piece asked for, so all of them start decoding at once
        .find(|(_, pieces)| pieces.iter().filter(|piece| !Image::loaded(piece)).count() == 0)
        .map(|(radius, pieces)| (radius, pieces.clone()))
}

/*
 * where each piece goes, in the order of Pieces, around the body moved down by the drop. A
 * corner reaches a blur past where the curve ends, so it meets its edge where the shadow no
 * longer changes along it. The edges only cover what shows past the body: every pixel drawn
 * costs gpu time mid-morph, and the body hides the rest
 */
fn place(body: Rect, radius: u32, style: ShadowStyle) -> [Rect; 8] {
    let radius = radius as f32;
    let reach = style.reach() as f32;

    let drop = style.drop as f32;
    let overlap = OVERLAP as f32;

    let left = body.x;
    let top = body.y + drop;
    let right = left + body.width;
    let bottom = top + body.height;

    let corner_width = radius + 2.0 * reach;
    let corner_height = radius + reach;

    let corner = |x, y| Rect {
        x,
        y,
        width: corner_width,
        height: corner_height,
    };

    let across = Rect {
        x: left + radius + reach,
        y: top - reach,
        width: body.width - 2.0 * (radius + reach),
        height: reach - drop + overlap,
    };

    let down = Rect {
        x: left - reach,
        y: top + radius,
        width: reach + overlap,
        height: body.height - 2.0 * radius,
    };

    [
        corner(left - reach, top - reach),
        corner(right - radius - reach, top - reach),
        corner(left - reach, bottom - radius),
        corner(right - radius - reach, bottom - radius),
        across,
        Rect {
            y: bottom - drop - overlap,
            height: reach + drop + overlap,
            ..across
        },
        down,
        Rect {
            x: right - overlap,
            ..down
        },
    ]
}

// blurs a corner of this radius and cuts it into the eight pieces, written as pngs
fn pieces(radius: u32, style: ShadowStyle) -> Pieces {
    cut(radius, style).map(|(width, height, pixels)| {
        raster::write("shadows", "png", &raster::png(width, height, &pixels))
    })
}

// the eight pieces' width, height and rgba pixels, in the order of Pieces
fn cut(radius: u32, style: ShadowStyle) -> [(u32, u32, Vec<u8>); 8] {
    let field = Field::blurred(radius, style);

    let alpha = f32::from(style.color.alpha());
    let tint = [style.color.red(), style.color.green(), style.color.blue()];

    let reach = style.reach() * SCALE;
    let curve = radius * SCALE;

    // a corner, as far as Field holds it bar the last column and row the edges take
    let (width, height) = (field.width - 1, field.height - 1);

    let write = |width: u32, height: u32, at: &dyn Fn(u32, u32) -> (u32, u32)| {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);

        for y in 0..height {
            for x in 0..width {
                let (x, y) = at(x, y);

                let shade = (field.at(x, y) * alpha).round() as u8;

                pixels.extend(tint);
                pixels.push(shade);
            }
        }

        (width, height, pixels)
    };

    // the top edge is the column just past the corner, the left edge the row just below the curve
    let column = width;
    let row = reach + curve;

    // as far as each edge reaches, from the shadow's outside in
    let above = reach - style.drop * SCALE + OVERLAP * SCALE;
    let below = reach + style.drop * SCALE + OVERLAP * SCALE;
    let beside = reach + OVERLAP * SCALE;

    [
        write(width, height, &|x, y| (x, y)),
        write(width, height, &|x, y| (width - 1 - x, y)),
        write(width, height, &|x, y| (x, height - 1 - y)),
        write(width, height, &|x, y| (width - 1 - x, height - 1 - y)),
        write(1, above, &|_, y| (column, y)),
        write(1, below, &|_, y| (column, below - 1 - y)),
        write(beside, 1, &|x, _| (x, row)),
        write(beside, 1, &|x, _| (beside - 1 - x, row)),
    ]
}

/*
 * the top left of a body with straight sides running on forever, blurred, in texels from a
 * reach above and left of its corner: across to a reach past the curve and one column more,
 * down to the curve's end and one row more
 */
struct Field {
    width: u32,
    height: u32,
    values: Vec<f32>,
}

impl Field {
    fn blurred(radius: u32, style: ShadowStyle) -> Self {
        let reach = (style.reach() * SCALE) as i64;
        let curve = (radius * SCALE) as i64;
        let kernel = style.kernel() as i64;

        let width = (2 * reach + curve + 1) as u32;
        let height = (reach + curve + 1) as u32;

        let weights = gaussian(style.blur * SCALE, style.kernel());

        // the body's coverage, a kernel wider on every side than what the blur fills
        let (left, top) = (-reach - kernel, -reach - kernel);
        let covered_width = width as i64 + 2 * kernel;
        let covered_height = height as i64 + 2 * kernel;

        let covered: Vec<f32> = (0..covered_height)
            .flat_map(|y| (0..covered_width).map(move |x| coverage(left + x, top + y, curve)))
            .collect();

        // across, then down: a gaussian blurs one direction at a time
        let mut across = vec![0.0; (width as i64 * covered_height) as usize];

        for y in 0..covered_height {
            for x in 0..width as i64 {
                across[(y * width as i64 + x) as usize] = weights
                    .iter()
                    .enumerate()
                    .map(|(tap, weight)| {
                        weight * covered[(y * covered_width + x + tap as i64) as usize]
                    })
                    .sum();
            }
        }

        let mut values = vec![0.0; (width * height) as usize];

        for y in 0..height as i64 {
            for x in 0..width as i64 {
                values[(y * width as i64 + x) as usize] = weights
                    .iter()
                    .enumerate()
                    .map(|(tap, weight)| {
                        weight * across[((y + tap as i64) * width as i64 + x) as usize]
                    })
                    .sum();
            }
        }

        Self {
            width,
            height,
            values,
        }
    }

    fn at(&self, x: u32, y: u32) -> f32 {
        self.values[(y * self.width + x) as usize].clamp(0.0, 1.0)
    }
}

// how much of the texel right and below of (x, y) the corner covers, its curve centered at (curve, curve)
fn coverage(x: i64, y: i64, curve: i64) -> f32 {
    if x < 0 || y < 0 {
        return 0.0;
    }

    if x >= curve || y >= curve {
        return 1.0;
    }

    let center = curve as f32;
    let step = 1.0 / SAMPLES as f32;

    let inside = (0..SAMPLES * SAMPLES)
        .filter(|sample| {
            let dx = x as f32 + (sample % SAMPLES) as f32 * step + step / 2.0 - center;
            let dy = y as f32 + (sample / SAMPLES) as f32 * step + step / 2.0 - center;

            dx * dx + dy * dy <= center * center
        })
        .count();

    inside as f32 / (SAMPLES * SAMPLES) as f32
}

// weights from -kernel to kernel, adding up to one
fn gaussian(spread: u32, kernel: u32) -> Vec<f32> {
    let spread = spread.max(1) as f32;
    let kernel = kernel as i64;

    let weights: Vec<f32> = (-kernel..=kernel)
        .map(|tap| (-((tap * tap) as f32) / (2.0 * spread * spread)).exp())
        .collect();

    let total: f32 = weights.iter().sum();

    weights.into_iter().map(|weight| weight / total).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLE: ShadowStyle = ShadowStyle::island();

    // where (x, y), in texels from the top left of the body moved down by the drop, reads from the
    // corner's Field
    fn folded(x: i64, y: i64, width: i64, height: i64, radius: i64) -> Option<(u32, u32)> {
        let reach = i64::from(STYLE.reach() * SCALE);

        let x = if x < width / 2 { x } else { width - 1 - x };
        let y = if y < height / 2 { y } else { height - 1 - y };

        // past the corner along the top and bottom, the edge's column
        let x = x.min(radius + reach);

        // down the sides, the edge's row; the middle is the body's
        let y = match (x < radius, y < radius) {
            (_, true) => y,
            (true, false) => radius,
            (false, false) => return None,
        };

        Some(((x + reach) as u32, (y + reach) as u32))
    }

    // the whole body's coverage blurred at once, the shadow the pieces stand in for
    fn exact(width: i64, height: i64, radius: i64) -> (Vec<f32>, i64) {
        let pad = i64::from(STYLE.reach() * SCALE + STYLE.kernel());
        let size = (width + 2 * pad, height + 2 * pad);

        let covered = |x: i64, y: i64| {
            let (x, y) = (x - pad, y - pad);

            if x < 0 || y < 0 || x >= width || y >= height {
                return 0.0;
            }

            // each corner seen as the top left one
            let x = x.min(width - 1 - x);
            let y = y.min(height - 1 - y);

            coverage(x, y, radius)
        };

        let weights = gaussian(STYLE.blur * SCALE, STYLE.kernel());
        let kernel = i64::from(STYLE.kernel());

        let blur = |value: &dyn Fn(i64, i64) -> f32, x: i64, y: i64, across: bool| -> f32 {
            weights
                .iter()
                .zip(-kernel..)
                .map(|(weight, tap)| {
                    let (x, y) = if across { (x + tap, y) } else { (x, y + tap) };

                    if x < 0 || y < 0 || x >= size.0 || y >= size.1 {
                        0.0
                    } else {
                        weight * value(x, y)
                    }
                })
                .sum()
        };

        let across: Vec<f32> = (0..size.1)
            .flat_map(|y| (0..size.0).map(move |x| (x, y)))
            .map(|(x, y)| blur(&covered, x, y, true))
            .collect();

        let read = |x: i64, y: i64| across[(y * size.0 + x) as usize];

        let values = (0..size.1)
            .flat_map(|y| (0..size.0).map(move |x| (x, y)))
            .map(|(x, y)| blur(&read, x, y, false))
            .collect();

        (values, pad)
    }

    #[test]
    fn pieces_match_a_blur_of_the_whole_body_wherever_it_shows() {
        // a pill, a short rounded body, and a Surface's
        for (width, height, radius) in [(150, 32, 16), (220, 60, 24), (300, 120, 32)] {
            let field = Field::blurred(radius, STYLE);

            let (width, height, curve) = (
                i64::from(width * SCALE),
                i64::from(height * SCALE),
                i64::from(radius * SCALE),
            );

            let (exact, pad) = exact(width, height, curve);
            let reach = i64::from(STYLE.reach() * SCALE);
            let drop = i64::from(STYLE.drop * SCALE);

            let mut worst = 0.0f32;

            for y in -reach..height + reach {
                for x in -reach..width + reach {
                    let truth = exact[((y + pad) * (width + 2 * pad) + x + pad) as usize];

                    let drawn = match folded(x, y, width, height, curve) {
                        Some((x, y)) => field.at(x, y),
                        None => {
                            // the middle has no piece, the body moved up by the drop covers it
                            let under =
                                (0..width).contains(&x) && (-drop..height - drop).contains(&(y));
                            assert!(under, "uncovered middle at ({x}, {y})");

                            continue;
                        }
                    };

                    // the body hides the shadow under it
                    let hidden = (0..width).contains(&x)
                        && (-drop..height - drop).contains(&y)
                        && coverage(
                            x.min(width - 1 - x),
                            (y + drop).min(height - 1 - y - drop),
                            curve,
                        ) == 1.0;

                    if !hidden {
                        worst = worst.max((truth - drawn).abs());
                    }
                }
            }

            // in alpha, about a percent at a pill's ends, whose curve the corners take as straight
            // below them; a smooth difference, never a seam
            let alpha = worst * f32::from(STYLE.color.alpha()) / 255.0;

            assert!(alpha < 0.015, "{width}x{height} r{radius}: off by {alpha}");
        }
    }

    #[test]
    fn pieces_cover_what_the_body_leaves_showing_once() {
        for shape in geometry::SHAPES {
            let body = geometry::body(shape);
            let radius = shape.radius as u32;
            let placed = place(body, radius, STYLE);

            let reach = STYLE.reach() as f32;
            let drop = STYLE.drop as f32;
            let overlap = OVERLAP as f32;

            // the body pulled in by the overlap, which the pieces may leave bare
            let (left, top) = (body.x + overlap, body.y + overlap);
            let (right, bottom) = (
                body.x + body.width - overlap,
                body.y + body.height - overlap,
            );
            let curve = shape.radius - overlap;

            let bare = |x: f32, y: f32| {
                let dx = (left + curve - x).max(x - right + curve).max(0.0);
                let dy = (top + curve - y).max(y - bottom + curve).max(0.0);

                (left..right).contains(&x)
                    && (top..bottom).contains(&y)
                    && dx * dx + dy * dy <= curve * curve
            };

            // the middle of each half pixel
            let steps = |from: f32, to: f32| {
                let count = ((to - from) * 2.0) as u32;
                (0..count).map(move |step| from + (step as f32 + 0.5) / 2.0)
            };

            for y in steps(body.y + drop - reach, body.y + drop + body.height + reach) {
                for x in steps(body.x - reach, body.x + body.width + reach) {
                    let covering = placed
                        .iter()
                        .filter(|at| {
                            (at.x..at.x + at.width).contains(&x)
                                && (at.y..at.y + at.height).contains(&y)
                        })
                        .count();

                    assert!(covering <= 1, "{shape:?} overlaps at ({x}, {y})");
                    assert!(covering == 1 || bare(x, y), "{shape:?} gap at ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn each_texel_reads_the_field_where_its_piece_lies() {
        // a pill and a Surface's body, on whole pixels so texels line up with the field's
        for (body, radius) in [
            (
                Rect {
                    x: 40.0,
                    y: 30.0,
                    width: 150.0,
                    height: 32.0,
                },
                16,
            ),
            (
                Rect {
                    x: 40.0,
                    y: 30.0,
                    width: 300.0,
                    height: 120.0,
                },
                32,
            ),
        ] {
            let field = Field::blurred(radius, STYLE);
            let alpha = f32::from(STYLE.color.alpha());

            let top = body.y + STYLE.drop as f32;
            let texels = |length: f32| (length * SCALE as f32) as i64;

            let (width, height) = (texels(body.width), texels(body.height));
            let curve = i64::from(radius * SCALE);

            for (at, (columns, rows, pixels)) in place(body, radius, STYLE)
                .into_iter()
                .zip(cut(radius, STYLE))
                // a pill has no side edges to draw
                .filter(|(at, _)| at.width > 0.0 && at.height > 0.0)
            {
                for row in 0..rows {
                    for column in 0..columns {
                        let x = texels(at.x - body.x) + i64::from(column);
                        let y = texels(at.y - top) + i64::from(row);

                        let (x, y) = folded(x, y, width, height, curve)
                            .unwrap_or_else(|| panic!("a piece over the middle at ({x}, {y})"));

                        let shade = (field.at(x, y) * alpha).round() as u8;

                        assert_eq!(pixels[((row * columns + column) * 4 + 3) as usize], shade);
                    }
                }
            }
        }
    }

    #[test]
    fn the_largest_shadow_fits_the_canvas_below_and_beside_the_body() {
        let body = geometry::body(geometry::EXPANDED_MAX);
        let reach = STYLE.reach() as f32;

        assert!(body.x - reach >= 0.0);
        assert!(body.x + body.width + reach <= geometry::CANVAS_WIDTH);
        assert!(body.y + body.height + STYLE.drop as f32 + reach <= geometry::CANVAS_HEIGHT);
    }

    #[test]
    fn the_body_hides_the_middle_at_every_radius() {
        assert!(radii().iter().all(|radius| *radius >= STYLE.drop));
        assert_eq!(radii(), [16, 19, 26, 32]);
    }

    #[test]
    fn every_radius_is_written_once_as_eight_pngs() {
        for radius in radii() {
            prepare_radius(radius, STYLE);
        }

        let drawn = lock();

        for radius in radii() {
            let pieces = &drawn[&key(radius, STYLE)];

            for piece in pieces {
                assert!(std::fs::read(piece).unwrap().starts_with(b"\x89PNG"));
            }
        }

        // each piece is turned its own way, so no two share a file
        let pill = &drawn[&key(16, STYLE)];
        let files: std::collections::HashSet<_> = pill.iter().collect();

        assert_eq!(files.len(), 8);
    }
}
