//! State backup ring for Umbriel configuration.
//!
//! Before Kanade changes Umbriel's config, its bytes are kept in
//! `$XDG_STATE_HOME/kanade/umbriel-config/` (directory 0700, files 0600) as
//! `<file>.<key>.<unix-ms>.bak`, the newest 20, and the first bytes Kanade ever saw as
//! `<file>.<key>.orig`, which is never rotated away. `<file>.<key>.last` records the last change:
//! the hash of the file it left and the backup it can be undone from.
//!
//! The key is eight hex digits of the SHA-256 of the config's canonical absolute path, so each
//! config has its own ring and its own pinned original even when others share its file name and
//! this directory, and numbering never mixes.

use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::atomic::{self, Mode};

/// The backups kept, besides the pinned original.
pub const KEPT: usize = 20;

pub struct Ring {
    dir: PathBuf,
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
    pub config: PathBuf,
    pub after: String,
    pub before: String,
}

impl Ring {
    pub fn new(state: &Path, config: &Path) -> Ring {
        let path = fs::canonicalize(config)
            .or_else(|_| std::path::absolute(config))
            .unwrap_or_else(|_| config.to_owned());
        let file = path.file_name().map_or_else(
            || String::from("config.toml"),
            |name| name.to_string_lossy().into_owned(),
        );
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

    /// `bytes` kept as a staged backup, at `now_ms` or just after the newest; the pinned
    /// original too if there is none yet. Older backups are not pruned until `prune` is called.
    pub fn stage(&self, bytes: &[u8], now_ms: u64) -> io::Result<String> {
        if !self.orig().exists() {
            atomic::write(&self.orig(), bytes, Mode::Private)?;
        }

        let newest = self.list().first().map_or(0, |backup| backup.unix_ms);
        let unix_ms = now_ms.max(newest + 1);
        let file = format!("{}.{unix_ms}.bak", self.name);

        atomic::write(&self.dir.join(&file), bytes, Mode::Private)?;
        Ok(file)
    }

    /// Drops backups beyond `KEPT`.
    pub fn prune(&self) {
        for old in self.list().into_iter().skip(KEPT) {
            let _ = fs::remove_file(self.dir.join(old.file));
        }
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
    use std::process;

    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kanade-umbriel-ring-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn ring_keeps_twenty_and_pinned_original() {
        let state = dir("rotate").join("state");
        let ring = Ring::new(&state, Path::new("/test/config.toml"));

        for n in 0..25u64 {
            ring.stage(format!("v{n}").as_bytes(), 1000).unwrap();
            ring.prune();
        }

        let list = ring.list();
        assert_eq!(list.len(), KEPT);
        assert_eq!(list[0].number, 1);

        // Original is preserved
        let (orig_name, orig_bytes) = ring.read("orig").unwrap();
        assert!(orig_name.ends_with(".orig"));
        assert_eq!(orig_bytes, b"v0");

        // Newest backup is v24
        let (_, newest_bytes) = ring.read("1").unwrap();
        assert_eq!(newest_bytes, b"v24");
    }

    #[test]
    fn record_and_read_last() {
        let state = dir("last").join("state");
        let config = Path::new("/test/config.toml");
        let ring = Ring::new(&state, config);

        let last = Last {
            config: config.to_path_buf(),
            after: "abcdef123456".to_string(),
            before: "config.toml.key.1000.bak".to_string(),
        };

        ring.record(&last).unwrap();
        let read = ring.last().unwrap();
        assert_eq!(read, last);
    }
}
