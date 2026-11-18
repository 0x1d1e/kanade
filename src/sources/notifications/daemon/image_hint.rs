use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU32, Ordering};

use png::{BitDepth, ColorType, Encoder};
use rustix::fs::OFlags;
use rustix::process::geteuid;

use crate::atomic::{self, Mode};
use crate::bus::Value;

// the spec renamed this hint twice, senders still use all three names
const DATA_HINTS: [&str; 3] = ["image-data", "image_data", "icon_data"];

const PATH_HINTS: [&str; 2] = ["image-path", "image_path"];

const FOLDER: &str = "kanade-notifications";

// each saved image gets a new name, so a replaced notification never shows a cached old one
static NEXT_NAME: AtomicU32 = AtomicU32::new(0);

/*
 * raw pixels are written to a png, so they are drawn like any image
 * file; a sender's own image path comes back as it was sent
 */
pub fn read(hints: &Value) -> String {
    for name in DATA_HINTS {
        if let Some(path) = save(hints.get(name)) {
            return path.to_string_lossy().into_owned();
        }
    }

    for name in PATH_HINTS {
        let path = hints.get(name).text();

        if !path.is_empty() {
            return String::from(path);
        }
    }

    String::new()
}

// only images saved here are removed, never a file the sender owns
pub fn remove(image: &str) {
    remove_in(image, env::var_os("XDG_RUNTIME_DIR").as_deref());
}

fn remove_in(image: &str, runtime: Option<&OsStr>) {
    if image.is_empty() {
        return;
    }

    let Some(root) = root_dir(runtime.map(OsStr::to_os_string)) else {
        return;
    };
    let own = root.join(process::id().to_string());
    let path = Path::new(image);

    if path.parent() == Some(own.as_path())
        && fs::symlink_metadata(&root)
            .is_ok_and(|meta| !meta.file_type().is_symlink() && meta.is_dir())
        && fs::symlink_metadata(&own)
            .is_ok_and(|meta| !meta.file_type().is_symlink() && meta.is_dir())
    {
        let _ = fs::remove_file(path);
    }
}

pub(super) fn sweep() {
    let Some(root) = root_dir(env::var_os("XDG_RUNTIME_DIR")) else {
        return;
    };

    sweep_in(&root);
}

fn sweep_in(root: &Path) {
    let euid = geteuid().as_raw();
    let Ok(meta) = fs::symlink_metadata(root) else {
        return;
    };
    if meta.file_type().is_symlink() || !meta.is_dir() || meta.uid() != euid {
        return;
    }

    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if !fs::symlink_metadata(&path)
            .is_ok_and(|meta| !meta.file_type().is_symlink() && meta.is_dir() && meta.uid() == euid)
        {
            continue;
        }

        let name = entry.file_name();
        let pid = name.to_str().and_then(|name| name.parse::<u32>().ok());
        if pid.is_some_and(|pid| !atomic::alive(pid)) {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

fn root_dir(runtime: Option<OsString>) -> Option<PathBuf> {
    let runtime = PathBuf::from(runtime?);
    if !runtime.is_absolute() {
        return None;
    }

    let meta = fs::symlink_metadata(&runtime).ok()?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return None;
    }

    Some(runtime.join(FOLDER))
}

fn ensure_private_dir(path: &Path) -> io::Result<()> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }

    let flags = OFlags::DIRECTORY | OFlags::NOFOLLOW;
    let dir = OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits() as i32)
        .open(path)?;
    let meta = dir.metadata()?;
    if !meta.is_dir() || meta.uid() != geteuid().as_raw() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "notification image directory is not private",
        ));
    }

    if meta.permissions().mode() & 0o777 != 0o700 {
        dir.set_permissions(Permissions::from_mode(0o700))?;
        if dir.metadata()?.permissions().mode() & 0o777 != 0o700 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "notification image directory mode is not 0700",
            ));
        }
    }

    Ok(())
}

fn save(hint: &Value) -> Option<PathBuf> {
    save_in(hint, env::var_os("XDG_RUNTIME_DIR").as_deref())
}

