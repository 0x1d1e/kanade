//! `kanade sddm`: the login screen's look, written from the config by a command that runs without a
//! shell.
//!
//! SDDM reads its theme as the `sddm` user, so what it reads lives in a root-owned overlay, outside
//! `/usr/share`: `<prefix>/themes/<theme>` (for Kanade's own theme a copy of the one built into this
//! binary, for another a directory of links to the installed theme's files, both with a
//! `theme.conf.user` of Kanade's). The one mutable file, the backdrop, is deliberately outside the
//! overlay SDDM loads code from: `/<DATA>/backdrop.png`, a directory owned by the desktop user, which
//! is the only path the theme loads from outside its own tree (ADR 0120). The prefix is made once,
//! by root; Kanade only prints those commands, never runs sudo and never writes outside what the
//! user owns.
//! The picture is a re-encoded, blurred and dimmed copy, so it holds none of the original's
//! metadata, and the login screen's user can read it: `sddm.sync` is off by default for that reason.

mod class;
mod commands;
mod follow;

pub use commands::{disable, enable, info, list, use_theme};
pub use follow::wallpaper_changed;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io;
#[cfg(test)]
use std::os::unix::fs::symlink;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use kanade_lock::Picture;
use rustix::fs::OFlags;
use rustix::process::{getgid, getuid};

use self::class::Class;
use crate::atomic::{self, Mode};
use crate::cli;
use crate::clock;
use crate::config::{self, Config};
use crate::look::SddmBackground;
use crate::sources::json::Json;
use crate::sources::{niri, wallpaper};

// SDDM loads theme code only from this root-owned tree.
const PREFIX: &str = "usr/local/share/kanade-sddm";

// Mutable wallpaper data is kept outside every SDDM-loaded theme directory.
const DATA: &str = "usr/local/share/kanade-sddm-data";

// where themes are installed, which SDDM's `ThemeDir` defaults to
const INSTALLED: &str = "usr/share/sddm/themes";

// Kanade's own theme, by its directory name
const OWN: &str = "kanade";

/*
 * the root everything above is under, `/` but for a test (the hidden `KANADE_SDDM_ROOT`, an
 * absolute directory), so a run never touches `/usr/local` or `/etc`. Test-only by intent: it ships
 * in the binary so a root `kanade sddm setup` can install elsewhere for a test, and it is not a
 * supported way to run Kanade, which writes the real overlay when it is unset
 */
const ROOT: &str = "KANADE_SDDM_ROOT";

// the most bytes the picture is, so the login screen's memory and the disk stay small
const MOST_BYTES: usize = 16 << 20;

// a smaller picture, by this fraction of each side, when the one at the screen's size is over
const SHRINK: f32 = 0.75;
const TRIES: u32 = 4;

// the size when niri does not say
const FALLBACK: (u32, u32) = (1920, 1080);

// Kanade's theme, as built into this binary: the same files as `dist/sddm/kanade`
const THEME: &[(&str, &str)] = &[
    (
        "metadata.desktop",
        include_str!("../dist/sddm/kanade/metadata.desktop"),
    ),
    ("theme.conf", include_str!("../dist/sddm/kanade/theme.conf")),
    ("Main.qml", include_str!("../dist/sddm/kanade/Main.qml")),
    (
        "Greeter.qml",
        include_str!("../dist/sddm/kanade/Greeter.qml"),
    ),
    ("Pill.qml", include_str!("../dist/sddm/kanade/Pill.qml")),
    ("Chip.qml", include_str!("../dist/sddm/kanade/Chip.qml")),
    ("Glyph.qml", include_str!("../dist/sddm/kanade/Glyph.qml")),
    ("Avatar.qml", include_str!("../dist/sddm/kanade/Avatar.qml")),
    ("Line.qml", include_str!("../dist/sddm/kanade/Line.qml")),
    (
        "TimeLine.qml",
        include_str!("../dist/sddm/kanade/TimeLine.qml"),
    ),
    (
        "CapsGlyph.qml",
        include_str!("../dist/sddm/kanade/CapsGlyph.qml"),
    ),
    (
        "Sanitize.js",
        include_str!("../dist/sddm/kanade/Sanitize.js"),
    ),
    ("LICENSE", include_str!("../dist/sddm/kanade/LICENSE")),
    ("NOTICE", include_str!("../dist/sddm/kanade/NOTICE")),
];

const USER: &str = "theme.conf.user";

// the SDDM config file Kanade prints a step for, last in /etc/sddm.conf.d by its name
const CONF: &str = "zz-kanade.conf";

// ---- where things are

struct Where {
    root: PathBuf,

    // the file whose lock makes the commands that write the overlay take turns, none without a
    // state directory (they then run unlocked)
    lock: Option<PathBuf>,
}

impl Where {
    fn here() -> Where {
        let overridden = std::env::var_os(ROOT)
            .map(PathBuf::from)
            .filter(|root| root.is_absolute());
        let home = std::env::var("HOME").ok();

        match overridden {
            // a test's: its lock is in its own root, so a test never waits on the user's shell
            Some(root) => {
                let lock = Some(root.join("kanade-sddm.lock"));

                Where { root, lock }
            }
            None => Where {
                root: PathBuf::from("/"),
                lock: config::base("XDG_STATE_HOME", ".local/state", home.as_deref())
                    .map(|dir| dir.join("kanade/sddm.lock")),
            },
        }
    }

    #[cfg(test)]
    fn under(root: PathBuf) -> Where {
        let lock = Some(root.join("kanade-sddm.lock"));

        Where { root, lock }
    }

    fn prefix(&self) -> PathBuf {
        self.root.join(PREFIX)
    }

    fn installed(&self, theme: &str) -> PathBuf {
        self.root.join(INSTALLED).join(theme)
    }

    fn overlay(&self, theme: &str) -> PathBuf {
        self.prefix().join("themes").join(theme)
    }

    fn data(&self) -> PathBuf {
        self.root.join(DATA)
    }

    fn picture(&self) -> PathBuf {
        self.data().join("backdrop.png")
    }

    // SDDM reads system drop-ins, local drop-ins, then /etc/sddm.conf; later entries win.
    // ASCII lowercase approximates Qt's locale-aware drop-in ordering.
    fn configs(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();

        for dir in ["usr/lib/sddm/sddm.conf.d", "etc/sddm.conf.d"] {
            let mut found: Vec<PathBuf> = fs::read_dir(self.root.join(dir))
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "conf"))
                .collect();

            found.sort_by_key(|path| path.file_name().map(|name| name.to_ascii_lowercase()));
            files.extend(found);
        }
        files.push(self.root.join("etc/sddm.conf"));

        files
    }

    // the file Kanade writes the `[Theme]` group to: named to be read after any other in its
    // directory, as a file like `theme.conf` is, since digits and most letters sort before `zz`
    fn ours(&self) -> PathBuf {
        self.root.join("etc/sddm.conf.d").join(CONF)
    }

    // whether SDDM reads `file` after Kanade's own, which then cannot outdo it by its name
    fn after_ours(&self, file: &Path) -> bool {
        let name = |path: &Path| path.file_name().map(|name| name.to_ascii_lowercase());

        file == self.root.join("etc/sddm.conf")
            || (file.parent() == self.ours().parent() && name(file) > name(&self.ours()))
    }

    // the prefix as text, which a path in the theme's config is made of
    fn text(&self) -> Result<String, String> {
        let prefix = self.prefix();
        let text = prefix
            .to_str()
            .ok_or("the overlay's path is not UTF-8")?
            .to_owned();

        if text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/_.-".contains(c))
        {
            Ok(text)
        } else {
            Err(format!(
                "{text}: plain characters only in the overlay's path"
            ))
        }
    }
}

// ---- what to check, which the theme checks again (`dist/sddm/kanade/Sanitize.js`)

/*
 * whether the theme would read `path` as its backdrop: under `prefix` and nowhere else, every
 * segment letters, digits, `_`, `.` or `-` and not empty or `..`, a .png, .jpg or .webp file, at most
 * 128 characters. The same rule as `Sanitize.backdrop`; a path it refuses shows no picture
 */
#[cfg(test)]
fn backdrop_ok(prefix: &str, path: &str) -> bool {
    let Some(rest) = path
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return false;
    };

    path.len() <= 128
        && [".png", ".jpg", ".webp"]
            .iter()
            .any(|extension| path.ends_with(extension))
        && rest.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
}

// a theme's directory name: one plain segment
fn theme_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        // the names the overlay builds a theme under, then moves aside
        && !name.ends_with(".new")
        && !name.ends_with(".old")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

// `#rrggbb`
/*
 * the `theme.conf.user` for `theme`. Kanade's theme reads `backdrop` (a blurred, dimmed picture)
 * and `surface` (the unshaded color); its file is Kanade's whole. Other themes keep installed
 * `theme.conf.user` via `user_ini`, replacing `key` only when a picture exists, so user settings stay.
 * Reject values the theme would refuse.
 */
#[cfg(test)]
fn conf_user(
    prefix: &str,
    theme: &str,
    picture: Option<&str>,
    installed: Option<&str>,
    key: &str,
) -> Result<String, String> {
    if !theme_ok(theme) {
        return Err(format!("{theme}: not a theme's directory name"));
    }
    if let Some(path) = picture
        && path != format!("/{DATA}/backdrop.png")
        && !backdrop_ok(prefix, path)
    {
        return Err(format!("{path}: the theme would not read a picture there"));
    }
    if theme != OWN {
        return Ok(with_background(installed.unwrap_or(""), key, picture));
    }

    Ok(String::from(
        "# Written by `kanade sddm setup`; user settings are preserved across setup.\n[General]\nbackdrop=/usr/local/share/kanade-sddm-data/backdrop.png\n",
    ))
}

// whether `text` is an ini file whole: sections, `key=value`, comments and blank lines, nothing else
fn preserve_user_settings(text: &str) -> String {
    // INI keys before the first section belong to General in QSettings.
    let mut section = String::from("general");
    let mut lines = Vec::new();
    for line in class::unbom(text).lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed[1..trimmed.len() - 1].trim().to_ascii_lowercase();
        }
        if section == "general"
            && let Some((key, _)) = trimmed.split_once('=')
            && matches!(
                key.trim().to_ascii_lowercase().as_str(),
                "backdrop" | "shade"
            )
        {
            continue;
        }
        lines.push(line);
    }
    let mut out = lines.join("\n");
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("[General]\n");
    out.push_str(&format!("backdrop=/{DATA}/backdrop.png\n"));
    out
}

/*
 * `text` with the lines an ini file may hold: sections, `key=value`, comments and blank lines, each
 * as it was. A line that is none of those is dropped, not the whole file with it: one stray line in
 * the `theme.conf.user` a setup preserves must not lose every setting the user made. Over 64 KiB,
 * whole lines are kept up to the limit
 */
fn user_ini(text: &str) -> String {
    const MOST: usize = 64 << 10;

    let mut out = String::new();

    for line in class::unbom(text).lines() {
        let trimmed = line.trim();
        let section = trimmed.starts_with('[') && trimmed.ends_with(']');
        let comment = trimmed.starts_with('#') || trimmed.starts_with(';');
        let pair = trimmed.contains('=') && !trimmed.starts_with('[');

        if !(trimmed.is_empty() || section || comment || pair) {
            continue;
        }
        if out.len() + line.len() + 1 > MOST {
            break;
        }

        out.push_str(line);
        out.push('\n');
    }

    out
}

/*
 * the ini `base` with `key` set to `picture` in `[General]` (where the keys before any section are
 * too), and every other line as it was. Without a picture Kanade manages nothing and `base` goes
 * through. A byte order mark first is dropped, which would otherwise hide the first section
 */
