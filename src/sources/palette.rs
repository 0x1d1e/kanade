//! Colors taken from an image, the wallpaper or a cover, most common first (ADR 0027 step 1).
//! Polled, so both a new file at the same path and a new path are picked up.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

use kanade_runtime::Color;
use kanade_runtime::service::Service;

use super::wallpaper;

mod median_cut;
mod pick;
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

// polled, so both a new file at the same path and a new path are picked up
impl Service for Palette {
    fn new() -> Self {
        Self::default()
    }

    fn interval() -> Duration {
        Duration::from_millis(500)
    }

    /*
     * the image is decoded without holding the service,
     * so the window keeps drawing while a large wallpaper loads
     */
    fn listen() {
        loop {
            thread::sleep(Self::interval());

            let (path, count, modified) = {
                let palette = Self::read();

                let Some(path) = palette.path.clone() else {
                    continue;
                };

                (path, palette.count, palette.modified)
            };

            let now = modified_time(&path);

            if now.is_none() || now == modified {
                continue;
            }

            let colors = quantize(&path, count);

            // a half written file, the next change brings the rest
            if colors.is_empty() {
                continue;
            }

            let mut palette = Self::write();

            // opened again while decoding, the newer path wins
            if palette.path.as_deref() != Some(path.as_path()) {
                continue;
            }

            palette.modified = now;
            palette.colors = colors;
        }
    }
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

    // the most vivid color, for highlights; none before the image gave colors
    pub fn accent(&self) -> Option<Color> {
        pick::most_vivid(&self.colors)
    }

    // the darkest color, so light text always reads on it
    pub fn background(&self) -> Option<Color> {
        pick::darkest(&self.colors)
    }

    // the most contrasting color readable on `background`, none when none is
    pub fn readable_on(&self, background: Color) -> Option<Color> {
        pick::readable_on(background, &self.colors)
    }
}

fn quantize(path: &Path, count: usize) -> Vec<Color> {
    /*
     * a wallpaper can be read while it's still being written,
     * and a half written file must not take the shell down with it
     */
    let Some((width, height, rgba)) = wallpaper::decoded(path) else {
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
