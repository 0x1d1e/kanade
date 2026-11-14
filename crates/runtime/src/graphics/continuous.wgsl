// Continuous shell corners; islandcn MIT attribution in THIRD_PARTY_NOTICES.md.
fn continuous_edge(r: f32, room: f32) -> vec4<f32> {
    let s = clamp(room / r - 1.0, 0.0, 0.72);
    let a = 0.7853981634 * s;
    let p = (1.0 + s) * r;
    let along = r * (1.0 - sin(a));
    let across = r * (1.0 - cos(a));
    let c = r * tan(a / 2.0) * cos(a);
    return vec4<f32>(p, max(0.0, (p - c - along) / 3.0), along, across);
}
fn continuous_sample(c: array<vec2<f32>, 4>, t: f32) -> mat3x2<f32> {
    let u = 1.0 - t;
    let p = u*u*u*c[0] + 3.0*u*u*t*c[1] + 3.0*u*t*t*c[2] + t*t*t*c[3];
    let d = 3.0 * (u*u*(c[1]-c[0]) + 2.0*u*t*(c[2]-c[1]) + t*t*(c[3]-c[2]));
    let dd = 6.0 * (u*(c[2]-2.0*c[1]+c[0]) + t*(c[3]-2.0*c[2]+c[1]));
    return mat3x2<f32>(p, d, dd);
}
fn continuous_nearest(c: array<vec2<f32>, 4>, q: vec2<f32>) -> vec2<f32> {
    var t = 0.0;
    var best = 1e20;
    for (var i = 0u; i <= 4u; i++) {
        let trial = f32(i) / 4.0;
        let d = continuous_sample(c, trial)[0] - q;
        let squared = dot(d, d);
        if squared < best { best = squared; t = trial; }
    }
    for (var i = 0u; i < 5u; i++) {
        let s = continuous_sample(c, t);
        let d = s[0] - q;
        let denominator = dot(s[1], s[1]) + dot(d, s[2]);
        if abs(denominator) > 0.000001 { t = clamp(t - dot(d, s[1]) / denominator, 0.0, 1.0); }
    }
    let s = continuous_sample(c, t);
    if abs(s[1].x) + abs(s[1].y) < 0.000001 { return vec2<f32>(1e20, 1.0); }
    let d = q - s[0];
    let side = select(1.0, -1.0, s[1].x*d.y - s[1].y*d.x <= 0.0);
    return vec2<f32>(dot(d, d), side);
}
fn continuous_distance(point: vec2<f32>, half: vec2<f32>, radius: f32) -> f32 {
    let q = half - abs(point);
    if radius <= 0.001 { return -min(q.x, q.y); }
    let r = min(radius, min(half.x, half.y));
    let x = continuous_edge(r, half.x);
    let y = continuous_edge(r, half.y);
    if q.x >= x.x || q.y >= y.x { return -min(q.x, q.y); }
    let ax = 0.7853981634 * clamp(half.x/r - 1.0, 0.0, 0.72);
    let ay = 0.7853981634 * clamp(half.y/r - 1.0, 0.0, 0.72);
    let h = (4.0/3.0) * tan((1.5707963268 - ax - ay)/4.0) * r;
    let first = array<vec2<f32>, 4>(vec2<f32>(x.x, 0.0), vec2<f32>(x.x-2.0*x.y, 0.0), vec2<f32>(x.x-3.0*x.y, 0.0), x.zw);
    let middle = array<vec2<f32>, 4>(x.zw, x.zw + h*vec2<f32>(-cos(ax), sin(ax)), y.wz + h*vec2<f32>(sin(ay), -cos(ay)), y.wz);
    let last = array<vec2<f32>, 4>(y.wz, vec2<f32>(0.0, y.x-3.0*y.y), vec2<f32>(0.0, y.x-2.0*y.y), vec2<f32>(0.0, y.x));
    var nearest = continuous_nearest(first, q);
    let m = continuous_nearest(middle, q);
    let l = continuous_nearest(last, q);
    if m.x < nearest.x { nearest = m; }
    if l.x < nearest.x { nearest = l; }
    return sqrt(nearest.x) * nearest.y;
}
