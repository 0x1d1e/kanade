// Kanade's Frosted Glass, under a pane's content (src/glass/, ADR 0048): colors sampled around the
// body, softened across it, under a tint and restrained edge highlights.
// The runtime gives uv (0 to 1 across the body), size in logical pixels, `values` below and
// `backdrop`: a copy of the screen around the body, transparent where there is none:
//   values[0]: radius, highlight, tone (1 dark, 0 light), highlight strength
//   values[1]: light x, y in body pixels (the pointer, or a resting light), its reach, its share
//   values[2] to [4], only when values[4].z is 1, a body united with a second one, as the Island
//   with the Dock (ADR 0031), the whole rect being the two's bounds
//   values[5]: the tint over the frost, plain rgba
//   values[6] to [9]: the captured body's rects and radii, then its corner and copy size; captured
//     ahead of a morphing body, these values place the frost samples
//   values[10].x: 1 when a captured copy can frost this pane
// It never reads the clock, so a body at rest draws no frames of its own.
//
// The edge light follows liquid-glass-studio's glare and Fresnel, kwin-glass's concave bevel and
// damascene's inner shadow. Frost follows hyprglass's edge samples and saturation lift.

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

// the frost: what lies under the body is the body in the copy, so the middle takes the colors
// around the body instead, blurred and blended by how near each side is. From each pixel a ray goes
// out along each of the four axes to the outline; there the copy is read just outside it, at a few
// points along the edge, and the four reads are weighed by 1 over the squared distance, as a
// smooth fill of the body from its edge.
// The copy comes with blurrier sizes of itself (runtime backdrops.rs), so each read is already a
// blur, wider the further the ray went from the edge, and a hard edge behind the body, as a
// window's top, fades in over its width as it passes the outline, not as a line
const FROST_TAPS = 5;
const FROST_SPREAD = 0.375;
const FROST_BLUR = 4.0;
const FROST_BLUR_MAX = 5.0;
// A read at blur level n averages a bilinear 2 by 2 of texels 2 ^ n px wide, so it reaches 0.75 of
// that around its center (the footprint, 24 px at most). The center keeps a footprint clear of the
// outline, so nothing from inside the body is averaged in, and its farthest reach, 1 + 2 footprints
// out or 2 taps of FROST_SPREAD plus a footprint along the edge, stays within the capture margin
// in capture.rs; a read past the copy at the screen's edge is dropped within a footprint of the body
const FROST_SATURATION = 1.18;
// a body's half height over this, as a power of two, caps the blur: a 40 px capsule blurs to level 2
const FROST_SCALE = 5.0;

// the frost at `uv`: plain rgba, nothing where the copy holds none of what lies around
fn frost(uv: vec2<f32>) -> vec4<f32> {
    let first = values[6];
    let second = values[7];
    let shape = values[8];
    let place = values[9];
    let joined = shape.w > 0.5;

    let low = select(first.xy, min(first.xy, second.xy), joined);
    let high = select(first.xy + first.zw, max(first.xy + first.zw, second.xy + second.zw), joined);
    let p = uv * (high - low);

    var sum = vec3<f32>(0.0);
    var weight = 0.0;

    let small = max(log2(min(first.z, first.w) * 0.5 / FROST_SCALE), 1.0);

    for (var side = 0; side < 4; side++) {
        let angle = f32(side) * 1.5707963;
        let direction = vec2<f32>(cos(angle), sin(angle));
        let along = vec2<f32>(-direction.y, direction.x);

        /*
         * out to the outline: the steps grow with the reach, as plain sphere tracing crawls along
         * a wall near it and ran out of steps well short of the sides, leaving the far ray alone to
         * color a wedge at the top; then the last step is bisected back to the outline
         */
        var reach = 0.0;
        var inside = 0.0;
        var met = false;

        for (var step = 0; step < 16; step++) {
            let gap = rim_outline(p + direction * reach, first, second, low);

            if gap > -0.25 {
                met = true;
                break;
            }

            inside = reach;
            reach += max(-gap, 1.0 + 0.5 * reach);
        }

        if !met {
            continue;
        }

        var outside = reach;

        for (var step = 0; step < 8; step++) {
            let middle = 0.5 * (inside + outside);

            if rim_outline(p + direction * middle, first, second, low) > -0.25 {
                outside = middle;
            } else {
                inside = middle;
            }
        }

        reach = outside;

        var got = vec3<f32>(0.0);
        var kept = 0.0;

        // the blur grows with the reach, but never past the body's own scale: a small body reads
        // only just outside its edge, so a window a few pixels off does not tint it; the copy
        // holds the body's own pixels inside the outline
        let blur = min(min(FROST_BLUR + log2(1.0 + reach / 10.0), FROST_BLUR_MAX), small);
        let width = exp2(blur);
        let footprint = 0.75 * exp2(ceil(blur));

        for (var tap = 0; tap < FROST_TAPS; tap++) {
            let spot = p + direction * (reach + footprint + 1.0) + along * (f32(tap - FROST_TAPS / 2) * width * FROST_SPREAD);
            let at = place.xy + spot;
            let coord = at / place.zw;

            // only outside the body
            if rim_outline(spot, first, second, low) < 0.5 {
                continue;
            }

            /*
             * past the copy, at the screen's edge, its last pixels go on; but where the edge is
             * within a footprint of the body, they are the body's own, so the read is dropped
             */
            let edge = clamp(coord, vec2<f32>(0.0), vec2<f32>(1.0));

            if any(edge != coord) && rim_outline(edge * place.zw - place.xy, first, second, low) < footprint {
                continue;
            }

            let texel = textureSampleLevel(backdrop, backdrop_sampler, edge, blur);

            got += texel.rgb * texel.a;
            kept += texel.a;
        }

        if kept > 0.0 {
            let near = 1.0 / ((reach + 8.0) * (reach + 8.0));
            let share = kept / f32(FROST_TAPS);

            sum += got / kept * near * share;
            weight += near * share;
        }
    }

    if weight <= 0.0 {
        return vec4<f32>(0.0);
    }

    // a little more color through the frosting, as hyprglass lifts it
    let color = sum / weight;
    let gray = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));

    return vec4<f32>(clamp(mix(vec3<f32>(gray), color, FROST_SATURATION), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
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

    // soften the captured colors beneath the pane's tint
    var frosted = vec4<f32>(0.0);
    if values[10].x > 0.5 {
        frosted = frost(uv);
    }

    let tint = values[5];
    let under = tint.a + frosted.a * (1.0 - tint.a);
    let under_color = (tint.rgb * tint.a + frosted.rgb * frosted.a * (1.0 - tint.a)) / max(under, 1e-4);

    let covered = clamp(0.5 - d, 0.0, 1.0);
    let total = alpha + under * (1.0 - alpha);
    let color = (vec3<f32>(shade) * alpha + under_color * under * (1.0 - alpha)) / max(total, 1e-4);

    return vec4<f32>(color, total * covered);
}
