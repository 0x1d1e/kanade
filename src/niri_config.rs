//! The niri config editor's engine and CLI (ADR 0119, docs/archived/design.md Config): `kanade niri-config`
//! reads, sets and removes single settings in niri's `config.kdl`, undoes the last change and
//! restores a backup. The file is the user's: it is changed only on request, by splicing text into
//! its own bytes (`edit`), never rewritten from a tree, and only after `niri validate` accepted
//! the result.
//!
//! The flow of a change, which any step refuses by leaving the file as it is:
//! 1. the file must be a regular file of the user's or a symlink to one (with symlink and target
//!    owned by the user, target having one link and not under /etc, and both target and its parent
//!    writable); a hard link is refused;
//! 2. its bytes are read and a snapshot taken (modification time, length, SHA-256);
//! 3. `niri validate` runs on the file as it is: a config niri rejects, a v2 syntax or a missing
//!    include included, is not edited;
//! 4. the new text is computed from the bytes (a node an include owns, or one under `/-`, is
//!    refused), and staged in a hidden file beside the target, with its mode, flushed;
//! 5. `niri validate` runs on the staged file, beside the target so its relative includes
//!    resolve; on an error the staged file is deleted and the error shown;
//! 6. the old bytes are kept in the backup ring (`backup`);
//! 7. the target is read and hashed again (and any symlink checked that it was not repointed);
//!    if it differs from the snapshot the change is aborted as 'changed on disk';
//! 8. the staged file is renamed over the target and the change recorded for `undo`.
//!
//! Step 7 leaves the time between the check and the rename, microseconds, in which an editor's
//! save would be lost; no lock is shared with other programs to close it.
//!
//! Undo and restore skip step 3, as a broken config is when they are needed, and need the
//! restored bytes to pass step 5.

mod backup;
mod edit;

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
use crate::sources::binds;
use backup::{Last, Ring};
use edit::{Block, Hit, OutputBlock};
pub use edit::{Op, Value};

/// How long `niri validate` may take; it takes about 10 ms.
const VALIDATE: Duration = Duration::from_secs(10);

/// Checks a config file the way niri will read it: why not, in one message.
pub type Validate<'a> = &'a dyn Fn(&Path) -> Result<(), String>;

/// niri's config for this run: `--config` if given, else as niri looks for it (`binds::config`).
pub fn target(explicit: Option<&Path>) -> Result<PathBuf, String> {
    match explicit {
        Some(file) => Ok(file.to_owned()),
        None => binds::config().ok_or_else(|| String::from("HOME is not set")),
    }
}

/// Where the backups are kept.
pub fn state() -> Result<PathBuf, String> {
    let home = env::var("HOME").ok();

    config::base("XDG_STATE_HOME", ".local/state", home.as_deref())
        .map(|dir| dir.join("kanade/niri-config"))
        .ok_or_else(|| String::from("neither XDG_STATE_HOME nor HOME is set"))
}

pub struct Editor<'a> {
    pub file: PathBuf,
    pub state: PathBuf,
    pub validate: Validate<'a>,

    /// The time backups are named by, in milliseconds since the epoch.
    pub now_ms: fn() -> u64,
}

/// What a path holds in the config and the files it includes.
#[derive(Debug, Clone, PartialEq)]
pub enum Reading {
    /// No file sets it: niri's default applies (a flag is off; but a bare `layout.border` block in
    /// the config itself turns the border on, see `Editor::sets_in_config`).
    Unset,

    /// The config itself sets it, to this (`true` for a flag, `{ ... }` for a block).
    Own(String),

    /// An included file sets it, after or instead of the config: Kanade does not edit that.
    Included(String, PathBuf),

    /// The config's own node is the value niri uses, but an included file sets the path too,
    /// before it; a change is refused as for an include, since the include would still set it.
    Overridden(String, PathBuf),
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

// the file as it stood when read
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

/*
 * `output["name"].key`, as the command line spells it, with what the block's name may hold made
 * safe for a terminal first: a name comes from the config or from a monitor's EDID, so a control
 * character in it would otherwise reach the terminal as a sequence, a newline included
 */
fn output_label(name: &str, key: &str) -> String {
    let name = plain(name)
        .replace(|c: char| c.is_control(), "")
        .replace('\\', "\\\\")
        .replace('"', "\\\"");

    if key.is_empty() {
        format!("output[\"{name}\"]")
    } else {
        format!("output[\"{name}\"].{key}")
    }
}

// the paths niri reads only when it starts, so a change to one shows after a restart and not before
pub const AT_RESTART: [&str; 3] = ["spawn-at-startup", "spawn-sh-at-startup", "environment"];

// what to say of a change to one of those, if it is one
fn at_restart(ops: &[(&str, Op)]) -> Option<String> {
    let first = ops.iter().find_map(|(path, _)| {
        let node = path.split(['.', '[']).next()?;

        AT_RESTART.contains(&node).then_some(node)
    })?;

    Some(format!(
        "niri reads {first} when it starts, so this applies after a restart"
    ))
}

// what to say of blocks past the first that match the monitor
fn shadowed(blocks: &[(PathBuf, OutputBlock)]) -> Option<String> {
    let later = blocks.len().checked_sub(1).filter(|&later| later > 0)?;

    Some(format!(
        "{later} later output block{} match{} the same monitor and {} shadowed: niri uses \
         the first one whole",
        if later == 1 { "" } else { "s" },
        if later == 1 { "es" } else { "" },
        if later == 1 { "is" } else { "are" },
    ))
}

// why a change to a key of a block is not made, when what is there is not Kanade's to change: a
// mode with `custom=true` or a modeline is set by hand, and a mode beside a modeline is not
// the one niri uses
fn held_back(
    text: &str,
    ordinal: usize,
    key: &str,
    op: &Op,
    was: Option<&str>,
) -> Result<(), String> {
    if key == "modeline" || key.starts_with("modeline.") {
        return Err(String::from(
            "a modeline is set in the file, which Kanade does not edit",
        ));
    }

    if key == "mode" && was.is_some_and(|shown| shown.contains("custom=")) {
        return Err(String::from(
            "a custom mode is set in the file, which Kanade does not edit",
        ));
    }

    if key == "mode"
        && matches!(op, Op::Set(_))
        && !edit::hits_in(text, ordinal, &["modeline"])?.is_empty()
    {
        return Err(String::from(
            "a modeline is set in the block, which Kanade does not edit; a mode would not be the one niri uses",
        ));
    }

    Ok(())
}

fn hash(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);

