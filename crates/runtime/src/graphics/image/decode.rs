//! An image file to pixels: png, jpeg, the first frame of a webp or gif, and svg, each refused past
//! `MOST_PIXELS` as its header tells, so a huge file costs no huge allocation.

use std::fs;
use std::io::{self, Read};
use std::num::NonZeroU64;
use std::path::Path;

use super::svg;

// the most pixels an image is decoded with, an 8K screen's
pub const MOST_PIXELS: usize = 7680 * 4320;

/*
 * the longest side an svg may be drawn with, so that `side * side` (its most pixels, whichever way
 * it is proportioned) stays under `MOST_PIXELS`; a request past it is refused rather than drawn
 */
pub fn most_side() -> f32 {
    (MOST_PIXELS as f32).sqrt()
}

// whether an svg asked for at `longest` pixels across fits under `MOST_PIXELS`, whatever its shape
pub fn side_fits(longest: f32) -> bool {
    longest.is_finite() && (0.0..=most_side()).contains(&longest)
}

// the most bytes an image of `MOST_PIXELS` takes as 8-bit RGBA, what a decode may allocate
const MOST_BYTES: usize = MOST_PIXELS * 4;

/*
 * how many pixels a header claims, none for one past `MOST_PIXELS`
 *
 * the product is taken in `u64` and only then narrowed, so a header claiming two huge dimensions
 * cannot wrap around to a small count that passes, whatever the width of `usize`
 */
fn count(width: u64, height: u64) -> Option<usize> {
    let count = width.checked_mul(height)?;

    usize::try_from(count)
        .ok()
        .filter(|count| *count <= MOST_PIXELS)
}

// whether a buffer the decoder would allocate is no more than an image of `MOST_PIXELS` takes
fn bytes_fit(bytes: usize) -> bool {
    bytes <= MOST_BYTES
}

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
    count(u64::from(width), u64::from(height))?;

    // the whole frame at once, so its size is checked before it is allocated
    let size = reader.output_buffer_size()?;
    if !bytes_fit(size) {
        return None;
    }

    let mut into = vec![0; size];
    let frame = reader.next_frame(&mut into).ok()?;
    into.truncate(frame.buffer_size());

    let rgba = match frame.color_type.samples() {
        4 => into,
        3 => rgba(&into, 3, |pixel| [pixel[0], pixel[1], pixel[2], 255]),
        2 => rgba(&into, 2, |pixel| [pixel[0], pixel[0], pixel[0], pixel[1]]),
        1 => rgba(&into, 1, |pixel| [pixel[0], pixel[0], pixel[0], 255]),
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
            count(width as u64, height as u64)?;

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
            count(u64::from(width), u64::from(height))?;

            // sized by those dimensions, so the check above bounds it too
            let size = decoder.output_buffer_size()?;
            if !bytes_fit(size) {
                return None;
            }

            let mut pixels = vec![0; size];
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
            let pixels = count(width as u64, height as u64)?;

            // the first frame, which may cover only part of the screen, on a clear one
            let frame = decoder.read_next_frame().ok()??;
            let mut canvas = vec![0; pixels * 4];
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

            // an svg is drawn at whatever side is asked for, so the ask is bounded before it is
            if !side_fits(vector_side) {
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

    // an svg drawn at a side large enough to take more than `MOST_PIXELS` pixels is refused
    #[test]
    fn a_bomb_is_refused_before_it_is_drawn() {
        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\">\
                    <rect width=\"10\" height=\"10\" fill=\"red\"/></svg>";

        // 20000 across is 400 million pixels, and a 1.6 GB allocation, before this was checked
        assert_eq!(
            super::super::draw_svg(svg, 20000.0),
            None,
            "an svg past MOST_PIXELS was drawn and allocated"
        );

        assert!(
            super::super::draw_svg(svg, 256.0).is_some(),
            "an icon was refused"
        );

        // the ask is what bounds an svg, not its own size, so an infinite or a negative one is refused
        assert_eq!(super::super::draw_svg(svg, f32::INFINITY), None);
        assert_eq!(super::super::draw_svg(svg, -1.0), None);
    }

    // a header claiming more pixels than `MOST_PIXELS` is refused, whatever its two dimensions
    #[test]
    fn a_header_claiming_more_pixels_than_most_is_refused() {
        assert_eq!(count(7680, 4320), Some(MOST_PIXELS));
        assert_eq!(count(7681, 4320), None);

        // two dimensions whose product wraps a 32-bit count around to a small, passing one
        assert_eq!(count(65_536, 65_536), None);
        assert_eq!(count(u64::from(u32::MAX), u64::from(u32::MAX)), None);
    }

    // no decode may allocate past the pixels `MOST_PIXELS` takes as RGBA
    #[test]
    fn a_buffer_past_the_pixels_is_refused() {
        assert!(bytes_fit(MOST_PIXELS * 4));
        assert!(!bytes_fit(MOST_PIXELS * 4 + 1));
    }
}