// the hint is (width, height, rowstride, has_alpha, bits_per_sample, channels, data)
fn save_in(hint: &Value, runtime: Option<&OsStr>) -> Option<PathBuf> {
    let [width, height, rowstride, _has_alpha, bits, channels, data] = hint.list() else {
        return None;
    };

    let width = width.number() as usize;
    let height = height.number() as usize;
    let rowstride = rowstride.number() as usize;
    let channels = channels.number() as usize;

    // the spec only allows 8 bit rgb or rgba
    if bits.number() != 8.0 || !(channels == 3 || channels == 4) || width == 0 || height == 0 {
        return None;
    }

    let data = data.list();

    // a row is at least as wide as its pixels, and every row must be there, so the sender's
    // sizes cannot ask for more than the data it sent
    let row = width.checked_mul(channels)?;
    let needed = (height - 1).checked_mul(rowstride)?.checked_add(row)?;

    if rowstride < row || needed > data.len() {
        return None;
    }

    let root = root_dir(runtime.map(OsStr::to_os_string))?;

    let mut pixels = Vec::with_capacity(width * height * 4);

    for y in 0..height {
        for x in 0..width {
            let start = y * rowstride + x * channels;

            let pixel = data.get(start..start + channels)?;

            for channel in pixel {
                pixels.push(channel.number() as u8);
            }

            if channels == 3 {
                pixels.push(255);
            }
        }
    }

    let mut png_bytes = Vec::new();
    {
        let mut encoder = Encoder::new(&mut png_bytes, width as u32, height as u32);
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(BitDepth::Eight);
        encoder
            .write_header()
            .ok()?
            .write_image_data(&pixels)
            .ok()?;
    }

    ensure_private_dir(&root).ok()?;
    let own = root.join(process::id().to_string());
    ensure_private_dir(&own).ok()?;

    let number = NEXT_NAME.fetch_add(1, Ordering::Relaxed);
    let path = own.join(format!("{number}.png"));
    atomic::write(&path, &png_bytes, Mode::Private).ok()?;

    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(width: f64, height: f64, rowstride: f64, bytes: usize) -> Value {
        let number = Value::Number;

        Value::List(vec![
            number(width),
            number(height),
            number(rowstride),
            Value::Bool(true),
            number(8.0),
            number(4.0),
            Value::List(vec![number(0.0); bytes]),
        ])
    }

    // the sender's sizes must not ask for more memory than the bytes it sent
    #[test]
    fn sizes_beyond_the_data_are_refused() {
        assert!(save(&hint(65536.0, 65536.0, 0.0, 4)).is_none());
        assert!(save(&hint(1e9, 1e9, 4e9, 16)).is_none());
        assert!(save(&hint(2.0, 2.0, 8.0, 15)).is_none());
        assert!(save(&hint(-2.0, 2.0, 8.0, 16)).is_none());
    }

    #[test]
    fn saves_privately_under_runtime_dir_and_rejects_missing_or_symlinked_dirs() {
        let base = env::temp_dir().join(format!("kanade-image-hint-{}", process::id()));
        let _ = fs::remove_dir_all(&base);
        let runtime = base.join("runtime");
        fs::create_dir_all(&runtime).unwrap();

        let valid = hint(2.0, 2.0, 8.0, 16);

        // Missing or relative XDG_RUNTIME_DIR is refused (no /tmp fallback).
        assert!(save_in(&valid, None).is_none());
        assert!(save_in(&valid, Some(OsStr::new("relative/runtime"))).is_none());
        assert!(!env::temp_dir().join(FOLDER).exists());

        // Symlinked runtime directory is refused.
        let link_runtime = base.join("link-runtime");
        std::os::unix::fs::symlink(&runtime, &link_runtime).unwrap();
        assert!(save_in(&valid, Some(link_runtime.as_os_str())).is_none());

        // Valid runtime saves under <runtime>/kanade-notifications/<pid>/<n>.png with 0700/0600.
        let path = save_in(&valid, Some(runtime.as_os_str())).unwrap();
        let root = runtime.join(FOLDER);
        let own = root.join(process::id().to_string());
        assert_eq!(path.parent(), Some(own.as_path()));

        let mode = |p: &Path| fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(&own), 0o700);
        assert_eq!(mode(&path), 0o600);

        // Symlinked root directory is refused.
        let other = base.join("other-runtime");
        fs::create_dir_all(&other).unwrap();
        std::os::unix::fs::symlink(&runtime, other.join(FOLDER)).unwrap();
        assert!(save_in(&valid, Some(other.as_os_str())).is_none());

        // remove_in only removes files in our own pid folder.
        let foreign = root.join("foreign.png");
        fs::write(&foreign, b"keep").unwrap();
        remove_in(&foreign.to_string_lossy(), Some(runtime.as_os_str()));
        assert!(foreign.exists());

        remove_in(&path.to_string_lossy(), Some(runtime.as_os_str()));
        assert!(!path.exists());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn sweep_removes_dead_pid_dirs_and_preserves_live_and_symlinks() {
        let base = env::temp_dir().join(format!("kanade-image-hint-sweep-{}", process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join(FOLDER);
        fs::create_dir_all(&root).unwrap();

        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead_pid = child.id();
        child.wait().unwrap();

        let dead_dir = root.join(dead_pid.to_string());
        let live_dir = root.join(process::id().to_string());
        let non_pid_dir = root.join("not-a-pid");
        let decoy = base.join("decoy");
        fs::create_dir_all(&dead_dir).unwrap();
        fs::write(dead_dir.join("0.png"), b"stale").unwrap();
        fs::create_dir_all(&live_dir).unwrap();
        fs::write(live_dir.join("0.png"), b"live").unwrap();
        fs::create_dir_all(&non_pid_dir).unwrap();
        fs::create_dir_all(&decoy).unwrap();
        fs::write(decoy.join("keep"), b"keep").unwrap();
        let link_dir = root.join("99999999");
        std::os::unix::fs::symlink(&decoy, &link_dir).unwrap();

        sweep_in(&root);

        assert!(!dead_dir.exists(), "dead PID directory should be swept");
        assert!(
            live_dir.join("0.png").exists(),
            "live PID directory must be untouched"
        );
        assert!(
            non_pid_dir.exists(),
            "non-numeric directory must be untouched"
        );
        assert!(link_dir.exists(), "symlink entry must not be removed");
        assert!(
            decoy.join("keep").exists(),
            "symlink target must never be swept"
        );

        let _ = fs::remove_dir_all(&base);
    }
}
