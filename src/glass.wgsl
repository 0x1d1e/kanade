// Kanade's Liquid Glass, under a pane's content (src/pane/glass.rs, ADR 0052): clear glass, the
// screen itself showing through the body, with an edge that reflects what lies around the body.
// The runtime gives uv (0 to 1 across the body), size in logical pixels, `values` below and
// `backdrop`: a copy of the screen around the body, transparent where there is none:
//   values[0]: radius
//   values[2] to [4], only when values[4].z is 1, a body united with a second one, as the Island
//   with the Dock (ADR 0031), the whole rect being the two's bounds
//   values[6] to [9]: the captured body's rects and radii, then its corner and copy size; captured
//     ahead of a morphing body, these values place the samples
//   values[10] and [11]: two other panes on the copy, x, y, width and height in its pixels, which
//     are read as none of what lies around this one
//   values[12].x: how far out the edge reads at most, in pixels; 0 when no copy can fill this pane
//   values[13].x: how black the glass is under the body, 0 to 1, over a fairly bright page
// It never reads the clock, so a body at rest draws no frames of its own.
//
// This is the final pass of liquid-glass-studio (src/shaders-wgsl/fragment-main.wgsl) at its default
// values: the Snell edge profile over 20 px (less on a small body), per-channel dispersion, Fresnel
// (in the hue of what the edge reflects) and glare in LCH, no tint, and a refraction distance of 2%
// of the output's height; hyprglass's faint shade inside the bottom edge is added. A copy of the screen holds the
// pane itself, so what lies under the body cannot be read: where the studio reads inward, the edge
// here reads the screen just outside the outline, along the same profile. The middle is not drawn:
// the compositor shows the screen through it.

const PI = 3.14159265359;

// the studio's defaults, but for the edge's thickness, which is its at most
const THICKNESS = 20.0;
const THIN = 8.0;
const THIN_SHARE = 0.35;
const FACTOR = 1.4;
const DISPERSION = 1.0;
const N_R = 0.98;
const N_B = 1.02;
const FRESNEL_RANGE = 30.0;
const FRESNEL_HARDNESS = 0.2;
const FRESNEL_FACTOR = 0.2;
const FRESNEL_TINT = 0.5;
const GLARE_RANGE = 30.0;
const GLARE_HARDNESS = 0.2;
const GLARE_FACTOR = 0.9;
const GLARE_CONVERGENCE = 0.5;
const GLARE_OPPOSITE = 0.8;
const GLARE_ANGLE = -0.78539816339;

// the shade just inside the bottom edge, as hyprglass has it, at most this opaque
const SHADE = 0.06;

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
    if values[4].z < 0.5 {
        return rounded(p, half, radius);
    }

    let first = values[2];
    let second = values[3];
    let a = rounded(p - (first.xy + first.zw * 0.5 - half), first.zw * 0.5, radius);
    let b = rounded(p - (second.xy + second.zw * 0.5 - half), second.zw * 0.5, values[4].x);

    return united(a, b, values[4].y);
}

// the copied body's outline at `p`, from the corner of the rect holding it
fn rim_outline(p: vec2<f32>, first: vec4<f32>, second: vec4<f32>, corner: vec2<f32>) -> f32 {
    let shape = values[8];
    let a_half = first.zw * 0.5;
    let a = rounded(p - (first.xy - corner + a_half), a_half, min(shape.x, min(a_half.x, a_half.y)));

    if shape.w < 0.5 {
        return a;
    }

    let b_half = second.zw * 0.5;
    let b = rounded(p - (second.xy - corner + b_half), b_half, min(shape.y, min(b_half.x, b_half.y)));

    return united(a, b, shape.z);
}

// whether the copy at `at` (its pixels) holds another pane of Kanade, not the screen
fn other(at: vec2<f32>) -> bool {
    let a = values[10];
    let b = values[11];

    return (a.z > 0.0 && all(at >= a.xy) && all(at <= a.xy + a.zw))
        || (b.z > 0.0 && all(at >= b.xy) && all(at <= b.xy + b.zw));
}


// sRGB and LCH, from the studio's lib/color.wgsl
const D65_WHITE = vec3<f32>(0.95045592705, 1.0, 1.08905775076);

fn uncompand(a: f32) -> f32 {
    if a > 0.04045 {
        return pow((a + 0.055) / 1.055, 2.4);
    }

    return a / 12.92;
}

fn compand(a: f32) -> f32 {
    if a <= 0.0031308 {
        return 12.92 * a;
    }

    return 1.055 * pow(a, 0.41666666666) - 0.055;
}