#[cfg(test)]
fn with_background(base: &str, key_name: &str, picture: Option<&str>) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut end = None;
    let mut general = true;

    for line in class::unbom(base).lines() {
        let trimmed = line.trim();

        if let Some(name) = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            general = name == "General";
        } else if general
            && picture.is_some()
            && trimmed
                .split_once('=')
                .is_some_and(|(key, _)| key.trim() == key_name)
        {
            continue;
        }

        lines.push(line.to_owned());
        if general && !trimmed.is_empty() {
            end = Some(lines.len());
        }
    }

    if let Some(path) = picture {
        let at = end.unwrap_or_else(|| {
            lines.push(String::from("[General]"));
            lines.len()
        });
        lines.insert(at, format!("{key_name}={path}"));
    } else if base.trim().is_empty() {
        lines.push(String::from("[General]"));
    }

    let mut text = format!(
        "# Written by `kanade sddm sync`: the installed theme's theme.conf.user, its `{key_name}` replaced.\n"
    );
    for line in lines {
        text.push_str(&line);
        text.push('\n');
    }

    text
}

// a `[Theme]` key's last value, and the file it came from
type Said = Option<(String, PathBuf)>;

/*
 * `ThemeDir` and `Current` from the `[Theme]` group of one config file's `text`, replacing what
 * earlier files said; an unreadable file says nothing
 */
fn read_theme(text: &str, file: &Path, dir: &mut Said, current: &mut Said) {
    let mut group = "";

    for line in class::unbom(text).lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            group = name;
        } else if group == "Theme"
            && let Some((key, value)) = line.split_once('=')
        {
            let said = Some((value.trim().to_owned(), file.to_owned()));

            match key.trim() {
                "ThemeDir" => *dir = said,
                "Current" => *current = said,
                _ => {}
            }
        }
    }
}

// ---- the picture

// the size of the largest output niri says, else a 1080p one
fn largest_output() -> (u32, u32) {
    niri::ask("\"Outputs\"")
        .ok()
        .and_then(|reply| largest(&reply))
        .unwrap_or(FALLBACK)
}

// the current mode of the output with the most pixels
fn largest(reply: &Json) -> Option<(u32, u32)> {
    reply
        .get("Outputs")?
        .as_object()?
        .values()
        .filter_map(|output| {
            let mode = output
                .get("modes")?
                .get(usize::try_from(output.get("current_mode")?.as_u64()?).ok()?)?;
            let side = |key| mode.get(key)?.as_u64().and_then(|n| u32::try_from(n).ok());

            Some((side("width")?, side("height")?))
        })
        .filter(|&(width, height)| width > 0 && height > 0)
        .max_by_key(|&(width, height)| u64::from(width) * u64::from(height))
}

// an 8-bit RGB png of `picture`, which is opaque
fn encode(picture: &Picture) -> Result<Vec<u8>, String> {
    let rgb: Vec<u8> = picture
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[red, green, blue, _]| [red, green, blue])
        .collect();
    let mut bytes = Vec::new();

    let mut encoder = png::Encoder::new(&mut bytes, picture.width, picture.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);

    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&rgb))
        .map_err(|error| format!("cannot encode the picture: {error}"))?;

    Ok(bytes)
}

/*
 * decode the source as the lock screen does, cover `size`, blur and dim it, then re-encode without
 * source metadata; shrink until under `MOST_BYTES`. Follow source links to regular files; writes
 * refuse links.
 */
fn backdrop(source: &Path, size: (u32, u32), blur: u32, dim: u32) -> Result<Vec<u8>, String> {
    let named = class::plain(&source.display().to_string());
    let source = &fs::canonicalize(source).map_err(|error| format!("{named}: {error}"))?;
    let meta = fs::metadata(source).map_err(|error| format!("{named}: {error}"))?;

    if !meta.is_file() {
        return Err(format!("{named} is not a file"));
    }

    let (width, height) = size;
    let (decoded_width, decoded_height, rgba) =
        wallpaper::decoded(source, width.max(height) as f32).ok_or_else(|| {
            format!("{named}: not a png, jpeg, webp, gif or svg that Kanade decodes")
        })?;
    let original = Picture {
        width: decoded_width,
        height: decoded_height,
        rgba,
    };

    let mut scale = 1.0;
    for _ in 0..TRIES {
        let at = (
            ((width as f32 * scale) as u32).max(1),
            ((height as f32 * scale) as u32).max(1),
        );
        let shown = original
            .backdrop(at, (blur as f32 * scale) as u32, dim as f32 / 100.0)
            .ok_or_else(|| format!("{named}: no picture to draw"))?;
        let bytes = encode(&shown)?;

        if bytes.len() <= MOST_BYTES {
            return Ok(bytes);
        }

        scale *= SHRINK;
    }

    Err(format!(
        "{named}: still over {} MiB at {}% of the screen's size ({}% smaller each side, {} times)",
        MOST_BYTES >> 20,
        (SHRINK.powi(TRIES as i32) * 100.0).round() as u32,
        ((1.0 - SHRINK) * 100.0).round() as u32,
        TRIES
    ))
}

// ---- the overlay

// the overlay's directory is Kanade's: a directory the user owns, not a link, nobody else writes to
fn owned(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path)
        .map_err(|error| format!("{}: {error}", class::plain(&path.display().to_string())))?;

    if !meta.is_dir() {
        return Err(format!(
            "{} is not a directory",
            class::plain(&path.display().to_string())
        ));
    }
    if meta.uid() != getuid().as_raw() {
        return Err(format!(
            "{} is not yours",
            class::plain(&path.display().to_string())
        ));
    }
    if meta.permissions().mode() & 0o022 != 0 {
        return Err(format!(
            "{} is writable by others",
            class::plain(&path.display().to_string())
        ));
    }

    Ok(())
}

fn root_owned_ancestors(trusted_root: &Path, path: &Path) -> Result<(), String> {
    let expected_owner = if cfg!(test) { getuid().as_raw() } else { 0 };
    let relative = path.strip_prefix(trusted_root).map_err(|_| {
        format!(
            "{} is outside trusted root {}",
            class::plain(&path.display().to_string()),
            class::plain(&trusted_root.display().to_string())
        )
    })?;
    let mut current = trusted_root.to_owned();
    let mut chain = vec![current.clone()];
    for component in relative.components() {
        current.push(component);
        chain.push(current.clone());
    }
    for ancestor in chain {
        let meta = fs::symlink_metadata(&ancestor).map_err(|error| {
            format!("{}: {error}", class::plain(&ancestor.display().to_string()))
        })?;
        let mode = meta.permissions().mode();
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != expected_owner
            || mode & 0o022 != 0
        {
            return Err(format!(
                "{} must be a root-owned directory not writable by group or others",
                class::plain(&ancestor.display().to_string())
            ));
        }
    }
    Ok(())
}

pub(crate) fn root_owned_read_only(trusted_root: &Path, path: &Path) -> Result<(), String> {
    fn visit(path: &Path, expected_owner: u32) -> Result<(), String> {
        let meta = fs::symlink_metadata(path)
            .map_err(|error| format!("{}: {error}", class::plain(&path.display().to_string())))?;
        let mode = meta.permissions().mode();
        let safe_type = if meta.is_dir() {
            mode & 0o005 == 0o005
        } else if meta.is_file() {
            mode & 0o004 != 0
        } else {
            false
        };
        if meta.file_type().is_symlink()
            || meta.uid() != expected_owner
            || mode & 0o022 != 0
            || !safe_type
        {
            return Err(format!(
                "{} must be root-owned, readable by SDDM, and not writable by group or others",
                class::plain(&path.display().to_string())
            ));
        }
        if meta.is_dir() {
            for entry in fs::read_dir(path).map_err(|error| {
                format!("{}: {error}", class::plain(&path.display().to_string()))
            })? {
                let entry = entry.map_err(|error| {
                    format!("{}: {error}", class::plain(&path.display().to_string()))
                })?;
                visit(&entry.path(), expected_owner)?;
            }
        }
        Ok(())
    }

    let expected_owner = if cfg!(test) { getuid().as_raw() } else { 0 };
    root_owned_ancestors(trusted_root, path)?;
    visit(path, expected_owner)
}

// the root commands that make the overlay, which only the user may run
fn make_prefix(prefix: &str) -> String {
    format!(
        "install -d -o root -g root -m 755 {prefix} {prefix}/themes && install -d -o {} -g {} -m 755 /{DATA}",
        getuid().as_raw(),
        getgid().as_raw()
    )
}

/*
 * the root command that points SDDM at the overlay. It does nothing unless the overlay holds the
 * theme (its `metadata.desktop`), so run before `kanade sddm sync` it cannot leave the login screen
 * with a theme that is not there
 */
fn point_sddm(w: &Where, theme: &str) -> Result<String, String> {
    let prefix = w.text()?;

    Ok(format!(
        "test -f {prefix}/themes/{theme}/metadata.desktop && printf '[Theme]\\nThemeDir={prefix}/themes\\nCurrent={theme}\\n' | install -D -m 644 /dev/stdin {}",
        class::plain(&w.ours().display().to_string())
    ))
}

// the overlay of a theme that is installed: what it should hold, relative to it, a directory or
// the file it links to; `USER` is Kanade's and not among them
fn plan(installed: &Path) -> Result<Vec<(PathBuf, Option<PathBuf>)>, String> {
    // a theme is a few dozen files; this much is a mistake or a loop
    const MOST: usize = 4000;
    const DEEPEST: usize = 8;

    fn walk(
        dir: &Path,
        at: &Path,
        depth: usize,
        out: &mut Vec<(PathBuf, Option<PathBuf>)>,
    ) -> Result<(), String> {
        if depth > DEEPEST || out.len() > MOST {
            return Err(format!(
                "{}: too large for a theme",
                class::plain(&dir.display().to_string())
            ));
        }

        let mut entries: Vec<_> = fs::read_dir(dir)
            .map_err(|error| format!("{}: {error}", class::plain(&dir.display().to_string())))?
            .flatten()
            .collect();
        entries.sort_by_key(fs::DirEntry::file_name);

        for entry in entries {
            let path = entry.path();
            let rel = at.join(entry.file_name());

            if depth == 0 && entry.file_name() == USER {
                continue;
            }
            // what a link points at decides what it is
            match fs::metadata(&path) {
                Ok(meta) if meta.is_dir() => {
                    out.push((rel.clone(), None));
                    walk(&path, &rel, depth + 1, out)?;
                }
                Ok(meta) if meta.is_file() => out.push((rel, Some(path))),
                _ => {}
            }

            // counted as it goes, so one huge directory trips the cap while it is read, not whole
            // before the next directory it enters finds it too large
            if out.len() > MOST {
                return Err(format!(
                    "{}: too large for a theme",
                    class::plain(&dir.display().to_string())
                ));
            }
        }

        Ok(())
    }

    let mut out = Vec::new();
    walk(installed, Path::new(""), 0, &mut out)?;

    Ok(out)
}

// whether the overlay of an installed theme has exactly the links `plan` gives
fn farm_current(overlay: &Path, wanted: &[(PathBuf, Option<PathBuf>)]) -> bool {
    let mut expected: BTreeSet<&Path> = wanted.iter().map(|(rel, _)| rel.as_path()).collect();

    expected.insert(Path::new(USER));
    let found = fs::read_dir(overlay).map_or(0, |dir| dir.count());
    let top = expected
        .iter()
        .filter(|rel| rel.components().count() == 1)
        .count();

    found == top
        && wanted.iter().all(|(rel, target)| {
            let path = overlay.join(rel);
            match target {
                Some(target) => fs::read_link(&path).is_ok_and(|link| &link == target),
                None => fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()),
            }
        })
}

/*
 * a directory (and the missing ones above it) the sddm user can walk into whatever the umask is:
 * `DirBuilder`'s mode is only trimmed by the umask, so the bits are set outright after
 */
fn make_dir(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        make_dir(parent)?;
    }

    DirBuilder::new().mode(0o755).create(path)?;
    fs::set_permissions(path, Permissions::from_mode(0o755))
}

// `overlay` with `.new` or `.old` after its name, beside it
fn beside(overlay: &Path, ending: &str) -> PathBuf {
    let mut name = overlay.file_name().unwrap_or_default().to_owned();

    name.push(ending);
    overlay.with_file_name(name)
}

