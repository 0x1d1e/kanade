/*
 * pictures Kanade draws itself, the icons and the body's shadow, written once as files Amane
 * decodes and then draws like any image
 */

use std::env;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

// numbers each write, so two threads writing the same file never share its partial one
static WRITES: AtomicU64 = AtomicU64::new(0);

// the most a stored deflate block holds
const BLOCK: usize = 65_535;

/*
 * named by its contents, so a file left by an older build never stands in for a changed one;
 * written whole under another name first, as Amane may read it on another thread
 */
pub(crate) fn write(kind: &str, extension: &str, contents: &[u8]) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    contents.hash(&mut hasher);

    let folder = folder().join(kind);

    let path = folder.join(format!("{:016x}.{extension}", hasher.finish()));

    if path.exists() {
        return path;
    }

    let write = WRITES.fetch_add(1, Ordering::Relaxed);
    let partial = path.with_extension(format!("{}.{write}.part", process::id()));

    let written = fs::create_dir_all(&folder)
        .and_then(|()| fs::write(&partial, contents))
        .and_then(|()| fs::rename(&partial, &path));

    // a picture that could not be written is left out, the island still works
    if let Err(error) = written {
        eprintln!("kanade: failed to write {}: {error}", path.display());
    }

    path
}

#[cfg(not(test))]
fn folder() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
        .map_or_else(env::temp_dir, PathBuf::from)
        .join("kanade")
}

// tests keep their pictures away from the ones a running island reads
#[cfg(test)]
fn folder() -> PathBuf {
    env::temp_dir().join("kanade-test")
}

/*
 * rgba pixels with plain alpha as a png, left uncompressed: the files are small, written once,
 * and src/ has no crates to compress with
 */
pub(crate) fn png(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    assert_eq!(pixels.len(), (width * height * 4) as usize);

    // every row starts with its filter, none
    let row = (width * 4) as usize;
    let mut raw = Vec::with_capacity(pixels.len() + height as usize);

    for line in pixels.chunks_exact(row) {
        raw.push(0);
        raw.extend_from_slice(line);
    }

    // zlib: a header, stored blocks, then the adler-32 of what they hold
    let mut zlib = vec![0x78, 0x01];
    let mut blocks = raw.chunks(BLOCK).peekable();

    if blocks.peek().is_none() {
        zlib.extend([1, 0, 0, 0xFF, 0xFF]);
    }

    while let Some(block) = blocks.next() {
        let last = u8::from(blocks.peek().is_none());
        let length = block.len() as u16;

        zlib.push(last);
        zlib.extend(length.to_le_bytes());
        zlib.extend((!length).to_le_bytes());
        zlib.extend_from_slice(block);
    }

    zlib.extend(adler32(&raw).to_be_bytes());

    let mut header = Vec::with_capacity(13);
    header.extend(width.to_be_bytes());
    header.extend(height.to_be_bytes());

    // 8 bits per channel, rgba, deflate, adaptive filtering, not interlaced
    header.extend([8, 6, 0, 0, 0]);

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);

    png
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend((data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);

    let mut crc = Crc::default();
    crc.add(kind);
    crc.add(data);

    png.extend(crc.finish().to_be_bytes());
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut low, mut high) = (1u32, 0u32);

    for &byte in bytes {
        low = (low + u32::from(byte)) % 65_521;
        high = (high + low) % 65_521;
    }

    (high << 16) | low
}

// the crc-32 png checks each chunk with
struct Crc(u32);

impl Default for Crc {
    fn default() -> Self {
        Self(0xFFFF_FFFF)
    }
}

impl Crc {
    fn add(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u32::from(byte);

            for _ in 0..8 {
                let mask = (self.0 & 1).wrapping_neg();

                self.0 = (self.0 >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
    }

    fn finish(self) -> u32 {
        !self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_their_known_values() {
        let mut crc = Crc::default();
        crc.add(b"123456789");

        assert_eq!(crc.finish(), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn a_png_holds_its_pixels_in_stored_blocks() {
        // wider than one stored block, so it takes two
        let (width, height) = (200, 100);
        let pixels: Vec<u8> = (0..width * height * 4).map(|index| index as u8).collect();

        let png = png(width, height, &pixels);

        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..24], &[0, 0, 0, 200, 0, 0, 0, 100]);
        assert!(png.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));

        // the first stored block, not the last, is a full one
        let data = 8 + 25 + 8;
        assert_eq!(&png[data + 2..data + 7], &[0, 0xFF, 0xFF, 0, 0]);

        // after the block header comes the first row: its filter, then its pixels
        assert_eq!(png[data + 7], 0);
        assert_eq!(&png[data + 8..data + 12], &pixels[..4]);
    }

    #[test]
    fn the_same_contents_share_a_file() {
        let first = write("raster", "txt", b"same");
        let second = write("raster", "txt", b"same");
        let other = write("raster", "txt", b"other");

        assert_eq!(first, second);
        assert_ne!(first, other);
        assert_eq!(fs::read(&first).unwrap(), b"same");
    }

    #[test]
    fn threads_writing_the_same_file_each_find_it_whole() {
        let contents = vec![7; 1 << 20];

        // left by an earlier run, it would spare every thread the writing
        let _ = fs::remove_file(write("raster", "bin", &contents));

        let paths: Vec<PathBuf> = std::thread::scope(|scope| {
            let writers: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| write("raster", "bin", &contents)))
                .collect();

            writers
                .into_iter()
                .map(|writer| writer.join().unwrap())
                .collect()
        });

        for path in &paths {
            assert_eq!(fs::read(path).unwrap(), contents);
        }

        let partial = fs::read_dir(paths[0].parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "part")
            });

        assert!(!partial);
    }
}
