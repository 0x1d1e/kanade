// Kanade's pane, under a body's content (src/pane/, ADR 0049): the body's outline filled with one
// solid color. The runtime gives uv (0 to 1 across the body), size in logical pixels and `values`:
//   values[0]: radius, then three unused, the last 1 when the body is united with a second one
//   values[1] to [3], only when values[0].w is 1, a body united with a second one, as the Island
//   with the Dock (ADR 0031), the whole rect being the two's bounds: the body's rect, the second
//   rect, then the second's radius and the blend
//   values[4]: the fill, plain rgba
// It never reads the clock, so a body at rest draws no frames of its own.

// signed distance to a rounded rectangle centered on 0, negative inside
fn rounded(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(r, r);

    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
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

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let radius = min(values[0].x, min(size.x, size.y) * 0.5);
    let half = size * 0.5;
    let d = outline(uv * size - half, half, radius);

    let fill = values[4];

    return vec4<f32>(fill.rgb, fill.a * clamp(0.5 - d, 0.0, 1.0));
}
