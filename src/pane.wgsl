// Kanade's pane, under a body's content (src/pane/, ADR 0050, ADR 0156): the body's outline filled
// with a solid color a hair lighter at the top, a directional top light edge and rim, and a bevel
// just inside the outline of a translucent pane.
// The runtime gives uv (0 to 1 across the body), size in logical pixels and `values`:
//   values[0]: radius, then two unused, the last 1 when the body is united with a second one
//   values[1] to [3], only when values[0].w is 1, a body united with a second one, as the Island
//   with the Dock (ADR 0031), the whole rect being the two's bounds: the body's rect, the second
//   rect, then the second's radius and the fillet radius
//   values[4]: the fill, plain rgba, reached 70% of the way down
//   values[5]: the fill at the top, a hair lighter
//   values[6]: the top light edge, rgba laid over the fill: the 1 px along the top of the outline
//   values[7]: the rim, rgba laid over the fill: the 1 px inside the whole outline
// Every pane takes this path, one rounded rect or two united, so a standalone body has the same
// outline, light and bevel as a merged one. It never reads the clock, and it reads nothing behind
// it (ADR 0121), so a body at rest draws no frames of its own.

// Shared with shell clips, blur spans, Satellite joins and shadows.
fn rounded(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    return continuous_distance(p, half, r);
}

// the exact circular fillet of two distances: where both contours are within `blend`, their edges
// meet in an arc of radius `blend`, tangent to both, its deepest point (sqrt(2) - 1) * blend from
// their crossing; beyond that arc's band the nearer one alone
fn united(a: f32, b: f32, blend: f32) -> f32 {
    if abs(a - b) >= blend {
        return min(a, b);
    }

    return max(blend, min(a, b)) - length(max(vec2<f32>(blend) - vec2<f32>(a, b), vec2<f32>(0.0)));
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

// a slightly darker, more saturated fill for the bevel
fn deeper(fill: vec3<f32>) -> vec3<f32> {
    let grey = dot(fill, vec3<f32>(0.2126, 0.7152, 0.0722));
    let saturated = mix(vec3<f32>(grey), fill, 1.25);

    return clamp(saturated * 0.86, vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let radius = min(values[0].x, min(size.x, size.y) * 0.5);
    let half = size * 0.5;
    let p = uv * size - half;
    let d = outline(p, half, radius);
    let inside = cover(d);

    // the outward normal from the distance field and one fixed top-left light: no backdrop read,
    // and a zero gradient at rest reads as the rim's own dim weight rather than a NaN
    let gradient = vec2<f32>(dpdx(d), dpdy(d));
    let normal = gradient / max(length(gradient), 1e-5);
    let light = normalize(vec2<f32>(-0.45, -0.89));
    let lit = max(dot(normal, light), 0.0);

    var fill = values[4];

    // a translucent pane darkens a little just inside its outline, a bevel that its tint barely
    // suggests; an opaque pane and paper stay flat
    if fill.a < 0.95 {
        let width = min(10.0, min(size.x, size.y) * 0.15);
        let bevel = 1.0 - smoothstep(0.0, width, -d);

        fill = vec4<f32>(mix(fill.rgb, deeper(fill.rgb), bevel * 0.35), fill.a);
    }

    var color = mix(values[5].rgb, fill.rgb, clamp(uv.y / 0.7, 0.0, 1.0));

    // the rim: inside the outline, not inside it drawn 1 px smaller, brightest on the lit side
    let rim = values[7];
    color = mix(
        color,
        rim.rgb,
        rim.a * inside * (1.0 - cover(d + 1.0)) * (0.35 + 0.65 * lit),
    );

    // the top light edge: inside the outline, not inside it moved 1 px down, brightest on the top
    let edge = values[6];
    let lower = cover(outline(p - vec2<f32>(0.0, 1.0), half, radius));
    color = mix(
        color,
        edge.rgb,
        edge.a * inside * (1.0 - lower) * (0.25 + 0.75 * lit),
    );

    return vec4<f32>(color, fill.a * inside);
}
