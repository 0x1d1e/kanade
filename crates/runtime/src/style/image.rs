use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::graphics::Area;
use crate::graphics::image::{self, Bitmap, ImageState};

pub struct Image {
    pub(crate) path: PathBuf,
    pub(crate) fit: Fit,

    // the size it is shrunk to cover when decoded, none keeps every pixel
    pub(crate) thumbnail: Option<(u32, u32)>,

    // pixels a caller already decoded, drawn as they are and no file read
    pub(crate) picture: Option<Arc<Bitmap>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    // fills the rectangle exactly, bending the image's shape to match
    Stretch,

    // fills the whole rectangle, cutting off what spills past the edges
    Cover,

    // shows the whole image, leaving the rest of the rectangle uncovered
    Contain,
}

impl Image {
    pub fn cover(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            fit: Fit::Cover,
            thumbnail: None,
            picture: None,
        }
    }

    pub fn contain(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            fit: Fit::Contain,
            thumbnail: None,
            picture: None,
        }
    }

    pub fn stretch(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            fit: Fit::Stretch,
            thumbnail: None,
            picture: None,
        }
    }

    /*
     * pixels the caller decoded itself, drawn at once with no file to read: a
     * picture whose frames arrive one at a time hands over each as it comes
     */
    pub fn bitmap(picture: Arc<Bitmap>) -> Self {
        Self {
            path: PathBuf::new(),
            fit: Fit::Cover,
            thumbnail: None,
            picture: Some(picture),
        }
    }

    /*
     * keeps a small copy big enough to cover width by height, for pictures
     * shown far smaller than their file, like a list of wallpapers
     */
    pub fn thumbnail(mut self, width: u32, height: u32) -> Self {
        self.thumbnail = Some((width, height));

        self
    }

    /*
     * starts decoding the file if nothing asked for it yet, and says whether
     * it is ready to draw; a file that can't be read never becomes ready
     */
    pub fn loaded(path: impl AsRef<Path>) -> bool {
        let path = path.as_ref();

        image::load(path, None).is_some()
    }

    /*
     * how the copy this draws stands, starting its decode if nothing asked for it yet, so a
     * picture still loading or unreadable can show something in its place; a window that reads
     * one loading is drawn again once it is done
     */
    pub fn state(&self) -> ImageState {
        if self.picture.is_some() {
            return ImageState::Ready;
        }

        image::state(&self.path, self.thumbnail)
    }
}

impl Fit {
    pub fn place(self, area: Area, image_width: f32, image_height: f32) -> Area {
        let horizontal_scale = area.width / image_width;
        let vertical_scale = area.height / image_height;

        let scale = match self {
            Fit::Stretch => return area,
            Fit::Cover => f32::max(horizontal_scale, vertical_scale),
            Fit::Contain => f32::min(horizontal_scale, vertical_scale),
        };

        let width = image_width * scale;
        let height = image_height * scale;

        let x = area.x + (area.width - width) / 2.0;
        let y = area.y + (area.height - height) / 2.0;

        Area::new(x, y, width, height)
    }
}
