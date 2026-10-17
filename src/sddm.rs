//! `kanade sddm`: the login screen's look, written from the config by a command that runs without a
//! shell.
//!
//! SDDM reads its theme as the `sddm` user, so what it reads lives in an overlay Kanade owns, outside
//! `/usr/share`: `<prefix>/themes/<theme>` (for Kanade's own theme a copy of the one built into this
//! binary, for another a directory of links to the installed theme's files, both with a
//! `theme.conf.user` of Kanade's) and `<prefix>/bg/backdrop.png`. The prefix is made once, by root;
//! Kanade only prints those commands, never runs sudo and never writes outside what the user owns.
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
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use kanade_lock::Picture;
use rustix::fs::OFlags;
use rustix::process::{getgid, getuid};

use self::class::Class;
use crate::atomic::{self, Mode};
use crate::cli;
use crate::config::{self, Config};
use crate::look::SddmBackground;
use crate::sources::json::Json;
use crate::sources::{niri, wallpaper};

// where the overlay lives: fixed, since the theme reads a picture from here and nowhere else
const PREFIX: &str = "usr/local/share/kanade-sddm";

// where themes are installed, which SDDM's `ThemeDir` defaults to
const INSTALLED: &str = "usr/share/sddm/themes";

// Kanade's own theme, by its directory name
const OWN: &str = "kanade";

/*
 * the root everything above is under, `/` but for a test (the hidden `KANADE_SDDM_ROOT`, an
 * absolute directory), so a run never touches `/usr/local` or `/etc`
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

    fn picture(&self) -> PathBuf {
        self.prefix().join("bg/backdrop.png")
    }

    // SDDM's own config, in the order it reads it (sddm.conf(5)), later winning: the system's
    // directory, the local one, then /etc/sddm.conf. A directory's files go by name, ignoring case
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
fn hex_ok(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/*
 * the `theme.conf.user` for `theme`. Kanade's own theme reads `backdrop` (a picture already
 * blurred and dimmed, so no shade over it), `surface` (the color without one); its file is Kanade's
 * whole. Any other theme reads the picture from `key` (`background` for most, told by its class): its
 * file is the installed theme's own `theme.conf.user` (`installed`, already checked by `user_ini`)
 * with that one key replaced when there is a picture, so what the user set for the theme stays. A value the theme
 * would refuse is an error here, not a file that says nothing.
 */
fn conf_user(
    prefix: &str,
    theme: &str,
    picture: Option<&str>,
    surface: Option<&str>,
    installed: Option<&str>,
    key: &str,
) -> Result<String, String> {
    if !theme_ok(theme) {
        return Err(format!("{theme}: not a theme's directory name"));
    }
    if let Some(path) = picture
        && !backdrop_ok(prefix, path)
    {
        return Err(format!("{path}: the theme would not read a picture there"));
    }
    if let Some(color) = surface
        && !hex_ok(color)
    {
        return Err(format!("{color}: the surface is #rrggbb"));
    }

    if theme != OWN {
        return Ok(with_background(installed.unwrap_or(""), key, picture));
    }

    let mut text = String::from(
        "# Written by `kanade sddm sync`; the next run replaces the whole file.\n[General]\n",
    );

    let _ = writeln!(text, "backdrop={}", picture.unwrap_or(""));
    if picture.is_some() {
        text.push_str("shade=0\n");
    }
    if let Some(color) = surface {
        let _ = writeln!(text, "surface={color}");
    }

    Ok(text)
}

// whether `text` is an ini file whole: sections, `key=value`, comments and blank lines, nothing else
fn user_ini(text: &str) -> Result<(), String> {
    if text.len() > 64 << 10 {
        return Err(String::from("over 64 KiB"));
    }

    for (n, line) in class::unbom(text).lines().map(str::trim).enumerate() {
        let section = line.starts_with('[') && line.ends_with(']');
        let comment = line.starts_with('#') || line.starts_with(';');
        let pair = line.contains('=') && !line.starts_with('[');

        if !(line.is_empty() || section || comment || pair) {
            return Err(format!(
                "line {} is not a section, a key or a comment",
                n + 1
            ));
        }
    }

    Ok(())
}

