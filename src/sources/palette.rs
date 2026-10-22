//! Colors taken from an image, the wallpaper or a cover, most common first (ADR 0027 step 1).
//! Read once when an image is opened: `open` takes them right away, so a new file at the same path
//! or a new path is read where it is opened, not polled for.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use kanade_runtime::Color;

use super::wallpaper;

// the longest side an svg is drawn at to be sampled: its pixels are read, not shown
const SAMPLE_SIDE: f32 = 256.0;

mod median_cut;
mod sample;

// colors taken from an image, usually the wallpaper, most common first
#[derive(Default)]
pub struct Palette {
    path: Option<PathBuf>,

    count: usize,

    // when the file was last read, so a rewrite of the same path is noticed
    modified: Option<SystemTime>,

    colors: Vec<Color>,
}

impl Palette {
    /*
     * reads the image right away, so the first frame already has its
     * colors; 16 is enough for a whole shell's theme
     */
    pub fn open(&mut self, path: &str, count: usize) {
        let path = PathBuf::from(path);

        self.modified = modified_time(&path);
        self.colors = quantize(&path, count);

        self.path = Some(path);
        self.count = count;
    }

    // sorted by how much of the image each one covers
    pub fn colors(&self) -> &[Color] {
        &self.colors
    }
}

fn quantize(path: &Path, count: usize) -> Vec<Color> {
    /*
     * a wallpaper can be read while it's still being written,
     * and a half written file must not take the shell down with it
     */
    let Some((width, height, rgba)) = wallpaper::decoded(path, SAMPLE_SIDE) else {
        return Vec::new();
    };

    let pixels = sample::pixels(width, height, &rgba);

    median_cut::quantize(pixels, count)
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}
