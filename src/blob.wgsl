// Kanade's Satellites as one liquid shape with the Island's body (src/pane/blob.rs, ADR 0101): the
// dots beside the body, each smoothly united with it and with the dots before it, so a dot that
// comes out of the body pulls a neck that thins and snaps as it parts. Drawn under the body, and
// only outside it, apart from the dots themselves: the body's own fill is the pane's.
// The runtime gives uv (0 to 1 across the rect), size in logical pixels and `values`:
//   values[0]: the body's rect: x, y, width, height, in the rect's own pixels
//   values[1]: the body's radius, the neck's reach, the gap at which the neck is gone, the dots' count
//   values[2]: the color at the body, rgba
//   values[3]: the color at a dot, rgba
//   values[4] to [9]: a dot, nearest the body first: its center, radius, opacity
// It never reads the clock, so dots at rest draw no frames of their own.

const MOST = 6;
const SQUIRCLE = 4.0;

// as `pane.wgsl`'s: the body's corners, a squircle's
fn rounded(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(r, r);
    let out = max(q, vec2<f32>(0.0, 0.0));

    let n = mix(SQUIRCLE, 2.0, smoothstep(0.6, 1.0, r / max(min(half.x, half.y), 1e-4)));
    let norm = pow(pow(out.x, n) + pow(out.y, n), 1.0 / n);

    return norm + min(max(q.x, q.y), 0.0) - r;
}

fn body(p: vec2<f32>) -> f32 {
    let rect = values[0];

    return rounded(p - (rect.xy + rect.zw * 0.5), rect.zw * 0.5, values[1].x);
}

// the smooth minimum of two distances, as `pane.wgsl`'s `united`: a fillet, not a crease, within
// `blend` of each other, and exactly the nearer one beyond
fn united(a: f32, b: f32, blend: f32) -> f32 {
    let pull = max(blend - abs(a - b), 0.0) / max(blend, 1e-4);

    return min(a, b) - pull * pull * blend * 0.25;
}

fn cover(d: f32) -> f32 {
    return clamp(0.5 - d, 0.0, 1.0);
}

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let p = uv * size;
    let count = min(i32(values[1].w), MOST);

    let from_body = body(p);
    var d = from_body;

    // the nearest dot and how far it is, and the dots drawn as they are
    var nearest = 1.0e5;
    var nearest_opacity = 0.0;
    var dots = 0.0;

    for (var i = 0; i < count; i++) {
        let bud = values[4 + i];
        let center = bud.xy;
        let own = distance(p, center) - bud.z;

        // how far this dot is from what it joins: the body and the dots before it
        var apart = body(center);

        for (var j = 0; j < i; j++) {
            let earlier = values[4 + j];

            apart = min(apart, distance(center, earlier.xy) - earlier.z);
        }

        // the neck is wide while the dot is within the body and thins as it parts, to none at the snap
        let reach = values[1].y * (1.0 - clamp((apart - bud.z) / max(values[1].z, 1e-4), 0.0, 1.0));

        d = united(d, own, reach);
        dots = max(dots, cover(own) * bud.w);

        if own < nearest {
            nearest = own;
            nearest_opacity = bud.w;
        }
    }

    // the neck lies outside the body, which draws its own fill
    let neck = cover(d) * (1.0 - cover(from_body)) * nearest_opacity;
    let alpha = max(dots, neck);

    // the body's color where the shape leaves the body, the dot's where it reaches the dot
    var far = 1.0;

    if nearest > 0.0 {
        let near = max(from_body, 0.0);
        far = near / max(near + nearest, 1e-4);
    }

    return vec4<f32>(mix(values[2].rgb, values[3].rgb, far), alpha);
}
