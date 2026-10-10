// Kanade's liquid glass over a pane's body, under its content (src/glass/, ADRs 0024, 0037): what
// is behind the edge, bent into a rim, a tint over it, then the highlights.
// the runtime gives uv (0 to 1 across the body), size in logical pixels, the `values` below and
// `backdrop`: a copy of the screen around the body, transparent where there is none:
//   values[0]: radius, highlight (0 none, 1 standard), tone (1 dark, 0 light), strength (0 to 1)
//   values[1]: light x, y in body pixels (the pointer, or a resting light), its reach, its share
//   values[2] to [4], only when values[4].z is 1, a body united with a second one, as the Island
//   with the Dock (ADR 0031), the whole rect being the two's bounds:
//     values[2]: the first rect's left, top, width, height, its radius in values[0].x
//     values[3]: the second's, values[4].x its radius
//     values[4]: its radius, how far the two pull together, 1 if united
//   values[5]: the tint over what is behind, plain rgba. The shader draws it itself and cuts all
//   of the pane to the outline, as the outline is no rounded rect to clip layers to
//   values[6] to [9], when values[10].x is 1, the body the copy was captured around, which the rim
//   is worked out for and then stretched to this one (captured ahead of a morphing body):
//     values[6], [7], [8]: its rects and radii, as values[2], [3], [4] have them
//     values[9]: its corner in the copy and the copy's size, in logical pixels
//   values[10]: x is 1 where the rim shows
// It never reads the clock, so a body at rest draws no frames of its own.
//
// The terms follow liquid-glass-studio's glare and Fresnel, kwin-glass's concave bevel and
// damascene's inner shadow.

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
    if values[4].z < 0.5 {
        return rounded(p, half, radius);
    }

    let first = values[2];
    let second = values[3];
    let a = rounded(p - (first.xy + first.zw * 0.5 - half), first.zw * 0.5, radius);
    let b = rounded(p - (second.xy + second.zw * 0.5 - half), second.zw * 0.5, values[4].x);

    return united(a, b, values[4].y);
}

// the rim bends what lies outside the edge in along the edge's normal (src/glass/capture.rs, `MARGIN`
// reaches as far out as the deepest sample): the edge itself shows furthest out, the inner side of
// the band what is just outside, the right way round, blue a little further than red
const BAND = 12.0;
const REACH = 7.0;
const GAP = 1.5;
const SPREAD = vec3<f32>(1.0, 1.04, 1.08);

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

// how far a ray from `at` along `direction` goes in a copy `size` big before it leaves it
fn room(at: vec2<f32>, direction: vec2<f32>, size: vec2<f32>) -> f32 {
    var along = vec2<f32>(1e9, 1e9);

    if direction.x > 1e-6 {
        along.x = (size.x - at.x) / direction.x;
    } else if direction.x < -1e-6 {
        along.x = -at.x / direction.x;
    }

    if direction.y > 1e-6 {
        along.y = (size.y - at.y) / direction.y;
    } else if direction.y < -1e-6 {
        along.y = -at.y / direction.y;
    }

    return min(along.x, along.y);
}

