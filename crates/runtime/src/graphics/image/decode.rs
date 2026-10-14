//! An image file to pixels: png, jpeg, the first frame of a webp or gif, and svg, each refused past
//! `MOST_PIXELS` as its header tells, so a huge file costs no huge allocation.

use std::fs;
use std::io::{self, Read};
use std::num::NonZeroU64;
use std::path::Path;

use super::svg;

// the most pixels an image is decoded with, an 8K screen's
pub const MOST_PIXELS: usize = 7680 * 4320;

// the most bytes an svg is read from
const MOST_SVG: u64 = 8 << 20;

// what can be decoded: all but svg told by how they start, svg by name
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
    Gif,
    Svg,
}

pub fn format(path: &Path) -> Option<Format> {
    let mut start = [0; 12];
    let read = fs::File::open(path)
        .and_then(|mut file| file.read(&mut start))
        .ok()?;
    let start = &start[..read];

    if start.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if start.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(Format::Jpeg)
    } else if start.starts_with(b"RIFF")
        && start.get(8..).is_some_and(|rest| rest.starts_with(b"WEBP"))
    {
        Some(Format::Webp)
    } else if start.starts_with(b"GIF8") {
        Some(Format::Gif)
    } else {
        path.extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
            .then_some(Format::Svg)
    }
}

// a png whole, as 8-bit RGBA with plain alpha; none for one unreadable, or past `MOST_PIXELS`
pub fn decode_png(source: impl io::BufRead + io::Seek) -> Option<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(source);
    decoder.set_transformations(png::Transformations::normalize_to_color8());

    let mut reader = decoder.read_info().ok()?;
    let info = reader.info();
    let (width, height) = (info.width, info.height);
    if width as usize * height as usize > MOST_PIXELS {
        return None;
    }

    let mut pixels = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut pixels).ok()?;
    pixels.truncate(frame.buffer_size());

    let rgba = match frame.color_type.samples() {
        4 => pixels,
        3 => rgba(&pixels, 3, |pixel| [pixel[0], pixel[1], pixel[2], 255]),
        2 => rgba(&pixels, 2, |pixel| [pixel[0], pixel[0], pixel[0], pixel[1]]),
        1 => rgba(&pixels, 1, |pixel| [pixel[0], pixel[0], pixel[0], 255]),
        _ => return None,
    };

    Some((width, height, rgba))
}

/*
 * an image whole, as 8-bit RGBA with plain alpha: the first frame of a webp or gif, an svg drawn
 * with its longest side `vector_side` pixels across; none for one unreadable, or past
 * `MOST_PIXELS`
 */
pub fn decode(path: &Path, vector_side: f32) -> Option<(u32, u32, Vec<u8>)> {
    match format(path)? {
        Format::Png => decode_png(io::BufReader::new(fs::File::open(path).ok()?)),
        Format::Jpeg => {
            let file = io::BufReader::new(fs::File::open(path).ok()?);
            let options = zune_jpeg::zune_core::options::DecoderOptions::default()
                .jpeg_set_out_colorspace(zune_jpeg::zune_core::colorspace::ColorSpace::RGBA);
            let mut decoder = zune_jpeg::JpegDecoder::new_with_options(file, options);
            decoder.decode_headers().ok()?;

            let (width, height) = decoder.dimensions()?;
            if width * height > MOST_PIXELS {
                return None;
            }

            let pixels = decoder.decode().ok()?;

            Some((
                u32::try_from(width).ok()?,
                u32::try_from(height).ok()?,
                pixels,
            ))
        }
        Format::Webp => {
            let file = io::BufReader::new(fs::File::open(path).ok()?);
            let mut decoder = image_webp::WebPDecoder::new(file).ok()?;

            let (width, height) = decoder.dimensions();
            if width as usize * height as usize > MOST_PIXELS {
                return None;
            }

            let mut pixels = vec![0; decoder.output_buffer_size()?];
            decoder.read_image(&mut pixels).ok()?;

            let rgba = if decoder.has_alpha() {
                pixels
            } else {
                rgba(&pixels, 3, |pixel| [pixel[0], pixel[1], pixel[2], 255])
            };

            Some((width, height, rgba))
        }
        Format::Gif => {
            let file = io::BufReader::new(fs::File::open(path).ok()?);
            let mut options = gif::DecodeOptions::new();
            options.set_color_output(gif::ColorOutput::RGBA);

            // the crate's own 50 MB would refuse a screen past 12 MP, below `MOST_PIXELS`
            options.set_memory_limit(gif::MemoryLimit::Bytes(NonZeroU64::new(
                MOST_PIXELS as u64 * 4,
            )?));

            let mut decoder = options.read_info(file).ok()?;
            let (width, height) = (usize::from(decoder.width()), usize::from(decoder.height()));
            if width * height > MOST_PIXELS {
                return None;
            }

            // the first frame, which may cover only part of the screen, on a clear one
            let frame = decoder.read_next_frame().ok()??;
            let mut canvas = vec![0; width * height * 4];
            let (left, top) = (usize::from(frame.left), usize::from(frame.top));
            let across = usize::from(frame.width);

            // what lies past the screen's edge is clipped; a frame wholly past it draws nothing
            let visible = across.min(width.saturating_sub(left)) * 4;

            if visible > 0 {
                for (row, pixels) in frame.buffer.chunks_exact(across * 4).enumerate() {
                    if top + row >= height {
                        break;
                    }

                    let start = ((top + row) * width + left) * 4;
                    canvas[start..start + visible].copy_from_slice(&pixels[..visible]);
                }
            }

            Some((
                u32::from(decoder.width()),
                u32::from(decoder.height()),
                canvas,
            ))
        }
        Format::Svg => {
            // read up to a byte past the cap, so a longer one is refused and a special file is no wait
            let mut bytes = Vec::new();
            fs::File::open(path)
                .ok()?
                .take(MOST_SVG + 1)
                .read_to_end(&mut bytes)
                .ok()?;
            if bytes.len() as u64 > MOST_SVG {
                return None;
            }

            svg::rasterize(&bytes, vector_side)
        }
    }
}

// pixels of `components` each, made RGBA one by one
fn rgba(pixels: &[u8], components: usize, each: impl Fn(&[u8]) -> [u8; 4]) -> Vec<u8> {
    pixels.chunks_exact(components).flat_map(each).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // a GIF is outside input: a first frame placed past the screen's edge must not panic
    #[test]
    fn a_frame_past_the_screen_draws_nothing() {
        let path = std::env::temp_dir().join(format!("kanade-past-{}.gif", std::process::id()));
        let mut file = std::fs::File::create(&path).unwrap();
        let mut encoder = gif::Encoder::new(&mut file, 4, 4, &[0, 0, 0, 255, 255, 255]).unwrap();

        let mut frame = gif::Frame::from_indexed_pixels(2, 2, vec![1; 4], None);
        frame.left = 100;
        encoder.write_frame(&frame).unwrap();
        drop(encoder);

        let decoded = decode(&path, 0.0);
        std::fs::remove_file(&path).unwrap();

        let (width, height, pixels) = decoded.unwrap();
        assert_eq!((width, height), (4, 4));
        assert!(pixels.iter().all(|&byte| byte == 0));
    }
}
