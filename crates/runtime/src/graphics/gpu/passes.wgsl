@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<uniform> settings: vec4<f32>;

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // corners at (-1, -1), (3, -1) and (-1, 3) make a triangle that covers the whole screen
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;

    return vec4<f32>(x, y, 0.0, 1.0);
}

// settings x is the opacity, y is 1 when the source still needs its colors premultiplied
@fragment
fn composite(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    var color = textureLoad(source, vec2<i32>(position.xy), 0);

    if settings.y == 1.0 {
        color = vec4<f32>(color.rgb * color.a, color.a);
    }

    return color * settings.x;
}