// the rim at `uv`: the bent colors and how much of them shows, plain rgba
fn rim(uv: vec2<f32>) -> vec4<f32> {
    let first = values[6];
    let second = values[7];
    let shape = values[8];
    let place = values[9];
    let joined = shape.w > 0.5;

    let low = select(first.xy, min(first.xy, second.xy), joined);
    let high = select(first.xy + first.zw, max(first.xy + first.zw, second.xy + second.zw), joined);
    let extent = high - low;

    let thin = select(
        min(first.z, first.w),
        min(min(first.z, first.w), min(second.z, second.w)),
        joined,
    ) * 0.5;
    let band = max(min(BAND, thin * 0.6), 1.0);

    // stretched from the copied body to this one
    let p = uv * extent;
    let d = rim_outline(p, first, second, low);
    let inside = -d;

    if inside < 0.0 || inside > band {
        return vec4<f32>(0.0);
    }

    let e = 0.5;
    let n = normalize(vec2<f32>(
        rim_outline(p + vec2<f32>(e, 0.0), first, second, low) - rim_outline(p - vec2<f32>(e, 0.0), first, second, low),
        rim_outline(p + vec2<f32>(0.0, e), first, second, low) - rim_outline(p - vec2<f32>(0.0, e), first, second, low),
    ) + vec2<f32>(1e-6, 1e-6));

    // squeezed into what the copy holds past the edge, where the screen ends close by
    let at = place.xy + p;
    let spare = room(at, n, place.zw) - inside;
    let reach = REACH * clamp((spare - GAP) / (REACH * SPREAD.z), 0.0, 1.0);
    let squeeze = (1.0 - inside / band) * (1.0 - inside / band);

    var color = vec3<f32>(0.0);

    for (var channel = 0; channel < 3; channel++) {
        let depth = inside + GAP + reach * SPREAD[channel] * squeeze;
        let texel = textureSampleLevel(backdrop, backdrop_sampler, (at + n * depth) / place.zw, 0.0);

        color[channel] = texel[channel];
    }

    // with no room at all the rim fades rather than repeat the screen's edge
    let deepest = at + n * (inside + GAP + reach * SPREAD.z * squeeze);
    let beyond = max(max(-deepest.x, deepest.x - place.z), max(-deepest.y, deepest.y - place.w));
    let kept = 1.0 - smoothstep(0.0, 3.0, max(beyond, 0.0));

    // full at the edge, gone well before the band ends, into the backdrop seen through the middle
    let fade = 1.0 - smoothstep(0.35, 1.0, inside / band);

    return vec4<f32>(color, fade * kept);
}

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let shape = values[0];
    let light = values[1];

    let radius = min(shape.x, min(size.x, size.y) * 0.5);
    let dark = shape.z;
    let strength = shape.w;
    let highlight = shape.y;

    let half = size * 0.5;
    let p = uv * size - half;
    let d = outline(p, half, radius);
    let inside = max(-d, 0.0);

    // the outward normal, from the distance's slope
    let e = 0.75;
    let n = normalize(vec2<f32>(
        outline(p + vec2<f32>(e, 0.0), half, radius) - outline(p - vec2<f32>(e, 0.0), half, radius),
        outline(p + vec2<f32>(0.0, e), half, radius) - outline(p - vec2<f32>(0.0, e), half, radius),
    ) + vec2<f32>(1e-5, 1e-5));

    // light falls from the top left
    let sun = normalize(vec2<f32>(-0.5, -0.85));
    let facing = dot(n, sun);

    // the glare: two lobes on opposite corners, the far one weaker, converged to a bright arc
    let near = pow(clamp(max(facing, 0.0) * 1.2, 0.0, 1.0), 1.1);
    let far = pow(clamp(max(-facing, 0.0) * 1.2, 0.0, 1.0), 1.1) * mix(0.3, 0.4, 1.0 - dark);

    // how lit each point of the rim is: full toward the light, faint on the far corner, almost
    // nothing along the sides between, so the edge reads as a glint, not a drawn outline
    let lit = 0.12 + 0.88 * clamp(near + far, 0.0, 1.0);

    // a small body, as the Rest, catches less, so its highlight never outshines what it shows
    let small = mix(0.6, 1.0, smoothstep(32.0, 120.0, size.y));

    // a specular band 2 to 3 pixels in, falling off as the fifth power, and a hairline at the edge
    let geo = clamp(pow(max(1.2 - 0.185 * inside, 0.0), 5.0), 0.0, 1.0);
    let hairline = (1.0 - smoothstep(0.0, 1.0, inside)) * lit;
    let glare = geo * (near + far) * mix(0.70, 0.45, dark);

    // the Fresnel lift: the whole rim a little brighter, however it faces
    let fresnel = geo * mix(0.08, 0.05, dark);

    // the bevel: a concave edge band of up to 14 pixels, lit on the light's side, shaded opposite
    let band = min(14.0, min(half.x, half.y) * 0.6);
    let edge = 1.0 - clamp(inside / band, 0.0, 1.0);
    let concave = 1.0 - sqrt(max(0.0, 1.0 - pow(smoothstep(0.0, 1.0, edge), 3.0)));
    let bevel = concave * facing * 0.10;

    // a sheen over the top, like a curved top catching the sky, and a faint diagonal streak
    let top = 1.0 - smoothstep(0.03, 0.42, uv.y);
    let slant = uv.x + (1.0 - uv.y) * 0.65;
    let diagonal = smoothstep(0.18, 0.64, slant) * (1.0 - smoothstep(0.64, 1.16, slant));
    let sheen = top * mix(0.07, 0.03, dark) + diagonal * 0.025;

    // the light that follows the pointer while it moves over the glass
    let to_light = uv * size - light.xy;
    let spread = 2.0 * light.z * light.z;
    let glow = light.w * exp(-dot(to_light, to_light) / spread);
    let pointer_rim = light.w * geo * exp(-dot(to_light, to_light) / (spread * 0.6)) * 0.8;

    var white = glare + fresnel + max(bevel, 0.0) + sheen + glow * 0.08 + pointer_rim
        + hairline * mix(0.45, 0.20, dark);
    white = white * strength * highlight * small;

    // the inner shadow, all but gone: only the bevel's far side darkens, faintly
    var black = max(-bevel, 0.0) * 0.4 * strength * highlight * small;

    // light glass on a bright backdrop keeps a faint dark outline outside its hairline, unlit or not,
    // so it stays apart from it
    black = black + (1.0 - dark) * (1.0 - smoothstep(0.0, 0.5, inside)) * 0.08;

    white = clamp(white, 0.0, 1.0);
    black = clamp(black, 0.0, 1.0);

    let alpha = clamp(white + black, 0.0, 1.0);
    let shade = select(0.0, white / (white + black), white + black > 0.0001);

    // what is behind, bent, under the tint, under the highlights, all of it cut to the outline,
    // antialiased
    let bent = select(vec4<f32>(0.0), rim(uv), values[10].x > 0.5);
    let tint = values[5];

    let under = tint.a + bent.a * (1.0 - tint.a);
    let under_color = (tint.rgb * tint.a + bent.rgb * bent.a * (1.0 - tint.a)) / max(under, 1e-4);

    let covered = clamp(0.5 - d, 0.0, 1.0);
    let total = alpha + under * (1.0 - alpha);
    let color = (vec3<f32>(shade) * alpha + under_color * under * (1.0 - alpha)) / max(total, 1e-4);

    return vec4<f32>(color, total * covered);
}
