use std::collections::HashMap;

use ttf_parser::Face;
use vello::wgpu::{
    Device, Extent3d, Origin3d, Queue, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture,
    TextureAspect, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};

use crate::graphics::letter;

const SIZE: u32 = 1024;

// a gap between letters, so none reads a neighbour's pixels
const PADDING: u32 = 1;

// where one letter's pixels are in the atlas, and where they sit from the pen on the baseline
#[derive(Clone, Copy)]
pub struct Letter {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub left: f32,
    pub top: f32,
}

// what asking for a letter came to
pub enum Placement {
    // its pixels are in the atlas
    Letter(Letter),

    // it has none, like a space
    Nothing,

    /*
     * it needs the atlas to start over, which it may not while letters it holds are still waiting
     * to be drawn: the caller draws what it gathered, then asks again
     */
    Overflow,
}

// the face, the letter and its size in pixels
type Key = (usize, u16, u32);

/*
 * the rows letters fill: where the next one goes, and whether the atlas has to start over to hold
 * it. Drawing over what waits to be drawn is the one thing it may not do, so `may_start_over` decides
 */
#[derive(Default)]
struct Rows {
    // the row being filled: its top, how far along it is and its tallest letter
    top: u32,
    x: u32,
    height: u32,
}

// where a letter went, and whether the atlas started over to hold it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Spot {
    x: u32,
    y: u32,
    started_over: bool,
}

impl Rows {
    // where a letter goes, none when it needs the atlas to start over and that is not allowed yet
    fn place(&mut self, width: u32, height: u32, may_start_over: bool) -> Option<Spot> {
        // a letter too wide for what is left of the row takes the next one
        let wraps = self.x != 0 && self.x + width + PADDING > SIZE;

        let (top, height_so_far) = if wraps {
            (self.top + self.height + PADDING, 0)
        } else {
            (self.top, self.height)
        };

        /*
         * a letter with no place left waits for one, so the rows stay where they are: moving them
         * on for a letter that was never placed would lose the room this row still has
         */
        if top + height + PADDING > SIZE {
            if !may_start_over {
                return None;
            }

            *self = Rows::default();

            return self.place(width, height, false).map(|spot| Spot {
                started_over: true,
                ..spot
            });
        }

        if wraps {
            self.top = top;
            self.x = 0;
            self.height = height_so_far;
        }

        let spot = Spot {
            x: self.x,
            y: self.top,
            started_over: false,
        };

        self.x += width + PADDING;
        self.height = self.height.max(height);

        Some(spot)
    }
}

/*
 * every letter drawn so far, in one texture of coverage values; letters fill
 * it in rows, and a full atlas starts over empty and fills again from what is shown
 */
pub struct Atlas {
    /*
     * made when the first letter needs a place, so a window that draws no text, like the ones
     * that only reserve room or take gestures, holds no megabyte of it
     */
    texture: Option<Texture>,

    // what an atlas with no texture binds instead: one texel, since a pass binds an atlas anyway
    blank: Texture,

    letters: HashMap<Key, Option<Letter>>,

    rows: Rows,
}

