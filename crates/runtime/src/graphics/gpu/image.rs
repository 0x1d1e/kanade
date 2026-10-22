use std::sync::Arc;

use vello::Scene;
use vello::peniko::{Blob, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat, ImageQuality};

use crate::graphics::image::Bitmap;
use crate::graphics::{Area, Corners, Transform};

use super::Gpu;
use super::convert::{affine, bezier};

impl Gpu {
    pub(super) fn draw_image(
        &mut self,
        scene: &mut Scene,
        image: Arc<Bitmap>,
        transform: Transform,
        area: Area,
        radius: Corners,
        clip_transform: Transform,
    ) {
        let Some(clip) = area.trace(radius) else {
            return;
        };

        self.note_shown(&image);

        // bicubic keeps a large image smooth when it shrinks to fit
        let brush = ImageBrush::new(self.image(&image)).with_quality(ImageQuality::High);

        scene.push_clip_layer(Fill::NonZero, affine(clip_transform), &bezier(&clip));

        scene.draw_image(&brush, affine(transform));

        scene.pop_layer();
    }

    /*
     * vello's image atlas lives in the shared renderer, and it goes when that renderer is replaced
     * (shared.rs, when a normal window closes). the images are still counted as uploaded in the
     * atlas that went, so they are sent again; the atlas is shared, so every window does this
     *
     * a scene that resolves no image no longer reaches this: it would leave the atlas a texel wide
     * and free the one every window's images were in, so flush.rs draws those on a renderer of
     * their own, which never holds an image
     */
    pub(super) fn keep_atlas(&mut self) {
        if self.atlas_seen == self.atlas_drops.get() {
            return;
        }

        for image in self.images.values() {
            self.vello.borrow_mut().mark_override_image_dirty(image);
        }

        for glyph in self.glyphs.values().flatten() {
            self.vello
                .borrow_mut()
                .mark_override_image_dirty(glyph.image());
        }

        self.atlas_seen = self.atlas_drops.get();
    }

    // a shown image stays where it is, so where it lives says which image it is
    fn image(&mut self, image: &Bitmap) -> ImageData {
        let key = std::ptr::from_ref(image) as usize;

        let converted = self.images.entry(key).or_insert_with(|| ImageData {
            data: Blob::new(image.pixels.clone()),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: image.width(),
            height: image.height(),
        });

        converted.clone()
    }
}
