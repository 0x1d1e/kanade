use std::collections::HashMap;

use vello::wgpu::{
    AddressMode, Device, Extent3d, FilterMode, MipmapFilterMode, Origin3d, Queue, Sampler,
    SamplerDescriptor, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureAspect,
    TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureView,
};

use crate::backdrop::{self, Pixels, Spot};

use super::texture;

/*
 * the copies of what is behind each pane (backdrop.rs) as gpu textures, one upload per copy
 * for every window, and what a shader sees where it has none: one transparent texel
 */
pub struct Backdrops {
    sampler: Sampler,
    blank: TextureView,

    // the generation each pane's texture was uploaded from
    uploaded: HashMap<Spot, (u64, TextureView)>,
}

impl Backdrops {
    pub fn new(device: &Device, queue: &Queue) -> Self {
        let blank = upload(
            device,
            queue,
            &Pixels {
                generation: 0,
                width: 1,
                height: 1,
                rgba: vec![0; 4],
            },
        );

        // smooth between pixels, and the edge repeated past the copy
        let sampler = device.create_sampler(&SamplerDescriptor {
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Nearest,
            ..Default::default()
        });

        Self {
            sampler,
            blank: texture::view(&blank),
            uploaded: HashMap::new(),
        }
    }

    pub fn sampler(&self) -> &Sampler {
        &self.sampler
    }

    // a pane's newest copy, uploaded when it is new to the gpu; the transparent texel without one
    pub fn view(&mut self, device: &Device, queue: &Queue, spot: Option<&Spot>) -> TextureView {
        let Some(pixels) = spot.and_then(backdrop::pixels) else {
            self.forget();

            return self.blank.clone();
        };

        let spot = spot.expect("a copy has a pane");

        if let Some((generation, view)) = self.uploaded.get(spot)
            && *generation == pixels.generation
        {
            return view.clone();
        }

        self.forget();

        let view = texture::view(&upload(device, queue, &pixels));

        self.uploaded
            .insert(spot.clone(), (pixels.generation, view.clone()));

        view
    }

    // panes let go or gone, or paused, no longer hold textures
    fn forget(&mut self) {
        self.uploaded
            .retain(|spot, _| backdrop::pixels(spot).is_some());
    }
}

fn upload(device: &Device, queue: &Queue, pixels: &Pixels) -> Texture {
    let size = Extent3d {
        width: pixels.width,
        height: pixels.height,
        depth_or_array_layers: 1,
    };

    let texture = device.create_texture(&TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    });

    let target = TexelCopyTextureInfo {
        texture: &texture,
        mip_level: 0,
        origin: Origin3d::ZERO,
        aspect: TextureAspect::All,
    };

    let layout = TexelCopyBufferLayout {
        offset: 0,
        bytes_per_row: Some(pixels.width * 4),
        rows_per_image: Some(pixels.height),
    };

    queue.write_texture(target, &pixels.rgba, layout, size);

    texture
}