/*
 * the ini `base` with `key` set to `picture` in `[General]` (where the keys before any section are
 * too), and every other line as it was. Without a picture Kanade manages nothing and `base` goes
 * through. A byte order mark first is dropped, which would otherwise hide the first section
 */
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
 * the picture at `source` as a png to show behind the login screen: decoded as the lock screen's
 * wallpaper is (the first frame, a drawing rasterised, none too large), covering `size`, blurred
 * `blur` pixels and dimmed `dim` percent as the lock screen does, then encoded afresh so none of
 * the original's metadata is left. Smaller by steps if it comes over `MOST_BYTES`. The source is only
 * read, so a link to it is followed (a wallpaper is often one) and the file it leads to must be a
 * regular file; every write refuses links
 */
fn backdrop(source: &Path, size: (u32, u32), blur: u32, dim: u32) -> Result<Vec<u8>, String> {
    let named = source.display();
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
        "{named}: still over {} MiB at a third of the screen's size",
        MOST_BYTES >> 20
    ))
}

// ---- the overlay

// the overlay's directory is Kanade's: a directory the user owns, not a link, nobody else writes to
fn owned(path: &Path) -> Result<(), String> {
    let meta =
        fs::symlink_metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;

    if !meta.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    if meta.uid() != getuid().as_raw() {
        return Err(format!("{} is not yours", path.display()));
    }
    if meta.permissions().mode() & 0o022 != 0 {
        return Err(format!("{} is writable by others", path.display()));
    }

    Ok(())
}

// the root commands that make the overlay, which only the user may run
fn make_prefix(prefix: &str) -> String {
    format!(
        "install -d -o {} -g {} -m 755 {prefix} {prefix}/themes {prefix}/bg",
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
        w.ours().display()
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
            return Err(format!("{}: too large for a theme", dir.display()));
        }

        let mut entries: Vec<_> = fs::read_dir(dir)
            .map_err(|error| format!("{}: {error}", dir.display()))?
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
 * makes the overlay of an installed theme: its directories and links to its files, with `user` as
 * its `USER`. It is built beside the overlay as `<theme>.new` and moved into place, so a failure
 * partway leaves the overlay that was there; a crash between the two renames leaves the last
 * working theme in `<theme>.old`, which the next run puts back before anything else. The overlay is
 * Kanade's, and is replaced whole
 */
fn farm(overlay: &Path, wanted: &[(PathBuf, Option<PathBuf>)], user: &str) -> Result<(), String> {
    let said = |error: io::Error| format!("{}: {error}", overlay.display());
    let (new, old) = (beside(overlay, ".new"), beside(overlay, ".old"));

    // a run that died between the two renames left the overlay gone and the last working theme in
    // `.old`: put it back before the cleanup below would remove it
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

    // two renames, the first aside: a directory is not renamed over one that holds something
    let aside = fs::symlink_metadata(overlay).is_ok();
    if aside && let Err(error) = fs::rename(overlay, &old) {
        let _ = fs::remove_dir_all(&new);
        return Err(said(error));
    }
    if let Err(error) = fs::rename(&new, overlay) {
        if aside {
            let _ = fs::rename(&old, overlay);
        }
        let _ = fs::remove_dir_all(&new);
        return Err(said(error));
    }
    let _ = fs::remove_dir_all(&old);

    Ok(())
}

// said of any theme but Kanade's, which is built to be read from here
const OTHER_THEMES: &str = "note: a theme other than kanade is best effort; one that imports from a sibling or parent directory or names /usr/share/sddm/themes paths may not work from the overlay (check `kanade sddm status`, and the login screen)";

// the installed theme's own `theme.conf.user`, which the overlay's starts from: nothing when there is
// none, or when it cannot be read (said in `text`) by the sddm user or as an ini file
fn installed_user(installed: &Path, text: &mut String) -> Option<String> {
    let at = installed.join(USER);
    let meta = match fs::metadata(&at) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            let _ = writeln!(text, "{}: {error}; starting empty", at.display());
            return None;
        }
    };

    // a file the sddm user cannot read is no setting of the theme's to carry into a file it can
    let read = if !meta.is_file() || meta.permissions().mode() & 0o004 == 0 {
        Err(String::from("not a file everyone can read"))
    } else {
        class::read(&at)
            .map(|read| class::unbom(&read).to_owned())
            .ok_or_else(|| String::from("not a file of a reasonable size"))
            .and_then(|read| user_ini(&read).map(|()| read))
    };

    match read {
        Ok(read) => Some(read),
        Err(why) => {
            let _ = writeln!(
                text,
                "{}: cannot be used as it is ({why}); starting empty",
                at.display()
            );
            None
        }
    }
}

