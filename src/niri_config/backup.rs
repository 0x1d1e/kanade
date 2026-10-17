//! The backups of niri's config (ADR 0119): before Kanade changes the file, its bytes are kept in
//! `$XDG_STATE_HOME/kanade/niri-config/` (directory 0700, files 0600) as
//! `<file>.<key>.<unix-ms>.bak`, the newest 20, and the first bytes Kanade ever saw as
//! `<file>.<key>.orig`, which is never rotated away. `<file>.<key>.last` records the last change:
//! the hash of the file it left and the backup it can be undone from.
//!
//! The key is eight hex digits of the SHA-256 of the config's canonical absolute path, so each
//! config has its own ring and its own pinned original even when others share its file name and
//! this directory, and numbering never mixes. A backup of another key is refused by name.

use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::atomic::{self, Mode};

/// The backups kept, besides the pinned original.
pub const KEPT: usize = 20;

pub struct Ring {
    dir: PathBuf,

    // `<file name>.<key>`, what every file of the ring starts with before its own suffix
    name: String,
}

/// One backup, newest first at number 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backup {
    pub number: usize,
    pub file: String,
    pub unix_ms: u64,
    pub bytes: u64,
}

/// What the last change left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Last {
    /// The config it changed.
    pub config: PathBuf,

    /// The SHA-256 of the file as the change left it, in hex.
    pub after: String,

    /// The backup holding what it was before.
    pub before: String,
}

impl Ring {
    pub fn new(state: &Path, config: &Path) -> Ring {
        let file = config.file_name().map_or_else(
            || String::from("config.kdl"),
            |name| name.to_string_lossy().into_owned(),
        );
        let path = fs::canonicalize(config)
            .or_else(|_| std::path::absolute(config))
            .unwrap_or_else(|_| config.to_owned());
        let digest = ring::digest::digest(&ring::digest::SHA256, path.as_os_str().as_bytes());
        let key = digest.as_ref()[..4]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();

        Ring {
            dir: state.to_owned(),
            name: format!("{file}.{key}"),
        }
    }

    pub fn orig(&self) -> PathBuf {
        self.dir.join(format!("{}.orig", self.name))
    }

    fn last_file(&self) -> PathBuf {
        self.dir.join(format!("{}.last", self.name))
    }

    /// The backups, newest first.
    pub fn list(&self) -> Vec<Backup> {
        let prefix = format!("{}.", self.name);
        let mut found: Vec<(u64, String, u64)> = fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let file = entry.file_name().to_string_lossy().into_owned();
                let stamp = file.strip_prefix(&prefix)?.strip_suffix(".bak")?;
                let unix_ms = stamp.parse().ok()?;

                Some((unix_ms, file, entry.metadata().ok()?.len()))
            })
            .collect();

        found.sort_by_key(|(unix_ms, ..)| std::cmp::Reverse(*unix_ms));

        found
            .into_iter()
            .enumerate()
            .map(|(index, (unix_ms, file, bytes))| Backup {
                number: index + 1,
                file,
                unix_ms,
                bytes,
            })
            .collect()
    }

    /// `bytes` kept as the newest backup, at `now_ms` or just after the newest; the pinned
    /// original too if there is none yet; the oldest beyond `KEPT` dropped. Says the backup's name.
    pub fn save(&self, bytes: &[u8], now_ms: u64) -> io::Result<String> {
        if !self.orig().exists() {
            atomic::write(&self.orig(), bytes, Mode::Private)?;
        }

        let newest = self.list().first().map_or(0, |backup| backup.unix_ms);
        let unix_ms = now_ms.max(newest + 1);
        let file = format!("{}.{unix_ms}.bak", self.name);

        atomic::write(&self.dir.join(&file), bytes, Mode::Private)?;

        for old in self.list().into_iter().skip(KEPT) {
            let _ = fs::remove_file(self.dir.join(old.file));
        }

        Ok(file)
    }

    /// Drops a backup just saved, for a change that did not happen.
    pub fn discard(&self, file: &str) {
        let _ = fs::remove_file(self.dir.join(file));
    }

    /// The bytes of backup `number` (1 is the newest), or of the pinned original for `orig`.
    pub fn read(&self, which: &str) -> Result<(String, Vec<u8>), String> {
        if which == "orig" {
            let bytes =
                fs::read(self.orig()).map_err(|error| format!("no original kept: {error}"))?;

            return Ok((format!("{}.orig", self.name), bytes));
        }

        let number: usize = which
            .parse()
            .map_err(|_| format!("{which}: a backup number from `backups`, or orig"))?;
        let backup = self
            .list()
            .into_iter()
            .find(|backup| backup.number == number)
            .ok_or_else(|| format!("no backup {number}; `backups` lists them"))?;

        self.named(&backup.file)
    }

    /// A backup by its file name.
    pub fn named(&self, file: &str) -> Result<(String, Vec<u8>), String> {
        // a name from `.last`, so never a path out of the directory, nor another config's backup
        if file.contains('/') || !file.starts_with(&format!("{}.", self.name)) {
            return Err(format!("{file}: not a backup of this config"));
        }

        fs::read(self.dir.join(file))
            .map(|bytes| (file.to_owned(), bytes))
            .map_err(|error| format!("{file}: {error}"))
    }

    pub fn record(&self, last: &Last) -> io::Result<()> {
        let text = format!(
            "config={}\nafter={}\nbefore={}\n",
            last.config.display(),
            last.after,
            last.before
        );

        atomic::write(&self.last_file(), text.as_bytes(), Mode::Private)
    }

    pub fn last(&self) -> Option<Last> {
        let text = fs::read_to_string(self.last_file()).ok()?;
        let field = |key: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
                .map(str::to_owned)
        };

        Some(Last {
            config: PathBuf::from(field("config")?),
            after: field("after")?,
            before: field("before")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::process;

    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanade-ring-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_ring_keeps_twenty_and_the_first_bytes_for_good() {
        let state = dir("rotate").join("state");
        let ring = Ring::new(&state, Path::new("/x/config.kdl"));

        for n in 0..25u64 {
            // the same millisecond each time: the names still differ and order by when saved
            ring.save(format!("v{n}").as_bytes(), 1000).unwrap();
        }

        let list = ring.list();

        assert_eq!(list.len(), KEPT);
        assert_eq!(ring.read("1").unwrap().1, b"v24");
        assert_eq!(ring.read("20").unwrap().1, b"v5");
        assert!(ring.read("21").is_err());
        assert_eq!(
            ring.read("orig").unwrap().1,
            b"v0",
            "the first bytes, long rotated out"
        );

        let bits = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;

        assert_eq!(bits(&state), 0o700);
        assert_eq!(bits(&state.join(&list[0].file)), 0o600);
        assert_eq!(bits(&ring.orig()), 0o600);

        let _ = fs::remove_dir_all(state.parent().unwrap());
    }

    #[test]
    fn the_last_change_is_recorded_for_its_file() {
        let state = dir("last").join("state");
        let ring = Ring::new(&state, Path::new("/x/config.kdl"));
        let last = Last {
            config: PathBuf::from("/x/config.kdl"),
            after: String::from("ab12"),
            before: String::from("config.kdl.5.bak"),
        };

        assert_eq!(ring.last(), None);

        ring.record(&last).unwrap();

        assert_eq!(ring.last(), Some(last));
        assert!(ring.named("../escape").is_err());

        let _ = fs::remove_dir_all(state.parent().unwrap());
    }
}
