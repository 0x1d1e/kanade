use vello::wgpu::util::{BufferInitDescriptor, DeviceExt};
use vello::wgpu::{
    BindGroupDescriptor, BindGroupEntry, BindingResource, BufferUsages, Device, LoadOp, Operations,
    Queue, RenderPassColorAttachment, RenderPassDescriptor, StoreOp, Texture,
};

use ttf_parser::Face;

use crate::graphics::Color;
use crate::graphics::gpu::texture;

use super::Quads;
use super::clip::Clips;

impl Quads {
    /*
     * draws everything gathered onto the canvas, over what is already there
     *
     * a letter that would make a full atlas start over is never gathered here: `letter_in_order`
     * draws the batch first, so nothing reads the atlas, then places the letter in its own place
     */
    pub fn draw(&mut self, device: &Device, queue: &Queue, canvas: &Texture) {
        loop {
            let deferred = std::mem::take(&mut self.deferred);

            self.paint(device, queue, canvas);

            if deferred.is_empty() {
                return;
            }

            /*
             * nothing reads the atlas after `paint`, so these go in now; `letter_in_order` should
             * have emptied this already, so this only keeps a letter from being lost
             */
            for letter in deferred {
                self.letter(
                    device,
                    queue,
                    letter.face,
                    letter.id,
                    letter.size,
                    letter.color,
                    letter.x,
                    letter.y,
                    letter.clips,
                );
            }
        }
    }

    /*
     * one letter, drawn where it was gathered: a letter that would make a full atlas start over
     * waits until what reads the atlas is drawn, then goes in here, so it lands in place rather than
     * over the quads gathered after it
     */
    #[allow(clippy::too_many_arguments)]
    pub fn letter_in_order(
        &mut self,
        device: &Device,
        queue: &Queue,
        canvas: &Texture,
        face: &'static Face<'static>,
        id: u16,
        size: f32,
        color: Color,
        x: f32,
        y: f32,
        clips: Clips,
    ) {
        let put_off = self.deferred.len();

        self.letter(device, queue, face, id, size, color, x, y, clips);

        if self.deferred.len() == put_off {
            return;
        }

        // it was put off, so take it back, draw what reads the atlas, then place it where it belongs
        self.deferred.truncate(put_off);

        self.draw(device, queue, canvas);

        self.letter(device, queue, face, id, size, color, x, y, clips);
    }

    fn paint(&mut self, device: &Device, queue: &Queue, canvas: &Texture) {
        if self.waiting.is_empty() {
            return;
        }

        let numbers: Vec<u8> = self
            .waiting
            .drain(..)
            .flat_map(|value| value.to_ne_bytes())
            .collect();

        let quads = device.create_buffer_init(&BufferInitDescriptor {
            label: None,
            contents: &numbers,
            usage: BufferUsages::STORAGE,
        });

        let size: Vec<u8> = [canvas.width() as f32, canvas.height() as f32, 0.0, 0.0]
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect();

        let size = device.create_buffer_init(&BufferInitDescriptor {
            label: None,
            contents: &size,
            usage: BufferUsages::UNIFORM,
        });

        let atlas = texture::view(self.atlas.texture());

        let inputs = device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &self.inputs,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: quads.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: size.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::TextureView(&atlas),
                },
            ],
        });

        let target = texture::view(canvas);

        let mut encoder = device.create_command_encoder(&Default::default());

        {
            let attachment = RenderPassColorAttachment {
                view: &target,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Load,
                    store: StoreOp::Store,
                },
            };

            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                color_attachments: &[Some(attachment)],
                ..Default::default()
            });

            pass.set_pipeline(&self.pipeline);

            pass.set_bind_group(0, &inputs, &[]);

            for run in self.runs.drain(..) {
                let picture = run
                    .picture
                    .and_then(|key| self.pictures.get(&key))
                    .unwrap_or(&self.blank);

                pass.set_bind_group(1, picture, &[]);

                // six corners make each quad's two triangles
                pass.draw(0..6, run.first..run.first + run.count);
            }
        }

        queue.submit([encoder.finish()]);
    }
}