/*
 * a `farm` whose process died between its two renames leaves the overlay gone and the last working
 * theme in `<theme>.old`: puts it back. Anything else is left alone, and a rename that fails is not
 * papered over, since `.old` may be all there is left to show
 */
fn recover(overlay: &Path) -> io::Result<()> {
    let old = beside(overlay, ".old");

    if !matches!(fs::symlink_metadata(overlay), Err(error) if error.kind() == io::ErrorKind::NotFound)
    {
        return Ok(());
    }
    if !fs::symlink_metadata(&old).is_ok_and(|meta| meta.is_dir()) {
        return Ok(());
    }

    fs::rename(&old, overlay)
}

/*
 * the paths under `dir` (and it) that the sddm user cannot read: a directory without o+rx, a file
 * without o+r; links are not followed. With `fix`, those are given 0755 and 0644, the entries it
 * could not are an error. Bounded, as `plan` is: an overlay is a few dozen entries
 */
fn modes(dir: &Path, fix: bool) -> io::Result<Vec<PathBuf>> {
    fn visit(
        path: &Path,
        depth: usize,
        fix: bool,
        out: &mut Vec<PathBuf>,
        seen: &mut usize,
    ) -> io::Result<()> {
        *seen += 1;
        if depth > 12 || *seen > 10_000 {
            return Ok(());
        }

        // the top may be a link (an installed theme often is, to a store path), which is followed to be
        // checked; never when mending, as that would change what it leads to. Inside, links stay
        // as they are
        let meta = if depth == 0 && !fix {
            fs::metadata(path)?
        } else {
            fs::symlink_metadata(path)?
        };
        let bits = meta.permissions().mode();
        let (closed, mended) = if meta.is_dir() {
            (bits & 0o005 != 0o005, 0o755)
        } else if meta.is_file() {
            (bits & 0o004 == 0, 0o644)
        } else {
            return Ok(());
        };

        if closed {
            out.push(path.to_owned());
            if fix {
                fs::set_permissions(path, Permissions::from_mode(mended))?;
            }
        }
        if meta.is_dir()
            && let Ok(entries) = fs::read_dir(path)
        {
            for entry in entries.flatten() {
                visit(&entry.path(), depth + 1, fix, out, seen)?;
            }
        }

        Ok(())
    }

    let mut out = Vec::new();
    visit(dir, 0, fix, &mut out, &mut 0)?;

    Ok(out)
}

/*
 * build the overlay beside the active one, then replace it with `renameat2(RENAME_EXCHANGE)` so it
 * is never absent. If exchange is unsupported, fail with the active theme unchanged; the overlay is
 * Kanade's and is replaced whole.
 */
#[cfg(test)]
fn farm(overlay: &Path, wanted: &[(PathBuf, Option<PathBuf>)], user: &str) -> Result<(), String> {
    let said =
        |error: io::Error| format!("{}: {error}", class::plain(&overlay.display().to_string()));
    let (new, old) = (beside(overlay, ".new"), beside(overlay, ".old"));

    // a run that died with the overlay gone and the last working theme in `.old` is put back before
    // the cleanup below would remove it; nothing here makes a `.old` any more, so this is for an
    // older install or one that died before the exchange
    recover(overlay).map_err(said)?;

    // parts a run that died left
    for left in [&new, &old] {
        match fs::symlink_metadata(left) {
            Ok(meta) if meta.is_dir() => fs::remove_dir_all(left),
            Ok(_) => fs::remove_file(left),
            Err(_) => Ok(()),
        }
        .map_err(said)?;
    }

    let built = (|| {
        make_dir(&new)?;
        for (rel, target) in wanted {
            let path = new.join(rel);

            match target {
                None => make_dir(&path)?,
                Some(target) => symlink(target, &path)?,
            }
        }

        let at = new.join(USER);
        atomic::write(&at, user.as_bytes(), Mode::Exactly(0o644))
    })();

    if let Err(error) = built {
        let _ = fs::remove_dir_all(&new);
        return Err(said(error));
    }

    // After exchange, .new holds whatever the overlay was; clean it up after the atomic swap.
    if fs::symlink_metadata(overlay).is_ok() {
        if let Err(error) = clock::exchange(&new, overlay) {
            let _ = fs::remove_dir_all(&new);
            return Err(format!(
                "{}: atomic theme exchange is unavailable; active theme was left unchanged: {error}",
                class::plain(&overlay.display().to_string())
            ));
        }
        let _ = fs::remove_dir_all(&new).or_else(|_| fs::remove_file(&new));
        return Ok(());
    }

    // no overlay to exchange with, so the one built is moved into place; a run that dies before the
    // move leaves no overlay and a `.new` the next run sweeps, never a theme aside in `.old`
    if let Err(error) = fs::rename(&new, overlay) {
        let _ = fs::remove_dir_all(&new).or_else(|_| fs::remove_file(&new));
        return Err(said(error));
    }

    Ok(())
}

// said of any theme but Kanade's, which is built to be read from here
const OTHER_THEMES: &str = "note: a theme other than kanade is best effort; one that imports from a sibling or parent directory or names /usr/share/sddm/themes paths may not work from the overlay (check `kanade sddm status`, and the login screen)";

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn program_exists(program: &str) -> bool {
    if let Some(path) = std::env::var_os("PATH")
        && std::env::split_paths(&path)
            .map(|dir| dir.join(program))
            .any(|candidate| is_executable(&candidate))
    {
        return true;
    }
    for dir in [
        "/usr/bin",
        "/usr/libexec",
        "/usr/lib/sddm",
        "/usr/lib64/sddm",
    ] {
        if is_executable(&Path::new(dir).join(program)) {
            return true;
        }
    }
    false
}

fn installed_greeter_version(w: &Where) -> Option<u8> {
    if w.root != Path::new("/") {
        for (name, ver) in [("sddm-greeter-qt6", 6), ("sddm-greeter", 5)] {
            for sub in ["usr/bin", "usr/libexec", "usr/lib/sddm", "usr/lib64/sddm"] {
                if is_executable(&w.root.join(sub).join(name)) {
                    return Some(ver);
                }
            }
        }
        return None;
    }
    if program_exists("sddm-greeter-qt6") {
        Some(6)
    } else if program_exists("sddm-greeter") {
        Some(5)
    } else {
        None
    }
}

pub fn qt_version_of(metadata: &str) -> Option<u8> {
    for line in metadata.lines() {
        let trimmed = line.trim();
        if let Some((k, v)) = trimmed.split_once('=')
            && k.trim() == "QtVersion"
            && let Ok(ver) = v.trim().parse::<u8>()
        {
            return Some(ver);
        }
    }
    None
}

pub fn set_qt_version(text: &str, version: u8) -> String {
    let mut replaced = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        let is_qt_version = line
            .trim()
            .split_once('=')
            .is_some_and(|(k, _)| k.trim() == "QtVersion");
        if is_qt_version {
            lines.push(format!("QtVersion={version}"));
            replaced = true;
        } else {
            lines.push(line.to_owned());
        }
    }
    if !replaced {
        lines.push(format!("QtVersion={version}"));
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn target_qt_version(w: &Where, overlay: &Path) -> u8 {
    if let Some(ver) = installed_greeter_version(w) {
        return ver;
    }

    let overlay_meta = overlay.join("metadata.desktop");
    if let Some(content) = class::text(&overlay_meta)
        && let Some(ver) = qt_version_of(&content)
    {
        return ver;
    }

    6
}

pub fn configured_qt_version(config: &Config) -> u8 {
    configured_qt_version_under(&Where::here(), config)
}

fn configured_qt_version_under(w: &Where, config: &Config) -> u8 {
    let theme = config.sddm_theme.as_deref().unwrap_or(OWN);
    let overlay_meta = w.overlay(theme).join("metadata.desktop");
    if let Some(content) = class::text(&overlay_meta)
        && let Some(v) = qt_version_of(&content)
    {
        return v;
    }

    if theme == OWN {
        return 6;
    }

    let installed_meta = w.installed(theme).join("metadata.desktop");
    if let Some(content) = class::text(&installed_meta)
        && let Some(v) = qt_version_of(&content)
    {
        return v;
    }

    5
}

// Kanade's theme into its overlay; the files that already read the same are left alone
#[cfg(test)]
fn copy_own(w: &Where, overlay: &Path) -> Result<usize, String> {
    let mut written = 0;
    let target_qt = target_qt_version(w, overlay);

    for (name, text) in THEME {
        let path = overlay.join(name);
        let content: std::borrow::Cow<str> = if *name == "metadata.desktop" {
            std::borrow::Cow::Owned(set_qt_version(text, target_qt))
        } else {
            std::borrow::Cow::Borrowed(*text)
        };

        if class::text(&path).as_deref() == Some(content.as_ref()) {
            continue;
        }

        atomic::write(&path, content.as_bytes(), Mode::Exactly(0o644))
            .map_err(|error| format!("{}: {error}", class::plain(&path.display().to_string())))?;
        written += 1;
    }

    Ok(written)
}

// Root-only install of the built-in executable theme. Assemble the whole directory beside the
// active tree and exchange it only after every file is written successfully.
fn remove_tree_entry(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn legacy_settings(overlay: &Path) -> (Option<String>, Option<String>) {
    if !fs::symlink_metadata(overlay)
        .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
    {
        return (None, None);
    }
    let read_plain = |name: &str| {
        // a seed cut at the limit is not the file the user wrote, so it is not written back
        class::read(&overlay.join(name))
            .ok()
            .filter(|read| read.whole)
            .map(|read| read.text)
    };
    let qt =
        read_plain("metadata.desktop").and_then(|text| qt_version_of(&text).map(|v| v.to_string()));
    let user = read_plain(USER).map(|text| user_ini(&text));
    (qt, user)
}

/*
 * an overlay that fails the root-owned, read-only check is moved whole to `<prefix>.legacy`, beside
 * it, and replaced by a fresh root-owned one. Never recover the user's copy. Read `QtVersion` and
 * `theme.conf.user` before the rename, since setup carries those settings over.
 */
fn migrate_legacy_prefix(w: &Where) -> Result<(Option<String>, Option<String>), String> {
    let prefix = w.prefix();
    let present = match fs::symlink_metadata(&prefix) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(format!(
                "{} is not a plain directory",
                class::plain(&prefix.display().to_string())
            ));
        }
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(format!(
                "{}: {error}",
                class::plain(&prefix.display().to_string())
            ));
        }
    };
    if !present || root_owned_read_only(&w.root, &prefix).is_ok() {
        return Ok((None, None));
    }

    let parent = prefix.parent().expect("prefix has a parent");
    root_owned_ancestors(&w.root, parent)?;
    let legacy = beside(&prefix, ".legacy");
    let settings = legacy_settings(&w.overlay(OWN));
    remove_tree_entry(&legacy)
        .map_err(|error| format!("{}: {error}", class::plain(&legacy.display().to_string())))?;
    fs::rename(&prefix, &legacy)
        .map_err(|error| format!("{}: {error}", class::plain(&prefix.display().to_string())))?;
    Ok(settings)
}