    digest.as_ref().iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

// what a scan of one file's text finds, with the byte offset of each
type Scan<'a, T> = &'a dyn Fn(&str) -> Result<Vec<(usize, T)>, String>;

// a setting or an include, where a file has it
type Event<T> = Result<T, (PathBuf, bool)>;

/// A monitor as niri names it: its connector, and what its EDID gives (`None` for what it does
/// not). The block niri uses for it is the first whose name `matches`.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputId {
    pub connector: String,
    pub make: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
}

impl OutputId {
    /// A monitor known only by the name given for its block (the command line's way): it matches
    /// the blocks of that name, whatever the case.
    pub fn named(name: &str) -> OutputId {
        OutputId {
            connector: name.to_owned(),
            make: None,
            model: None,
            serial: None,
        }
    }

    /// Whether an `output "target"` block is this monitor's: niri's `OutputName::matches`
    /// (niri-config/src/output.rs) with its comparisons by ASCII case.
    pub fn matches(&self, target: &str) -> bool {
        if target.eq_ignore_ascii_case(&self.connector) {
            return true;
        }

        // with nothing else known, only the connector can match
        if self.make.is_none() && self.model.is_none() && self.serial.is_none() {
            return false;
        }

        // "make model serial", `Unknown` for what is missing
        let part = |part: &Option<String>| part.clone().unwrap_or_else(|| String::from("Unknown"));
        let (make, model, serial) = (part(&self.make), part(&self.model), part(&self.serial));
        let Some(rest) = after(target, &make) else {
            return false;
        };
        let Some(rest) = after(rest, &model) else {
            return false;
        };

        rest.eq_ignore_ascii_case(&serial)
    }

    /// The name a new block gets: make, model and serial when all three are known (the monitor
    /// then keeps its place when it moves to another port), else the connector. A built-in panel
    /// (`eDP-*`) is named by its connector.
    pub fn block_name(&self) -> String {
        match (&self.make, &self.model, &self.serial) {
            (Some(make), Some(model), Some(serial))
                if !self.connector.to_ascii_lowercase().starts_with("edp-") =>
            {
                format!("{make} {model} {serial}")
            }
            _ => self.connector.clone(),
        }
    }
}

// what follows `word` and a space at the start of `text`, `word` matched by ASCII case
fn after<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let head = text.get(..word.len())?;

    if !head.eq_ignore_ascii_case(word) {
        return None;
    }

    text[word.len()..].strip_prefix(' ')
}

/// A path of the form `output["name"]` or `output["name"].key`: the monitor it names, by the name
/// of its block, and the dotted key under it (empty for the block). The name is in double quotes,
/// with `\"` and `\\` for those two characters; `None` for any other path.
pub fn output_form(path: &str) -> Option<Result<(OutputId, String), String>> {
    let rest = path.strip_prefix("output[")?;
    let mut name = String::new();
    let bad = || {
        Some(Err(format!(
            "{path}: write it like output[\"DP-1\"].scale, the name in double quotes"
        )))
    };
    let Some(quoted) = rest.strip_prefix('"') else {
        return bad();
    };
    let mut chars = quoted.chars();

    loop {
        match chars.next() {
            Some('"') => break,
            Some('\\') => match chars.next() {
                Some(escaped @ ('"' | '\\')) => name.push(escaped),
                _ => return bad(),
            },
            Some(char) => name.push(char),
            None => return bad(),
        }
    }

    let rest = chars.as_str();
    let key = match rest.strip_prefix(']') {
        Some("") => "",
        Some(key) => match key.strip_prefix('.') {
            Some(key) if !key.is_empty() => key,
            _ => return bad(),
        },
        None => return bad(),
    };

    if name.is_empty() {
        return bad();
    }

    if let Some(char) = name.chars().find(|&char| edit::bidi(char)) {
        return Some(Err(format!(
            "{path}: the name holds a bidirectional control character (U+{:04X}), which could make \
             it read as another monitor's name",
            char as u32
        )));
    }

    Some(Ok((OutputId::named(&name), key.to_owned())))
}

/// The keys of an `output` block that Kanade sets, which a reset removes (and the block with them
/// when nothing else is in it: `focus-at-startup`, `hot-corners`, a comment keep it).
pub const OUTPUT_KEYS: &[&str] = &[
    "off",
    "scale",
    "transform",
    "position",
    "mode",
    "variable-refresh-rate",
];

