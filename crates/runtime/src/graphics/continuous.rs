//! Continuous shell corners, adapted from islandcn (MIT, Copyright (c) 2026 Leo).
//! See THIRD_PARTY_NOTICES.md. Circular controls keep their existing outline.

use super::PathBuilder;

pub const WGSL: &str = include_str!("continuous.wgsl");

type Point = [f32; 2];
type Cubic = [Point; 4];

#[derive(Clone, Copy)]
pub struct ContinuousCorner {
    curves: [Cubic; 3],
    extent: Point,
}

fn edge(radius: f32, room: f32) -> [f32; 5] {
    let smoothing = (room / radius - 1.0).clamp(0.0, 0.72);
    let angle = std::f32::consts::FRAC_PI_4 * smoothing;
    let p = (1.0 + smoothing) * radius;
    let along = radius * (1.0 - angle.sin());
    let across = radius * (1.0 - angle.cos());
    let c = radius * (angle / 2.0).tan() * angle.cos();
    let b = ((p - c - along) / 3.0).max(0.0);
    [p, b, along, across, angle]
}

impl ContinuousCorner {
    pub fn new(radius: f32, half_width: f32, half_height: f32) -> Self {
        let r = radius.clamp(0.001, half_width.min(half_height).max(0.001));
        let [px, bx, ax, cx, angle_x] = edge(r, half_width);
        let [py, by, ay, cy, angle_y] = edge(r, half_height);
        let handle =
            (4.0 / 3.0) * ((std::f32::consts::FRAC_PI_2 - angle_x - angle_y) / 4.0).tan() * r;
        Self {
            extent: [px, py],
            curves: [
                [
                    [px, 0.0],
                    [px - 2.0 * bx, 0.0],
                    [px - 3.0 * bx, 0.0],
                    [ax, cx],
                ],
                [
                    [ax, cx],
                    [ax - handle * angle_x.cos(), cx + handle * angle_x.sin()],
                    [cy + handle * angle_y.sin(), ay - handle * angle_y.cos()],
                    [cy, ay],
                ],
                [
                    [cy, ay],
                    [0.0, py - 3.0 * by],
                    [0.0, py - 2.0 * by],
                    [0.0, py],
                ],
            ],
        }
    }

    pub(super) fn trace(
        self,
        path: &mut PathBuilder,
        corner: Point,
        incoming: Point,
        outgoing: Point,
    ) {
        let point = |[x, y]: Point| {
            (
                corner[0] + incoming[0] * x + outgoing[0] * y,
                corner[1] + incoming[1] * x + outgoing[1] * y,
            )
        };
        let (x, y) = point(self.curves[0][0]);
        path.line_to(x, y);
        for curve in self.curves {
            let (x1, y1) = point(curve[1]);
            let (x2, y2) = point(curve[2]);
            let (x, y) = point(curve[3]);
            path.cubic_to(x1, y1, x2, y2, x, y);
        }
    }

    // Coordinates point inward from a corner. The straight edges extend beyond its shoulders.
    pub fn distance(self, x: f32, y: f32) -> f32 {
        if x >= self.extent[0] || y >= self.extent[1] {
            return -x.min(y);
        }
        let mut nearest = f32::INFINITY;
        let mut signed = 0.0;
        for c in self.curves {
            let mut t = 0.0;
            for step in 0..=4 {
                let trial = step as f32 / 4.0;
                let p = sample(c, trial).0;
                let d = (p[0] - x).powi(2) + (p[1] - y).powi(2);
                let old = sample(c, t).0;
                if d < (old[0] - x).powi(2) + (old[1] - y).powi(2) {
                    t = trial;
                }
            }
            for _ in 0..5 {
                let (p, d, dd) = sample(c, t);
                let q = [p[0] - x, p[1] - y];
                let denominator = d[0] * d[0] + d[1] * d[1] + q[0] * dd[0] + q[1] * dd[1];
                if denominator.abs() > 1e-6 {
                    t = (t - (q[0] * d[0] + q[1] * d[1]) / denominator).clamp(0.0, 1.0);
                }
            }
            let (p, d, _) = sample(c, t);
            let q = [x - p[0], y - p[1]];
            let squared = q[0] * q[0] + q[1] * q[1];
            if squared < nearest && d[0].abs() + d[1].abs() > 1e-6 {
                nearest = squared;
                signed = if d[0] * q[1] - d[1] * q[0] <= 0.0 {
                    -1.0
                } else {
                    1.0
                };
            }
        }
        nearest.sqrt() * signed
    }
}

fn sample(c: Cubic, t: f32) -> (Point, Point, Point) {
    let u = 1.0 - t;
    let p = std::array::from_fn(|i| {
        u.powi(3) * c[0][i]
            + 3.0 * u * u * t * c[1][i]
            + 3.0 * u * t * t * c[2][i]
            + t.powi(3) * c[3][i]
    });
    let d = std::array::from_fn(|i| {
        3.0 * (u * u * (c[1][i] - c[0][i])
            + 2.0 * u * t * (c[2][i] - c[1][i])
            + t * t * (c[3][i] - c[2][i]))
    });
    let dd = std::array::from_fn(|i| {
        6.0 * (u * (c[2][i] - 2.0 * c[1][i] + c[0][i]) + t * (c[3][i] - 2.0 * c[2][i] + c[1][i]))
    });
    (p, d, dd)
}

#[cfg(test)]
mod tests {
    #[test]
    fn continuous_quad_shader_is_valid_wgsl() {
        use vello::wgpu::naga::{
            front::wgsl,
            valid::{Capabilities, ValidationFlags, Validator},
        };
        let source = concat!(
            include_str!("continuous.wgsl"),
            include_str!("gpu/quads.wgsl")
        );
        let module = wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
        Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .unwrap();
    }
}