fn install_own(w: &Where) -> Result<(), String> {
    let overlay = w.overlay(OWN);
    let themes = overlay.parent().expect("theme has a parent");
    let prefix = w.prefix();
    let temp = beside(&overlay, ".new");
    let (legacy_qt, legacy_user) = migrate_legacy_prefix(w)?;

    if fs::symlink_metadata(&prefix).is_err() {
        make_dir(&prefix)
            .map_err(|error| format!("{}: {error}", class::plain(&prefix.display().to_string())))?;
    }
    root_owned_read_only(&w.root, &prefix)?;
    if fs::symlink_metadata(themes).is_err() {
        make_dir(themes)
            .map_err(|error| format!("{}: {error}", class::plain(&themes.display().to_string())))?;
    }
    root_owned_read_only(&w.root, themes)?;
    recover(&overlay)
        .map_err(|error| format!("{}: {error}", class::plain(&overlay.display().to_string())))?;

    let target_qt = legacy_qt
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| target_qt_version(w, &overlay));
    let previous_user = legacy_user.or_else(|| legacy_settings(&overlay).1);
    let user = preserve_user_settings(previous_user.as_deref().unwrap_or(""));
    match fs::symlink_metadata(&temp) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(&temp),
        Ok(_) => fs::remove_file(&temp),
        Err(_) => Ok(()),
    }
    .map_err(|error| format!("{}: {error}", class::plain(&temp.display().to_string())))?;
    make_dir(&temp)
        .map_err(|error| format!("{}: {error}", class::plain(&temp.display().to_string())))?;

    let built = (|| -> io::Result<()> {
        for (name, text) in THEME {
            let content = if *name == "metadata.desktop" {
                set_qt_version(text, target_qt)
            } else {
                (*text).to_owned()
            };
            atomic::write(&temp.join(name), content.as_bytes(), Mode::Exactly(0o644))?;
        }
        atomic::write(&temp.join(USER), user.as_bytes(), Mode::Exactly(0o644))
    })();
    if let Err(error) = built {
        let _ = fs::remove_dir_all(&temp);
        return Err(format!(
            "{}: {error}",
            class::plain(&overlay.display().to_string())
        ));
    }

    if fs::symlink_metadata(&overlay).is_ok() {
        if let Err(error) = clock::exchange(&temp, &overlay) {
            let _ = fs::remove_dir_all(&temp);
            return Err(format!(
                "{}: atomic theme exchange is unavailable; active theme was left unchanged: {error}",
                class::plain(&overlay.display().to_string())
            ));
        }
        let _ = fs::remove_dir_all(&temp);
    } else {
        fs::rename(&temp, &overlay).map_err(|error| {
            format!("{}: {error}", class::plain(&overlay.display().to_string()))
        })?;
    }
    Ok(())
}

// ---- the commands

struct Wanted {
    theme: String,
    background: SddmBackground,
    path: Option<PathBuf>,
    blur: u32,
    dim: u32,
}

impl Wanted {
    fn of(config: &Config) -> Result<Wanted, String> {
        let theme = config.sddm_theme.clone().unwrap_or_else(|| OWN.to_owned());

        if !theme_ok(&theme) {
            return Err(format!(
                "sddm.theme {theme}: a theme's directory name, letters, digits, _ . and -"
            ));
        }

        Ok(Wanted {
            theme,
            background: config.sddm_background,
            path: config.sddm_background_path.as_ref().map(PathBuf::from),
            blur: config.sddm_blur,
            dim: config.sddm_dim,
        })
    }
}

// what SDDM's config files say of `ThemeDir` and `Current`, with the file that wins each
fn pointed(w: &Where) -> (Said, Said) {
    let (mut dir, mut current) = (None, None);

    for file in w.configs() {
        if let Some(text) = class::text(&file) {
            read_theme(&text, &file, &mut dir, &mut current);
        }
    }

    (dir, current)
}

/*
 * whether SDDM reads the overlay's `theme`, else why not: what the winning file says, and when that
 * is not Kanade's own, which file it is and that it must change. A file read after Kanade's can only
 * be edited, not outdone by a name
 */
fn reads(w: &Where, prefix: &str, theme: &str) -> Result<(), (Vec<String>, bool)> {
    let (dir, current) = pointed(w);
    let themes = format!("{prefix}/themes");
    let mut said = Vec::new();

    // whether writing Kanade's file helps: not when only a file read after it is in the way
    let mut writes = false;

    for (key, found, wanted) in [
        ("ThemeDir", &dir, themes.as_str()),
        ("Current", &current, theme),
    ] {
        match found {
            Some((value, _)) if value == wanted => {}
            Some((value, file)) if w.after_ours(file) => {
                said.push(format!(
                    "{key}={value} wins, from {}, which SDDM reads after Kanade's {CONF}; remove or change its [Theme] {key}",
                    class::plain(&file.display().to_string())
                ));
            }
            Some((value, _)) => {
                writes = true;
                said.push(format!("{key}={value}, not {wanted}"));
            }
            None => {
                writes = true;
                said.push(format!("{key} is not set (SDDM's default)"));
            }
        }
    }

    if said.is_empty() {
        Ok(())
    } else {
        Err((said, writes))
    }
}

fn sizes(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / f64::from(1 << 20))
}

// the line `on_stderr` writes: `message` filtered, as a path in it may carry a control character
// whose escape sequence would rewrite the line around it
fn stderr_line(message: &str) -> String {
    format!("kanade: sddm: {}", class::plain(message))
}

// a message on stderr, which is a terminal's. The one seam every stderr line here goes through, so
// a new one cannot print its path raw
fn on_stderr(message: &str) {
    eprintln!("{}", stderr_line(message));
}

fn fail(error: &str) -> ExitCode {
    on_stderr(error);
    ExitCode::FAILURE
}

// the overlay's `theme` as `status` words it: whether what is there is what `sync` would write
fn theme_state(w: &Where, theme: &str) -> String {
    let overlay = w.overlay(theme);

    if theme == OWN {
        if root_owned_read_only(&w.root, &w.prefix().join("themes")).is_err()
            || root_owned_read_only(&w.root, &overlay).is_err()
        {
            return String::from("missing or not root-owned");
        }
        let target_qt = target_qt_version(w, &overlay);
        return match THEME
            .iter()
            .filter(|(name, text)| {
                let expected = if *name == "metadata.desktop" {
                    std::borrow::Cow::Owned(set_qt_version(text, target_qt))
                } else {
                    std::borrow::Cow::Borrowed(*text)
                };
                class::text(&overlay.join(name)).as_deref() != Some(expected.as_ref())
            })
            .count()
        {
            0 => String::from("current"),
            stale => format!("{stale} of {} files missing or out of date", THEME.len()),
        };
    }

    match plan(&w.installed(theme)) {
        Ok(wanted) if farm_current(&overlay, &wanted) => String::from("current"),
        Ok(_) => String::from("missing or out of date"),
        Err(error) => format!("the installed theme cannot be read: {error}"),
    }
}

// the class of `theme`, read from the installed theme's files
fn class_of(w: &Where, theme: &str) -> Class {
    class::classify(&w.installed(theme), theme == OWN)
}

// a thing still to do for the login screen
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    // a command for root, which Kanade never runs
    Root(String),

    // `kanade sddm sync`, which the user runs
    Sync,
}

impl Step {
    // the command, to copy
    pub fn command(&self) -> &str {
        match self {
            Step::Root(command) => command,
            Step::Sync => "kanade sddm sync",
        }
    }
}

// what `status` finds: its lines and the root commands still missing
pub struct Status {
    // `key: value` lines, in the order `status` prints them
    pub lines: Vec<String>,

    // what is left to do, in the order to do it: the conf step is last, after the sync that
    // makes the theme it names, and guards itself besides
    pub steps: Vec<Step>,

    // a file SDDM reads after Kanade's is in the way, which only editing it helps
    pub blocked: bool,

    // SDDM reads the overlay's theme
    pub enabled: bool,

    // the overlay files and mutable data are ready for the selected theme
    pub ready: bool,
}

impl Status {
    fn render(&self) -> String {
        let mut text = String::new();

        for line in &self.lines {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str(&self.then());

        text
    }

    // the root commands, or that there are none
    pub fn then(&self) -> String {
        let mut text = String::new();

        if self.steps.is_empty() && self.blocked {
            text.push_str(
                "as root: edit the file named above, which no command of Kanade's can outdo\n",
            );
        } else if self.steps.is_empty() {
            text.push_str(
                "nothing to do as root; `kanade sddm sync` writes only the backdrop data\n",
            );
        } else {
            text.push_str("in this order (Kanade never runs the commands for root):\n");
            for step in &self.steps {
                match step {
                    Step::Root(command) => {
                        let _ = writeln!(text, "  as root: {command}");
                    }
                    Step::Sync => text.push_str("  kanade sddm sync\n"),
                }
            }
        }

        text
    }
}

fn gather(w: &Where, config: &Config) -> Result<Status, String> {
    let want = Wanted::of(config)?;
    let prefix = w.text()?;
    let mut lines: Vec<String> = Vec::new();
    let mut steps: Vec<Step> = Vec::new();
    let mut blocked = false;

    lines.push(format!("theme: {} (sddm.theme)", want.theme));
    lines.push(format!(
        "background: {} (sddm.background)",
        want.background.name()
    ));

    match root_owned_read_only(&w.root, &w.prefix()).and_then(|()| {
        if w.prefix().join("themes").is_dir() && w.overlay(OWN).is_dir() {
            root_owned_read_only(&w.root, &w.prefix().join("themes"))
                .and_then(|()| root_owned_read_only(&w.root, &w.overlay(OWN)))
        } else {
            Ok(())
        }
    }) {
        Ok(()) => lines.push(format!(
            "executable themes: {prefix}/themes root-owned and read-only"
        )),
        Err(error) => {
            lines.push(format!("executable themes: {error}"));
            steps.push(Step::Root(make_prefix(&prefix)));
        }
    }
    match owned(&w.data()) {
        Ok(()) => lines.push(format!(
            "mutable data: {} ready",
            class::plain(&w.data().display().to_string())
        )),
        Err(error) => lines.push(format!("mutable data: {error}")),
    }

    let overlay = w.overlay(&want.theme);
    let class = class_of(w, &want.theme);
    let supported = want.theme == OWN;
    let made = if want.theme == OWN {
        root_owned_read_only(&w.root, &w.prefix()).is_ok()
            && root_owned_read_only(&w.root, &w.prefix().join("themes")).is_ok()
            && root_owned_read_only(&w.root, &overlay).is_ok()
    } else {
        false
    };

    let state = theme_state(w, &want.theme);
    lines.push(format!("theme class: {}", class.brief()));
    lines.push(format!("theme files: {state}"));
    if want.theme != OWN {
        lines.push(String::from(OTHER_THEMES));
        if !class.takes_picture() {
            lines.push(format!("note: {}", class.does()));
        }
    }

    // the sddm user is neither the owner nor in its group
    match modes(&w.prefix().join("themes"), false) {
        Ok(closed) if closed.is_empty() => {}
        Ok(closed) => lines.push(format!(
            "modes: {} entries sddm cannot read, as {}; root must fix executable theme permissions",
            closed.len(),
            class::plain(&closed[0].display().to_string())
        )),
        Err(error) => lines.push(format!("modes: {error}")),
    }

    lines.push(match fs::metadata(w.picture()) {
        Ok(meta) => format!("picture: {}", sizes(meta.len())),
        Err(_) => String::from("picture: none"),
    });

    let user = class::read(&overlay.join(USER));
    let names_backdrop = user.as_ref().is_ok_and(|read| {
        read.whole
            && class::pairs(&read.text).iter().any(|(_, key, value)| {
                key == "backdrop" && value == &format!("/{DATA}/backdrop.png")
            })
    });
    let written = if want.theme == OWN {
        names_backdrop
    } else {
        user.is_ok()
    };
    lines.push(format!(
        "{USER}: {}",
        match &user {
            Ok(_) if want.theme == OWN && !names_backdrop => {
                format!("stale: does not name /{DATA}/backdrop.png")
            }
            Ok(_) => String::from("written"),
            Err(_) => String::from("none"),
        }
    ));

    // setup is required before SDDM is pointed at the executable theme
    let current = made && state == "current" && written;
    if !current {
        if (root_owned_read_only(&w.root, &w.prefix()).is_err()
            || (w.prefix().join("themes").is_dir()
                && root_owned_read_only(&w.root, &w.prefix().join("themes")).is_err()))
            && !steps
                .iter()
                .any(|step| matches!(step, Step::Root(command) if command == &make_prefix(&prefix)))
        {
            steps.push(Step::Root(make_prefix(&prefix)));
        }
        if want.theme == OWN {
            steps.push(Step::Root(String::from("kanade sddm setup")));
            steps.push(Step::Sync);
        } else {
            blocked = true;
        }
    }

    let mut enabled = false;
    match reads(w, &prefix, &want.theme) {
        Ok(()) => {
            enabled = true;
            lines.push(format!("sddm: reads the overlay, theme {}", want.theme));
            if !current {
                lines.push(format!(
                    "sddm: WARNING it reads the overlay, whose theme {} is not current; run `kanade sddm setup` as root, then `kanade sddm sync`, or the login screen may fall back to SDDM's default",
                    want.theme
                ));
            }
        }
        Err((why, writes)) => {
            for line in why {
                lines.push(format!("sddm: {line}"));
            }
            if writes && supported {
                steps.push(Step::Root(point_sddm(w, &want.theme)?));
            } else if !writes {
                blocked = true;
            }
        }
    }

    let ready = steps.is_empty() && !blocked && state == "current" && owned(&w.data()).is_ok();

    Ok(Status {
        lines,
        steps,
        blocked,
        enabled,
        ready,
    })
}

// `status` for the Settings page, which shows its lines and the root commands to copy
pub fn overview(config: &Config) -> Result<Status, String> {
    gather(&Where::here(), config)
}

fn report(w: &Where, config: &Config) -> Result<String, String> {
    gather(w, config).map(|status| status.render())
}

// a line of `kanade doctor`'s about the login screen
pub struct Finding {
    // false is a warning
    pub ok: bool,
    pub text: String,
    pub detail: Vec<String>,
}

impl Finding {
    fn ok(text: impl Into<String>) -> Finding {
        Finding {
            ok: true,
            text: text.into(),
            detail: Vec::new(),
        }
    }

