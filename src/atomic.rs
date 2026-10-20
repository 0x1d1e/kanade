//! Whole-file replacement: a crash leaves the old file or the new one, never part of either.

use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
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
fn replace(path: &Path, bytes: &[u8], mode: Mode, sequence: impl FnMut() -> u64) -> io::Result<()> {
    stage_with(path, bytes, mode, sequence)?.commit(path)
}

/// A file written and flushed beside the one it will replace, not yet renamed over it, so that it
/// can be checked first (a config another program validates). Dropped without `commit`, it is removed.
pub struct Staged {
    temporary: PathBuf,
    kept: bool,
}

/// The first phase of `write`: `bytes` flushed to a hidden file in `path`'s directory.
pub fn stage(path: &Path, bytes: &[u8], mode: Mode) -> io::Result<Staged> {
    stage_with(path, bytes, mode, || WRITES.fetch_add(1, Ordering::Relaxed))
}

impl Staged {
    /// Where the staged file is, in the directory of the file it will replace.
    pub fn path(&self) -> &Path {
        &self.temporary
    }

    /// The second phase: renamed over `path`. On an error it is removed.
    pub fn commit(mut self, path: &Path) -> io::Result<()> {
        let renamed = fs::rename(&self.temporary, path);

        if renamed.is_ok() {
            self.kept = true;
        }

        renamed
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.kept {
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

/// Removes the `.<name>.<pid>.<n>.part` files beside `path` whose process is gone: what a write
/// killed between staging and renaming leaves. A live process's part is left alone, as is this one's.
pub fn sweep(path: &Path) {
    let Some(name) = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return;
    };
    let Some(dir) = path.parent() else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let ours = process::id();

    for entry in entries.flatten() {
        let part = entry.file_name().to_string_lossy().into_owned();

        // `.{name}.{pid}.{n}.part`, and nothing else
        let Some(rest) = part
            .strip_prefix(&format!(".{name}."))
            .and_then(|rest| rest.strip_suffix(".part"))
        else {
            continue;
        };
        let mut fields = rest.split('.');
        let (Some(pid), Some(number), None) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        let (Ok(pid), Ok(_)) = (pid.parse::<u32>(), number.parse::<u64>()) else {
            continue;
        };

        if pid != ours && !alive(pid) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

// whether a process of that number exists: 0 and the negatives name no process, so a part named
// with one is not this sweep's to remove
fn alive(pid: u32) -> bool {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return true;
    };

    // signal 0: only the lookup, which fails when no such process exists
    rustix::process::test_kill_process(pid).is_ok()
}

fn stage_with(
    path: &Path,
    bytes: &[u8],
    mode: Mode,
    mut sequence: impl FnMut() -> u64,
) -> io::Result<Staged> {
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

    // a part left by a crash whose process is gone is swept here, so one signal does not leave
    // the directory with a hidden file in it for good
    sweep(path);

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

    // from here the guard removes the part on any error
    let staged = Staged {
        temporary,
        kept: false,
    };

    // the umask has trimmed the mode `open` was given, so it is set outright
    if let Mode::Exactly(bits) = mode {
        file.set_permissions(Permissions::from_mode(bits))?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;

    Ok(staged)
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
    fn a_part_of_a_process_that_is_gone_is_swept_and_one_of_a_live_one_is_kept() {
        let dir = env::temp_dir().join(format!("kanade-atomic-sweep-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // a child, reaped, so its number is certainly not a live process's
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();

        let gone = dir.join(format!(".file.{dead}.0.part"));
        let live = dir.join(format!(".file.{}.0.part", process::id()));
        let other = dir.join(".file.0.0.part");
        let mine = dir.join(".another.0.0.part");
        for part in [&gone, &live, &other, &mine] {
            fs::write(part, b"part").unwrap();
        }

        write(&dir.join("file"), b"new", Mode::Shared).unwrap();

        assert!(!gone.exists(), "the dead process's part is swept");
        assert!(live.exists(), "a live process's part is left alone");
        assert!(other.exists(), "a name that is not a pid is left alone");
        assert!(mine.exists(), "another file's part is left alone");
        assert_eq!(fs::read(dir.join("file")).unwrap(), b"new");

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