fn lab_f(x: f32) -> f32 {
    if x > 0.00885645167 {
        return pow(x, 0.333333333);
    }

    return 7.78703703704 * x + 0.13793103448;
}

fn xyz_f(x: f32) -> f32 {
    if x > 0.206897 {
        return x * x * x;
    }

    return 0.12841854934 * (x - 0.137931034);
}

fn srgb_to_lch(srgb: vec3<f32>) -> vec3<f32> {
    let rgb = vec3<f32>(uncompand(srgb.x), uncompand(srgb.y), uncompand(srgb.z));
    let xyz = vec3<f32>(
        dot(rgb, vec3<f32>(0.4124, 0.3576, 0.1805)),
        dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722)),
        dot(rgb, vec3<f32>(0.0193, 0.1192, 0.9505)),
    );
    let f = vec3<f32>(lab_f(xyz.x / D65_WHITE.x), lab_f(xyz.y / D65_WHITE.y), lab_f(xyz.z / D65_WHITE.z));
    let lab = vec3<f32>(116.0 * f.y - 16.0, 500.0 * (f.x - f.y), 200.0 * (f.y - f.z));

    return vec3<f32>(lab.x, length(lab.yz), atan2(lab.z, lab.y) * 57.2957795131);
}

fn lch_to_srgb(lch: vec3<f32>) -> vec3<f32> {
    let lab = vec3<f32>(lch.x, lch.y * cos(lch.z * 0.01745329251), lch.y * sin(lch.z * 0.01745329251));
    let w = (lab.x + 16.0) / 116.0;
    let xyz = D65_WHITE * vec3<f32>(xyz_f(w + lab.y / 500.0), xyz_f(w), xyz_f(w - lab.z / 200.0));
    let rgb = vec3<f32>(
        dot(xyz, vec3<f32>(3.2406255, -1.537208, -0.4986286)),
        dot(xyz, vec3<f32>(-0.9689307, 1.8757561, 0.0415175)),
        dot(xyz, vec3<f32>(0.0557101, -0.2040211, 1.0569959)),
    );

    return vec3<f32>(compand(rgb.x), compand(rgb.y), compand(rgb.z));
}

// `top` over `bottom`, both plain rgba
fn over(top: vec4<f32>, bottom: vec4<f32>) -> vec4<f32> {
    let a = top.a + bottom.a * (1.0 - top.a);
    let rgb = (top.rgb * top.a + bottom.rgb * bottom.a * (1.0 - top.a)) / max(a, 1e-4);

    return vec4<f32>(rgb, a);
}

// how thick the edge is: the studio's 20 px, less on a body too small to carry it, as the Island at
// rest, so its middle is not all edge
fn thickness() -> f32 {
    return clamp(THIN_SHARE * min(size.x, size.y), THIN, THICKNESS);
}

// the studio's Snell edge profile: how far the edge reads, 0 to 1 at the edge, 0 from `thickness()` in
fn profile(depth: f32) -> f32 {
    let thick = thickness();

    if depth >= thick {
        return 0.0;
    }

    let x = clamp(1.0 - depth / thick, 0.0, 1.0);
    let incident = asin(clamp(x * x, -1.0, 1.0));
    let transmitted = asin(clamp(sin(incident) / FACTOR, -1.0, 1.0));

    return -tan(transmitted - incident);
}

