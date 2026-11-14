// Signed-distance Gaussian edge shadow. No clock, images, radius banks or orientation switches.
fn gaussian_tail(z: f32) -> f32 {
    let a = abs(z);
    let t = 1.0 / (1.0 + 0.2316419 * a);
    let tail = 0.3989422804 * exp(-0.5*a*a) * t * (0.31938153 + t*(-0.356563782 + t*(1.781477937 + t*(-1.821255978 + t*1.330274429))));
    return select(tail, 1.0-tail, z < 0.0);
}
@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let rect = values[0];
    let half = rect.zw * 0.5;
    let p = uv*size - rect.xy - half;
    let radius = values[1].x;
    let outside = 1.0 - clamp(0.5-continuous_distance(p, half, radius), 0.0, 1.0);
    if outside <= 0.0 { return vec4<f32>(0.0); }
    let d = continuous_distance(p - vec2<f32>(0.0, values[1].z), half, radius);
    let alpha = values[2].a * outside * gaussian_tail(d / max(values[1].y, 0.001));
    return vec4<f32>(values[2].rgb, alpha);
}
