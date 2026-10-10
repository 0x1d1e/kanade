/*
 * the body's drop shadow, blurred once per corner radius and drawn as eight images around the
 * body: four corners at their own size and four edges stretched along it. A corner reaches a blur
 * along the body's long sides, so a body standing upright, as on a side edge, takes corners turned
 * to reach down it rather than across, which on a narrow pill would overlap. The runtime's own shadow
 * goes through Vello, which on every morphing frame cost enough gpu time to drop frames (#45);
 * images only move textures. The body is see-through glass, so every piece is cut out where the
 * body covers it, and there is no piece for its middle. Each piece is
 * a draw of its own, which on an integrated gpu costs more than the pixels it fills, so the
 * corners stay whole rather than cut down to what the body leaves showing
 */

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};
use std::thread;

use kanade_runtime::{Color, Image, Rectangle, Widget};

use crate::island::geometry::{self, Rect};
use crate::raster;
use crate::theme;

// texels per logical pixel, like the icons; the smaller copies the runtime prepares fit scale 1
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
    /*
     * soft and wide, like CSS's `0 4px 24px`: a 24 pixel blur is a spread of 12. The drop stays
     * under 4.8, past which a side edge's piece, a drop lower than the see-through body, would
     * reach into the curve of a corner of radius 12
     */
    pub(crate) const AMBIENT: Self = Self {
        blur: 12,
        drop: 4,
        color: theme::SHADOW_AMBIENT,
    };

    // close to the edge, like CSS's `0 2px 6px`
    pub(crate) const CONTACT: Self = Self {
        blur: 3,
        drop: 2,
        color: theme::SHADOW_CONTACT,
    };

    // both, the ambient one under; the ambient one reaches furthest
    pub(crate) const ALL: [Self; 2] = [Self::AMBIENT, Self::CONTACT];

    // how far past the body's edge the blur reaches, in logical pixels
    pub(crate) fn reach(self) -> u32 {
        (self.kernel() + 1).div_ceil(SCALE)
    }

    // the blur's half width in texels, three spreads, past which it adds nothing visible
    fn kernel(self) -> u32 {
        3 * self.blur * SCALE
    }
}

// a radius, turn and style blurred into the same pieces every time: radius, upright, blur, drop
// and color
type Key = (u32, bool, u32, u32, [u8; 4]);

// the corners top left, top right, bottom left, bottom right, then the top, bottom, left, right edges
type Pieces = [PathBuf; PIECES];

const PIECES: usize = 8;

static DRAWN: LazyLock<Mutex<HashMap<Key, Pieces>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/*
 * blurs and starts decoding the pieces for every radius a body rests at, the only place that
 * blurs: the resting body's right away, so the first frames have a shadow to draw, the rest on
 * their own thread. The runtime lets go of an image no window draws anymore, so later morphs decode
 * them again, the nearest ready radius standing in for the frame or two
 */
pub(crate) fn prepare() {
    for style in ShadowStyle::ALL {
        for upright in [false, true] {
            prepare_radius(geometry::REST.radius as u32, upright, style);
        }
    }

    thread::spawn(move || {
        for radius in radii() {
            for style in ShadowStyle::ALL {
                for upright in [false, true] {
                    prepare_radius(radius, upright, style);
                }
            }
        }
    });
}

