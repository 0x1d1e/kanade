//! The lock screen's rim refraction, worked out on the CPU from a capture around the body. The shell's
//! glass bends its backdrop in its shader (`src/glass.wgsl`, ADR 0037) the same way; this crate draws
//! on the CPU and keeps its own, so it never depends on the shell's GPU (ADR 0025).
//!
//! A rim pixel shows what lies outside the edge beyond it, pulled in along the edge's normal and
//! squeezed toward the edge as a lens's rim does: the edge itself shows furthest out, the inner
//! side of the band what is just outside, the right way round. Blue reaches a little further than
//! red. Only what is outside the body is ever
//! sampled, so the body's own pixels never feed back into its rim; what the Island draws beside
//! it is bent in as part of the backdrop.

// a body inside its canvas, in logical pixels from the canvas's top left corner
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
}

// how far into the body the rim bends what is behind it, in logical pixels
const BAND: f32 = 12.0;

// how much further out than the edge the captured area reaches, enough for the deepest sample
pub const MARGIN: f32 = 13.0;

// how far past the edge the edge itself shows
const REACH: f32 = 7.0;

// how much further each channel reaches: red, green, blue
const SPREAD: [f32; 3] = [1.0, 1.04, 1.08];

// past the edge before the first sample, clear of the antialiased edge and its hairline
const GAP: f32 = 1.5;

// how far out the ring the backdrop's light is read from lies
const RING: f32 = 12.0;

// the capture holds every sample and its bilinear neighbour
const _: () = assert!(GAP + REACH * SPREAD[2] + 1.0 <= MARGIN && RING + 1.0 <= MARGIN);

// a captured area: rgba pixels, and where they are, in logical pixels relative to the body's corner
pub struct Capture<'a> {
    pub pixels: &'a [u8],
    pub width: usize,
    pub height: usize,

    // the body's top left corner in the capture, in logical pixels
    pub body_x: f32,
    pub body_y: f32,

    // physical pixels per logical pixel, across and down: a fractional scale rounds each apart
    pub scale: (f32, f32),
}

impl Capture<'_> {
    /*
     * how far a logical point relative to the body's corner lies past the captured area, 0 inside:
     * the area stops at the screen's edge, and an Island hangs a few pixels from it
     */
    fn beyond(&self, x: f32, y: f32) -> f32 {
        let (x, y) = (x + self.body_x, y + self.body_y);
        let (width, height) = (
            self.width as f32 / self.scale.0,
            self.height as f32 / self.scale.1,
        );

        let dx = (-x).max(x - width).max(0.0);
        let dy = (-y).max(y - height).max(0.0);

        dx.max(dy)
    }

    /*
     * how far a logical point relative to the body's corner can go along a direction before it
     * leaves the captured area
     */
    fn room(&self, x: f32, y: f32, direction: (f32, f32)) -> f32 {
        let (x, y) = (x + self.body_x, y + self.body_y);
        let (width, height) = (
            self.width as f32 / self.scale.0,
            self.height as f32 / self.scale.1,
        );

        let along = |at: f32, step: f32, size: f32| match step {
            step if step > 1e-6 => (size - at) / step,
            step if step < -1e-6 => -at / step,
            _ => f32::INFINITY,
        };

        along(x, direction.0, width).min(along(y, direction.1, height))
    }

    // one channel at a logical point relative to the body's corner, bilinear, clamped to the edges
    fn sample(&self, x: f32, y: f32, channel: usize) -> f32 {
        let px = ((x + self.body_x) * self.scale.0 - 0.5).clamp(0.0, (self.width - 1) as f32);
        let py = ((y + self.body_y) * self.scale.1 - 0.5).clamp(0.0, (self.height - 1) as f32);

        let (x0, y0) = (px.floor() as usize, py.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = (px - x0 as f32, py - y0 as f32);

        let at = |x: usize, y: usize| f32::from(self.pixels[(y * self.width + x) * 4 + channel]);

        let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
        let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;

        top * (1.0 - fy) + bottom * fy
    }
}

