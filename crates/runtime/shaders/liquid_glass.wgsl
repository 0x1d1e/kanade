// Native GPU liquid glass: a captured texture is sampled in the shader.
// No CPU refraction, temporary PNG, disk transfer, or repeated image decoding.
struct Glass {
    body: vec4<f32>,      // width, height, radius, optical band in pixels
    optics: vec4<f32>,    // reach, chroma, tint strength, highlight
    capture: vec4<f32>,   // capture-relative body origin, texture dimensions
    tint: vec4<f32>,      // color and opacity
    light: vec4<f32>,     // upper-left light direction
};
@group(0) @binding(0) var backdrop: texture_2d<f32>;
@group(0) @binding(1) var backdrop_sampler: sampler;
@group(0) @binding(2) var<uniform> glass: Glass;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vertex_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let uv = corners[index];
    var out: VertexOutput;
    out.position = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}
fn distance_to_body(pixel: vec2<f32>) -> f32 {
    let half = glass.body.xy * 0.5;
    let radius = min(glass.body.z, min(half.x, half.y));
    let q = abs(pixel - half) - (half - vec2<f32>(radius));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}
fn outward_normal(pixel: vec2<f32>) -> vec2<f32> {
    let dx = vec2<f32>(1.0, 0.0);
    let dy = vec2<f32>(0.0, 1.0);
    let gradient = vec2<f32>(
        distance_to_body(pixel + dx) - distance_to_body(pixel - dx),
        distance_to_body(pixel + dy) - distance_to_body(pixel - dy),
    );
    return gradient * inverseSqrt(max(dot(gradient, gradient), 0.000001));
}
fn backdrop_at(pixel: vec2<f32>) -> vec3<f32> {
    let uv = (glass.capture.xy + pixel) / max(glass.capture.zw, vec2<f32>(1.0));
    return textureSample(backdrop, backdrop_sampler, uv).rgb;
}
@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let pixel = input.uv * glass.body.xy;
    let signed = distance_to_body(pixel);
    let band = max(glass.body.w, 0.001);
    let edge = 1.0 - smoothstep(0.0, band, max(-signed, 0.0));
    let normal = outward_normal(pixel);
    let amount = max(glass.optics.x, 0.0) * edge * edge;
    let chroma = max(glass.optics.y, 0.0) * edge;
    let base = pixel + normal * amount;
    let r = backdrop_at(base - normal * chroma).r;
    let g = backdrop_at(base).g;
    let b = backdrop_at(base + normal * chroma).b;
    let refraction = vec3<f32>(r, g, b);
    let tint_amount = clamp(glass.tint.a * glass.optics.z, 0.0, 1.0);
    let transmitted = mix(refraction, glass.tint.rgb, tint_amount);
    let lighting = normalize(glass.light.xy + vec2<f32>(0.00001));
    let facing = max(dot(normal, lighting), 0.0);
    let specular = pow(facing, 5.0) * edge * max(glass.optics.w, 0.0);
    let color = clamp(transmitted + vec3<f32>(specular), vec3<f32>(0.0), vec3<f32>(1.0));
    // Premultiplied AA output. No discard, so implicit texture derivatives
    // remain defined for every pixel and corners do not flicker.
    let coverage = 1.0 - smoothstep(-0.5, 0.5, signed);
    return vec4<f32>(color * coverage, coverage);
}
