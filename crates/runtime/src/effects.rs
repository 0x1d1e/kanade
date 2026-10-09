//! Direct GPU liquid-glass shader and its layout contract.
//!
//! The capture/render backend binds a texture and uploads this uniform.
//! These definitions contain no disk or CPU refraction pipeline.

pub const LIQUID_GLASS: &str = include_str!("../shaders/liquid_glass.wgsl");

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassUniform {
    pub body: [f32; 4],
    pub optics: [f32; 4],
    pub capture: [f32; 4],
    pub tint: [f32; 4],
    pub light: [f32; 4],
}

impl GlassUniform {
    pub fn new(body: [f32; 3], origin: [f32; 2], extent: [f32; 2]) -> Self {
        Self {
            body: [body[0], body[1], body[2], 12.0],
            optics: [7.0, 0.65, 1.0, 0.12],
            capture: [origin[0], origin[1], extent[0], extent[1]],
            tint: [0.10, 0.12, 0.16, 0.25],
            light: [-0.7, -0.7, 0.0, 0.0],
        }
    }

    pub fn words(&self) -> [f32; 20] {
        let mut words = [0.0; 20];
        for (i, row) in [self.body, self.optics, self.capture, self.tint, self.light]
            .into_iter()
            .enumerate()
        {
            words[i * 4..(i + 1) * 4].copy_from_slice(&row);
        }
        words
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_layout_is_five_aligned_vec4_rows() {
        let uniform = GlassUniform::new([260.0, 40.0, 20.0], [16.0, 16.0], [292.0, 72.0]);
        assert_eq!(std::mem::size_of::<GlassUniform>(), 80);
        assert_eq!(uniform.words()[0], 260.0);
        assert_eq!(uniform.words()[10], 292.0);
    }

    #[test]
    fn liquid_glass_wgsl_parses_and_validates() {
        let module = naga::front::wgsl::parse_str(LIQUID_GLASS)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(LIQUID_GLASS)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("valid liquid-glass shader");
        assert!(module.entry_points.iter().any(|e| e.name == "fragment_main"));
        assert!(module.entry_points.iter().any(|e| e.name == "vertex_main"));
    }
}