// the rim as rgba with plain alpha, as big as the body in physical pixels, and the backdrop's light
pub fn rim(capture: &Capture, body: Body) -> (u32, u32, Vec<u8>, f32) {
    let width = (body.width * capture.scale.0).round().max(1.0) as u32;
    let height = (body.height * capture.scale.1).round().max(1.0) as u32;

    let half = (body.width * 0.5, body.height * 0.5);
    let radius = body.radius.min(half.0).min(half.1);
    let band = BAND.min(half.0.min(half.1) * 0.6).max(1.0);

    let mut pixels = vec![0; (width * height * 4) as usize];

    for row in 0..height {
        for column in 0..width {
            let x = (column as f32 + 0.5) / capture.scale.0;
            let y = (row as f32 + 0.5) / capture.scale.1;

            let (distance, normal) = edge(x - half.0, y - half.1, half, radius);
            let inside = -distance;

            if inside < 0.0 || inside > band {
                continue;
            }

            let at = ((row * width + column) * 4) as usize;

            // squeezed into what the capture holds past the edge, where the screen ends close by
            let room = capture.room(x, y, normal) - inside;
            let reach = REACH * ((room - GAP) / (REACH * SPREAD[2])).clamp(0.0, 1.0);

            // how far past the edge it shows, the most at the edge, falling off as a lens's rim
            let past = |spread: f32| GAP + reach * spread * (1.0 - inside / band).powi(2);

            for (channel, spread) in SPREAD.into_iter().enumerate() {
                let out = inside + past(spread);

                pixels[at + channel] =
                    capture.sample(x + normal.0 * out, y + normal.1 * out, channel) as u8;
            }

            // with no room at all the rim fades rather than repeat the screen's edge
            let deepest = inside + past(SPREAD[2]);
            let beyond = capture.beyond(x + normal.0 * deepest, y + normal.1 * deepest);
            let kept = 1.0 - smoothstep(0.0, 3.0, beyond);

            // full at the edge, gone well before the band ends, into the backdrop seen through the middle
            let depth = inside / band;
            let fade = 1.0 - smoothstep(0.35, 1.0, depth);

            pixels[at + 3] = (fade * kept * 255.0).round() as u8;
        }
    }

    (
        width,
        height,
        pixels,
        light(capture, body.width, body.height),
    )
}

/*
 * how bright the backdrop just around the body is, 0 to 1: the mean luma of a ring outside it, for
 * the tint to keep the body's content readable over it
 */
fn light(capture: &Capture, body_width: f32, body_height: f32) -> f32 {
    let ring = RING;
    let step = 3.0;

    let mut total = 0.0;
    let mut count = 0.0;

    let mut along = |x: f32, y: f32| {
        // past the screen's edge, a sample would only repeat it
        if capture.beyond(x, y) > 0.0 {
            return;
        }

        let luma = 0.2126 * capture.sample(x, y, 0)
            + 0.7152 * capture.sample(x, y, 1)
            + 0.0722 * capture.sample(x, y, 2);

        total += luma / 255.0;
        count += 1.0;
    };

    let mut x = -ring;

    while x < body_width + ring {
        along(x, -ring);
        along(x, body_height + ring);
        x += step;
    }

    let mut y = 0.0;

    while y < body_height {
        along(-ring, y);
        along(body_width + ring, y);
        y += step;
    }

    if count == 0.0 { 0.0 } else { total / count }
}

/*
 * signed distance from a point, relative to the body's center, to its rounded edge, negative
 * inside, and the edge's outward normal there
 */
fn edge(x: f32, y: f32, half: (f32, f32), radius: f32) -> (f32, (f32, f32)) {
    let qx = x.abs() - half.0 + radius;
    let qy = y.abs() - half.1 + radius;

    let (distance, nx, ny) = if qx > 0.0 && qy > 0.0 {
        let length = (qx * qx + qy * qy).sqrt();

        (length - radius, qx / length, qy / length)
    } else if qx > qy {
        (qx - radius, 1.0, 0.0)
    } else {
        (qy - radius, 0.0, 1.0)
    };

    (distance, (nx * x.signum(), ny * y.signum()))
}

fn smoothstep(low: f32, high: f32, value: f32) -> f32 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);

    t * t * (3.0 - 2.0 * t)
}
