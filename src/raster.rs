/*
 * pictures Kanade draws itself, the icons and the body's shadow, written once as files the runtime
 * decodes and then draws like any image
 */

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

// numbers each write, so two threads writing the same file never share its partial one
static WRITES: AtomicU64 = AtomicU64::new(0);

// the most a stored deflate block holds
const BLOCK: usize = 65_535;

/*
 * named by its contents, so a file left by an older build never stands in for a changed one;
 * written whole under another name first, as the runtime may read it on another thread
 */
pub(crate) fn write(kind: &str, extension: &str, contents: &[u8]) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    contents.hash(&mut hasher);

    let path = folder()
        .join(kind)
        .join(format!("{:016x}.{extension}", hasher.finish()));

    if !path.exists() {
        let _ = store(&path, contents);
    }

    path
}

// whether it was written
fn store(path: &Path, contents: &[u8]) -> bool {
    let write = WRITES.fetch_add(1, Ordering::Relaxed);
    let partial = path.with_extension(format!("{}.{write}.part", process::id()));

    let written = path
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::write(&partial, contents))
        .and_then(|()| fs::rename(&partial, path));

    // a part left behind would hold the room a full disk frees
    if written.is_err() {
        let _ = fs::remove_file(&partial);
    }

    /*
     * a picture that could not be written is left out, the island still works; said once for each
     * folder until a write there works again, as a full disk fails every one
     */
    static FAILING: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

    let mut failing = FAILING.lock().unwrap_or_else(PoisonError::into_inner);
    let folder = path.parent().unwrap_or(path).to_owned();

    match &written {
        Ok(()) => {
            failing.remove(&folder);
        }

        Err(error) => {
            if failing.insert(folder) {
                eprintln!("kanade: failed to write {}: {error}", path.display());
            }
        }
    }

    written.is_ok()
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
 * rgba pixels with plain alpha as a png, left uncompressed: compressing would cost more than its
 * smaller file saves
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

    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);

    png.extend(crc.finalize().to_be_bytes());
}

fn adler32(bytes: &[u8]) -> u32 {
    let mut adler = simd_adler32::Adler32::new();
    adler.write(bytes);

    adler.finish()
}