// a value typed for a key of an output block: the nodes with properties take `x=1 y=2`, and
// `transform` is always text (`90` is `"90"`), as niri reads it
fn output_value(key: &str, text: &str) -> Result<Value, String> {
    let last = key.rsplit('.').next().unwrap_or(key);

    match last {
        "position" | "variable-refresh-rate" => Value::parse_props(text),
        "transform" => Ok(match Value::parse(text)? {
            Value::Int(number) => Value::Str(number.to_string()),
            value => value,
        }),
        _ => Value::parse(text),
    }
}

/// What an option of a monitor's block holds, and what to know of the blocks beside it.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputRead {
    /// As for a path: `Own` in the config's block, `Included` in an include's, `Overridden` when
    /// the config's block is the one niri uses but an include has a block for the monitor too.
    pub reading: Reading,

    /// Set when more than one block matches: niri uses the first, whole, and drops the rest.
    pub note: Option<String>,

    /// The include file that owns a matching block for the monitor, if any.
    pub block_owner: Option<PathBuf>,
}

// what a change comes to
enum Plan {
    // the file is as asked already
    Same(String),

    // the new bytes, and what to say of them
    Write(Vec<u8>, String),
}

// one file under two spellings (relative, absolute, through a symlinked directory) is the same file
fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}

