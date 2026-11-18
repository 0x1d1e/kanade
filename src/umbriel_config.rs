//! The Umbriel config editor's engine and CLI: `kanade umbriel-config`
//! reads, sets and removes single settings in Umbriel's `config.toml`, undoes the last change and
//! restores a backup. The file is the user's: it is changed only on request, by mutating TOML
//! preserving comments, formatting, and ordering, and only after validation accepted the result.

pub mod backup;
pub mod edit;
pub mod rules;

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustix::fs::Access;

use crate::atomic::{self, Mode};
use crate::config;
use backup::{Last, Ring};
pub use rules::{RuleInfo, RuleStatus};

/// What an option holds in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    Own(String),
    Unset,
}

/// Default Umbriel configuration template.
pub const DEFAULT_CONFIG: &str = include_str!("../dist/umbriel/config.toml");

/// Installs the default Umbriel config to `dest` if missing.
pub fn create_default(dest: &Path) -> Result<(), String> {
    if dest.starts_with("/etc")
        || dest
            .parent()
            .and_then(|p| fs::canonicalize(p).ok())
            .is_some_and(|p| p.starts_with("/etc"))
    {
        return Err(format!(
            "{} is the system config, which Kanade does not write",
            dest.display()
        ));
    }
    if fs::symlink_metadata(dest).is_ok() {
        return Err(format!("{}: file already exists", dest.display()));
    }
    atomic::write(dest, DEFAULT_CONFIG.as_bytes(), Mode::Shared)
        .map_err(|e| format!("cannot write default config {}: {e}", dest.display()))?;
    Ok(())
}