    fn warn(text: impl Into<String>, detail: Vec<String>) -> Finding {
        Finding {
            ok: false,
            text: text.into(),
            detail,
        }
    }
}

// the doctor's checks of the overlay: whether it is there, SDDM reads it, and the sddm user can
pub fn findings(config: &Config) -> Vec<Finding> {
    diagnose(&Where::here(), config)
}

fn diagnose(w: &Where, config: &Config) -> Vec<Finding> {
    let (want, prefix) = match (Wanted::of(config), w.text()) {
        (Ok(want), Ok(prefix)) => (want, prefix),
        (Err(error), _) | (_, Err(error)) => {
            return vec![Finding::warn(format!("sddm: {error}"), Vec::new())];
        }
    };
    let shown = prefix.as_str();

    match fs::symlink_metadata(w.prefix()) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return vec![Finding::ok(format!(
                "sddm: no overlay at {shown}, so the login screen keeps SDDM's own theme (`kanade sddm status` prints the root command to make it)"
            ))];
        }
        _ => {}
    }
    if want.theme == OWN
        && let Err(error) = root_owned_read_only(&w.root, &w.prefix())
            .and_then(|()| root_owned_read_only(&w.root, &w.prefix().join("themes")))
            .and_then(|()| root_owned_read_only(&w.root, &w.overlay(OWN)))
    {
        return vec![Finding::warn(
            format!("sddm: executable themes are not safely installed: {error}"),
            vec![format!("as root: {}", make_prefix(shown))],
        )];
    }

    let mut out = Vec::new();
    let state = theme_state(w, &want.theme);
    if state == "current" {
        out.push(Finding::ok(format!(
            "sddm: the overlay's theme {} is current",
            want.theme
        )));
    } else {
        // only root installs the executable theme; `sync` writes the picture and never QML
        out.push(Finding::warn(
            format!("sddm: the overlay's theme {} is {state}", want.theme),
            if want.theme == OWN {
                vec![String::from("as root: kanade sddm setup")]
            } else {
                vec![format!(
                    "only the built-in kanade theme is installed root-owned; SDDM loads {} from the overlay as best effort",
                    want.theme
                )]
            },
        ));
    }

    match reads(w, shown, &want.theme) {
        Ok(()) => out.push(Finding::ok(format!(
            "sddm: reads the overlay's theme {} ({CONF} wins [Theme])",
            want.theme
        ))),
        Err((why, writes)) => {
            let mut detail = why;
            if writes
                && want.theme == OWN
                && let Ok(step) = point_sddm(w, &want.theme)
            {
                detail.push(format!("as root: {step}"));
            }
            out.push(Finding::warn(
                "sddm: does not read the overlay's theme",
                detail,
            ));
        }
    }

    // the sddm user is neither the owner nor in the group: the overlay and the installed theme it links
    let mut closed: Vec<PathBuf> = Vec::new();
    let mut unchecked: Vec<String> = Vec::new();
    for dir in [
        Some(w.prefix().join("themes")),
        Some(w.data()),
        // a removed installed theme is said once, above; checking its modes again only adds a
        // warning about a directory that is gone
        (want.theme != OWN && w.installed(&want.theme).exists()).then(|| w.installed(&want.theme)),
    ]
    .into_iter()
    .flatten()
    {
        match modes(&dir, false) {
            Ok(found) => closed.extend(found),
            Err(error) => unchecked.push(format!(
                "{}: {error}",
                class::plain(&dir.display().to_string())
            )),
        }
    }
    if !unchecked.is_empty() {
        out.push(Finding::warn(
            "sddm: the modes could not be checked, so whether the sddm user can read the theme is unknown",
            unchecked,
        ));
    } else if closed.is_empty() {
        out.push(Finding::ok(
            "sddm: the sddm user can read the theme (modes)",
        ));
    } else {
        // `apply` mends the overlay's own modes, so this is a command that works, and the doctor
        // itself writes nothing
        out.push(Finding::warn(
            format!(
                "sddm: {} entries the sddm user cannot read, as {}",
                closed.len(),
                class::plain(&closed[0].display().to_string())
            ),
            vec![String::from(
                "`kanade sddm sync` mends the overlay's; an installed theme's are its package's",
            )],
        ));
    }

    out
}

/*
 * a login screen that SDDM reads through the overlay while the overlay's theme is out of date (the
 * installed theme was upgraded or removed, so the links lead nowhere), which can leave it on SDDM's
 * default or without a theme. None when all is well or SDDM does not read the overlay
 */
fn stale(w: &Where, config: &Config) -> Option<String> {
    let want = Wanted::of(config).ok()?;
    let prefix = w.text().ok()?;

    fs::symlink_metadata(w.prefix()).ok()?;
    reads(w, &prefix, &want.theme).ok()?;

    let state = theme_state(w, &want.theme);
    (state != "current").then(|| {
        format!(
            "SDDM reads the overlay, whose theme {} is {state}; run `kanade sddm setup` as root, then `kanade sddm sync`. If the login screen is already broken, from a text console: sudo rm /etc/sddm.conf.d/{CONF}, then restart sddm",
            want.theme
        )
    })
}

// says once, on stderr, if `stale`; on a thread of its own, as it reads files
pub fn check_at_start() {
    let spawned = std::thread::Builder::new()
        .name(String::from("sddm check"))
        .spawn(|| {
            if let Some(note) = stale(&Where::here(), &crate::config::get()) {
                on_stderr(&note);
            }
        });

    if let Err(error) = spawned {
        on_stderr(&format!("cannot check the overlay: {error}"));
    }
}

pub fn status() -> ExitCode {
    let w = Where::here();

    match report(&w, &config::get()) {
        Ok(text) => cli::print(&text, ExitCode::SUCCESS),
        Err(error) => fail(&error),
    }
}

/*
 * holds the lock that makes the commands writing the overlay take turns (`kanade sddm sync`, the
 * shell following the wallpaper, `use` and `enable`) until the guard drops. A blocking exclusive
 * flock on a file in the user's state directory, never one in the overlay, which the login screen's
 * user reads. Not a link: the file is opened without following one
 */
fn hold(w: &Where) -> Result<Option<File>, String> {
    let Some(path) = &w.lock else {
        return Ok(None);
    };
    let said = |error: io::Error| format!("{}: {error}", class::plain(&path.display().to_string()));

    if let Some(dir) = path.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(said)?;
    }

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .map_err(said)?;
    file.lock().map_err(said)?;

    Ok(Some(file))
}

// runs `write` holding the lock, so two writers of the overlay take turns
fn locked<T>(w: &Where, write: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let _held = hold(w)?;

    write()
}

fn source_of(want: &Wanted) -> Option<PathBuf> {
    match want.background {
        SddmBackground::Wallpaper => wallpaper::showing_now(),
        SddmBackground::Path => want.path.clone(),
        SddmBackground::Solid | SddmBackground::None => None,
    }
}

pub fn setup() -> ExitCode {
    if getuid().as_raw() != 0 {
        return fail("`kanade sddm setup` must run as root");
    }
    let w = Where::here();
    match install_own(&w) {
        Ok(()) => cli::print(
            "kanade SDDM theme installed root-owned and read-only to the desktop user\n",
            ExitCode::SUCCESS,
        ),
        Err(error) => fail(&error),
    }
}

pub fn sync() -> ExitCode {
    let w = Where::here();
    let config = config::get();
    let done = locked(&w, || {
        let mut text = apply(&w, &config, source_of, largest_output)?;

        text.push_str(&pointing(&w, &Wanted::of(&config)?.theme)?);
        Ok(text)
    });

    match done {
        Ok(text) => cli::print(&text, ExitCode::SUCCESS),
        Err(error) => fail(&error),
    }
}

/*
 * whether `path` may be replaced by a picture: absent, or a plain file with no other name. A
 * link would send the write (or the user's idea of the file) somewhere else, and a file with other
 * names is one the user did not make for this
 */
pub(super) fn plain_target(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(format!(
            "{} is a symbolic link; remove it, and Kanade writes the file",
            class::plain(&path.display().to_string())
        )),
        Ok(meta) if !meta.is_file() => Err(format!(
            "{} is not a file",
            class::plain(&path.display().to_string())
        )),
        Ok(meta) if meta.nlink() > 1 => Err(format!(
            "{} has {} names (a hard link); remove it, and Kanade writes the file",
            class::plain(&path.display().to_string()),
            meta.nlink()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "{}: {error}",
            class::plain(&path.display().to_string())
        )),
    }
}

// the picture at `path`, replaced whole: what the login screen reads is never half written
pub(super) fn put_picture(path: &Path, bytes: &[u8]) -> Result<(), String> {
    plain_target(path)?;

    atomic::write(path, bytes, Mode::Exactly(0o644))
        .map_err(|error| format!("{}: {error}", class::plain(&path.display().to_string())))
}

/*
 * gives the overlay's own entries the modes the sddm user reads them with, and says which it mended.
 * Only what is Kanade's to write: `setup` runs as root and leaves the tree root-owned, so an entry it
 * cannot set is the user's, and the error says so rather than a mode change that failed halfway
 */
fn mend(w: &Where) -> Result<Vec<PathBuf>, String> {
    let mut mended = Vec::new();

    for dir in [w.prefix(), w.prefix().join("themes"), w.data()] {
        match fs::symlink_metadata(&dir) {
            Ok(_) => mended.extend(modes(&dir, true).map_err(|error| {
                format!("{}: {error}", class::plain(&dir.display().to_string()))
            })?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "{}: {error}",
                    class::plain(&dir.display().to_string())
                ));
            }
        }
    }

    Ok(mended)
}

/*
 * the directories of the overlay `sync` and the shell write into, checked: the data directory is the
 * user's and closed to others, and the executable theme tree is root-owned, with the built-in theme in
 * it. `sync` never writes into the theme tree; `setup`, as root, is what makes it
 */
fn ready(w: &Where, prefix: &str) -> Result<(), String> {
    let data = w.data();
    if !data.exists() {
        make_dir(&data)
            .map_err(|error| format!("{}: {error}", class::plain(&data.display().to_string())))?;
    }
    if let Err(error) = owned(&data) {
        return Err(format!(
            "{error}; as root: {}\nthen run this again (`kanade sddm status` says more)",
            make_prefix(prefix)
        ));
    }

    // the executable theme tree must be root-owned and not writable by the desktop user
    let themes = w.prefix().join("themes");
    root_owned_read_only(&w.root, &themes)
        .map_err(|error| format!("{error}; run `kanade sddm setup` as root"))?;
    if !w.overlay(OWN).join("metadata.desktop").is_file() {
        return Err(format!(
            "{} is missing; run `kanade sddm setup` as root",
            class::plain(&w.overlay(OWN).display().to_string())
        ));
    }

    // a link in the way would send a write somewhere else
    if !data.exists() {
        make_dir(&data)
            .map_err(|error| format!("{}: {error}", class::plain(&data.display().to_string())))?;
    }
    owned(&data)?;

    Ok(())
}

