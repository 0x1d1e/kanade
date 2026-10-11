use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{Options, Tree};

/*
 * an svg drawn once with its longest side `longest` pixels across, as width, height and plain
 * alpha RGBA, then cached like any image; drawing the paths again every frame cost a full launcher
 * of icons most of its frame
 */
pub fn rasterize(bytes: &[u8], longest: f32) -> Option<(u32, u32, Vec<u8>)> {
    let tree = Tree::from_data(bytes, &Options::default()).ok()?;

    let size = tree.size();

    let scale = longest / size.width().max(size.height());

    let width = (size.width() * scale).ceil() as u32;
    let height = (size.height() * scale).ceil() as u32;

    // none for an svg with no size to draw at
    let mut pixmap = Pixmap::new(width, height)?;

    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    // tiny-skia keeps premultiplied alpha, a bitmap holds plain alpha
    let mut pixels = Vec::with_capacity(pixmap.data().len());

    for pixel in pixmap.pixels() {
        let plain = pixel.demultiply();

        pixels.extend([plain.red(), plain.green(), plain.blue(), plain.alpha()]);
    }

    Some((width, height, pixels))
}