/// Whether the `umbriel` binary is installed on PATH.
pub fn is_installed() -> bool {
    Command::new("umbriel")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// How long `umbriel config validate` may take.
const VALIDATE: Duration = Duration::from_secs(10);

/// Checks a config file the way Umbriel will read it: why not, in one message.
pub type Validate<'a> = &'a dyn Fn(&Path) -> Result<(), String>;

/// Umbriel's config for this run: `--config` if given, else `$UMBRIEL_CONFIG`,
/// else `$XDG_CONFIG_HOME/umbriel/config.toml`, else `~/.config/umbriel/config.toml`.
pub fn target(explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(file) = explicit {
        return Ok(file.to_owned());
    }

    if let Some(path) = env::var_os("UMBRIEL_CONFIG").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    let home = env::var("HOME").ok();
    config::base("XDG_CONFIG_HOME", ".config", home.as_deref())
        .map(|dir| dir.join("umbriel/config.toml"))
        .ok_or_else(|| String::from("HOME is not set"))
}

/// Where the backups are kept.
pub fn state() -> Result<PathBuf, String> {
    let home = env::var("HOME").ok();

    config::base("XDG_STATE_HOME", ".local/state", home.as_deref())
        .map(|dir| dir.join("kanade/umbriel-config"))
        .ok_or_else(|| String::from("neither XDG_STATE_HOME nor HOME is set"))
}

pub struct Editor<'a> {
    pub file: PathBuf,
    pub state: PathBuf,
    pub validate: Validate<'a>,
    pub now_ms: fn() -> u64,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

struct Snapshot {
    modified: (i64, i64),
    len: u64,
    hash: String,
}

impl Snapshot {
    fn of(file: &Path) -> Result<(Snapshot, Vec<u8>), String> {
        let bytes = fs::read(file).map_err(|error| format!("{}: {error}", file.display()))?;
        let meta = fs::metadata(file).map_err(|error| format!("{}: {error}", file.display()))?;

        Ok((
            Snapshot {
                modified: (meta.mtime(), meta.mtime_nsec()),
                len: meta.len(),
                hash: hash(&bytes),
            },
            bytes,
        ))
    }
}

pub enum Plan {
    Same(String),
    Write(Vec<u8>, String),
}

fn hash(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);

    digest.as_ref().iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

impl Editor<'_> {
    fn ring(&self) -> Ring {
        Ring::new(&self.state, &self.file)
    }

    /// Checks the config as it is, as `kanade umbriel-config validate`.
    pub fn check(&self) -> Result<String, String> {
        (self.validate)(&self.file).map(|()| String::from("valid"))
    }

    /// Retrieves the value at `path`.
    pub fn get(&self, path: &str) -> Result<String, String> {
        let text = fs::read_to_string(&self.file)
            .map_err(|error| format!("{}: {error}", self.file.display()))?;
        edit::get(&text, path)
    }

    /// As `get`, in a form to match on.
    pub fn read(&self, path: &str) -> Reading {
        match self.get(path) {
            Ok(val) => Reading::Own(val),
            Err(_) => Reading::Unset,
        }
    }

    /// Sets `path` to `value`.
    pub fn set(&self, path: &str, value: &str) -> Result<String, String> {
        self.change(true, |bytes| {
            let text = std::str::from_utf8(bytes)
                .map_err(|_| String::from("the config is not UTF-8; Umbriel would not read it"))?;
            let includes = self.load_includes(text)?;
            ensure_not_included(&includes, &self.file, path)?;

            let was = edit::get(text, path).ok();

            match edit::set(text, path, value)? {
                None => Ok(Plan::Same(format!("{path} is {value} already"))),
                Some(edited) => {
                    let said = format!(
                        "{path} = {value} (was {})",
                        was.as_deref().unwrap_or("unset")
                    );
                    Ok(Plan::Write(edited.into_bytes(), said))
                }
            }
        })
    }

    /// Removes `path`.
    pub fn remove(&self, path: &str) -> Result<String, String> {
        self.change(true, |bytes| {
            let text = std::str::from_utf8(bytes)
                .map_err(|_| String::from("the config is not UTF-8; Umbriel would not read it"))?;
            let includes = self.load_includes(text)?;
            ensure_not_included(&includes, &self.file, path)?;

            let was = edit::get(text, path).ok();

            match edit::remove(text, path)? {
                None => Ok(Plan::Same(format!("{path} is not set"))),
                Some(edited) => {
                    let said =
                        format!("{path} removed (was {})", was.as_deref().unwrap_or("unset"));
                    Ok(Plan::Write(edited.into_bytes(), said))
                }
            }
        })
    }

    /// Restores what the last change replaced.
    pub fn undo(&self) -> Result<String, String> {
        self.change(false, |bytes| {
            let ring = self.ring();
            let last = ring
                .last()
                .ok_or("nothing to undo: Kanade has not changed this config")?;

            if !same_file(&last.config, &self.file) {
                return Err(format!(
                    "the last change was to {}, not this config",
                    last.config.display()
                ));
            }
            if hash(bytes) != last.after {
                return Err(String::from(
                    "the config changed since Kanade's last change; undo would lose that. Pick a backup with `backups` and `restore`",
                ));
            }
            let (name, old) = ring.named(&last.before)?;
            Ok(Plan::Write(old, format!("undone: restored {name}")))
        })
    }

    /// Lists backups newest first.
    pub fn backups(&self) -> String {
        let ring = self.ring();
        let mut lines = Vec::new();

        for backup in ring.list() {
            let when =
                chrono::DateTime::<chrono::Utc>::from_timestamp_millis(backup.unix_ms as i64)
                    .map_or_else(
                        || backup.unix_ms.to_string(),
                        |when| when.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
                    );

            lines.push(format!("{}\t{when}\t{} bytes", backup.number, backup.bytes));
        }

        if let Ok((_, bytes)) = ring.read("orig") {
            lines.push(format!(
                "orig\tthe first bytes Kanade saw\t{} bytes",
                bytes.len()
            ));
        }

        if lines.is_empty() {
            return String::from("no backups: Kanade has not changed this config");
        }

        lines.join("\n")
    }

    /// Restores a backup.
    pub fn restore(&self, which: &str) -> Result<String, String> {
        self.change(false, |bytes| {
            let (name, old) = self.ring().read(which)?;
            if bytes == old {
                return Ok(Plan::Same(format!("the config is {name} already")));
            }
            Ok(Plan::Write(old, format!("restored {name}")))
        })
    }

    /// Creates the default configuration if the file does not exist yet.
    pub fn create_default(&self) -> Result<(), String> {
        if fs::symlink_metadata(&self.file).is_ok() {
            return Err(format!("{}: file already exists", self.file.display()));
        }
        create_default(&self.file)
    }

    /// Several settings in one change, one backup and one `undo`: each path set to its value,
    /// or removed when none.
    pub fn apply(&self, changes: &[(&str, Option<&str>)]) -> Result<String, String> {
        self.change(true, |bytes| {
            let mut text = std::str::from_utf8(bytes)
                .map_err(|_| String::from("the config is not UTF-8; Umbriel would not read it"))?
                .to_owned();
            let includes = if changes.is_empty() {
                Vec::new()
            } else {
                self.load_includes(&text)?
            };
            let mut said = Vec::new();
            let mut written = false;

            for &(path, value) in changes {
                ensure_not_included(&includes, &self.file, path)?;
                let was = edit::get(&text, path).ok();
                match value {
                    Some(val) => match edit::set(&text, path, val)? {
                        None => {
                            if changes.len() == 1 {
                                said.push(format!("{path} is {val} already"));
                            }
                        }
                        Some(edited) => {
                            text = edited;
                            written = true;
                            said.push(format!(
                                "{path} = {val} (was {})",
                                was.as_deref().unwrap_or("unset")
                            ));
                        }
                    },
                    None => match edit::remove(&text, path)? {
                        None => {
                            if changes.len() == 1 {
                                said.push(format!("{path} is not set"));
                            }
                        }
                        Some(edited) => {
                            text = edited;
                            written = true;
                            said.push(format!(
                                "{path} removed (was {})",
                                was.as_deref().unwrap_or("unset")
                            ));
                        }
                    },
                }
            }

            let said = if said.is_empty() {
                String::from("nothing changed")
            } else {
                said.join("; ")
            };

            Ok(if written {
                Plan::Write(text.into_bytes(), said)
            } else {
                Plan::Same(said)
            })
        })
    }

    /// Undoes only if the current file hash matches `after`.
    pub fn undo_only(&self, after: Option<&str>) -> Result<String, String> {
        self.change(false, |bytes| {
            let ring = self.ring();
            let last = ring
                .last()
                .ok_or("nothing to undo: Kanade has not changed this config")?;

            if !same_file(&last.config, &self.file) {
                return Err(format!(
                    "the last change was to {}, not this config",
                    last.config.display()
                ));
            }
            if let Some(expected) = after {
                if hash(bytes) != expected || last.after != expected {
                    return Err(String::from(
                        "the config changed since Kanade's last change; undo would lose that. Pick a backup with `backups` and `restore`",
                    ));
                }
            } else if hash(bytes) != last.after {
                return Err(String::from(
                    "the config changed since Kanade's last change; undo would lose that. Pick a backup with `backups` and `restore`",
                ));
            }
            let (name, old) = ring.named(&last.before)?;
            Ok(Plan::Write(old, format!("undone: restored {name}")))
        })
    }

    /// Hash of the file after the last change.
    pub fn last_change_hash(&self) -> Result<String, String> {
        let ring = self.ring();
        let last = ring.last().ok_or("no last change recorded")?;
        Ok(last.after)
    }

    #[allow(dead_code)]
    pub fn editable(&self) -> Result<(), String> {
        self.writable().map(|_| ())
    }

    pub(crate) fn change(
        &self,
        validate_original: bool,
        plan: impl FnOnce(&[u8]) -> Result<Plan, String>,
    ) -> Result<String, String> {
        // Check before locking so `/etc`, links and missing files do not create state lockfiles,
        // then re-check under the lock so target metadata is resolved atomically with the edit.
        self.writable()?;
        let _lock = self.lock()?;
        let meta = self.writable()?;
        let symlink_meta = fs::symlink_metadata(&self.file)
            .map_err(|error| format!("{}: {error}", self.file.display()))?;
        let is_symlink = symlink_meta.file_type().is_symlink();
        let target = if is_symlink {
            self.file
                .canonicalize()
                .map_err(|error| format!("{}: {error}", self.file.display()))?
        } else {
            self.file.clone()
        };
        let (snapshot, old) = Snapshot::of(&target)?;

        if validate_original {
            (self.validate)(&self.file).map_err(|why| {
                format!("Umbriel rejects the config as it is, so nothing was changed: {why}")
            })?;
        }

        let (new, said) = match plan(&old)? {
            Plan::Same(said) => return Ok(said),
            Plan::Write(new, said) => (new, said),
        };

        let staged = atomic::stage(
            &target,
            &new,
            Mode::Exactly(meta.permissions().mode() & 0o7777),
        )
        .map_err(|error| {
            format!(
                "cannot stage the change beside {}: {error}",
                target.display()
            )
        })?;

        let validation_staged = if is_symlink && self.file.parent() != target.parent() {
            let stage_file = if self
                .file
                .parent()
                .is_some_and(|parent| parent.as_os_str().is_empty())
            {
                Path::new(".").join(&self.file)
            } else {
                self.file.clone()
            };
            match atomic::stage(&stage_file, &new, Mode::Private) {
                Ok(val_staged) => Some(val_staged),
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => None,
                Err(error) => {
                    return Err(format!(
                        "cannot stage the change beside {}: {error}",
                        self.file.display()
                    ));
                }
            }
        } else {
            None
        };

        let validate_path = validation_staged
            .as_ref()
            .map_or_else(|| staged.path(), |s| s.path());

        (self.validate)(validate_path).map_err(|why| {
            let part = validate_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let file = self
                .file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();

            format!(
                "Umbriel rejects the change, so nothing was changed: {}",
                why.replace(&part, &file)
            )
        })?;

        drop(validation_staged);

        let ring = self.ring();
        let before = ring
            .stage(&old, (self.now_ms)())
            .map_err(|error| format!("cannot keep a backup, so nothing was changed: {error}"))?;

        // Read again right before committing to detect disk changes
        let (now, _) = match Snapshot::of(&target) {
            Ok(now) => now,
            Err(error) => {
                ring.discard(&before);
                return Err(error);
            }
        };

        if now.hash != snapshot.hash || now.len != snapshot.len || now.modified != snapshot.modified
        {
            ring.discard(&before);
            return Err(String::from(
                "config changed on disk since it was read; nothing was written, try again",
            ));
        }

        if is_symlink && self.file.canonicalize().ok().as_ref() != Some(&target) {
            ring.discard(&before);
            return Err(String::from(
                "config symlink changed on disk since it was read; nothing was written, try again",
            ));
        }

        if !is_symlink {
            match fs::symlink_metadata(&self.file) {
                Ok(self_meta) if !self_meta.file_type().is_symlink() => {}
                Ok(_) => {
                    ring.discard(&before);
                    return Err(String::from(
                        "config changed to a symlink on disk since it was read; nothing was written, try again",
                    ));
                }
                Err(error) => {
                    ring.discard(&before);
                    return Err(format!("{}: {error}", self.file.display()));
                }
            }
        }

        match fs::symlink_metadata(&target) {
            Ok(target_meta)
                if !target_meta.file_type().is_symlink()
                    && target_meta.is_file()
                    && target_meta.nlink() == 1
                    && !target.starts_with("/etc") => {}
            Ok(_) => {
                ring.discard(&before);
                return Err(format!(
                    "{}: target changed to an unsafe file on disk; nothing was written, try again",
                    target.display()
                ));
            }
            Err(error) => {
                ring.discard(&before);
                return Err(format!("{}: {error}", target.display()));
            }
        }

        let after = hash(&new);

        if let Err(error) = staged.commit(&target) {
            ring.discard(&before);
            return Err(format!("cannot replace {}: {error}", target.display()));
        }

        ring.prune();

        let recorded = ring.record(&Last {
            config: target,
            after,
            before,
        });

        Ok(match recorded {
            Ok(()) => format!("{said}; backup 1, `kanade umbriel-config undo` reverts"),
            Err(error) => format!(
                "{said}; the change was written but could not be recorded for undo: {error}"
            ),
        })
    }

    fn lock(&self) -> Result<File, String> {
        let canonical = self
            .file
            .canonicalize()
            .or_else(|_| {
                let absolute = std::path::absolute(&self.file)?;
                let parent = absolute
                    .parent()
                    .ok_or_else(|| std::io::Error::other("no parent directory"))?;
                Ok::<PathBuf, std::io::Error>(
                    parent
                        .canonicalize()?
                        .join(absolute.file_name().unwrap_or_default()),
                )
            })
            .map_err(|error| {
                format!(
                    "{}: cannot resolve config path for its lock: {error}",
                    self.file.display()
                )
            })?;
        let digest = ring::digest::digest(
            &ring::digest::SHA256,
            canonical.as_os_str().as_encoded_bytes(),
        );
        let key = digest.as_ref()[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        fs::create_dir_all(&self.state)
            .map_err(|error| format!("{}: {error}", self.state.display()))?;
        fs::set_permissions(&self.state, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("{}: {error}", self.state.display()))?;
        let path = self.state.join(format!("config.{key}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        file.lock()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(file)
    }

    fn writable(&self) -> Result<fs::Metadata, String> {
        let shown = self.file.display();

        if self.file.starts_with("/etc") {
            return Err(format!(
                "{shown} is the system config, which Kanade does not write. Make your own: \
                 mkdir -p ~/.config/umbriel && cp {shown} ~/.config/umbriel/config.toml"
            ));
        }

        let meta = fs::symlink_metadata(&self.file).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => format!(
                "{shown}: no such file; Umbriel runs on its default config, which Kanade does not create"
            ),
            _ => format!("{shown}: {error}"),
        })?;

        if meta.file_type().is_symlink() {
            if meta.uid() != rustix::process::geteuid().as_raw() {
                return Err(format!("{shown} is not owned by you"));
            }

            let canonical = self.file.canonicalize().map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    format!("{shown}: dangling symbolic link")
                } else if error.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) {
                    format!("{shown}: symbolic link loop")
                } else {
                    format!("{shown}: {error}")
                }
            })?;

            if canonical.starts_with("/etc") {
                return Err(format!(
                    "{shown} points to {}, which is under /etc and not edited by Kanade",
                    canonical.display()
                ));
            }

            let canonical_meta = fs::metadata(&canonical)
                .map_err(|error| format!("{}: {error}", canonical.display()))?;

            if !canonical_meta.is_file() {
                return Err(format!("{} is not a regular file", canonical.display()));
            }

            if canonical_meta.nlink() > 1 {
                return Err(format!(
                    "{} is a hard link (another name for the same file); Kanade edits only a file with one name in this version",
                    canonical.display()
                ));
            }

            if canonical_meta.uid() != rustix::process::geteuid().as_raw() {
                return Err(format!("{} is not owned by you", canonical.display()));
            }

            let dir = canonical
                .parent()
                .filter(|dir| !dir.as_os_str().is_empty())
                .unwrap_or(Path::new("."));

            if rustix::fs::access(&canonical, Access::WRITE_OK).is_err()
                || rustix::fs::access(dir, Access::WRITE_OK).is_err()
            {
                return Err(format!(
                    "{} or its directory is not writable",
                    canonical.display()
                ));
            }

            return Ok(canonical_meta);
        }

        if !meta.is_file() {
            return Err(format!("{shown} is not a regular file"));
        }

        if meta.nlink() > 1 {
            return Err(format!(
                "{shown} is a hard link (another name for the same file); Kanade edits only a file with one name in this version"
            ));
        }

        if meta.uid() != rustix::process::geteuid().as_raw() {
            return Err(format!("{shown} is not owned by you"));
        }

        let dir = self
            .file
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or(Path::new("."));

        if rustix::fs::access(&self.file, Access::WRITE_OK).is_err()
            || rustix::fs::access(dir, Access::WRITE_OK).is_err()
        {
            return Err(format!("{shown} or its directory is not writable"));
        }

        Ok(meta)
    }

    fn load_includes(&self, text: &str) -> Result<Vec<(PathBuf, String)>, String> {
        let mut found = Vec::new();
        self.walk_includes(&self.file, false, &mut Vec::new(), Some(text), &mut found)?;
        Ok(found)
    }

    fn walk_includes(
        &self,
        file: &Path,
        optional: bool,
        walking: &mut Vec<PathBuf>,
        given: Option<&str>,
        found: &mut Vec<(PathBuf, String)>,
    ) -> Result<(), String> {
        let here = fs::canonicalize(file).unwrap_or_else(|_| file.components().collect());

        if walking.contains(&here) {
            return Err(format!("{}: includes itself", file.display()));
        }

        if walking.len() > crate::sources::binds::DEEPEST {
            return Err(format!("{}: includes too deeply", file.display()));
        }

        let text = match given {
            Some(text) => text.to_owned(),
            None => {
                match fs::symlink_metadata(file) {
                    Ok(_) => {}
                    Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(());
                    }
                    Err(error) => return Err(format!("{}: {error}", file.display())),
                }
                match fs::metadata(file) {
                    Ok(meta) if meta.is_file() => {}
                    Ok(_) if optional => return Ok(()),
                    Ok(_) => return Err(format!("{}: is not a regular file", file.display())),
                    Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(());
                    }
                    Err(error) => return Err(format!("{}: {error}", file.display())),
                }
                let text = fs::read_to_string(file)
                    .map_err(|error| format!("{}: {error}", file.display()))?;
                found.push((file.to_owned(), text.clone()));
                text
            }
        };

        let (_, body) = edit::unbom(&text);
        let doc = body
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| format!("{}: cannot parse TOML: {e}", file.display()))?;

        let mut includes = Vec::new();
        if let Some(inc) = doc.get("include") {
            if let Some(files) = inc.get("files").and_then(|f| f.as_array()) {
                for val in files {
                    if let Some(s) = val.as_str() {
                        includes.push((s.to_owned(), false));
                    }
                }
            }
            if let Some(opt) = inc.get("optional")
                && let Some(files) = opt.get("files").and_then(|f| f.as_array())
            {
                for val in files {
                    if let Some(s) = val.as_str() {
                        includes.push((s.to_owned(), true));
                    }
                }
            }
        }

        for (entry, opt) in includes {
            let inc_path = crate::sources::binds::beside(file, &entry);
            walking.push(here.clone());
            let walked = self.walk_includes(&inc_path, opt, walking, None, found);
            walking.pop();
            walked?;
        }

        Ok(())
    }
}