/*
 * writes the picture in the data directory and `theme.conf.user` in the overlay. Security
 * constraint: sync never writes QML, theme code, or executable paths; only data picture and overlay
 * config, mending overlay file modes so the sddm user can read them (ADR 0120)
 */
fn apply(
    w: &Where,
    config: &Config,
    source: impl FnOnce(&Wanted) -> Option<PathBuf>,
    size: impl FnOnce() -> (u32, u32),
) -> Result<String, String> {
    let want = Wanted::of(config)?;
    let prefix = w.text()?;
    if want.theme != OWN {
        return Err(String::from(
            "user sync supports only the root-installed kanade theme",
        ));
    }
    ready(w, &prefix)?;

    let mut text = String::new();
    match mend(w) {
        Ok(mended) if mended.is_empty() => {}
        Ok(mended) => {
            let _ = writeln!(text, "modes: mended {} entries", mended.len());
        }
        Err(error) => {
            let _ = writeln!(text, "modes: not mended ({error})");
        }
    }

    let bytes = match want.background {
        SddmBackground::Wallpaper => source(&want)
            .map(|path| backdrop(&path, size(), want.blur, want.dim))
            .transpose()?,
        SddmBackground::Path => {
            let path = source(&want).ok_or_else(|| {
                String::from("sddm.background is \"path\" but sddm.background_path is not set")
            })?;
            Some(backdrop(&path, size(), want.blur, want.dim)?)
        }
        SddmBackground::Solid | SddmBackground::None => None,
    };

    let shown = w.picture();
    match bytes {
        Some(bytes) => {
            put_picture(&shown, &bytes)?;
            let _ = writeln!(
                text,
                "picture: {} ({})",
                class::plain(&shown.display().to_string()),
                sizes(bytes.len() as u64)
            );
        }
        None => {
            plain_target(&shown)?;
            match fs::remove_file(&shown) {
                Ok(()) => {
                    let _ = writeln!(
                        text,
                        "picture: removed {}",
                        class::plain(&shown.display().to_string())
                    );
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    text.push_str("picture: none\n");
                }
                Err(error) => {
                    return Err(format!(
                        "{}: {error}",
                        class::plain(&shown.display().to_string())
                    ));
                }
            }
        }
    }

    Ok(text)
}

/*
 * whether SDDM reads the overlay's `theme` yet, in words: a line that it does, or why it does not
 * and the root command that makes it. `pointing` is what the commands that write the overlay end on
 */
