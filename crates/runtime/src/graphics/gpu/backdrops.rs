use std::collections::HashMap;

use vello::wgpu::{
    AddressMode, Device, Extent3d, FilterMode, MipmapFilterMode, Origin3d, Queue, Sampler,
    SamplerDescriptor, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureAspect,
    TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureView,
};

use crate::backdrop::{self, Pixels, Spot};

use super::{picture, texture};

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

        // smooth between pixels and between the blurred sizes, and the edge repeated past the copy
        let sampler = device.create_sampler(&SamplerDescriptor {
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Linear,
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

// how many smaller, blurrier sizes of a copy a shader may read, besides the copy itself
const BLURS: u32 = 5;

// a copy as a gpu texture with `BLURS` smaller sizes, each pixel the average of the four above it,
// for a shader to read a blur of what is around a pane in one sample
fn upload(device: &Device, queue: &Queue, pixels: &Pixels) -> Texture {
    let mut levels = vec![(pixels.width, pixels.height, pixels.rgba.clone())];

    for _ in 0..BLURS {
        let (width, height, last) = levels.last().expect("a copy has a first size");

        if *width <= 1 && *height <= 1 {
            break;
        }

        let smaller = picture::halve(last, *width, *height);

        levels.push(((*width / 2).max(1), (*height / 2).max(1), smaller));
    }

    let texture = device.create_texture(&TextureDescriptor {
        label: None,
        size: Extent3d {
            width: pixels.width,
            height: pixels.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    });

    for (level, (width, height, rgba)) in levels.iter().enumerate() {
        let target = TexelCopyTextureInfo {
            texture: &texture,
            mip_level: level as u32,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        };

        let layout = TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(*height),
        };

        let size = Extent3d {
            width: *width,
            height: *height,
            depth_or_array_layers: 1,
        };

        queue.write_texture(target, rgba, layout, size);
    }

    texture
}