fn prepare_radius(radius: u32, upright: bool, style: ShadowStyle) {
    let key = key(radius, upright, style);

    if lock().contains_key(&key) {
        return;
    }

    // blurred outside the lock, so drawing a frame never waits on this
    let pieces = pieces(radius, upright, style);

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
 * textures the runtime decodes and uploads again, which mid-morph costs more than the pixels (#45).
 * Pieces still decoding are stood in for by the nearest radius that is ready, so the shadow
 * never blinks out
 */
pub(crate) fn draw(layers: &mut Vec<Box<dyn Widget>>, body: Rect, radius: f32) {
    for style in ShadowStyle::ALL {
        draw_style(layers, body, radius, style);
    }
}

/*
 * as many layers as a shadow, drawing nothing: the runtime knows what the pointer pressed or drags by
 * its place among a window's targets, so what comes after a shadow keeps its place whether one
 * is cast or not, all of it or some
 */
pub(crate) fn none(layers: &mut Vec<Box<dyn Widget>>) {
    for _ in 0..ShadowStyle::ALL.len() * PIECES {
        layers.push(Box::new(nothing()));
    }
}

fn nothing() -> Rectangle {
    Rectangle::new().width(0.0).height(0.0)
}

fn draw_style(layers: &mut Vec<Box<dyn Widget>>, body: Rect, radius: f32, style: ShadowStyle) {
    let fits = (body.width.min(body.height) / 2.0).floor().max(0.0) as u32;
    let upright = body.height > body.width;

    let Some((radius, pieces)) = ready(radius, fits, upright, style) else {
        layers.extend((0..PIECES).map(|_| Box::new(nothing()) as Box<dyn Widget>));
        return;
    };

    let placed = place(body, radius, upright, style);

    for (piece, at) in pieces.into_iter().zip(placed) {
        if at.width <= 0.0 || at.height <= 0.0 {
            layers.push(Box::new(nothing()));
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

fn key(radius: u32, upright: bool, style: ShadowStyle) -> Key {
    let color = style.color;

    (
        radius,
        upright,
        style.blur,
        style.drop,
        [color.red(), color.green(), color.blue(), color.alpha()],
    )
}

/*
 * the pieces of the prepared radius nearest the wanted one that are all decoded and still fit
 * the body, else none. Only looks: asking starts a prepared radius decoding, and the runtime draws
 * the window again when it is done
 */
fn ready(wanted: f32, fits: u32, upright: bool, style: ShadowStyle) -> Option<(u32, Pieces)> {
    let drawn = lock();

    let mut prepared: Vec<(u32, &Pieces)> = radii()
        .into_iter()
        // a larger one would overlap its corners on a pill
        .filter(|radius| *radius <= fits)
        .filter_map(|radius| Some((radius, drawn.get(&key(radius, upright, style))?)))
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
 * corner reaches a blur past where the curve ends along the long sides, so it meets its edge where the shadow no
 * longer changes along it. The edges only cover what shows past the body: every pixel drawn
 * costs gpu time mid-morph, and the body hides the rest
 */
fn place(body: Rect, radius: u32, upright: bool, style: ShadowStyle) -> [Rect; PIECES] {
    let radius = radius as f32;
    let reach = style.reach() as f32;

    let drop = style.drop as f32;
    let overlap = OVERLAP as f32;

    let left = body.x;
    let top = body.y + drop;
    let right = left + body.width;
    let bottom = top + body.height;

    let (long, short) = (radius + 2.0 * reach, radius + reach);
    let (corner_width, corner_height) = if upright {
        (short, long)
    } else {
        (long, short)
    };

    // how far into the body a corner reaches across and down
    let (inner_width, inner_height) = (corner_width - reach, corner_height - reach);

    let corner = |x, y| Rect {
        x,
        y,
        width: corner_width,
        height: corner_height,
    };

    let across = Rect {
        x: left + inner_width,
        y: top - reach,
        width: body.width - 2.0 * inner_width,
        height: reach - drop + overlap,
    };

    let down = Rect {
        x: left - reach,
        y: top + inner_height,
        width: reach + overlap,
        height: body.height - 2.0 * inner_height,
    };

    [
        corner(left - reach, top - reach),
        corner(right - inner_width, top - reach),
        corner(left - reach, bottom - inner_height),
        corner(right - inner_width, bottom - inner_height),
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
fn pieces(radius: u32, upright: bool, style: ShadowStyle) -> Pieces {
    cut(radius, upright, style).map(|(width, height, pixels)| {
        raster::write("shadows", "png", &raster::png(width, height, &pixels))
    })
}

// the eight pieces' width, height and rgba pixels, in the order of Pieces
fn cut(radius: u32, upright: bool, style: ShadowStyle) -> [(u32, u32, Vec<u8>); PIECES] {
    let field = Field::blurred(radius, style);

    let alpha = f32::from(style.color.alpha());
    let tint = [style.color.red(), style.color.green(), style.color.blue()];

    let reach = style.reach() * SCALE;
    let curve = radius * SCALE;

    // a corner, a blur past the curve along the long sides; the edges take the column and row past it
    let (long, short) = (2 * reach + curve, reach + curve);
    let (width, height) = if upright {
        (short, long)
    } else {
        (long, short)
    };

    /*
     * the body itself is see-through glass, so the shadow is cut out where the body covers it:
     * the body sits `drop` above the shadow's body, which in a bottom piece's flipped field is
     * further in rather than further out
     */
    let drop = i64::from(style.drop * SCALE);
    let outside = |x: u32, y: u32, shift: i64| {
        let reach = i64::from(reach);

        1.0 - coverage(
            i64::from(x) - reach,
            i64::from(y) - reach + shift,
            i64::from(curve),
        )
    };

    let write = |width: u32, height: u32, shift: i64, at: &dyn Fn(u32, u32) -> (u32, u32)| {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);

        for y in 0..height {
            for x in 0..width {
                let (x, y) = at(x, y);

                let shade = (field.at(x, y) * outside(x, y, shift) * alpha).round() as u8;

                pixels.extend(tint);
                pixels.push(shade);
            }
        }

        (width, height, pixels)
    };

    // the top edge is the column just past the corner, the left edge the row just below it
    let column = width;
    let row = height;

    // as far as each edge reaches, from the shadow's outside in
    let above = reach - style.drop * SCALE + OVERLAP * SCALE;
    let below = reach + style.drop * SCALE + OVERLAP * SCALE;
    let beside = reach + OVERLAP * SCALE;

    [
        write(width, height, drop, &|x, y| (x, y)),
        write(width, height, drop, &|x, y| (width - 1 - x, y)),
        write(width, height, -drop, &|x, y| (x, height - 1 - y)),
        write(width, height, -drop, &|x, y| {
            (width - 1 - x, height - 1 - y)
        }),
        write(1, above, drop, &|_, y| (column, y)),
        write(1, below, -drop, &|_, y| (column, below - 1 - y)),
        write(beside, 1, 0, &|x, _| (x, row)),
        write(beside, 1, 0, &|x, _| (beside - 1 - x, row)),
    ]
}

/*
 * the top left of a body with straight sides running on forever, blurred, in texels from a
 * reach above and left of its corner: across and down to a reach past the curve and one texel
 * more. The corner is the same turned on its diagonal, so its blur is too
 */
struct Field {
    width: u32,
    values: Vec<f32>,
}

impl Field {
    fn blurred(radius: u32, style: ShadowStyle) -> Self {
        let reach = (style.reach() * SCALE) as i64;
        let curve = (radius * SCALE) as i64;
        let kernel = style.kernel() as i64;

        // square, so a corner turned upright reads it as well as one lying wide
        let width = (2 * reach + curve + 1) as u32;
        let height = width;

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

        Self { width, values }
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
