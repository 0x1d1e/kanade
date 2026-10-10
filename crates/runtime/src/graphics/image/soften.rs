use std::sync::Arc;

use super::Bitmap;

/*
 * blurs the image by averaging each pixel with its neighbours up to
 * radius away, first along rows and then along columns; three rounds
 * of that look close to a gaussian blur
 */
pub fn soften(mut image: Bitmap, radius: u32) -> Bitmap {
    if radius == 0 {
        return image;
    }

    for _ in 0..3 {
        image.pixels = Arc::new(pass(&image, radius, 1, 0));
        image.pixels = Arc::new(pass(&image, radius, 0, 1));
    }

    image
}

// one box blur along x when step_x is 1, or along y when step_y is 1
fn pass(image: &Bitmap, radius: u32, step_x: u32, step_y: u32) -> Vec<u8> {
    let width = image.width as i64;
    let height = image.height as i64;
    let radius = radius as i64;

    let mut blurred = vec![0; image.pixels.len()];

    for y in 0..height {
        for x in 0..width {
            let mut total = [0u32; 4];
            let mut count = 0;

            for offset in -radius..=radius {
                // past the edge, the edge pixel repeats
                let sample_x = (x + offset * step_x as i64).clamp(0, width - 1);
                let sample_y = (y + offset * step_y as i64).clamp(0, height - 1);

                let index = ((sample_y * width + sample_x) * 4) as usize;

                for (sum, &value) in total.iter_mut().zip(&image.pixels[index..index + 4]) {
                    *sum += u32::from(value);
                }

                count += 1;
            }

            let index = ((y * width + x) * 4) as usize;

            for channel in 0..4 {
                blurred[index + channel] = (total[channel] / count) as u8;
            }
        }
    }

    blurred
}