impl Editor<'_> {
    fn ring(&self) -> Ring {
        Ring::new(&self.state, &self.file)
    }

    /// `niri validate` on the file, as `kanade niri-config validate`.
    pub fn check(&self) -> Result<String, String> {
        (self.validate)(&self.file)?;

        Ok(String::from("valid"))
    }

    /// The value at `path` as niri will have it (the last of the file and its includes), with the
    /// file that sets it when that is not the config itself.
    pub fn get(&self, path: &str) -> Result<String, String> {
        if let Some(output) = output_form(path) {
            let (id, key) = output?;

            return self.get_output(&id, &key);
        }

        if path.contains("[new]") {
            return Err(format!(
                "{path}: [new] is only for appending a new node with set"
            ));
        }

        if edit::is_repeated_bare(path)? {
            return self.get_repeated(path);
        }

        match self.read(path)? {
            Reading::Own(shown) | Reading::Overridden(shown, _) => Ok(shown),
            Reading::Included(shown, file) => Ok(format!("{shown}  # from {}", file.display())),
            Reading::Unset if edit::is_flag(path.rsplit('.').next().unwrap_or(path)) => {
                Ok(String::from("false"))
            }
            Reading::Unset => Err(format!("{path}: not set (niri's default applies)")),
        }
    }

    /// As `get`, in a form to match on: where the value stands, if the path is set at all.
    pub fn read(&self, path: &str) -> Result<Reading, String> {
        if path.contains("[new]") {
            return Err(format!(
                "{path}: [new] is only for appending a new node with set"
            ));
        }

        if edit::is_repeated_bare(path)? {
            let text = fs::read_to_string(&self.file)
                .map_err(|error| format!("{}: {error}", self.file.display()))?;
            let found = self.gather(&text, &[path])?;

            if found.is_empty() {
                return Ok(Reading::Unset);
            }

            let shown = self.get_repeated(path)?;
            let owner = found.iter().find(|(file, _)| *file != self.file);

            return Ok(match owner {
                Some((file, _)) if found.iter().all(|(f, _)| f != &self.file) => {
                    Reading::Included(shown, file.clone())
                }
                Some((file, _)) => Reading::Overridden(shown, file.clone()),
                None => Reading::Own(shown),
            });
        }

        let segments = edit::segments(path)?;
        let text = fs::read_to_string(&self.file)
            .map_err(|error| format!("{}: {error}", self.file.display()))?;
        let found = self.gather(&text, &segments)?;

        // the first include is the one a change is refused for (`edit`)
        let owner = found.iter().find(|(file, _)| *file != self.file);

        Ok(match (found.last(), owner) {
            (None, _) => Reading::Unset,
            (Some((file, hit)), _) if *file != self.file => {
                Reading::Included(hit.shown.clone(), file.clone())
            }
            (Some((_, hit)), Some((file, _))) => {
                Reading::Overridden(hit.shown.clone(), file.clone())
            }
            (Some((_, hit)), None) => Reading::Own(hit.shown.clone()),
        })
    }

    fn get_repeated(&self, kind: &str) -> Result<String, String> {
        let segments = [kind];
        let text = fs::read_to_string(&self.file)
            .map_err(|error| format!("{}: {error}", self.file.display()))?;
        let found = self.gather(&text, &segments)?;

        if found.is_empty() {
            return Err(format!("{kind}: not set (niri's default applies)"));
        }

        let lines: Vec<String> = found
            .iter()
            .enumerate()
            .map(|(i, (file, hit))| {
                if *file != self.file {
                    format!("[{i}] {}  # from {}", hit.shown, file.display())
                } else {
                    format!("[{i}] {}", hit.shown)
                }
            })
            .collect();

        Ok(lines.join("\n"))
    }

    /// Every file the config includes, in the order niri reads them, with the config itself left
    /// out. A file is listed even when it sets none of the options read from it: it may set one
    /// later, and a watcher needs it now.
    pub fn includes(&self) -> Result<Vec<PathBuf>, String> {
        let mut found = Vec::new();

        self.walk(
            &self.file,
            false,
            &mut Vec::new(),
            None,
            &|_| Ok(vec![(0usize, ())]),
            &mut found,
        )?;

        let mut files: Vec<PathBuf> = Vec::new();

        for (file, ()) in found {
            if file != self.file && !files.contains(&file) {
                files.push(file);
            }
        }

        Ok(files)
    }

    /// Whether the config file itself has a node at `path` (a block counts), whatever the
    /// includes hold. niri reads some blocks differently there than in an include.
    pub fn sets_in_config(&self, path: &str) -> Result<bool, String> {
        let segments = edit::segments(path)?;
        let text = fs::read_to_string(&self.file)
            .map_err(|error| format!("{}: {error}", self.file.display()))?;

        Ok(self
            .gather(&text, &segments)?
            .iter()
            .any(|(file, _)| *file == self.file))
    }

    // the blocks of the config and its includes that are the monitor's, in the order niri reads
    // them: it uses the first, whole
    fn matching(&self, text: &str, id: &OutputId) -> Result<Vec<(PathBuf, OutputBlock)>, String> {
        let mut blocks = self.blocks(text)?;

        blocks.retain(|(_, block)| id.matches(&block.name));
        Ok(blocks)
    }

    // what `file` holds at `below` in its block `ordinal`; the config's own text is `text`
    fn block_hits(
        &self,
        text: &str,
        file: &Path,
        ordinal: usize,
        below: &[&str],
    ) -> Result<Vec<Hit>, String> {
        let hits = if file == self.file {
            edit::hits_in(text, ordinal, below)
        } else {
            let text =
                fs::read_to_string(file).map_err(|error| format!("{}: {error}", file.display()))?;

            edit::hits_in(&text, ordinal, below)
        };

        hits.map_err(|why| format!("{}: {why}", file.display()))
    }

    /// A key of the block niri uses for the monitor (`scale`, `layout.gaps`): where it stands,
    /// and a note when other blocks match the monitor too.
    pub fn read_output(&self, id: &OutputId, key: &str) -> Result<OutputRead, String> {
        let below = edit::segments(key)?;
        let text = fs::read_to_string(&self.file)
            .map_err(|error| format!("{}: {error}", self.file.display()))?;
        let blocks = self.matching(&text, id)?;
        let Some((file, block)) = blocks.first() else {
            return Ok(OutputRead {
                reading: Reading::Unset,
                note: None,
                block_owner: None,
            });
        };
        let note = shadowed(&blocks);
        let hit = self.block_hits(&text, file, block.ordinal, &below)?.pop();
        let included = blocks.iter().find(|(other, _)| *other != self.file);

        Ok(OutputRead {
            reading: match (hit, included) {
                (None, _) => Reading::Unset,
                (Some(hit), _) if *file != self.file => Reading::Included(hit.shown, file.clone()),
                (Some(hit), Some((owner, _))) => Reading::Overridden(hit.shown, owner.clone()),
                (Some(hit), None) => Reading::Own(hit.shown),
            },
            note,
            block_owner: included.map(|(path, _)| path.clone()),
        })
    }

    /// As `get`, for a key of the monitor's block; with no key, the block as the file has it.
    pub fn get_output(&self, id: &OutputId, key: &str) -> Result<String, String> {
        let name = id.block_name();

        if key.is_empty() {
            let text = fs::read_to_string(&self.file)
                .map_err(|error| format!("{}: {error}", self.file.display()))?;
            let blocks = self.matching(&text, id)?;
            let Some((file, block)) = blocks.first() else {
                return Err(format!(
                    "{}: no block for it (niri's defaults apply)",
                    output_label(&name, "")
                ));
            };
            let shown = if *file == self.file {
                edit::block_text(&text, block.ordinal)
            } else {
                fs::read_to_string(file)
                    .map_err(|error| format!("{}: {error}", file.display()))
                    .and_then(|text| edit::block_text(&text, block.ordinal))
            }?;
            let from = if *file == self.file {
                String::new()
            } else {
                format!("\n# from {}", file.display())
            };
            let note = shadowed(&blocks).map_or_else(String::new, |note| format!("\n# {note}"));

            return Ok(format!("{shown}{from}{note}"));
        }

        let read = self.read_output(id, key)?;
        let note = read
            .note
            .map_or_else(String::new, |note| format!("  # {note}"));
        let path = output_label(&name, key);

        match read.reading {
            Reading::Own(shown) | Reading::Overridden(shown, _) => Ok(format!("{shown}{note}")),
            Reading::Included(shown, file) => {
                Ok(format!("{shown}  # from {}{note}", file.display()))
            }
            Reading::Unset if edit::is_flag(key.rsplit('.').next().unwrap_or(key)) => {
                Ok(format!("false{note}"))
            }
            Reading::Unset => Err(format!("{path}: not set (niri's default applies)")),
        }
    }

    /// Checks whether an output block can be edited, returning an error if an include owns a block
    /// for the monitor.
    pub fn output_editable(&self, id: &OutputId) -> Result<(), String> {
        let text = fs::read_to_string(&self.file)
            .map_err(|error| format!("{}: {error}", self.file.display()))?;
        let blocks = self.matching(&text, id)?;
        let name = blocks
            .first()
            .map_or_else(|| id.block_name(), |(_, block)| block.name.clone());
        let path = output_label(&name, "");

        if let Some((owner, _)) = blocks.iter().find(|(file, _)| *file != self.file) {
            let sync = if owner.file_name().is_some_and(|name| name == "kanade.kdl") {
                " (appearance.sync writes it)"
            } else {
                ""
            };

            return Err(format!(
                "{path}: set by {}{sync}, an include; Kanade edits only {}",
                owner.display(),
                self.file.display()
            ));
        }

        Ok(())
    }

    /// A key of the monitor's block set to `value`; the block is made at the end of the file when
    /// none matches the monitor.
    pub fn set_output(&self, id: &OutputId, key: &str, value: Value) -> Result<String, String> {
        self.edit_output(id, &[(key, Op::Set(value))])
    }

    /// A key of the monitor's block removed, so niri's default applies.
    pub fn remove_output(&self, id: &OutputId, key: &str) -> Result<String, String> {
        self.edit_output(id, &[(key, Op::Remove)])
    }

    /// Keys of the monitor's block removed, and the block too if nothing else is in it.
    pub fn reset_output(&self, id: &OutputId, keys: &[&str]) -> Result<String, String> {
        let mut ops: Vec<(&str, Op)> = keys.iter().map(|&key| (key, Op::Remove)).collect();

        ops.push(("", Op::DropEmpty));
        self.edit_output(id, &ops)
    }

    /// Several changes to the monitor's block in one change, one backup and one `undo`, nothing
    /// written unless every one can be. The block is the first that matches the monitor, in the
    /// order niri reads the config; it is refused when an include has a block for the monitor
    /// too, since the include could not be changed and would keep what it sets; `Op::DropEmpty`
    /// (key empty) removes the block when nothing is left in it.
    pub fn edit_output(&self, id: &OutputId, ops: &[(&str, Op)]) -> Result<String, String> {
        self.change(true, |bytes| {
            let mut text = std::str::from_utf8(bytes)
                .map_err(|_| String::from("the config is not UTF-8; niri would not read it"))?
                .to_owned();
            let mut said = Vec::new();
            let mut written = false;
            let mut created = false;

            for (key, op) in ops {
                let below = if key.is_empty() {
                    Vec::new()
                } else {
                    edit::segments(key)?
                };
                let blocks = self.matching(&text, id)?;
                let name = blocks
                    .first()
                    .map_or_else(|| id.block_name(), |(_, block)| block.name.clone());
                let path = output_label(&name, key);

                // an include has a block for it: the edit would not be the value niri uses, or
                // would be undone
                if let Some((owner, _)) = blocks.iter().find(|(file, _)| *file != self.file) {
                    let sync = if owner.file_name().is_some_and(|name| name == "kanade.kdl") {
                        " (appearance.sync writes it)"
                    } else {
                        ""
                    };

                    return Err(format!(
                        "{path}: set by {}{sync}, an include; Kanade edits only {}",
                        owner.display(),
                        self.file.display()
                    ));
                }

                let block = match blocks.first() {
                    Some((_, block)) => Block::Nth(block.ordinal),
                    None if matches!(op, Op::DropEmpty) => {
                        said.push(format!("{path} is not in the file"));
                        continue;
                    }
                    None => {
                        created = true;
                        Block::New(name.clone())
                    }
                };

                let was = match &block {
                    Block::Nth(ordinal) if !below.is_empty() => {
                        edit::hits_in(&text, *ordinal, &below)?
                            .pop()
                            .map(|hit| hit.shown)
                    }
                    _ => None,
                };

                if let Block::Nth(ordinal) = &block {
                    held_back(&text, *ordinal, key, op, was.as_deref())
                        .map_err(|why| format!("{path}: {why}"))?;
                }

                let edited = edit::apply_in(&text, &block, &below, op)?;

                // the emptied block goes and niri would use the next one that matches, which
                // may hold anything
                if matches!(op, Op::DropEmpty)
                    && edited.is_some()
                    && let Some((_, next)) = blocks.get(1)
                {
                    return Err(format!(
                        "{path}: another block, {}, is for the same monitor and comes after it; \
                         removing this one would make niri use that one. Remove the duplicate first",
                        output_label(&next.name, "")
                    ));
                }

                match edited {
                    // in a batch, a key that was not there is not worth a line
                    None if ops.len() > 1 && !matches!(op, Op::DropEmpty) => {}
                    None => said.push(match op {
                        Op::Set(value) => format!("{path} is {} already", value.shown()),
                        Op::Remove => format!("{path} is not set"),
                        Op::DropEmpty => format!("{path} stays: settings of its own are in it"),
                    }),
                    Some(edited) => {
                        text = edited;
                        written = true;
                        said.push(match op {
                            Op::Set(value) => format!(
                                "{path} = {} (was {})",
                                value.shown(),
                                was.as_deref().unwrap_or("unset")
                            ),
                            Op::Remove => format!(
                                "{path} removed (was {})",
                                was.as_deref().unwrap_or("unset")
                            ),
                            Op::DropEmpty => format!("{path} removed, nothing was left in it"),
                        });
                    }
                }
            }

            // what is true of the file after the changes, not before
            if let Some(note) = shadowed(&self.matching(&text, id)?) {
                said.push(note);
            }

            // a name typed is all the command line knows: a block of the same monitor under
            // another of its names (make model serial) is not seen, and would come first
            if created && id.make.is_none() {
                let others: Vec<String> = edit::outputs(&text)?
                    .into_iter()
                    .map(|block| block.name)
                    .filter(|name| name.contains(' ') && !id.matches(name))
                    .collect();

                if !others.is_empty() {
                    said.push(format!(
                        "other blocks ({}) are matched by make, model and serial, not by this name; \
                         if one is this monitor's it comes first and this one is shadowed",
                        others.join(", ")
                    ));
                }
            }

            if said.is_empty() {
                said.push(format!(
                    "{}: nothing to change",
                    output_label(&id.block_name(), "")
                ));
            }

            // what niri reads only when it starts, which a written change does not reach
            if written && let Some(note) = at_restart(ops) {
                said.push(note);
            }

            let said = said.join("; ");

            Ok(if written {
                Plan::Write(text.into_bytes(), said)
            } else {
                Plan::Same(said)
            })
        })
    }

    // the nodes at `segments` in the config and the files it includes, in the order niri reads
    // them, each with the file it is in
    fn gather(&self, text: &str, segments: &[&str]) -> Result<Vec<(PathBuf, Hit)>, String> {
        let mut found = Vec::new();

        self.walk(
            &self.file,
            false,
            &mut Vec::new(),
            Some(text),
            &|text| {
                Ok(edit::hits(text, segments)?
                    .into_iter()
                    .map(|hit| (hit.offset, hit))
                    .collect())
            },
            &mut found,
        )?;

        Ok(found)
    }

    // the same for the top-level `output` blocks
    fn blocks(&self, text: &str) -> Result<Vec<(PathBuf, OutputBlock)>, String> {
        let mut found = Vec::new();

        self.walk(
            &self.file,
            false,
            &mut Vec::new(),
            Some(text),
            &|text| {
                Ok(edit::outputs(text)?
                    .into_iter()
                    .map(|block| (block.offset, block))
                    .collect())
            },
            &mut found,
        )?;

        Ok(found)
    }

    // what `scan` finds in the file, with the files it includes where they are included
    fn walk<T>(
        &self,
        file: &Path,
        optional: bool,
        walking: &mut Vec<PathBuf>,
        given: Option<&str>,
        scan: Scan<T>,
        found: &mut Vec<(PathBuf, T)>,
    ) -> Result<(), String> {
        let here: PathBuf = file.components().collect();

        if walking.contains(&here) {
            return Err(format!("{}: includes itself", file.display()));
        }

        if walking.len() > binds::DEEPEST {
            return Err(format!("{}: includes too deeply", file.display()));
        }

        let text = match given {
            Some(text) => text.to_owned(),
            None => match fs::read_to_string(file) {
                Ok(text) => text,
                Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(());
                }
                Err(error) => return Err(format!("{}: {error}", file.display())),
            },
        };
        let named = |why: String| format!("{}: {why}", file.display());

        // in file order: what is set before an include is overridden by it
        let mut events: Vec<(usize, Event<T>)> = Vec::new();

        for (offset, hit) in scan(&text).map_err(named)? {
            events.push((offset, Ok(hit)));
        }

        for (offset, include, optional) in edit::includes(&text).map_err(named)? {
            events.push((offset, Err((binds::beside(file, &include), optional))));
        }

        events.sort_by_key(|(offset, _)| *offset);

        for (_, event) in events {
            match event {
                Ok(hit) => found.push((file.to_owned(), hit)),
                Err((include, optional)) => {
                    walking.push(here.clone());
                    let walked = self.walk(&include, optional, walking, None, scan, found);
                    walking.pop();
                    walked?;
                }
            }
        }

        Ok(())
    }

    pub fn set(&self, path: &str, value: &str) -> Result<String, String> {
        if let Some(output) = output_form(path) {
            let (id, key) = output?;

            if key.is_empty() {
                return Err(String::from(
                    "name what to set in the block, like output[\"DP-1\"].scale",
                ));
            }

            return self.set_output(&id, &key, output_value(&key, value)?);
        }

        let value = Value::parse(value)?;

        self.edit(&[(path, Op::Set(value))])
    }

    pub fn remove(&self, path: &str) -> Result<String, String> {
        if let Some(output) = output_form(path) {
            let (id, key) = output?;

            if key.is_empty() {
                return self.reset_output(&id, OUTPUT_KEYS);
            }

            return self.remove_output(&id, &key);
        }

        if path.contains("[new]") {
            return Err(format!(
                "{path}: [new] is only for appending a new node with set"
            ));
        }

        self.edit(&[(path, Op::Remove)])
    }

    /// Several settings in one change, one backup and one `undo`: each path set to its value,
    /// or removed when none. Nothing is written unless every one can be.
    pub fn apply(&self, changes: &[(&str, Option<&str>)]) -> Result<String, String> {
        let mut ops = Vec::with_capacity(changes.len());

        for &(path, value) in changes {
            ops.push((
                path,
                match value {
                    Some(value) => Op::Set(Value::parse(value)?),
                    None => Op::Remove,
                },
            ));
        }

        self.edit(&ops)
    }

    /// Why Kanade may not change the config, if it may not (`set` and the others say it too).
    pub fn editable(&self) -> Result<(), String> {
        self.writable().map(|_| ())
    }

    fn edit(&self, ops: &[(&str, Op)]) -> Result<String, String> {
        self.change(true, |bytes| {
            let mut text = std::str::from_utf8(bytes)
                .map_err(|_| String::from("the config is not UTF-8; niri would not read it"))?
                .to_owned();
            let mut said = Vec::new();
            let mut written = false;

            for (path, op) in ops {
                let segments = edit::segments(path)?;
                let found = self.gather(&text, &segments)?;

                // an include sets it: the edit would not be the value niri uses, or would be undone
                if let Some((owner, _)) = found.iter().find(|(file, _)| *file != self.file) {
                    let sync = if owner.file_name().is_some_and(|name| name == "kanade.kdl") {
                        " (appearance.sync writes it)"
                    } else {
                        ""
                    };

                    return Err(format!(
                        "{path}: set by {}{sync}, an include; Kanade edits only {}",
                        owner.display(),
                        self.file.display()
                    ));
                }

                let was = found.last().map(|(_, hit)| hit.shown.clone());

                match edit::apply(&text, &segments, op)? {
                    // in a batch, a key that was not there is not worth a line
                    None if ops.len() > 1 && !matches!(op, Op::DropEmpty) => {}
                    None => said.push(match op {
                        Op::Set(value) => format!("{path} is {} already", value.shown()),
                        Op::Remove | Op::DropEmpty => format!("{path} is not set"),
                    }),
                    Some(edited) => {
                        text = edited;
                        written = true;
                        said.push(match op {
                            Op::Set(value) => format!(
                                "{path} = {} (was {})",
                                value.shown(),
                                was.as_deref().unwrap_or("unset")
                            ),
                            Op::Remove | Op::DropEmpty => format!(
                                "{path} removed (was {})",
                                was.as_deref().unwrap_or("unset")
                            ),
                        });
                    }
                }
            }

            // what niri reads only when it starts, which a written change does not reach
            if written && let Some(note) = at_restart(ops) {
                said.push(note);
            }

            let said = said.join("; ");

            Ok(if written {
                Plan::Write(text.into_bytes(), said)
            } else {
                Plan::Same(said)
            })
        })
    }

    /// The backup ring's newest after-hash, when it belongs to this config.
    pub fn last_change_hash(&self) -> Result<String, String> {
        let last = self
            .ring()
            .last()
            .ok_or("nothing to undo: Kanade has not changed this config")?;
        if !same_file(&last.config, &self.file) {
            return Err(format!(
                "the last change was to {}, not this config",
                last.config.display()
            ));
        }
        Ok(last.after)
    }

    /// Puts back what the last change replaced, while the file is as that change left it.
    pub fn undo(&self) -> Result<String, String> {
        self.undo_only(None)
    }

    /// Undo only if the ring still ends with the change this caller made.
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
            if after.is_some_and(|expected| expected != last.after) {
                return Err(String::from(
                    "the last change is not this page's change; undo would lose that. Pick a backup with `backups` and `restore`",
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

    /// The backups, newest first, then the pinned original.
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

    /// Puts a backup back (`1` the newest, or `orig`), keeping what is there as a backup.
    pub fn restore(&self, which: &str) -> Result<String, String> {
        self.change(false, |bytes| {
            let (name, old) = self.ring().read(which)?;
            if bytes == old {
                return Ok(Plan::Same(format!("the config is {name} already")));
            }
            Ok(Plan::Write(old, format!("restored {name}")))
        })
    }

    // the flow in the module doc, for what `plan` makes of the bytes read
    fn change(
        &self,
        validate_original: bool,
        plan: impl FnOnce(&[u8]) -> Result<Plan, String>,
    ) -> Result<String, String> {
        // the lock first, so the mode below is the one the file has once it is Kanade's turn, not
        // one another writer may have replaced in between. What refuses the file outright (the
        // system config, a link, a missing file) is said before the lock, which needs a path to
        // resolve
        self.writable()?;
        let _lock = self.lock()?;
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
        let meta =
            fs::metadata(&target).map_err(|error| format!("{}: {error}", target.display()))?;
        let (snapshot, old) = Snapshot::of(&target)?;

        if validate_original {
            (self.validate)(&self.file).map_err(|why| {
                format!("niri rejects the config as it is, so nothing was changed: {why}")
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

        (self.validate)(staged.path()).map_err(|why| {
            let part = staged
                .path()
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let file = self
                .file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();

            format!(
                "niri rejects the change, so nothing was changed: {}",
                why.replace(&part, &file)
            )
        })?;

        let ring = self.ring();
        let before = ring
            .stage(&old, (self.now_ms)())
            .map_err(|error| format!("cannot keep a backup, so nothing was changed: {error}"))?;

        // read again at the last moment: a save by an editor since the snapshot is not lost
        let (now, _) = Snapshot::of(&target)?;

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
            Ok(()) => format!("{said}; backup 1, `kanade niri-config undo` reverts"),
            Err(error) => format!(
                "{said}; the change was written but could not be recorded for undo: {error}"
            ),
        })
    }

    // One advisory lock per canonical config path. The private state directory is shared by
    // Kanade processes, while the hash keeps configs with the same basename independent.
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

    // the config's metadata, if Kanade may change it
    fn writable(&self) -> Result<fs::Metadata, String> {
        let shown = self.file.display();

        if self.file.starts_with("/etc") {
            return Err(format!(
                "{shown} is the system config, which Kanade does not write. Make your own: \
                 mkdir -p ~/.config/niri && cp {shown} ~/.config/niri/config.kdl"
            ));
        }

        let meta = fs::symlink_metadata(&self.file).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => format!(
                "{shown}: no such file; niri runs on its built-in config, which Kanade does not create"
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

        // another name of the same file: a replace would split the two, and the other name keeps
        // the old bytes
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
}

// the most bytes read of one of `niri validate`'s pipes: its first error is far shorter, and a
// config that makes it print more must not grow Kanade's memory with it
const MOST_OUTPUT: u64 = 1 << 20;

// what a pipe gives until it closes, read on a thread so a full pipe never stops the child, and no
// more than `MOST_OUTPUT` of it
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();

        if let Some(pipe) = pipe {
            // the child is killed and reaped by then, so a read that stopped at the cap leaves no
            // writer blocked on a full pipe for long
            let _ = pipe.take(MOST_OUTPUT).read_to_end(&mut bytes);
        }

        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/*
 * the arguments `niri validate` is given for `file`: one `--config=<file>`, never `-c <file>` as
 * two, so a path that begins with `-` is a value and not an option of its own. `--config` also
 * takes a path that does not start with `-` the same way
 */
fn validate_args(file: &Path) -> Vec<std::ffi::OsString> {
    let mut config = std::ffi::OsString::from("--config=");

    config.push(file);
    vec![std::ffi::OsString::from("validate"), config]
}

/// `niri validate --config=<file>`: ok only if niri's own exit code says so and it printed no error;
/// else its first error, without color, with the line and column it points at.
pub fn niri_validate(file: &Path) -> Result<(), String> {
    let mut child = Command::new("niri")
        .args(validate_args(file))
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!("cannot run niri validate ({error}); not changing a config unchecked")
        })?;

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

                return Err(String::from("niri validate did not finish in 10 s"));
            }
            Err(error) => {
                // a child that cannot be waited on is killed rather than left running
                let _ = child.kill();
                let _ = child.wait();

                return Err(format!("niri validate: {error}"));
            }
        }
    };

    let output = format!(
        "{}\n{}",
        err.join().unwrap_or_default(),
        out.join().unwrap_or_default()
    );

    verdict(status.success(), &output)
}