fn ensure_not_included(
    includes: &[(PathBuf, String)],
    self_file: &Path,
    path: &str,
) -> Result<(), String> {
    edit::parse_path(path)?;
    for (owner, inc_text) in includes {
        if edit::get(inc_text, path).is_ok() {
            let sync = if owner.file_name().is_some_and(|name| name == "kanade.toml") {
                " (appearance.sync writes it)"
            } else {
                ""
            };
            return Err(format!(
                "{path}: set by {}{sync}, an include; Kanade edits only {}",
                owner.display(),
                self_file.display()
            ));
        }
    }
    Ok(())
}

const MOST_OUTPUT: u64 = 1 << 20;

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(pipe) = pipe {
            let _ = pipe.take(MOST_OUTPUT).read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// Fallback TOML syntax validator when umbriel CLI is not available.
pub fn fallback_syntax_validate(file: &Path) -> Result<(), String> {
    let content = fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let (_, body) = edit::unbom(&content);
    body.parse::<toml_edit::DocumentMut>()
        .map_err(|e| format!("syntax error: {e}"))?;
    Ok(())
}

/// Validates an Umbriel config file using `umbriel config validate -c <staged>`
/// when available, or fallback syntax validation otherwise.
pub fn validate_umbriel(file: &Path) -> Result<(), String> {
    let mut cmd = Command::new("umbriel");
    cmd.args(["config", "validate", "-c"])
        .arg(file.as_os_str())
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return fallback_syntax_validate(file);
        }
        Err(err) => {
            return Err(format!("cannot run umbriel config validate ({err})"));
        }
    };

    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());

    let until = Instant::now() + VALIDATE;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(String::from(
                    "umbriel config validate did not finish in 10 s",
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("umbriel config validate: {error}"));
            }
        }
    };

    let output = format!(
        "{}\n{}",
        err.join().unwrap_or_default(),
        out.join().unwrap_or_default()
    );

    if status.success() && !output.contains("Error:") && !output.contains("error:") {
        Ok(())
    } else {
        let first_err = output
            .lines()
            .map(str::trim)
            .find(|l| {
                !l.is_empty()
                    && (l.starts_with("warning:")
                        || l.starts_with("error:")
                        || l.contains("invalid")
                        || l.contains("Error")
                        || l.contains("error"))
            })
            .or_else(|| output.lines().map(str::trim).find(|l| !l.is_empty()))
            .unwrap_or("validation failed");
        Err(first_err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::process;

    use super::*;

    fn test_dir(name: &str) -> (PathBuf, PathBuf) {
        let base = env::temp_dir().join(format!("kanade-umbriel-test-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&base);
        let config_dir = base.join("config");
        let state_dir = base.join("state");
        fs::create_dir_all(&config_dir).unwrap();
        fs::create_dir_all(&state_dir).unwrap();
        (config_dir, state_dir)
    }

    #[test]
    fn editor_get_set_remove_undo_flow() {
        let (config_dir, state_dir) = test_dir("flow");
        let config_file = config_dir.join("config.toml");
        let initial = r#"
[layout]
mode = "scrolling"
gap = 8
"#;
        fs::write(&config_file, initial).unwrap();

        let editor = Editor {
            file: config_file.clone(),
            state: state_dir,
            validate: &fallback_syntax_validate,
            now_ms,
        };

        // Validate
        assert!(editor.editable().is_ok());
        assert_eq!(editor.check().unwrap(), "valid");

        // Get
        assert_eq!(editor.get("layout.gap").unwrap(), "8");

        // Set
        let set_res = editor.set("layout.gap", "12").unwrap();
        assert!(set_res.contains("layout.gap = 12"));
        assert_eq!(editor.get("layout.gap").unwrap(), "12");

        // Check backups
        let backups = editor.backups();
        assert!(backups.contains("orig"));
        assert!(backups.contains("1\t"));

        // Remove
        let rem_res = editor.remove("layout.gap").unwrap();
        assert!(rem_res.contains("layout.gap removed"));
        assert!(editor.get("layout.gap").is_err());

        // Undo
        let undo_res = editor.undo().unwrap();
        assert!(undo_res.contains("undone"));
        assert_eq!(editor.get("layout.gap").unwrap(), "12");
    }

    #[test]
    fn editor_apply_batch_and_undo_only() {
        let (config_dir, state_dir) = test_dir("apply_batch");
        let config_file = config_dir.join("config.toml");
        let initial = r#"
[layout]
mode = "scrolling"
gap = 8
"#;
        fs::write(&config_file, initial).unwrap();

        let editor = Editor {
            file: config_file.clone(),
            state: state_dir,
            validate: &fallback_syntax_validate,
            now_ms,
        };

        let changes = [
            ("layout.gap", Some("16")),
            ("appearance.border_width", Some("3")),
        ];
        let res = editor.apply(&changes).unwrap();
        assert!(res.contains("layout.gap = 16"));
        assert!(res.contains("appearance.border_width = 3"));
        assert_eq!(editor.read("layout.gap"), Reading::Own(String::from("16")));
        assert_eq!(
            editor.read("appearance.border_width"),
            Reading::Own(String::from("3"))
        );

        let after_hash = editor.last_change_hash().unwrap();
        let undo_res = editor.undo_only(Some(&after_hash)).unwrap();
        assert!(undo_res.contains("undone"));
        assert_eq!(editor.read("layout.gap"), Reading::Own(String::from("8")));
        assert_eq!(editor.read("appearance.border_width"), Reading::Unset);
    }

    #[test]
    fn editor_create_default_when_missing() {
        let (config_dir, state_dir) = test_dir("create_default");
        let config_file = config_dir.join("config.toml");
        assert!(!config_file.exists());

        let editor = Editor {
            file: config_file.clone(),
            state: state_dir,
            validate: &fallback_syntax_validate,
            now_ms,
        };

        assert!(editor.create_default().is_ok());
        assert!(config_file.exists());
        assert_eq!(
            editor.read("layout.mode"),
            Reading::Own(String::from("\"scrolling\""))
        );

        // Cannot recreate if already present
        assert!(editor.create_default().is_err());
    }

    #[test]
    fn fallback_validation_rejects_malformed_toml() {
        let (config_dir, _) = test_dir("malformed");
        let config_file = config_dir.join("config.toml");
        fs::write(&config_file, "this is not valid toml = [[[").unwrap();
        assert!(fallback_syntax_validate(&config_file).is_err());
    }

    #[test]
    fn create_default_refuses_dangling_symlink_and_etc() {
        use std::os::unix::fs::symlink;

        let (config_dir, state_dir) = test_dir("create_default_symlink");
        let config_file = config_dir.join("config.toml");
        let missing_target = config_dir.join("nowhere.toml");
        symlink(&missing_target, &config_file).unwrap();

        let editor = Editor {
            file: config_file.clone(),
            state: state_dir,
            validate: &fallback_syntax_validate,
            now_ms,
        };

        assert!(editor.create_default().is_err());
        assert!(create_default(&config_file).is_err());
        assert!(!missing_target.exists());
        assert!(create_default(Path::new("/etc/umbriel/config.toml")).is_err());
    }

    #[test]
    fn cross_directory_symlink_validates_beside_symlink_for_relative_includes() {
        use std::cell::RefCell;
        use std::os::unix::fs::symlink;

        let (config_dir, state_dir) = test_dir("cross_dir_symlink");
        let dotfiles = config_dir.parent().unwrap().join("dotfiles");
        fs::create_dir_all(&dotfiles).unwrap();
        let real = dotfiles.join("config.toml");
        let link = config_dir.join("config.toml");
        let extra = config_dir.join("extra.toml");

        fs::write(&extra, "[input]\nmiddle_click_paste = true\n").unwrap();
        fs::write(
            &real,
            "[layout]\ngap = 8\n\n[include]\nfiles = [\"extra.toml\"]\n",
        )
        .unwrap();
        symlink(&real, &link).unwrap();

        let validated_paths = RefCell::new(Vec::new());
        let validate = |file: &Path| -> Result<(), String> {
            validated_paths.borrow_mut().push(file.to_owned());
            let content = fs::read_to_string(file).map_err(|e| e.to_string())?;
            let (_, body) = edit::unbom(&content);
            let doc = body
                .parse::<toml_edit::DocumentMut>()
                .map_err(|e| e.to_string())?;
            let inc = doc
                .get("include")
                .and_then(|i| i.get("files"))
                .and_then(|f| f.as_array())
                .and_then(|a| a.get(0))
                .and_then(|v| v.as_str())
                .unwrap();
            let resolved = file.parent().unwrap().join(inc);
            if !resolved.exists() {
                return Err(format!("missing include beside {}", file.display()));
            }
            if content.contains("reject-me") {
                return Err(format!("reject-me in {}", file.display()));
            }
            Ok(())
        };

        let editor = Editor {
            file: link.clone(),
            state: state_dir,
            validate: &validate,
            now_ms,
        };

        editor.set("layout.gap", "12").unwrap();
        assert_eq!(editor.get("layout.gap").unwrap(), "12");
        assert!(fs::read_to_string(&real).unwrap().contains("gap = 12"));

        let paths = validated_paths.borrow();
        assert_eq!(paths[0], link);
        assert_eq!(paths[1].parent(), link.parent());
        assert_ne!(paths[1], link);
        drop(paths);

        // No stray .part files left beside symlink or target
        let strays_in = |dir: &Path| -> Vec<String> {
            fs::read_dir(dir)
                .unwrap()
                .filter_map(|e| {
                    let name = e.ok()?.file_name().to_string_lossy().into_owned();
                    name.ends_with(".part").then_some(name)
                })
                .collect()
        };
        assert!(strays_in(&config_dir).is_empty());
        assert!(strays_in(&dotfiles).is_empty());

        // Validation failure replaces staged part name with config.toml and cleans up
        let err = editor.set("layout.mode", "\"reject-me\"").unwrap_err();
        assert!(err.contains("config.toml"), "{err}");
        assert!(!err.contains(".part"), "{err}");
        assert!(strays_in(&config_dir).is_empty());
        assert!(strays_in(&dotfiles).is_empty());

        // Read-only symlink parent falls back cleanly to validating beside target
        let ro_validate = |file: &Path| fallback_syntax_validate(file);
        let ro_editor = Editor {
            file: link.clone(),
            state: editor.state.clone(),
            validate: &ro_validate,
            now_ms,
        };
        fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o555)).unwrap();
        let res = ro_editor.set("layout.gap", "16");
        fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o755)).unwrap();
        res.unwrap();
        assert!(fs::read_to_string(&real).unwrap().contains("gap = 16"));
    }

    #[test]
    fn utf8_bom_is_preserved_across_set_remove_and_undo() {
        let (config_dir, state_dir) = test_dir("bom");
        let config_file = config_dir.join("config.toml");
        let initial = "\u{feff}[layout]\ngap = 8\n";
        fs::write(&config_file, initial).unwrap();

        let editor = Editor {
            file: config_file.clone(),
            state: state_dir,
            validate: &fallback_syntax_validate,
            now_ms,
        };

        assert_eq!(editor.get("layout.gap").unwrap(), "8");

        editor.set("layout.gap", "12").unwrap();
        let after_set = fs::read_to_string(&config_file).unwrap();
        assert!(after_set.starts_with('\u{feff}'));
        assert!(after_set.contains("gap = 12"));

        editor.undo().unwrap();
        assert_eq!(fs::read_to_string(&config_file).unwrap(), initial);

        editor.set("appearance.corner_radius", "6").unwrap();
        let after_new = fs::read_to_string(&config_file).unwrap();
        assert!(after_new.starts_with('\u{feff}'));
        assert_eq!(editor.get("appearance.corner_radius").unwrap(), "6");

        editor.remove("appearance.corner_radius").unwrap();
        let after_remove = fs::read_to_string(&config_file).unwrap();
        assert!(after_remove.starts_with('\u{feff}'));
        assert!(editor.get("appearance.corner_radius").is_err());

        editor.restore("orig").unwrap();
        assert_eq!(fs::read_to_string(&config_file).unwrap(), initial);
    }

    #[test]
    fn include_optional_shadows_are_refused_and_new_tables_precede_include() {
        let (config_dir, state_dir) = test_dir("includes_and_order");
        let config_file = config_dir.join("config.toml");
        let kanade_file = config_dir.join("kanade.toml");

        let initial = "\u{feff}[general]\nautostart = [\"kanade\"]\n\n[layout]\ngap = 8\n\n[include.optional]\nfiles = [\"kanade.toml\"]\n";
        fs::write(&config_file, initial).unwrap();
        fs::write(
            &kanade_file,
            "\u{feff}[colors]\nbackground = \"#111111ff\"\naccent_primary = \"#abcdef\"\n",
        )
        .unwrap();

        let editor = Editor {
            file: config_file.clone(),
            state: state_dir,
            validate: &fallback_syntax_validate,
            now_ms,
        };

        // Editing a setting defined in `kanade.toml` is refused via set, remove, and apply
        let err = editor.set("colors.background", "\"#000000\"").unwrap_err();
        assert!(
            err.contains("kanade.toml") && err.contains("appearance.sync"),
            "{err}"
        );
        assert!(editor.remove("colors.accent_primary").is_err());
        assert!(
            editor
                .apply(&[
                    ("layout.gap", Some("16")),
                    ("colors.background", Some("\"#000000\"")),
                ])
                .is_err()
        );
        // Batch failure wrote nothing
        assert_eq!(fs::read_to_string(&config_file).unwrap(), initial);

        // Creating a brand-new top-level table inserts it before `[include.optional]`
        editor.set("window_rules.opacity", "0.95").unwrap();
        let text = fs::read_to_string(&config_file).unwrap();
        assert!(text.starts_with('\u{feff}'));
        let window_rules_pos = text.find("[window_rules]").unwrap();
        let include_pos = text.find("[include.optional]").unwrap();
        assert!(
            window_rules_pos < include_pos,
            "expected [window_rules] before [include.optional]:\n{text}"
        );
        assert_eq!(editor.get("window_rules.opacity").unwrap(), "0.95");
    }
}