impl Atlas {
    pub fn new(device: &Device) -> Self {
        let blank = device.create_texture(&TextureDescriptor {
            label: None,
            size: Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::R8Unorm,
            usage: TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        Self {
            texture: None,
            blank,
            letters: HashMap::new(),
            rows: Rows::default(),
        }
    }

    pub fn texture(&self) -> &Texture {
        self.texture.as_ref().unwrap_or(&self.blank)
    }

    fn start(&mut self, device: &Device) -> &Texture {
        self.texture.get_or_insert_with(|| {
            device.create_texture(&TextureDescriptor {
                label: None,
                size: Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::R8Unorm,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            })
        })
    }

    /*
     * one letter's place in the atlas, its pixels written there
     *
     * `reads` says whether quads already gathered read texels of this atlas: starting over draws
     * over the letters placed before, so while they do it waits, and the caller draws them first
     * rather than place a letter over what they read
     */
    pub fn letter(
        &mut self,
        device: &Device,
        queue: &Queue,
        face: &'static Face<'static>,
        id: u16,
        size: f32,
        reads: bool,
    ) -> Placement {
        let key = (std::ptr::from_ref(face) as usize, id, size.to_bits());

        if let Some(letter) = self.letters.get(&key) {
            return match letter {
                Some(letter) => Placement::Letter(*letter),
                None => Placement::Nothing,
            };
        }

        let Some(pixels) = letter::rasterize(face, id, size) else {
            self.letters.insert(key, None);

            return Placement::Nothing;
        };

        let (width, height) = (pixels.width, pixels.height);

        // too big for the atlas at all; it has no place now and none later either
        if width + PADDING > SIZE || height + PADDING > SIZE {
            self.letters.insert(key, None);

            return Placement::Nothing;
        }

        let Some(spot) = self.rows.place(width, height, !reads) else {
            return Placement::Overflow;
        };

        // starting over draws over every letter placed before, but the blank ones hold no pixels
        if spot.started_over {
            self.letters.retain(|_, letter| letter.is_none());
        }

        let (x, y) = (spot.x, spot.y);

        let texture = self.start(device);

        let target = TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: Origin3d { x, y, z: 0 },
            aspect: TextureAspect::All,
        };

        let layout = TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width),
            rows_per_image: Some(height),
        };

        let extent = Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        queue.write_texture(target, &pixels.coverage, layout, extent);

        let letter = Letter {
            x,
            y,
            width,
            height,
            left: pixels.left,
            top: pixels.top,
        };

        self.letters.insert(key, Some(letter));

        Placement::Letter(letter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // a full atlas: every row filled, so the next letter has nowhere to go
    fn full() -> Rows {
        Rows {
            top: SIZE - (1 + PADDING),
            x: 0,
            height: 1,
        }
    }

    /*
     * a letter that needs the atlas to start over waits for the quads reading it: starting over
     * draws over the texels they name, so it only happens once nothing reads them
     */
    #[test]
    fn starting_over_waits_for_what_reads_the_atlas() {
        let mut rows = full();

        assert!(
            rows.place(4, 4, false).is_none(),
            "a full atlas was drawn over while quads read it"
        );

        assert_eq!(
            rows.place(4, 4, true)
                .map(|spot| (spot.x, spot.y, spot.started_over)),
            Some((0, 0, true)),
            "a full atlas with nothing reading it never started over"
        );
    }

    // letters fill a row side by side, the next drops below the tallest of them
    #[test]
    fn a_row_fills_across_then_down() {
        let mut rows = Rows::default();

        let at = |x, y, started_over| Some(Spot { x, y, started_over });

        assert_eq!(rows.place(10, 12, false), at(0, 0, false));
        assert_eq!(rows.place(10, 5, false), at(10 + PADDING, 0, false));
        assert_eq!(rows.place(10, 7, false), at(2 * (10 + PADDING), 0, false));
        assert_eq!(rows.place(SIZE, 7, false), at(0, 12 + PADDING, false));
    }

    // a letter wider than what is left of the row starts the next one, wasting none of the row
    #[test]
    fn a_letter_too_wide_for_the_row_starts_the_next_one() {
        let mut rows = Rows::default();

        let at = |x, y, started_over| Some(Spot { x, y, started_over });

        assert_eq!(rows.place(1000, 10, false), at(0, 0, false));
        assert_eq!(rows.place(100, 5, false), at(0, 10 + PADDING, false));
    }

    // a letter with no place left leaves the rows as they were, so the room this row has is kept
    #[test]
    fn a_letter_that_waits_moves_no_row() {
        let mut rows = full();

        assert!(rows.place(4, 4, false).is_none());

        assert_eq!(
            (rows.top, rows.x, rows.height),
            (SIZE - (1 + PADDING), 0, 1)
        );
    }
}