// Kanade's theme into its overlay; the files that already read the same are left alone
fn copy_own(overlay: &Path) -> Result<usize, String> {
    let mut written = 0;

    for (name, text) in THEME {
        let path = overlay.join(name);

        if class::read(&path).is_some_and(|have| have == *text) {
            continue;
        }

        atomic::write(&path, text.as_bytes(), Mode::Exactly(0o644))
            .map_err(|error| format!("{}: {error}", path.display()))?;
        written += 1;
    }

    Ok(written)
}

// ---- the commands

struct Wanted {
    theme: String,
    background: SddmBackground,
    path: Option<PathBuf>,
    surface: Option<String>,
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
        if let Some(color) = &config.sddm_surface
            && !hex_ok(color)
        {
            return Err(format!("sddm.surface {color}: expected #rrggbb"));
        }

        Ok(Wanted {
            theme,
            background: config.sddm_background,
            path: config.sddm_background_path.as_ref().map(PathBuf::from),
            surface: config.sddm_surface.clone(),
            blur: config.sddm_blur,
            dim: config.sddm_dim,
        })
    }
}

// what SDDM's config files say of `ThemeDir` and `Current`, with the file that wins each
fn pointed(w: &Where) -> (Said, Said) {
    let (mut dir, mut current) = (None, None);

    for file in w.configs() {
        if let Some(text) = class::read(&file) {
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
                    file.display()
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

fn fail(error: &str) -> ExitCode {
    eprintln!("kanade: sddm: {error}");
    ExitCode::FAILURE
}

// the overlay's `theme` as `status` words it: whether what is there is what `sync` would write
fn theme_state(w: &Where, theme: &str) -> String {
    let overlay = w.overlay(theme);

    if theme == OWN {
        return match THEME
            .iter()
            .filter(|(name, text)| {
                !class::read(&overlay.join(name)).is_some_and(|have| have == *text)
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
            text.push_str("nothing to do as root; `kanade sddm sync` writes the look\n");
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

    match owned(&w.prefix()) {
        Ok(()) => lines.push(format!("overlay: {prefix} ready")),
        Err(error) => {
            lines.push(format!("overlay: {error}"));
            steps.push(Step::Root(make_prefix(&prefix)));
        }
    }

    let overlay = w.overlay(&want.theme);
    let class = class_of(w, &want.theme);
    let made = steps.is_empty();

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
    match modes(&w.prefix(), false) {
        Ok(closed) if closed.is_empty() => {}
        Ok(closed) => lines.push(format!(
            "modes: {} entries sddm cannot read, as {}; `kanade sddm sync` mends them",
            closed.len(),
            closed[0].display()
        )),
        Err(error) => lines.push(format!("modes: {error}")),
    }

    lines.push(match fs::metadata(w.picture()) {
        Ok(meta) => format!("picture: {}", sizes(meta.len())),
        Err(_) => String::from("picture: none"),
    });

    let written = class::read(&overlay.join(USER)).is_some();
    lines.push(format!(
        "{USER}: {}",
        if written { "written" } else { "none" }
    ));

    // the theme must be in the overlay before SDDM is pointed at it
    let current = made && state == "current" && written;
    if !current {
        steps.push(Step::Sync);
    }

    let mut enabled = false;
    match reads(w, &prefix, &want.theme) {
        Ok(()) => {
            enabled = true;
            lines.push(format!("sddm: reads the overlay, theme {}", want.theme));
            if !current {
                lines.push(format!(
                    "sddm: WARNING it reads the overlay, whose theme {} is not current; run `kanade sddm sync` or the login screen may fall back to SDDM's default",
                    want.theme
                ));
            }
        }
        Err((why, writes)) => {
            for line in why {
                lines.push(format!("sddm: {line}"));
            }
            if writes {
                steps.push(Step::Root(point_sddm(w, &want.theme)?));
            } else {
                blocked = true;
            }
        }
    }

    Ok(Status {
        lines,
        steps,
        blocked,
        enabled,
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
    if let Err(error) = owned(&w.prefix()) {
        return vec![Finding::warn(
            format!("sddm: the overlay cannot be written: {error}"),
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
        out.push(Finding::warn(
            format!("sddm: the overlay's theme {} is {state}", want.theme),
            vec![String::from("run `kanade sddm sync`")],
        ));
    }

    match reads(w, shown, &want.theme) {
        Ok(()) => out.push(Finding::ok(format!(
            "sddm: reads the overlay's theme {} ({CONF} wins [Theme])",
            want.theme
        ))),
        Err((why, writes)) => {
            let mut detail = why;
            if writes && let Ok(step) = point_sddm(w, &want.theme) {
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
        Some(w.prefix()),
        (want.theme != OWN).then(|| w.installed(&want.theme)),
    ]
    .into_iter()
    .flatten()
    {
        match modes(&dir, false) {
            Ok(found) => closed.extend(found),
            Err(error) => unchecked.push(format!("{}: {error}", dir.display())),
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
        out.push(Finding::warn(
            format!(
                "sddm: {} entries the sddm user cannot read, as {}",
                closed.len(),
                closed[0].display()
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
            "SDDM reads the overlay, whose theme {} is {state}; run `kanade sddm sync`. If the login screen is already broken, from a text console: sudo rm /etc/sddm.conf.d/{CONF}, then restart sddm",
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
                eprintln!("kanade: sddm: {note}");
            }
        });

    if let Err(error) = spawned {
        eprintln!("kanade: sddm: cannot check the overlay: {error}");
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
    let said = |error: io::Error| format!("{}: {error}", path.display());

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
fn plain_target(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(format!(
            "{} is a symbolic link; remove it, and Kanade writes the file",
            path.display()
        )),
        Ok(meta) if !meta.is_file() => Err(format!("{} is not a file", path.display())),
        Ok(meta) if meta.nlink() > 1 => Err(format!(
            "{} has {} names (a hard link); remove it, and Kanade writes the file",
            path.display(),
            meta.nlink()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

// the picture at `path`, replaced whole: what the login screen reads is never half written
fn put_picture(path: &Path, bytes: &[u8]) -> Result<(), String> {
    plain_target(path)?;

    atomic::write(path, bytes, Mode::Exactly(0o644))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/*
 * the directories of the overlay `sync` and the shell write into, checked: the prefix is yours and
 * closed to others, and `themes` and `bg` are directories, not links, made if absent
 */
fn ready(w: &Where, prefix: &str) -> Result<(), String> {
    if let Err(error) = owned(&w.prefix()) {
        return Err(format!(
            "{error}; as root: {}\nthen run this again (`kanade sddm status` says more)",
            make_prefix(prefix)
        ));
    }

    // a link in the way would send a write somewhere else
    for dir in [w.prefix().join("themes"), w.prefix().join("bg")] {
        match fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(format!("{} is not a directory", dir.display())),
            Err(_) => make_dir(&dir).map_err(|error| format!("{}: {error}", dir.display()))?,
        }
    }

    Ok(())
}

/*
 * writes the look the config asks for: the theme into the overlay, the picture (or none) and
 * `theme.conf.user`. Everything that can fail on the picture does before anything is written, and the
 * picture is in place before the config that names it. A theme that reads no picture setting (class
 * B, C or D) is linked and gets none, so what it shows is its own
 */
fn apply(
    w: &Where,
    config: &Config,
    source: impl FnOnce(&Wanted) -> Option<PathBuf>,
    size: impl FnOnce() -> (u32, u32),
) -> Result<String, String> {
    let want = Wanted::of(config)?;
    let prefix = w.text()?;
    let mut text = String::new();

    ready(w, &prefix)?;
    // the sddm user reads all of it, so a mode that shuts it out is mended, not left to fail a login
    let mended =
        modes(&w.prefix(), true).map_err(|error| format!("{}: {error}", w.prefix().display()))?;
    if !mended.is_empty() {
        let _ = writeln!(text, "modes mended: {} entries", mended.len());
    }

    let installed = w.installed(&want.theme);
    let (wanted_files, class) = if want.theme == OWN {
        (None, Class::A(String::from("backdrop")))
    } else {
        if !installed.join("metadata.desktop").is_file() {
            return Err(format!(
                "{} is not an installed SDDM theme",
                installed.display()
            ));
        }

        (Some(plan(&installed)?), class_of(w, &want.theme))
    };

    let mut picture = None;
    if !class.takes_picture() {
        let _ = writeln!(text, "theme class {}: {}", class.brief(), class.does());
    } else if let Some(path) = source(&want) {
        picture = Some(backdrop(&path, size(), want.blur, want.dim)?);
        let _ = writeln!(text, "picture from {}", path.display());
    } else if want.background == SddmBackground::Wallpaper {
        text.push_str("no wallpaper is shown (awww): the solid color instead\n");
    } else if want.background == SddmBackground::Path {
        return Err(String::from(
            "sddm.background is \"path\" but sddm.background_path is not set",
        ));
    }

    let overlay = w.overlay(&want.theme);
    match fs::symlink_metadata(&overlay) {
        Ok(meta) if !meta.is_dir() => {
            return Err(format!("{} is not a directory", overlay.display()));
        }
        _ => {}
    }

    let shown = w.picture();
    match &picture {
        Some(bytes) => {
            put_picture(&shown, bytes)?;
            let _ = writeln!(
                text,
                "picture: {} ({})",
                shown.display(),
                sizes(bytes.len() as u64)
            );
        }
        None => {
            plain_target(&shown)?;
            match fs::remove_file(&shown) {
                Ok(()) => text.push_str("picture: removed\n"),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("{}: {error}", shown.display())),
            }
        }
    }

    // the picture is in place, so the theme and the config that name it can come
    let named = format!("{prefix}/bg/backdrop.png");
    let seed = if wanted_files.is_some() {
        installed_user(&installed, &mut text)
    } else {
        None
    };
    let key = match &class {
        Class::A(key) => key.as_str(),
        _ => "background",
    };
    let conf = conf_user(
        &prefix,
        &want.theme,
        picture.as_ref().map(|_| named.as_str()),
        want.surface.as_deref(),
        seed.as_deref(),
        key,
    )?;

    match &wanted_files {
        None => {
            make_dir(&overlay).map_err(|error| format!("{}: {error}", overlay.display()))?;
            let written = copy_own(&overlay)?;
            let _ = writeln!(text, "theme {}: {written} files written", want.theme);

            let at = overlay.join(USER);
            atomic::write(&at, conf.as_bytes(), Mode::Exactly(0o644))
                .map_err(|error| format!("{}: {error}", at.display()))?;
        }
        Some(wanted) => {
            farm(&overlay, wanted, &conf)?;
            let _ = writeln!(
                text,
                "theme {}: {} entries linked",
                want.theme,
                wanted.len()
            );
            text.push_str(OTHER_THEMES);
            text.push('\n');
        }
    }
    let _ = writeln!(text, "{USER}: written");

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
        let picture = format!("{REAL}/bg/backdrop.png");

        let own = conf_user(
            REAL,
            "kanade",
            Some(&picture),
            Some("#0c0d11"),
            None,
            "background",
        )
        .unwrap();
        assert!(own.starts_with("# Written by"));
        assert_eq!(
            own.lines().skip(1).collect::<Vec<_>>(),
            [
                "[General]",
                &format!("backdrop={picture}"),
                "shade=0",
                "surface=#0c0d11"
            ]
        );

        // no picture: an empty backdrop, so a stale one in theme.conf cannot show
        let bare = conf_user(REAL, "kanade", None, None, None, "background").unwrap();
        assert_eq!(
            bare.lines().skip(1).collect::<Vec<_>>(),
            ["[General]", "backdrop="]
        );

        // another theme gets the key most take a picture from, and nothing without one
        let other = conf_user(
            REAL,
            "breeze",
            Some(&picture),
            Some("#000000"),
            None,
            "background",
        )
        .unwrap();
        assert_eq!(
            other.lines().skip(1).collect::<Vec<_>>(),
            ["[General]", &format!("background={picture}")]
        );
        assert_eq!(
            conf_user(REAL, "breeze", None, None, None, "background")
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
            None,
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
        assert!(user_ini(installed).is_ok());
        assert!(user_ini("[General]\nnot a setting\n").is_err());

        // a value that could add a line or that the theme would refuse is not written
        for (theme, picture, surface) in [
            ("kanade", Some("/etc/passwd.png"), None),
            ("kanade", Some(format!("{picture}\nshade=1").as_str()), None),
            ("kanade", None, Some("#0c0d1\n")),
            ("kanade", None, Some("red")),
            ("../x", None, None),
            ("a\nb", None, None),
            ("", None, None),
        ] {
            assert!(conf_user(REAL, theme, picture, surface, None, "background").is_err());
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

    // the sddm user is no one the overlay belongs to: a person's umask must not shut it out. The
    // umask is the process's, so the check runs in a child of its own
    // the doctor says nothing is wrong before the overlay exists, and warns of an overlay the sddm
    // user cannot read or SDDM does not read
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
        fs::create_dir_all(w.overlay(OWN)).unwrap();
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
            warned.iter().any(|text| text.contains("does not read")),
            "{warned:?}"
        );
        assert!(
            warned.iter().any(|text| text.contains("cannot read")),
            "{warned:?}"
        );

        // a prefix others can write is refused outright, as the writers refuse it
        fs::set_permissions(w.prefix(), Permissions::from_mode(0o777)).unwrap();
        let found = diagnose(&w, &config);
        assert_eq!(found.len(), 1);
        assert!(!found[0].ok && found[0].text.contains("writable by others"));

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
        assert_eq!(shape(&status), ["prefix", "sync", "conf"]);
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
        assert_eq!(shape(&gather(&w, &config).unwrap()), ["sync", "conf"]);

        // synced
        apply(&w, &config, |_| None, || (16, 16)).unwrap();
        assert_eq!(shape(&gather(&w, &config).unwrap()), ["conf"]);

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
        assert_eq!(class::read(&pipe), None);
        symlink("/dev/zero", root.join("zero")).unwrap();
        assert_eq!(class::read(&root.join("zero")), None);

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

        // the prefix is root's `install -d -m 755`; a third-party theme with a nested directory and
        // a theme.conf.user of its own, made under the same umask
        fs::create_dir_all(w.prefix()).unwrap();
        fs::set_permissions(w.prefix(), Permissions::from_mode(0o755)).unwrap();
        let theme = w.installed("breeze");
        fs::create_dir_all(theme.join("sub")).unwrap();
        fs::write(theme.join("metadata.desktop"), "[SddmGreeterTheme]\n").unwrap();
        fs::write(theme.join("Main.qml"), "").unwrap();
        fs::write(theme.join("sub/a.qml"), "").unwrap();
        fs::write(
            theme.join(USER),
            "[General]\nfont=Fira\nbackground=/x.png\n",
        )
        .unwrap();
        fs::set_permissions(theme.join(USER), Permissions::from_mode(0o644)).unwrap();
        let source = root.join("source.png");
        let tiny = Picture {
            width: 8,
            height: 8,
            rgba: vec![200; 8 * 8 * 4],
        };
        fs::write(&source, encode(&tiny).unwrap()).unwrap();

        let mut config = Config {
            sddm_background: SddmBackground::Path,
            sddm_background_path: source.to_str().map(str::to_owned),
            ..Config::default()
        };

        let shut = |w: &Where| modes(&w.prefix(), false).unwrap();
        let sync = |config: &Config| {
            apply(&w, config, |want| want.path.clone(), || (16, 16)).unwrap();
            assert_eq!(shut(&w), Vec::<PathBuf>::new());
        };

        config.sddm_theme = Some(String::from("breeze"));
        sync(&config);
        let farmed = w.overlay("breeze");
        assert_eq!(bits(&farmed), 0o755);
        assert_eq!(bits(&farmed.join("sub")), 0o755);
        assert_eq!(bits(&farmed.join(USER)), 0o644);
        assert_eq!(bits(&w.picture()), 0o644);
        let conf = fs::read_to_string(farmed.join(USER)).unwrap();
        assert!(
            conf.contains("font=Fira") && conf.contains("background=") && !conf.contains("/x.png")
        );

        config.sddm_theme = None;
        sync(&config);
        assert_eq!(bits(&w.overlay(OWN).join("Main.qml")), 0o644);

        // no part of a swap is left beside the themes
        let left: Vec<_> = fs::read_dir(w.prefix().join("themes"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(left.len(), 2, "{left:?}");

        // a mode that was shut is said, then mended
        fs::set_permissions(w.picture(), Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(w.prefix().join("bg"), Permissions::from_mode(0o700)).unwrap();
        assert!(report(&w, &config).unwrap().contains("modes: 2 entries"));
        sync(&config);
        assert_eq!(bits(&w.picture()), 0o644);

        let _ = fs::remove_dir_all(&root);
    }
}
