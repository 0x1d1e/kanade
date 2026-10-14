// Kanade's pane, under a body's content (src/pane/, ADR 0050): the body's outline filled with a
// solid color a hair lighter at the top, a top light edge and a rim.
// The runtime gives uv (0 to 1 across the body), size in logical pixels and `values`:
//   values[0]: radius, then three unused, the last 1 when the body is united with a second one
//   values[1] to [3], only when values[0].w is 1, a body united with a second one, as the Island
//   with the Dock (ADR 0031), the whole rect being the two's bounds: the body's rect, the second
//   rect, then the second's radius and the blend
//   values[4]: the fill, plain rgba, reached 70% of the way down
//   values[5]: the fill at the top, a hair lighter
//   values[6]: the top light edge, rgba laid over the fill: the 1 px along the top of the outline
//   values[7]: the rim, rgba laid over the fill: the 1 px inside the whole outline
// It never reads the clock, so a body at rest draws no frames of its own.

// how squarely a corner is drawn: the exponent of its norm, 2 being a circle's arc. A corner is a
// squircle's, as the system's rounded shapes are, until its radius takes the short side, when it is
// a pill's round cap
const SQUIRCLE = 4.0;

// distance, not exact, to a rounded rectangle centered on 0, negative inside. The corners are
// squircles, so a body is a hair fuller there than a circle of the same radius (content clipped to
// the circle stays inside)
fn rounded(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(r, r);
    let out = max(q, vec2<f32>(0.0, 0.0));

    let n = mix(SQUIRCLE, 2.0, smoothstep(0.6, 1.0, r / max(min(half.x, half.y), 1e-4)));
    let norm = pow(pow(out.x, n) + pow(out.y, n), 1.0 / n);

    return norm + min(max(q.x, q.y), 0.0) - r;
}

// the pull of the smooth minimum of two distances: the edges meet in a fillet, not a crease
fn united(a: f32, b: f32, blend: f32) -> f32 {
    let pull = max(blend - abs(a - b), 0.0) / max(blend, 1e-4);

    return min(a, b) - pull * pull * blend * 0.25;
}

// signed distance to the body's outline at `p`, from the center of the rect holding it
fn outline(p: vec2<f32>, half: vec2<f32>, radius: f32) -> f32 {
    if values[0].w < 0.5 {
        return rounded(p, half, radius);
    }

    let first = values[1];
    let second = values[2];
    let a = rounded(p - (first.xy + first.zw * 0.5 - half), first.zw * 0.5, radius);
    let b = rounded(p - (second.xy + second.zw * 0.5 - half), second.zw * 0.5, values[3].x);

    return united(a, b, values[3].y);
}

// how much of a pixel is inside a signed distance
fn cover(d: f32) -> f32 {
    return clamp(0.5 - d, 0.0, 1.0);
}

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let radius = min(values[0].x, min(size.x, size.y) * 0.5);
    let half = size * 0.5;
    let p = uv * size - half;
    let d = outline(p, half, radius);
    let inside = cover(d);

    let fill = values[4];
    var color = mix(values[5].rgb, fill.rgb, clamp(uv.y / 0.7, 0.0, 1.0));

    // the rim: inside the outline, not inside it drawn 1 px smaller
    let rim = values[7];
    color = mix(color, rim.rgb, rim.a * inside * (1.0 - cover(d + 1.0)));

    // the top light edge: inside the outline, not inside it moved 1 px down
    let edge = values[6];
    let lower = cover(outline(p - vec2<f32>(0.0, 1.0), half, radius));
    color = mix(color, edge.rgb, edge.a * inside * (1.0 - lower));

    return vec4<f32>(color, fill.a * inside);
}