// the screen around the body, bent at the edge: inside the outline, the copy read just outside it,
// the further out the nearer the edge, a channel at a time. Plain rgb, and how much the read covers
fn lens(uv: vec2<f32>) -> vec4<f32> {
    let first = values[6];
    let second = values[7];
    let shape = values[8];
    let place = values[9];
    let joined = shape.w > 0.5;

    let low = select(first.xy, min(first.xy, second.xy), joined);
    let high = select(first.xy + first.zw, max(first.xy + first.zw, second.xy + second.zw), joined);
    let p = uv * (high - low);

    let depth = -rim_outline(p, first, second, low);
    let edge = profile(depth);

    if edge <= 0.0 || depth < -0.5 {
        return vec4<f32>(0.0);
    }

    let e = 0.75;
    let out = normalize(vec2<f32>(
        rim_outline(p + vec2<f32>(e, 0.0), first, second, low) - rim_outline(p - vec2<f32>(e, 0.0), first, second, low),
        rim_outline(p + vec2<f32>(0.0, e), first, second, low) - rim_outline(p - vec2<f32>(0.0, e), first, second, low),
    ) + vec2<f32>(1e-5, 1e-5));

    // how far outside the outline this pixel reads
    let reach = 1.5 + edge * values[12].x;

    var color = vec3<f32>(0.0);
    var kept = 0.0;

    // another pane is no part of what lies around: a read of any of it, in any channel, reads none,
    // or the channels left would show a fringe of one color
    for (var channel = 0; channel < 3; channel++) {
        let split = 1.0 - (N_R + 0.02 * f32(channel) - 1.0) * DISPERSION;

        if other(place.xy + p + out * (max(depth, 0.0) + reach * split)) {
            return vec4<f32>(0.0);
        }
    }

    for (var channel = 0; channel < 3; channel++) {
        let split = 1.0 - (N_R + 0.02 * f32(channel) - 1.0) * DISPERSION;
        let spot = p + out * (max(depth, 0.0) + reach * split);
        let coord = (place.xy + spot) / place.zw;
        let clamped = clamp(coord, vec2<f32>(0.0), vec2<f32>(1.0));

        // past the copy is past the screen's edge: nothing there to reflect
        if any(clamped != coord) {
            continue;
        }

        let texel = textureSampleLevel(backdrop, backdrop_sampler, clamped, 0.0);

        color[channel] = texel[channel];
        kept += texel.a / 3.0;
    }

    // melts into the clear middle as the profile runs out
    return vec4<f32>(color, kept * smoothstep(0.02, 0.15, edge));
}

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let radius = min(values[0].x, min(size.x, size.y) * 0.5);

    let half = size * 0.5;
    let p = uv * size - half;
    let d = outline(p, half, radius);

    let covered = clamp(0.5 - d, 0.0, 1.0);

    if covered <= 0.0 {
        return vec4<f32>(0.0);
    }

    // the outward normal, from the distance's slope, y up as the studio has it
    let e = 0.75;
    let n = normalize(vec2<f32>(
        outline(p + vec2<f32>(e, 0.0), half, radius) - outline(p - vec2<f32>(e, 0.0), half, radius),
        outline(p + vec2<f32>(0.0, e), half, radius) - outline(p - vec2<f32>(0.0, e), half, radius),
    ) + vec2<f32>(1e-5, 1e-5));
    let up = vec2<f32>(n.x, -n.y);

    // the scrim under the edge, which the pane's light ink wants over a page growing bright
    var glass = vec4<f32>(0.0, 0.0, 0.0, values[13].x);

    if values[12].x > 0.5 {
        glass = over(lens(uv), glass);
    }

    // the Fresnel lift: the edge toward white, in the hue of what it reflects where it reflects any
    let fresnel = clamp(pow(max(1.0 + d / 1500.0 * pow(500.0 / FRESNEL_RANGE, 2.0) + FRESNEL_HARDNESS, 0.0), 5.0), 0.0, 1.0);

    let top = max(max(glass.r, glass.g), glass.b);
    let hue = select(vec3<f32>(1.0), glass.rgb / max(top, 1e-3), glass.a > 0.0 && top > 0.02);

    glass = over(vec4<f32>(mix(vec3<f32>(1.0), hue, FRESNEL_TINT), fresnel * FRESNEL_FACTOR * 0.7), glass);

    // the glare: two lobes on opposite corners, in the color of what the edge reflects, lifted
    let geo = clamp(pow(max(1.0 + d / 1500.0 * pow(500.0 / GLARE_RANGE, 2.0) + GLARE_HARDNESS, 0.0), 5.0), 0.0, 1.0);

    var angle = atan2(up.y, up.x);
    if angle < 0.0 {
        angle += 2.0 * PI;
    }

    let turn = (angle - PI / 4.0 + GLARE_ANGLE) * 2.0;
    let far = (turn > PI * 1.5 && turn < PI * 3.5) || turn < -PI * 0.5;
    let sides = select(1.2, 1.2 * GLARE_OPPOSITE, far);
    let lobe = clamp(pow((0.5 + sin(turn) * 0.5) * sides * GLARE_FACTOR, 0.1 + GLARE_CONVERGENCE * 2.0), 0.0, 1.0);

    var lifted = srgb_to_lch(select(vec3<f32>(1.0), glass.rgb, glass.a > 0.0));
    lifted.x = clamp(lifted.x + 150.0 * lobe * geo, 0.0, 120.0);
    lifted.y += 30.0 * lobe * geo;

    glass = over(vec4<f32>(clamp(lch_to_srgb(lifted), vec3<f32>(0.0), vec3<f32>(1.0)), lobe * geo), glass);

    // the shade inside the bottom edge, which the light does not reach
    let near = clamp(1.0 + d / thickness(), 0.0, 1.0);
    let below = clamp(n.y, 0.0, 1.0);

    glass = over(vec4<f32>(0.0, 0.0, 0.0, SHADE * near * near * below * below), glass);

    return vec4<f32>(glass.rgb, glass.a * covered);
}
