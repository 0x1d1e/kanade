use std::cell::RefCell;

use vello::peniko;
use vello::wgpu::{Device, Texture};
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene};

use super::{Gpu, texture};

/*
 * a vello of its own for the scenes that resolve no late bound resource at all, which are the cuts,
 * trims and shadows, and the frames that draw fills and paths only. vello sizes its image atlas
 * from the images a scene resolves, so a scene with none in it leaves the atlas a single texel
 * wide: that is a new atlas, the old one is freed, and every window's images and letters that were
 * in it are gone while vello still counts them as uploaded, so they sample texels that hold
 * nothing. nothing is ever sent to this one, so its atlas holds nothing to go stale
 *
 * the price is that vello's own drop of what a visit left (memory.rs) is taken by whichever
 * renderer resolves first, so this one may take it and the shared one keep those encodings a
 * visit longer. that is memory, not what is drawn, and the next visit asks for the drop again
 */
thread_local! {
    static PATHS: RefCell<Option<Renderer>> = const { RefCell::new(None) };
}

impl Gpu {
    // vello always clears what it draws into, so it draws apart and the result is laid on top
    pub(super) fn paint(&mut self, scene: &mut Scene, canvas: &Texture) {
        if scene.encoding().is_empty() {
            return;
        }

        // the same place in the frame usually gets the same scene as last frame
        let index = self.painting;

        self.painting += 1;

        if let Some(kept) = self.unchanged(index, scene, canvas) {
            self.lay(kept, canvas, 1.0, true);
        } else {
            let scratch = self.render(scene, canvas);

            self.lay(&scratch, canvas, 1.0, true);

            self.keep(index, scene, scratch);
        }

        scene.reset();
    }

    // the scene alone, on its own texture the size of the canvas, given back by the caller
    pub(super) fn render(&mut self, scene: &Scene, canvas: &Texture) -> Texture {
        let scratch = self.take_scratch(canvas.width(), canvas.height());

        let params = RenderParams {
            base_color: peniko::Color::TRANSPARENT,
            width: canvas.width(),
            height: canvas.height(),
            antialiasing_method: AaConfig::Area,
        };

        let target = texture::view(&scratch);

        if self.draws_alone(scene) {
            PATHS.with_borrow_mut(|paths| {
                let paths = paths.get_or_insert_with(|| renderer(&self.device));

                paths
                    .render_to_texture(&self.device, &self.queue, scene, &target, &params)
                    .expect("failed to draw scene");
            });
        } else {
            // the atlas is dropped when the renderer holding it is replaced (shared.rs)
            self.keep_atlas();

            self.vello
                .borrow_mut()
                .render_to_texture(&self.device, &self.queue, scene, &target, &params)
                .expect("failed to draw scene");
        }

        scratch
    }

    pub(super) fn lay(&self, source: &Texture, canvas: &Texture, opacity: f32, premultiply: bool) {
        let premultiply = if premultiply { 1.0 } else { 0.0 };

        let mut encoder = self.device.create_command_encoder(&Default::default());

        self.composite.run(
            &self.device,
            &mut encoder,
            &texture::view(source),
            &texture::view(canvas),
            [opacity, premultiply, 0.0, 0.0],
        );

        self.queue.submit([encoder.finish()]);
    }

    /*
     * whether the scene can go to the renderer of its own, which leaves the shared image atlas as
     * it was
     */
    fn draws_alone(&self, scene: &Scene) -> bool {
        resolves_no_resources(scene)
    }
}

/*
 * a scene with no patch in it resolves no late bound resource at all, images and gradients and
 * glyph runs alike, so vello would answer it with a single texel wide image atlas
 */
fn resolves_no_resources(scene: &Scene) -> bool {
    scene.encoding().resources.patches.is_empty()
}

// the options the shared renderer is made with (shared.rs), so a scene draws alike either way
fn renderer(device: &Device) -> Renderer {
    let options = RendererOptions {
        antialiasing_support: AaSupport::area_only(),
        ..RendererOptions::default()
    };

    Renderer::new(device, options).expect("failed to start vello")
}

#[cfg(test)]
mod tests {
    use vello::kurbo::BezPath;
    use vello::peniko::{
        Blob, ColorStop, Fill, Gradient, ImageAlphaType, ImageBrush, ImageData, ImageFormat,
    };

    use super::*;

    fn square() -> BezPath {
        let mut path = BezPath::new();

        path.move_to((0.0, 0.0));
        path.line_to((10.0, 0.0));
        path.line_to((10.0, 10.0));
        path.close_path();

        path
    }

    fn red() -> peniko::Color {
        peniko::Color::from_rgb8(255, 0, 0)
    }

    fn image() -> ImageData {
        ImageData {
            data: Blob::from(vec![255, 0, 0, 255]),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: 1,
            height: 1,
        }
    }

    /*
     * vello answers a scene that resolves no image with a one texel atlas, which throws away the
     * images and letters every window put in the shared one; those scenes draw on their own
     */
    #[test]
    fn a_scene_of_paths_alone_leaves_the_shared_atlas_alone() {
        let mut paths = Scene::new();

        paths.fill(Fill::NonZero, Default::default(), red(), None, &square());

        assert!(resolves_no_resources(&paths));

        let mut images = Scene::new();

        images.draw_image(&ImageBrush::new(image()), Default::default());

        assert!(
            !resolves_no_resources(&images),
            "a scene with an image was drawn by the renderer for paths alone"
        );
    }

    // a gradient is a patch but no image, so it keeps the atlas' size: it must not be drawn alone
    #[test]
    fn a_scene_of_a_gradient_stays_on_the_shared_renderer() {
        let gradient = Gradient::new_linear((0.0, 0.0), (10.0, 0.0)).with_stops([
            ColorStop::from((0.0, red())),
            ColorStop::from((1.0, peniko::Color::from_rgb8(0, 0, 255))),
        ]);

        let mut scene = Scene::new();

        scene.fill(
            Fill::NonZero,
            Default::default(),
            &gradient,
            None,
            &square(),
        );

        assert!(
            !resolves_no_resources(&scene),
            "a gradient went to the renderer for paths alone, which has no ramps"
        );
    }
}
