//! Whole-file replacement: a crash leaves the old file or the new one, never part of either.

use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

static WRITES: AtomicU64 = AtomicU64::new(0);

// names tried before giving up on a directory full of stale parts
const ATTEMPTS: u32 = 8;

/// The permissions of the file, and of the directories made for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The umask's.
    Shared,
    /// Only the owner reads it: the file 0600, the directories 0700.
    Private,
    /// These file bits, for a file that keeps the mode it had.
    Exactly(u32),
}

/// `bytes` to `path`, flushed to disk, then renamed over it. The directory is made when missing.
pub fn write(path: &Path, bytes: &[u8], mode: Mode) -> io::Result<()> {
    replace(path, bytes, mode, || WRITES.fetch_add(1, Ordering::Relaxed))
}

// `write`, with the numbers its temporary names take from `sequence`
fn replace(
    path: &Path,
    bytes: &[u8],
    mode: Mode,
    mut sequence: impl FnMut() -> u64,
) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("the path names no file"))?
        .to_string_lossy();
    let dir = path.parent().unwrap_or_else(|| Path::new("."));

    match mode {
        Mode::Private => DirBuilder::new().recursive(true).mode(0o700).create(dir)?,
        Mode::Shared | Mode::Exactly(_) => fs::create_dir_all(dir)?,
    }

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    // never wider than the owner's until the bits are set, so nobody opens it in between
    if let Mode::Private | Mode::Exactly(_) = mode {
        options.mode(0o600);
    }

    // hidden and unique, so it is never read as the file and two writers never share one; a part
    // left by a crash under a reused pid is stepped over, not taken over or deleted
    let mut attempt = 0;
    let (temporary, mut file) = loop {
        let temporary = dir.join(format!(".{name}.{}.{}.part", process::id(), sequence()));

        match options.open(&temporary) {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < ATTEMPTS => {
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    };

    let written = (|| {
        // the umask has trimmed the mode `open` was given, so it is set outright
        if let Mode::Exactly(bits) = mode {
            file.set_permissions(Permissions::from_mode(bits))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();

    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }

    written
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use rustix::fs::Mode as Bits;
    use rustix::process::umask;
    use std::env;

    use super::*;

    #[test]
    fn a_file_is_replaced_whole_and_leaves_nothing_beside_it() {
        let dir = env::temp_dir().join(format!("kanade-atomic-{}", process::id()));
        let path = dir.join("state/file");
        let _ = fs::remove_dir_all(&dir);

        write(&path, b"one", Mode::Private).unwrap();
        write(&path, b"two", Mode::Private).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(fs::read_dir(dir.join("state")).unwrap().count(), 1);

        let bits = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(bits(&path), 0o600);
        assert_eq!(bits(&dir.join("state")), 0o700);

        write(&path, b"three", Mode::Exactly(0o640)).unwrap();
        assert_eq!(bits(&path), 0o640);

        // the directory is the file, so the rename fails and the part is removed
        assert!(write(&dir.join("state"), b"x", Mode::Shared).is_err());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_part_is_stepped_over_and_kept() {
        let dir = env::temp_dir().join(format!("kanade-atomic-stale-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // the names a crashed process under this pid left, which the first writes would take
        let stale: Vec<_> = (0..3)
            .map(|n| dir.join(format!(".file.{}.{n}.part", process::id())))
            .collect();
        for part in &stale {
            fs::write(part, b"stale").unwrap();
        }

        let mut numbers = 0..;
        replace(&dir.join("file"), b"new", Mode::Shared, || {
            numbers.next().unwrap()
        })
        .unwrap();

        assert_eq!(fs::read(dir.join("file")).unwrap(), b"new");
        for part in &stale {
            assert_eq!(fs::read(part).unwrap(), b"stale");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    // the umask is the process's, so the check runs in a child of its own
    #[test]
    fn exactly_keeps_its_bits_under_a_restrictive_umask() {
        const NAME: &str = "atomic::tests::exactly_keeps_its_bits_under_a_restrictive_umask";

        if env::var_os("KANADE_UMASK_CHILD").is_none() {
            let status = std::process::Command::new(env::current_exe().unwrap())
                .args(["--exact", NAME, "--test-threads=1"])
                .env("KANADE_UMASK_CHILD", "1")
                .status()
                .unwrap();

            assert!(status.success());
            return;
        }

        let dir = env::temp_dir().join(format!("kanade-atomic-umask-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("file");
        let bits = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;

        umask(Bits::from_bits_truncate(0o077));
        write(&path, b"x", Mode::Exactly(0o664)).unwrap();

        assert_eq!(bits(&path), 0o664);

        let _ = fs::remove_dir_all(&dir);
    }
}