// niri's verdict on its exit status and what it printed
fn verdict(success: bool, output: &str) -> Result<(), String> {
    let text = plain(output);
    let errored = text
        .lines()
        .any(|line| line.trim_start().starts_with("Error:"));

    if success && !errored {
        return Ok(());
    }

    Err(first_error(&text))
}

/*
 * text without terminal color. Its lines stay: what niri printed is read line by line for its first
 * error
 */
fn plain(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(char) = chars.next() {
        if char == '\x1b' && chars.peek() == Some(&'[') {
            for skipped in chars.by_ref() {
                if skipped.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(char);
        }
    }

    plain
}

// niri's first specific error, as one line with where it points; its generic heads
// (`error loading config`) are passed over
fn first_error(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let message = |line: &str| {
        line.split_once('×')
            .map(|(_, message)| message.trim().to_owned())
    };
    let generic = |message: &str| {
        message.starts_with("error loading config") || message.starts_with("error parsing")
    };

    let Some((at, head)) = lines
        .iter()
        .enumerate()
        .filter_map(|(at, line)| Some((at, message(line)?)))
        .find(|(_, message)| !generic(message))
    else {
        // only the generic head: its causes say more
        return lines
            .iter()
            .rev()
            .find(|line| line.contains('▶') && !line.contains("error parsing KDL"))
            .and_then(|line| line.split_once('▶'))
            .map_or_else(
                || {
                    lines
                        .iter()
                        .find(|line| !line.trim().is_empty())
                        .map_or_else(
                            || String::from("niri validate failed"),
                            |line| line.trim().to_owned(),
                        )
                },
                |(_, cause)| cause.trim().to_owned(),
            );
    };

    let mut message = head;

    // the message goes on over lines that start with a bar
    for line in &lines[at + 1..] {
        match line.trim_start().strip_prefix('│') {
            Some(more) if !more.trim().is_empty() => {
                message.push(' ');
                message.push_str(more.trim());
            }
            _ => break,
        }
    }

    let location = lines[at..].iter().find_map(|line| {
        let (_, after) = line.split_once("╭─[")?;

        Some(after.split_once(']')?.0.to_owned())
    });

    match location {
        Some(location) => format!("{message} ({location})"),
        None => message,
    }
}

/// Why a monitor's block may not be changed, if an include has a block for it.
pub fn output_editable(id: &OutputId) -> Result<(), String> {
    let file = target(None)?;
    let editor = Editor {
        file,
        state: state()?,
        validate: &niri_validate,
        now_ms,
    };

    editor.output_editable(id)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod arg_tests {
    use super::*;

    // a config path that begins with `-` is a value, not an option: `-c -c.kdl` is read by clap as
    // `-c` with no value, so niri never opened the file
    #[test]
    fn a_config_path_that_looks_like_an_option_is_a_value() {
        let args = validate_args(Path::new("-c.kdl"));

        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "validate");
        assert_eq!(args[1], "--config=-c.kdl");

        let plain = validate_args(Path::new("/home/x/config.kdl"));
        assert_eq!(plain[1], "--config=/home/x/config.kdl");
    }

    // a block's name comes from the config or a monitor's EDID: neither may carry a control
    // character, a newline included
    #[test]
    fn an_output_label_carries_no_control_characters() {
        assert_eq!(
            output_label("DP-1\u{1b}[2J", "mode"),
            "output[\"DP-1\"].mode"
        );
        assert_eq!(output_label("DP-1\nEDID\u{7}", ""), "output[\"DP-1EDID\"]");
        assert_eq!(output_label("a\"b\\c", ""), "output[\"a\\\"b\\\\c\"]");
    }

    // niri colors its errors; its lines are what the first error is read from
    #[test]
    fn plain_strips_terminal_color() {
        assert_eq!(plain("\u{1b}[31mred\u{1b}[0m\nnext"), "red\nnext");
    }
}
