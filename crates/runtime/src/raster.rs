//! Deterministic, premultiplied ARGB8888 software fallback.
//! Native wgpu rendering is the target. This very small fallback lets the
//! Wayland surface handshake and buffer lifetime be exercised before the
//! Vulkan/DMABUF path is attached. It does not implement liquid glass.

/// Renders one antialiased rounded rectangle into a transparent surface.
/// wl_shm ARGB8888 is BGRA byte order on little-endian Linux.
pub fn pill(width: u32, height: u32, radius: f32) -> Result<Vec<u8>, String> {
    let size = usize::try_from(
        u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or("SHM dimensions overflow")?,
    )
    .map_err(|_| "SHM dimensions overflow")?;
    if size > i32::MAX as usize || width == 0 || height == 0 {
        return Err("invalid SHM size".into());
    }

    let mut pixels = vec![0; size];
    let radius = radius.clamp(0.0, width.min(height) as f32 * 0.5);
    let half = (width as f32 * 0.5, height as f32 * 0.5);
    for y in 0..height {
        for x in 0..width {
            let qx = ((x as f32 + 0.5 - half.0).abs() - (half.0 - radius)).max(0.0);
            let qy = ((y as f32 + 0.5 - half.1).abs() - (half.1 - radius)).max(0.0);
            // With a rounded-rectangle SDF, corners are antialiased and the
            // flat edges remain sharp, without assuming any compositor blur.
            let d = (qx * qx + qy * qy).sqrt()
                + ((x as f32 + 0.5 - half.0).abs() - (half.0 - radius))
                    .max((y as f32 + 0.5 - half.1).abs() - (half.1 - radius))
                    .min(0.0)
                - radius;
            let alpha = (0.5 - d).clamp(0.0, 1.0);
            let a = (alpha * 218.0).round() as u8;
            // Premultiply by the stored (quantized) alpha, not coverage:
            // ARGB8888 must have RGB <= A even at antialiased edges.
            let opacity = f32::from(a) / 255.0;
            let index = (y as usize * width as usize + x as usize) * 4;
            pixels[index..index + 4].copy_from_slice(&[
                (29.0 * opacity).round() as u8,
                (24.0 * opacity).round() as u8,
                (18.0 * opacity).round() as u8,
                a,
            ]);
        }
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_corners_and_opaque_center() {
        let bytes = pill(100, 40, 20.0).unwrap();
        assert_eq!(&bytes[..4], &[0, 0, 0, 0]);
        assert_eq!(bytes[(20 * 100 + 50) * 4 + 3], 218);
        assert!(bytes[(20 * 100 + 50) * 4] > 0);
        assert_eq!(&bytes[(39 * 100 + 99) * 4..][..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn every_pixel_uses_premultiplied_argb() {
        let pixels = pill(101, 41, 20.0).unwrap();
        let mut antialiased = 0;
        for [blue, green, red, alpha] in pixels.as_chunks::<4>().0 {
            let opacity = f32::from(*alpha) / 255.0;
            assert_eq!(*blue, (29.0 * opacity).round() as u8);
            assert_eq!(*green, (24.0 * opacity).round() as u8);
            assert_eq!(*red, (18.0 * opacity).round() as u8);
            if *alpha > 0 && *alpha < 218 {
                antialiased += 1;
            }
        }
        assert!(antialiased > 0);
        let center = (20 * 101 + 50) * 4;
        assert_eq!(&pixels[center..center + 4], &[25, 21, 15, 218]);
    }

    #[test]
    fn invalid_dimensions_are_rejected_without_allocating() {
        assert!(pill(0, 3, 2.0).is_err());
        assert!(pill(u32::MAX, u32::MAX, 2.0).is_err());
    }

    #[test]
    fn center_of_small_radius_stays_covered() {
        let bytes = pill(20, 20, 1.0).unwrap();
        assert_eq!(bytes[(10 * 20 + 10) * 4 + 3], 218);
    }
}