fn pointing(w: &Where, theme: &str) -> Result<String, String> {
    let prefix = w.text()?;
    let mut text = String::new();

    match reads(w, &prefix, theme) {
        Ok(()) => {
            let _ = writeln!(text, "SDDM reads the overlay, theme {theme}: enabled");
        }
        Err((why, writes)) => {
            for line in why {
                let _ = writeln!(text, "SDDM does not read it yet: {line}");
            }
            if writes {
                let _ = writeln!(text, "as root: {}", point_sddm(w, theme)?);
            }
        }
    }

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = "/usr/local/share/kanade-sddm";

    // the same cases as the theme's harness (`dist/sddm/preview/harness.qml`, "bg-" scenarios)
    #[test]
    fn a_backdrop_is_only_read_from_the_overlay() {
        let path = |rest: &str| format!("{REAL}/{rest}");

        for ok in ["bg/backdrop.png", "bg/a-b_c.1.webp", "x.jpg", ".png"] {
            assert!(backdrop_ok(REAL, &path(ok)), "{ok}");
        }

        for bad in [
            "bg/../backdrop.png",
            "bg//backdrop.png",
            "bg/backdrop.svg",
            "bg/back drop.png",
            "bg/backdrop.PNG",
            "bg/backdrop.png/",
            "",
            "..",
        ] {
            assert!(!backdrop_ok(REAL, &path(bad)), "{bad}");
        }

        for bad in [
            "/etc/passwd.png",
            "/usr/local/share/kanade-sddm-evil/bg/backdrop.png",
            "/usr/local/share/kanade-sddm/../../../etc/shadow.png",
            "/usr/local/share/kanade-sddm/",
            "/usr/local/share/kanade-sddm",
            "file:///usr/local/share/kanade-sddm/bg/backdrop.png",
            "usr/local/share/kanade-sddm/bg/backdrop.png",
            "data/backdrop.png",
        ] {
            assert!(!backdrop_ok(REAL, bad), "{bad}");
        }

        let long = path(&format!("{}.png", "a".repeat(120)));
        assert!(long.len() > 128 && !backdrop_ok(REAL, &long));
        let longest = path(&format!("{}.png", "a".repeat(128 - REAL.len() - 1 - 4)));
        assert!(longest.len() == 128 && backdrop_ok(REAL, &longest));
    }

    // `THEME` must be the built-in theme's directory whole: a part left out of it is not written by
    // `copy_own` and not counted by `theme_state`, so `sync` would install a theme that fails to load
    // and still call the overlay current
    #[test]
    fn every_part_of_the_built_in_theme_is_installed() {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/dist/sddm/kanade"));
        let mut on_disk: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        let mut listed: Vec<String> = THEME.iter().map(|(name, _)| String::from(*name)).collect();

        on_disk.sort();
        listed.sort();

        assert_eq!(listed, on_disk);
    }

    #[test]
    fn the_theme_conf_user_holds_only_values_the_theme_reads() {
        let picture = format!("/{DATA}/backdrop.png");

        let own = conf_user(REAL, "kanade", Some(&picture), None, "background").unwrap();
        assert!(own.starts_with("# Written by"));
        assert_eq!(
            own.lines().skip(1).collect::<Vec<_>>(),
            ["[General]", &format!("backdrop={picture}")]
        );

        // no picture: an empty backdrop, so a stale one in theme.conf cannot show
        let bare = conf_user(REAL, "kanade", None, None, "background").unwrap();
        assert_eq!(
            bare.lines().skip(1).collect::<Vec<_>>(),
            ["[General]", &format!("backdrop=/{DATA}/backdrop.png")]
        );

        // another theme gets the key most take a picture from, and nothing without one
        let other = conf_user(REAL, "breeze", Some(&picture), None, "background").unwrap();
        assert_eq!(
            other.lines().skip(1).collect::<Vec<_>>(),
            ["[General]", &format!("background={picture}")]
        );
        assert_eq!(
            conf_user(REAL, "breeze", None, None, "background")
                .unwrap()
                .lines()
                .skip(1)
                .collect::<Vec<_>>(),
            ["[General]"]
        );

        // the installed theme's own file stays but for the one key, wherever it sits
        let installed = "[Colors]\nbackground=keep\n\n[General]\nbackground=/usr/share/x.png\nfont=Fira\n\n[Other]\nx=1\n";
        let merged = conf_user(
            REAL,
            "breeze",
            Some(&picture),
            Some(installed),
            "background",
        )
        .unwrap();
        assert_eq!(
            merged.lines().skip(1).collect::<Vec<_>>(),
            [
                "[Colors]",
                "background=keep",
                "",
                "[General]",
                "font=Fira",
                &format!("background={picture}"),
                "",
                "[Other]",
                "x=1"
            ]
        );
        assert_eq!(user_ini(installed), installed);
        // a line that is none of those goes, and the settings around it stay
        assert_eq!(
            user_ini("[General]\nfont=Fira\nnot a setting\nshade=1\n"),
            "[General]\nfont=Fira\nshade=1\n"
        );
        // over the limit, whole lines are kept up to it rather than the file going whole
        let long = format!("[General]\nfont={}\n", "a".repeat(70 << 10));
        let kept = user_ini(&long);
        assert!(kept.len() <= 64 << 10 && kept == "[General]\n", "{kept:?}");

        // a value that could add a line or that the theme would refuse is not written
        for (theme, picture) in [
            ("kanade", Some("/etc/passwd.png")),
            ("kanade", Some(format!("{picture}\nshade=1").as_str())),
            ("../x", None),
            ("a\nb", None),
            ("", None),
        ] {
            assert!(conf_user(REAL, theme, picture, None, "background").is_err());
        }
    }

    // the trap: a user's own `theme.conf` in /etc/sddm.conf.d sorts after `10-` but before `zz-`
    #[test]
    fn the_file_read_last_decides_which_theme_sddm_reads() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-reads-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("etc/sddm.conf.d");
        fs::create_dir_all(&dir).unwrap();
        let w = Where::under(root.clone());
        let prefix = w.text().unwrap();
        let ours = format!("[Theme]\nThemeDir={prefix}/themes\nCurrent=kanade\n");

        fs::write(dir.join("theme.conf"), "[Theme]\nCurrent=ninesols\n").unwrap();
        fs::write(dir.join(CONF), &ours).unwrap();
        assert_eq!(reads(&w, &prefix, "kanade"), Ok(()));

        // sorting after ours, a file wins, and is named
        fs::write(dir.join("zzz-late.conf"), "[Theme]\nCurrent=ninesols\n").unwrap();
        let (why, _) = reads(&w, &prefix, "kanade").unwrap_err();
        assert!(
            why.len() == 1 && why[0].contains("zzz-late.conf"),
            "{why:?}"
        );

        // and /etc/sddm.conf is read after every file of the directory
        fs::remove_file(dir.join("zzz-late.conf")).unwrap();
        fs::write(
            root.join("etc/sddm.conf"),
            "[Theme]\nThemeDir=/usr/share/sddm/themes\n",
        )
        .unwrap();
        let (why, _) = reads(&w, &prefix, "kanade").unwrap_err();
        assert!(
            why.len() == 1 && why[0].contains("etc/sddm.conf,"),
            "{why:?}"
        );

        fs::remove_dir_all(&root).unwrap();
    }

    // the doctor is silent before an overlay exists; afterward it warns if SDDM cannot read it
    // or does not read the overlay.
    #[test]
    fn the_doctor_warns_of_an_overlay_sddm_cannot_use() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-doctor-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let w = Where::under(root.clone());
        let config = Config::default();

        let before = diagnose(&w, &config);
        assert_eq!(before.len(), 1);
        assert!(before[0].ok, "{}", before[0].text);

        fs::create_dir_all(w.prefix()).unwrap();
        fs::set_permissions(w.prefix(), Permissions::from_mode(0o755)).unwrap();
        fs::create_dir_all(w.prefix().join("themes")).unwrap();
        fs::set_permissions(w.prefix().join("themes"), Permissions::from_mode(0o755)).unwrap();
        fs::create_dir_all(w.overlay(OWN)).unwrap();
        fs::set_permissions(w.overlay(OWN), Permissions::from_mode(0o755)).unwrap();
        fs::write(w.overlay(OWN).join("Main.qml"), "").unwrap();
        fs::set_permissions(
            w.overlay(OWN).join("Main.qml"),
            Permissions::from_mode(0o600),
        )
        .unwrap();

        let found = diagnose(&w, &config);
        let warned: Vec<&str> = found
            .iter()
            .filter(|finding| !finding.ok)
            .map(|finding| finding.text.as_str())
            .collect();
        assert!(
            warned
                .iter()
                .any(|text| text.contains("not safely installed")),
            "{warned:?}"
        );

        // a prefix others can write is refused outright, as the writers refuse it
        fs::set_permissions(w.prefix(), Permissions::from_mode(0o777)).unwrap();
        let found = diagnose(&w, &config);
        assert!(
            found
                .iter()
                .any(|finding| { !finding.ok && finding.text.contains("not safely installed") })
        );

        fs::set_permissions(w.prefix(), Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    // what is printed is in an order that cannot leave SDDM on a theme that is not there: the
    // overlay, then the sync that makes the theme, then the conf that names it, which also checks
    // for the theme itself
    #[test]
    fn the_conf_step_comes_after_the_sync_and_guards_itself() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-order-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let w = Where::under(root.clone());
        let config = Config::default();
        let conf = |step: &Step| matches!(step, Step::Root(c) if c.contains(CONF));
        let shape = |status: &Status| -> Vec<&'static str> {
            status
                .steps
                .iter()
                .map(|step| match step {
                    Step::Root(c) if c.contains(CONF) => "conf",
                    Step::Root(_) => "prefix",
                    Step::Sync => "sync",
                })
                .collect()
        };

        // nothing made
        let status = gather(&w, &config).unwrap();
        assert_eq!(shape(&status), ["prefix", "prefix", "sync", "conf"]);
        let guarded = status
            .steps
            .iter()
            .find(|step| conf(step))
            .unwrap()
            .command();
        assert!(
            guarded.starts_with(&format!(
                "test -f {}/themes/kanade/metadata.desktop && printf",
                w.text().unwrap()
            )),
            "{guarded}"
        );

        // made, not synced
        fs::create_dir_all(w.prefix()).unwrap();
        fs::set_permissions(w.prefix(), Permissions::from_mode(0o755)).unwrap();
        fs::create_dir_all(w.prefix().join("themes")).unwrap();
        fs::set_permissions(w.prefix().join("themes"), Permissions::from_mode(0o755)).unwrap();
        install_own(&w).unwrap();
        let config = Config {
            sddm_background: SddmBackground::None,
            ..config
        };
        assert_eq!(shape(&gather(&w, &config).unwrap()), ["conf"]);

        // synced
        apply(&w, &config, |_| None, || (16, 16)).unwrap();
        assert_eq!(shape(&gather(&w, &config).unwrap()), ["conf"]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wallpaper_sync_without_a_wallpaper_keeps_the_theme_enabled_without_a_picture() {
        let root =
            std::env::temp_dir().join(format!("kanade-sddm-no-wallpaper-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let w = Where::under(root.clone());
        fs::create_dir_all(w.prefix()).unwrap();
        fs::set_permissions(w.prefix(), Permissions::from_mode(0o755)).unwrap();
        fs::create_dir_all(w.prefix().join("themes")).unwrap();
        fs::set_permissions(w.prefix().join("themes"), Permissions::from_mode(0o755)).unwrap();
        install_own(&w).unwrap();
        let config = Config::default();

        let result = apply(&w, &config, |_| None, || (16, 16)).unwrap();
        assert!(result.contains("picture: none"), "{result}");
        assert!(!w.picture().exists());
        assert!(
            fs::read_to_string(w.overlay(OWN).join(USER))
                .unwrap()
                .contains(&format!("backdrop=/{DATA}/backdrop.png"))
        );
        fs::create_dir_all(w.ours().parent().unwrap()).unwrap();
        fs::write(
            w.ours(),
            format!(
                "[Theme]\nThemeDir={}/themes\nCurrent=kanade\n",
                w.text().unwrap()
            ),
        )
        .unwrap();
        assert!(pointing(&w, OWN).unwrap().contains("enabled"));

        let explicit_path = Config {
            sddm_background: SddmBackground::Path,
            ..config
        };
        assert!(
            apply(&w, &explicit_path, |_| None, || (16, 16))
                .unwrap_err()
                .contains("background_path is not set")
        );

        fs::remove_dir_all(root).unwrap();
    }

    // SDDM pointed at the overlay with the theme gone is said at start, with the way out
    #[test]
    fn a_stale_overlay_that_sddm_reads_is_said() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-stale-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let w = Where::under(root.clone());
        let config = Config::default();

        fs::create_dir_all(w.prefix()).unwrap();
        fs::set_permissions(w.prefix(), Permissions::from_mode(0o755)).unwrap();
        fs::create_dir_all(w.prefix().join("themes")).unwrap();
        fs::set_permissions(w.prefix().join("themes"), Permissions::from_mode(0o755)).unwrap();
        install_own(&w).unwrap();
        let config = Config {
            sddm_background: SddmBackground::None,
            ..config
        };
        apply(&w, &config, |_| None, || (16, 16)).unwrap();

        // SDDM does not read the overlay: nothing to say of a stale one
        assert_eq!(stale(&w, &config), None);

        let ours = w.ours();
        fs::create_dir_all(ours.parent().unwrap()).unwrap();
        fs::write(
            &ours,
            format!(
                "[Theme]\nThemeDir={}/themes\nCurrent=kanade\n",
                w.text().unwrap()
            ),
        )
        .unwrap();
        assert_eq!(stale(&w, &config), None, "current");

        fs::remove_file(w.overlay(OWN).join("Main.qml")).unwrap();
        let note = stale(&w, &config).unwrap();
        assert!(
            note.contains("kanade sddm sync")
                && note.contains("rm /etc/sddm.conf.d/zz-kanade.conf"),
            "{note}"
        );

        fs::remove_dir_all(root).unwrap();
    }

    // the doctor said `kanade sddm sync` mends modes, so it does: the overlay's own entries are given
    // the bits the sddm user reads them with, and what is not Kanade's is said, not changed
    #[test]
    fn sync_mends_the_overlay_modes_it_says_it_does() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-mend-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let w = Where::under(root.clone());
        fs::create_dir_all(w.prefix()).unwrap();
        fs::set_permissions(w.prefix(), Permissions::from_mode(0o755)).unwrap();
        fs::create_dir_all(w.prefix().join("themes")).unwrap();
        fs::set_permissions(w.prefix().join("themes"), Permissions::from_mode(0o755)).unwrap();
        install_own(&w).unwrap();

        let script = w.overlay(OWN).join("Main.qml");
        fs::set_permissions(&script, Permissions::from_mode(0o600)).unwrap();
        assert_eq!(modes(&w.prefix(), false).unwrap().len(), 1, "closed first");

        let mended = mend(&w).unwrap();
        assert_eq!(mended, vec![script.clone()]);
        assert_eq!(
            fs::metadata(&script).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert!(modes(&w.prefix(), false).unwrap().is_empty());

        // a second run has nothing to mend
        assert!(mend(&w).unwrap().is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    // an installed theme that is a link is checked through it, and one that cannot be reached is
    // an error rather than a pass; a pipe where a file should be is never read
    #[test]
    fn modes_follow_a_linked_theme_and_a_pipe_is_never_read() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("real")).unwrap();
        fs::write(root.join("real/Main.qml"), "").unwrap();
        fs::set_permissions(root.join("real/Main.qml"), Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(root.join("real"), Permissions::from_mode(0o755)).unwrap();
        symlink(root.join("real"), root.join("linked")).unwrap();
        symlink(root.join("nowhere"), root.join("dangling")).unwrap();

        assert_eq!(
            modes(&root.join("linked"), false).unwrap(),
            vec![root.join("linked/Main.qml")]
        );
        assert!(modes(&root.join("dangling"), false).is_err());

        // mending never reaches through the top link
        let mended = modes(&root.join("linked"), true);
        assert!(mended.is_err() || mended.unwrap().is_empty());
        assert_eq!(
            fs::metadata(root.join("real/Main.qml"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let pipe = root.join("theme.conf.user");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &pipe,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        assert!(class::read(&pipe).is_err(), "a pipe is not a file");
        symlink("/dev/zero", root.join("zero")).unwrap();
        assert!(class::read(&root.join("zero")).is_err());

        fs::remove_dir_all(root).unwrap();
    }

    // a run that died between the farm's two renames leaves the overlay missing and the last working
    // theme in `.old`: the next run must put that back, not sweep it away as a leftover
    #[test]
    fn a_farm_that_finds_no_overlay_puts_the_one_it_left_aside_back() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-farm-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let overlay = root.join("themes/kanade");
        let (new, old) = (beside(&overlay, ".new"), beside(&overlay, ".old"));
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("a"), "a").unwrap();
        fs::write(source.join("b"), "b").unwrap();

        // the state one interrupted run leaves: the old overlay aside, a half-built new one, no overlay
        fs::create_dir_all(&overlay).unwrap();
        fs::write(overlay.join("Main.qml"), "the last working theme").unwrap();
        fs::write(overlay.join(USER), "font=Fira\n").unwrap();
        fs::rename(&overlay, &old).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("half"), "").unwrap();

        // a plan that cannot be built: a link under a link is ENOTDIR, so the overlay is left unbuilt
        let unbuildable = [
            (PathBuf::from("a"), Some(source.join("a"))),
            (PathBuf::from("a/b"), Some(source.join("b"))),
        ];

        assert!(farm(&overlay, &unbuildable, "font=Fira\n").is_err());
        assert!(fs::symlink_metadata(&overlay).is_ok_and(|meta| meta.is_dir()));
        assert_eq!(
            fs::read_to_string(overlay.join("Main.qml")).unwrap(),
            "the last working theme"
        );
        assert_eq!(
            fs::read_to_string(overlay.join(USER)).unwrap(),
            "font=Fira\n"
        );
        assert!(fs::symlink_metadata(&new).is_err());
        assert!(fs::symlink_metadata(&old).is_err());

        // an overlay that is there still wins: a stale `.old` is removed, not put back
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("stale"), "").unwrap();
        let plan = [(PathBuf::from("a"), Some(source.join("a")))];

        farm(&overlay, &plan, "font=Fira\n").unwrap();
        assert!(fs::symlink_metadata(&old).is_err());
        assert!(!overlay.join("stale").exists());
        assert!(overlay.join("a").exists());
        assert_eq!(
            fs::read_to_string(overlay.join(USER)).unwrap(),
            "font=Fira\n"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_farm_over_an_existing_overlay_leaves_the_new_content_with_no_leftovers() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-atomic-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let overlay = root.join("themes/kanade");
        let (new, old) = (beside(&overlay, ".new"), beside(&overlay, ".old"));
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("a"), "a").unwrap();
        fs::write(source.join("b"), "b").unwrap();

        // an overlay with one file
        fs::create_dir_all(&overlay).unwrap();
        fs::write(overlay.join("Main.qml"), "old").unwrap();
        fs::write(overlay.join(USER), "font=Old\n").unwrap();

        let plan = [(PathBuf::from("a"), Some(source.join("a")))];
        farm(&overlay, &plan, "font=New\n").unwrap();

        // the overlay holds the new content
        assert!(overlay.join("a").exists());
        assert_eq!(
            fs::read_to_string(overlay.join(USER)).unwrap(),
            "font=New\n"
        );
        // no .new or .old left
        assert!(fs::symlink_metadata(&new).is_err());
        assert!(fs::symlink_metadata(&old).is_err());

        fs::remove_dir_all(root).unwrap();
    }

    // after a successful exchange the .new path holds whatever the overlay held, which may be a
    // symlink; remove_dir_all does not remove a symlink, so the removal must fall back to
    // remove_file. This builds that case without live SDDM.
    #[test]
    fn a_farm_over_a_symlink_overlay_removes_the_stale_new_after_exchange() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-symlink-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let overlay = root.join("themes/kanade");
        let new = beside(&overlay, ".new");
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("a"), "a").unwrap();

        // the overlay is a symlink to a real directory (as a store-path theme might be)
        let real = root.join("real-overlay");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("Main.qml"), "old").unwrap();
        fs::write(real.join(USER), "font=Old\n").unwrap();
        fs::create_dir_all(overlay.parent().unwrap()).unwrap();
        symlink(&real, &overlay).unwrap();

        let plan = [(PathBuf::from("a"), Some(source.join("a")))];
        farm(&overlay, &plan, "font=New\n").unwrap();

        // the overlay is now a real directory with the new content
        assert!(overlay.join("a").exists());
        assert_eq!(
            fs::read_to_string(overlay.join(USER)).unwrap(),
            "font=New\n"
        );
        // no .new left (the symlink that exchange put there was removed)
        assert!(
            fs::symlink_metadata(&new).is_err(),
            ".new was not cleaned up"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_overlay_is_readable_by_others_under_a_restrictive_umask() {
        use rustix::fs::Mode as Bits;
        use rustix::process::umask;

        const NAME: &str =
            "sddm::tests::the_overlay_is_readable_by_others_under_a_restrictive_umask";

        if std::env::var_os("KANADE_UMASK_CHILD").is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--test-threads=1"])
                .env("KANADE_UMASK_CHILD", "1")
                .status()
                .unwrap();

            assert!(status.success());
            return;
        }

        let root = std::env::temp_dir().join(format!("kanade-sddm-umask-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        umask(Bits::from_bits_truncate(0o077));

        let w = Where::under(root.clone());
        let bits = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;

        // setup runs under a restrictive umask and must still leave executable files readable.
        fs::create_dir_all(w.prefix()).unwrap();
        fs::set_permissions(w.prefix(), Permissions::from_mode(0o755)).unwrap();
        let themes = w.prefix().join("themes");
        fs::create_dir_all(&themes).unwrap();
        fs::set_permissions(&themes, Permissions::from_mode(0o755)).unwrap();
        install_own(&w).unwrap();
        assert_eq!(bits(&w.overlay(OWN)), 0o755);
        assert_eq!(bits(&w.overlay(OWN).join("Main.qml")), 0o644);
        assert_eq!(bits(&w.overlay(OWN).join("theme.conf")), 0o644);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn setup_preserves_unmanaged_user_settings_and_replaces_managed_values() {
        let root =
            std::env::temp_dir().join(format!("kanade-sddm-preserve-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let w = Where::under(root.clone());
        let overlay = w.overlay(OWN);
        fs::create_dir_all(&overlay).unwrap();
        fs::set_permissions(&overlay, Permissions::from_mode(0o755)).unwrap();
        fs::write(
            overlay.join(USER),
            "font=BeforeSection\nbackdrop=/tmp/unsafe-before-section.png\n[General]\nfont=Fira\nBACKDROP=/tmp/unsafe.png\nshade=1\n[Other]\nbackdrop=keep-this\n",
        )
        .unwrap();
        fs::set_permissions(overlay.join(USER), Permissions::from_mode(0o644)).unwrap();
        fs::write(
            overlay.join("metadata.desktop"),
            "[SddmGreeterTheme]\nQtVersion=5\n",
        )
        .unwrap();
        fs::set_permissions(
            overlay.join("metadata.desktop"),
            Permissions::from_mode(0o644),
        )
        .unwrap();
        fs::create_dir_all(overlay.parent().unwrap()).unwrap();
        fs::set_permissions(overlay.parent().unwrap(), Permissions::from_mode(0o755)).unwrap();
        fs::create_dir_all(w.prefix()).unwrap();
        fs::set_permissions(w.prefix(), Permissions::from_mode(0o755)).unwrap();

        install_own(&w).unwrap();
        let written = fs::read_to_string(overlay.join(USER)).unwrap();
        assert!(written.contains("font=Fira"));
        assert!(written.contains("font=BeforeSection"));
        assert!(written.contains("[Other]\nbackdrop=keep-this"));
        assert!(!written.contains("unsafe-before-section.png"));
        assert!(!written.contains("BACKDROP=/tmp/unsafe.png"));
        assert!(!written.contains("shade=1"));
        assert!(written.ends_with(&format!("[General]\nbackdrop=/{DATA}/backdrop.png\n")));
        assert_eq!(
            qt_version_of(&fs::read_to_string(overlay.join("metadata.desktop")).unwrap()),
            Some(5)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn root_theme_validation_checks_nested_file_modes_and_symlinks() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-tree-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let nested = root.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::set_permissions(&root, Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&nested, Permissions::from_mode(0o755)).unwrap();
        let script = nested.join("Main.qml");
        fs::write(&script, "// theme").unwrap();
        fs::set_permissions(&script, Permissions::from_mode(0o600)).unwrap();
        assert!(root_owned_read_only(&root, &root).is_err());
        fs::set_permissions(&script, Permissions::from_mode(0o644)).unwrap();
        assert!(root_owned_read_only(&root, &root).is_ok());
        let link = nested.join("link");
        symlink(&script, &link).unwrap();
        assert!(root_owned_read_only(&root, &root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn qt_version_parsing_and_updating() {
        let meta = "[SddmGreeterTheme]\nName=Kanade\nQtVersion=6\n";
        assert_eq!(qt_version_of(meta), Some(6));

        let updated = set_qt_version(meta, 5);
        assert_eq!(qt_version_of(&updated), Some(5));
        assert!(updated.contains("QtVersion=5"));

        let no_ver = "[SddmGreeterTheme]\nName=Kanade\n";
        assert_eq!(qt_version_of(no_ver), None);
        let added = set_qt_version(no_ver, 6);
        assert_eq!(qt_version_of(&added), Some(6));

        let meta_spaces = "[SddmGreeterTheme]\nName=Kanade\nQtVersion = 5\n";
        assert_eq!(qt_version_of(meta_spaces), Some(5));
        let updated_spaces = set_qt_version(meta_spaces, 6);
        assert_eq!(qt_version_of(&updated_spaces), Some(6));
        assert_eq!(updated_spaces.matches("QtVersion").count(), 1);
    }

    #[test]
    fn sync_preserves_user_configured_qt5_metadata() {
        let root =
            std::env::temp_dir().join(format!("kanade-sddm-qt5setup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let w = Where::under(root.clone());
        let overlay = w.overlay(OWN);
        fs::create_dir_all(&overlay).unwrap();
        fs::create_dir_all(overlay.parent().unwrap()).unwrap();
        fs::set_permissions(overlay.parent().unwrap(), Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&overlay, Permissions::from_mode(0o755)).unwrap();

        let user_meta = "[SddmGreeterTheme]\nName=Kanade\nQtVersion=5\n";
        fs::write(overlay.join("metadata.desktop"), user_meta).unwrap();

        copy_own(&w, &overlay).unwrap();
        let content = fs::read_to_string(overlay.join("metadata.desktop")).unwrap();
        assert_eq!(
            qt_version_of(&content),
            Some(5),
            "preserves user-configured QtVersion=5"
        );
        assert_eq!(theme_state(&w, OWN), "current");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn greeter_version_detection_from_root() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-greeter-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let bin = root.join("usr/bin");
        fs::create_dir_all(&bin).unwrap();
        let w = Where::under(root.clone());

        let qt5_bin = bin.join("sddm-greeter");
        fs::write(&qt5_bin, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&qt5_bin, Permissions::from_mode(0o755)).unwrap();

        assert_eq!(installed_greeter_version(&w), Some(5));

        let qt6_bin = bin.join("sddm-greeter-qt6");
        fs::write(&qt6_bin, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&qt6_bin, Permissions::from_mode(0o755)).unwrap();

        assert_eq!(installed_greeter_version(&w), Some(6));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn installed_greeter_version_alternate_root_isolation() {
        let root =
            std::env::temp_dir().join(format!("kanade-sddm-isolated-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let w = Where::under(root.clone());
        assert_eq!(installed_greeter_version(&w), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn target_qt_version_recovers_to_qt6_when_greeter_upgraded() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-upgrade-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let bin = root.join("usr/bin");
        fs::create_dir_all(&bin).unwrap();
        let w = Where::under(root.clone());
        let overlay = w.overlay(OWN);
        fs::create_dir_all(&overlay).unwrap();

        // Existing overlay has QtVersion=5
        let user_meta = "[SddmGreeterTheme]\nName=Kanade\nQtVersion=5\n";
        fs::write(overlay.join("metadata.desktop"), user_meta).unwrap();

        // Greeter on system is Qt6
        let qt6_bin = bin.join("sddm-greeter-qt6");
        fs::write(&qt6_bin, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&qt6_bin, Permissions::from_mode(0o755)).unwrap();

        assert_eq!(installed_greeter_version(&w), Some(6));
        assert_eq!(target_qt_version(&w, &overlay), 6);

        let _ = fs::remove_dir_all(&root);
    }

    // setup writes metadata.desktop for the greeter it finds, so a Qt 5 SDDM needs no hand edit
    #[test]
    fn setup_writes_the_qt_version_of_the_greeter_it_finds() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-qt5find-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let bin = root.join("usr/bin");
        fs::create_dir_all(&bin).unwrap();
        let greeter = bin.join("sddm-greeter");
        fs::write(&greeter, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&greeter, Permissions::from_mode(0o755)).unwrap();
        let w = Where::under(root.clone());
        fs::create_dir_all(w.prefix()).unwrap();
        fs::set_permissions(w.prefix(), Permissions::from_mode(0o755)).unwrap();

        install_own(&w).unwrap();
        let meta = fs::read_to_string(w.overlay(OWN).join("metadata.desktop")).unwrap();
        assert_eq!(qt_version_of(&meta), Some(5), "{meta}");

        fs::remove_dir_all(root).unwrap();
    }

    // the theme's own files carry the data path the Rust side writes to, so the two cannot drift
    #[test]
    fn the_theme_names_the_data_path_the_rust_side_writes() {
        let path = format!("/{DATA}/backdrop.png");

        for (name, text) in THEME {
            if *name == "Sanitize.js" || *name == "theme.conf" {
                assert!(text.contains(&path), "{name} names {path}");
            }
        }
    }

    // a theme of more entries than a theme can hold is refused while it is read, not after
    #[test]
    fn a_directory_over_the_entry_cap_is_refused_as_it_is_read() {
        let dir = std::env::temp_dir().join(format!("kanade-sddm-many-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for n in 0..4001 {
            fs::write(dir.join(format!("file{n}")), "").unwrap();
        }

        let error = plan(&dir).unwrap_err();
        assert!(error.contains("too large for a theme"), "{error}");

        fs::remove_dir_all(dir).unwrap();
    }

    // a removed installed theme is said once; its modes are not checked again as a directory gone
    #[test]
    fn the_doctor_does_not_check_the_modes_of_a_removed_installed_theme() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-gone-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let w = Where::under(root.clone());
        fs::create_dir_all(w.prefix()).unwrap();
        fs::set_permissions(w.prefix(), Permissions::from_mode(0o755)).unwrap();
        fs::create_dir_all(w.prefix().join("themes")).unwrap();
        fs::set_permissions(w.prefix().join("themes"), Permissions::from_mode(0o755)).unwrap();
        fs::create_dir_all(w.data()).unwrap();
        fs::set_permissions(w.data(), Permissions::from_mode(0o755)).unwrap();
        fs::create_dir_all(w.overlay("maya")).unwrap();
        fs::set_permissions(w.overlay("maya"), Permissions::from_mode(0o755)).unwrap();

        let config = Config {
            sddm_theme: Some(String::from("maya")),
            ..Config::default()
        };
        let found = diagnose(&w, &config);
        let said: Vec<&str> = found.iter().map(|finding| finding.text.as_str()).collect();

        assert!(
            said.iter()
                .any(|text| text.contains("installed theme cannot be read")),
            "{said:?}"
        );
        assert!(
            !said
                .iter()
                .any(|text| text.contains("modes could not be checked")),
            "{said:?}"
        );

        fs::remove_dir_all(root).unwrap();
    }

    // the error a theme directory or mode file named with an escape reaches stderr through: one line,
    // and nothing in it moves the cursor. `fail` and the start check both go through `stderr_line`,
    // so this is the whole class and not the two sites that were caught
    #[test]
    fn a_path_on_stderr_cannot_rewrite_the_line() {
        let said = stderr_line("/etc/sddm.conf.d/a\u{1b}[2Jb: permission denied\nsudo rm -rf ~");

        assert_eq!(
            said,
            "kanade: sddm: /etc/sddm.conf.d/a[2Jb: permission deniedsudo rm -rf ~"
        );
        assert!(!said.chars().any(char::is_control), "{said:?}");
        // the prefix is Kanade's own, outside the filter, so it is there whatever the message says
        assert_eq!(stderr_line("\u{1b}"), "kanade: sddm: ");
    }
}
