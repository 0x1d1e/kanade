//! Kanade's own application color targets from the wallpaper scheme (ADR 0041): GTK3 and GTK4
//! CSS, qt5ct and qt6ct color schemes, a KDE color scheme, niri's window colors, kitty's colors,
//! and Pywalfox's colors for Firefox and Zen.
//!
//! Each generated file starts with `HEADER`, so a file of the user's at the same path is left
//! alone, never overwritten nor removed.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{SyncState, atomic_write, config_home, data_home, save_state};
use crate::appearance::pywalfox;
use crate::clock;
use crate::look::Tone;

const HEADER: &str = "Kanade appearance sync: wallpaper colors";
const GTK: &str = include_str!("templates/gtk.css");
const QT: &str = include_str!("templates/qtct.conf");
const KDE: &str = include_str!("templates/kde.colors");
const NIRI: &str = include_str!("templates/niri.kdl");
const KITTY: &str = include_str!("templates/kitty.conf");
// the lines that include Kanade's files, last in the user's config so their colors win
const NIRI_INCLUDE: &str = "include optional=true \"kanade.kdl\" // Kanade appearance sync";
const KITTY_INCLUDE: &str = "include kanade.conf";
// a kitty younger than this may not yet handle SIGUSR1, which would end it; it reads the new
// colors as it starts anyway
const KITTY_STARTING: u64 = 2;
// the KDE color scheme id, its file name without `.colors`
pub(super) const KDE_SCHEME: &str = "Kanade";
const GTK_DIRS: [&str; 2] = ["gtk-3.0", "gtk-4.0"];
const GTK_IMPORT: &str = "@import url(\"kanade.css\"); /* Kanade appearance sync */";

// every role Matugen gave for a wallpaper, in both modes, as `#rrggbb`
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Scheme {
    pub wallpaper: PathBuf,
    pub dark: BTreeMap<String, String>,
    pub light: BTreeMap<String, String>,
}

impl Scheme {
    fn roles(&self, tone: Tone) -> &BTreeMap<String, String> {
        match tone {
            Tone::Dark => &self.dark,
            Tone::Light => &self.light,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct SavedGtkCss {
    created: bool,

    // a gtk.css that was a symlink, as to a theme's own file, replaced by one importing it
    #[serde(default)]
    link: Option<PathBuf>,
}

pub(super) fn apply_gtk(scheme: &Scheme, tone: Tone, state: &mut SyncState) -> io::Result<()> {
    let css = render(GTK, scheme.roles(tone))?;
    for dir in GTK_DIRS {
        let root = config_home().join(dir);
        write_generated(&root.join("kanade.css"), &css)?;

        let path = root.join("gtk.css");
        let link = fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
            .then(|| fs::read_link(&path))
            .transpose()?;
        let original = match &link {
            Some(target) => Some(link_import(&root, target)?),
            None => read_regular(&path)?,
        };
        let text = original.clone().unwrap_or_default();
        let updated = gtk_import(&text, true);
        if link.is_none() && updated == text {
            continue;
        }
        if !state.gtk_css.contains_key(dir) {
            state.gtk_css.insert(
                dir.to_owned(),
                SavedGtkCss {
                    created: original.is_none(),
                    link: link.clone(),
                },
            );
            save_state(state).map_err(io::Error::other)?;
        }
        if link.is_some() {
            fs::remove_file(&path)?;
        }
        atomic_write(&path, updated.as_bytes())?;
    }
    Ok(())
}

pub(super) fn restore_gtk(state: &mut SyncState) -> io::Result<()> {
    for dir in GTK_DIRS {
        let root = config_home().join(dir);
        let path = root.join("gtk.css");
        let saved = state.gtk_css.get(dir);
        let text = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => None,
            _ => read_regular(&path)?,
        };
        let link = saved.and_then(|saved| saved.link.as_ref());
        match (text, link) {
            (None, Some(target)) if fs::symlink_metadata(&path).is_err() => {
                std::os::unix::fs::symlink(target, &path)?
            }
            (Some(text), Some(target))
                if gtk_import(&text, false) == link_import(&root, target)? =>
            {
                fs::remove_file(&path)?;
                std::os::unix::fs::symlink(target, &path)?;
            }
            (Some(text), _) => {
                let updated = gtk_import(&text, false);
                if saved.is_some_and(|saved| saved.created) && updated.trim().is_empty() {
                    fs::remove_file(&path)?;
                } else if updated != text {
                    atomic_write(&path, updated.as_bytes())?;
                }
            }
            (None, _) => {}
        }
        remove_generated(&root.join("kanade.css"))?;
        state.gtk_css.remove(dir);
        save_state(state).map_err(io::Error::other)?;
    }
    Ok(())
}

// what stands in for a gtk.css symlink: an import of the file it pointed to, by absolute path
fn link_import(root: &Path, target: &Path) -> io::Result<String> {
    let target = root.join(target);
    match target.to_str() {
        Some(path) if !path.contains(['"', '\\', '\n']) => {
            Ok(format!("@import url(\"{path}\");\n"))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("cannot import {} from CSS", target.display()),
        )),
    }
}

// the qt*ct color scheme for this tone, its path for `color_scheme_path`
pub(super) fn apply_qt(major: u8, scheme: &Scheme, tone: Tone) -> io::Result<PathBuf> {
    let path = qt_path(major);
    write_generated(&path, &render(QT, scheme.roles(tone))?)?;
    Ok(path)
}

pub(super) fn restore_qt(major: u8) -> io::Result<()> {
    remove_generated(&qt_path(major)).map(drop)
}

fn qt_path(major: u8) -> PathBuf {
    config_home().join(format!("qt{major}ct/colors/kanade.conf"))
}

// the KDE color scheme for this tone; KDE apps outside Plasma use Breeze over qt*ct's palette
// unless a scheme is chosen
pub(super) fn apply_kde(scheme: &Scheme, tone: Tone) -> io::Result<()> {
    write_generated(&kde_path(), &render(KDE, scheme.roles(tone))?).map(drop)
}

pub(super) fn restore_kde() -> io::Result<()> {
    remove_generated(&kde_path()).map(drop)
}

fn kde_path() -> PathBuf {
    data_home().join(format!("color-schemes/{KDE_SCHEME}.colors"))
}

// niri's focus ring, border and tab colors, through a file config.kdl includes; niri reloads both
// live. No config.kdl means niri runs its defaults, which one with only an include would replace
pub(super) fn apply_niri(scheme: &Scheme, tone: Tone) -> io::Result<()> {
    let root = config_home().join("niri");
    let config = root.join("config.kdl");
    let Some(text) = read_regular(&config)? else {
        return Ok(());
    };
    write_generated(&root.join("kanade.kdl"), &render(NIRI, scheme.roles(tone))?)?;
    let updated = last_line(&text, NIRI_INCLUDE, true);
    if updated != text {
        atomic_write(&config, updated.as_bytes())?;
    }
    Ok(())
}

pub(super) fn restore_niri() -> io::Result<()> {
    let root = config_home().join("niri");
    let config = root.join("config.kdl");
    if let Some(text) = read_regular(&config)? {
        let updated = last_line(&text, NIRI_INCLUDE, false);
        if updated != text {
            atomic_write(&config, updated.as_bytes())?;
        }
    }
    remove_generated(&root.join("kanade.kdl")).map(drop)
}

// kitty's colors through a file kitty.conf includes, then each running kitty told to reload
pub(super) fn apply_kitty(scheme: &Scheme, tone: Tone) -> io::Result<()> {
    let root = config_home().join("kitty");
    let config = root.join("kitty.conf");
    let Some(text) = read_regular(&config)? else {
        return Ok(());
    };
    let colors = render(KITTY, &terminal_roles(scheme, tone)?)?;
    let mut changed = write_generated(&root.join("kanade.conf"), &colors)?;
    let updated = last_line(&text, KITTY_INCLUDE, true);
    if updated != text {
        atomic_write(&config, updated.as_bytes())?;
        changed = true;
    }
    if changed {
        reload_kittys();
    }
    Ok(())
}

pub(super) fn restore_kitty() -> io::Result<()> {
    let root = config_home().join("kitty");
    let config = root.join("kitty.conf");
    let mut changed = false;
    if let Some(text) = read_regular(&config)? {
        let updated = last_line(&text, KITTY_INCLUDE, false);
        if updated != text {
            atomic_write(&config, updated.as_bytes())?;
            changed = true;
        }
    }
    changed |= remove_generated(&root.join("kanade.conf"))?;
    if changed {
        reload_kittys();
    }
    Ok(())
}

// the scheme's roles and the ANSI colors' names for kitty's template. The six hues are Matugen's
// custom colors, with a lighter tone as their bright ones on dark; black stays the darker gray and
// white the lighter one in both modes, as terminal programs expect
fn terminal_roles(scheme: &Scheme, tone: Tone) -> io::Result<BTreeMap<String, String>> {
    let mut roles = scheme.roles(tone).clone();
    let grays = match tone {
        Tone::Dark => [
            ("black", "surface_container_highest"),
            ("bright_black", "outline"),
            ("white", "on_surface_variant"),
            ("bright_white", "on_surface"),
        ],
        Tone::Light => [
            ("black", "on_surface_variant"),
            ("bright_black", "outline"),
            ("white", "outline_variant"),
            ("bright_white", "surface_container_highest"),
        ],
    };
    let mut aliases: Vec<(String, String)> = grays
        .iter()
        .map(|(name, role)| (format!("ansi_{name}"), role.to_string()))
        .collect();
    for hue in ["red", "green", "yellow", "blue", "magenta", "cyan"] {
        let bright = match tone {
            Tone::Dark => format!("on_kanade_{hue}_container"),
            Tone::Light => format!("kanade_{hue}"),
        };
        aliases.push((format!("ansi_bright_{hue}"), bright));
    }
    for (alias, role) in aliases {
        let color = role_color(&roles, &role)?;
        roles.insert(alias, color);
    }
    Ok(roles)
}

// SIGUSR1 to each of the user's kittys old enough to handle it
fn reload_kittys() {
    let Ok(entries) = fs::read_dir("/proc") else {
        return;
    };
    let uptime = fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|text| text.split_whitespace().next()?.parse::<f64>().ok());
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let path = entry.path();
        if fs::read_to_string(path.join("comm")).ok().as_deref() != Some("kitty\n") {
            continue;
        }
        // the start time, in ticks since boot, is the 22nd field, the 20th after the
        // parenthesized name, which may hold spaces
        let started = fs::read_to_string(path.join("stat")).ok().and_then(|stat| {
            let (_, fields) = stat.rsplit_once(')')?;
            fields.split_whitespace().nth(19)?.parse::<u64>().ok()
        });
        let old = match (uptime, started) {
            (Some(uptime), Some(started)) => {
                uptime - (started / clock::USER_HZ) as f64 >= KITTY_STARTING as f64
            }
            _ => false,
        };
        if old && let Err(error) = clock::user_signal(pid) {
            eprintln!("kanade: could not tell kitty {pid} to reload: {error}");
        }
    }
}

// `text` with `line` as its last line, or without it; the rest kept byte for byte
fn last_line(text: &str, line: &str, present: bool) -> String {
    let mut kept: String = text
        .split_inclusive('\n')
        .filter(|kept| kept.trim() != line)
        .collect();
    if present {
        if !kept.is_empty() && !kept.ends_with('\n') {
            kept.push('\n');
        }
        kept.push_str(line);
        kept.push('\n');
    }
    kept
}

// a style that draws the qt*ct palette, for one that ignores it (Kvantum draws its own theme)
pub(super) fn palette_style(major: u8) -> &'static str {
    if major == 6 && darkly_installed() {
        "Darkly"
    } else {
        "Fusion"
    }
}

fn darkly_installed() -> bool {
    env::var_os("QT_PLUGIN_PATH")
        .map(|paths| env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain(
            [
                "/usr/lib/qt6/plugins",
                "/usr/lib64/qt6/plugins",
                "/usr/lib/x86_64-linux-gnu/qt6/plugins",
            ]
            .map(PathBuf::from),
        )
        .any(|root| root.join("styles/darkly6.so").is_file())
}

pub(super) fn apply_pywalfox(scheme: &Scheme, tone: Tone, state: &mut SyncState) -> io::Result<()> {
    let palette = pywalfox_palette(scheme)?;
    let file = json!({
        "tone": tone.name(),
        "wallpaper": scheme.wallpaper,
        "colors": palette,
    });
    let path = pywalfox::colors_path();
    let bytes = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
    if fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
        atomic_write(&path, &bytes)?;
    }

    let Some(manifest) = pywalfox::manifest_path() else {
        return Ok(());
    };
    let host = host_path(env::current_exe()?);
    let bytes = serde_json::to_vec_pretty(&pywalfox::manifest(&host)).map_err(io::Error::other)?;
    match read_regular(&manifest)? {
        Some(text) if text.as_bytes() == bytes => return Ok(()),
        // Pywalfox's own host, kept to put back when sync goes off
        Some(text) if !pywalfox::owns_manifest(&text) => {
            state.pywalfox_manifest = Some(text);
            save_state(state).map_err(io::Error::other)?;
        }
        _ => {}
    }
    atomic_write(&manifest, &bytes)
}

// the path Linux gives a binary replaced while it runs, as by an install, ends in " (deleted)";
// the browser should start the new one at that path
fn host_path(exe: PathBuf) -> PathBuf {
    match exe
        .to_str()
        .and_then(|path| path.strip_suffix(" (deleted)"))
    {
        Some(path) => PathBuf::from(path),
        None => exe,
    }
}

pub(super) fn restore_pywalfox(state: &mut SyncState) -> io::Result<()> {
    if let Some(manifest) = pywalfox::manifest_path()
        && read_regular(&manifest)?.is_some_and(|text| pywalfox::owns_manifest(&text))
    {
        match &state.pywalfox_manifest {
            Some(before) => atomic_write(&manifest, before.as_bytes())?,
            None => fs::remove_file(&manifest)?,
        }
    }
    state.pywalfox_manifest = None;
    save_state(state).map_err(io::Error::other)?;
    match fs::remove_file(pywalfox::colors_path()) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

// Pywal's 16 colors as Pywalfox reads them: its dark template takes 0, 8, 10, 13 and 15, its
// light one 3, 5 and 7, and it derives lighter backgrounds and text from 0 and 7
fn pywalfox_palette(scheme: &Scheme) -> io::Result<Vec<String>> {
    let dark = |role: &str| role_color(&scheme.dark, role);
    let light = |role: &str| role_color(&scheme.light, role);
    [
        dark("surface"),
        dark("error"),
        dark("secondary"),
        light("primary"),
        dark("primary_container"),
        light("tertiary"),
        dark("tertiary_container"),
        light("surface_container"),
        dark("outline"),
        dark("error"),
        dark("primary"),
        dark("secondary"),
        dark("primary_container"),
        dark("tertiary"),
        dark("secondary_container"),
        dark("on_surface"),
    ]
    .into_iter()
    .collect()
}

fn role_color(roles: &BTreeMap<String, String>, role: &str) -> io::Result<String> {
    roles.get(role).cloned().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Matugen did not provide colors.{role}"),
        )
    })
}

// `{{role}}` replaced by its color; a role Matugen did not give is an error, not an empty color
fn render(template: &str, roles: &BTreeMap<String, String>) -> io::Result<String> {
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        rendered.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "unterminated template role")
        })?;
        rendered.push_str(&role_color(roles, after[..end].trim())?);
        rest = &after[end + 2..];
    }
    rendered.push_str(rest);
    Ok(rendered)
}

// the user's gtk.css with Kanade's import after its own imports (CSS takes imports first), or
// without it; the rest of the file kept byte for byte
fn gtk_import(text: &str, present: bool) -> String {
    let mut lines: Vec<String> = text
        .split_inclusive('\n')
        .filter(|line| line.trim() != GTK_IMPORT)
        .map(String::from)
        .collect();
    if present {
        let at = lines
            .iter()
            .rposition(|line| line.trim_start().starts_with("@import"))
            .map_or(0, |last| last + 1);
        if at > 0 && !lines[at - 1].ends_with('\n') {
            lines[at - 1].push('\n');
        }
        lines.insert(at, format!("{GTK_IMPORT}\n"));
    }
    lines.concat()
}

// whether it wrote
fn write_generated(path: &Path, text: &str) -> io::Result<bool> {
    match read_regular(path)? {
        Some(current) if current == text => Ok(false),
        Some(current) if !generated(&current) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("left {} alone because it is not Kanade's", path.display()),
        )),
        _ => atomic_write(path, text.as_bytes()).map(|()| true),
    }
}

// whether it removed
fn remove_generated(path: &Path) -> io::Result<bool> {
    match read_regular(path)? {
        Some(current) if generated(&current) => fs::remove_file(path).map(|()| true),
        _ => Ok(false),
    }
}

fn generated(text: &str) -> bool {
    text.lines()
        .next()
        .is_some_and(|line| line.contains(HEADER))
}

// a regular file's text, None when missing; a symlink or other file is an error
fn read_regular(path: &Path) -> io::Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a regular file", path.display()),
            ))
        }
        Ok(_) => fs::read_to_string(path).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gtk_import_goes_after_the_users_imports_and_leaves_the_rest_intact() {
        let original = "@import 'colors.css';\n@import url(\"libadwaita.css\");\n\n.title { font-weight: bold; }\n";
        let added = gtk_import(original, true);
        assert_eq!(
            added,
            format!(
                "@import 'colors.css';\n@import url(\"libadwaita.css\");\n{GTK_IMPORT}\n\n.title {{ font-weight: bold; }}\n"
            )
        );
        assert_eq!(gtk_import(&added, true), added);
        assert_eq!(gtk_import(&added, false), original);

        let bare = "window { color: red; }";
        assert_eq!(gtk_import(&gtk_import(bare, true), false), bare);
        let imports_only = "@import 'a.css';";
        assert_eq!(
            gtk_import(&gtk_import(imports_only, true), false),
            "@import 'a.css';\n"
        );
        assert_eq!(gtk_import("", true), format!("{GTK_IMPORT}\n"));
    }

    #[test]
    fn include_line_goes_last_once_and_leaves_the_users_config_intact() {
        let original = "layout {\n    gaps 4\n}\n\n// end";
        let added = last_line(original, NIRI_INCLUDE, true);
        assert_eq!(added, format!("{original}\n{NIRI_INCLUDE}\n"));
        assert_eq!(last_line(&added, NIRI_INCLUDE, true), added);
        assert_eq!(
            last_line(&added, NIRI_INCLUDE, false),
            format!("{original}\n")
        );

        let earlier = format!("{KITTY_INCLUDE}\nfont_size 14\n");
        assert_eq!(
            last_line(&earlier, KITTY_INCLUDE, true),
            format!("font_size 14\n{KITTY_INCLUDE}\n")
        );
        assert_eq!(
            last_line("", KITTY_INCLUDE, true),
            format!("{KITTY_INCLUDE}\n")
        );
    }

    #[test]
    fn shipped_templates_render_from_matugen_roles() {
        let roles: BTreeMap<String, String> = [
            "background",
            "error",
            "error_container",
            "inverse_on_surface",
            "inverse_primary",
            "inverse_surface",
            "on_background",
            "on_error",
            "on_error_container",
            "on_primary",
            "on_primary_container",
            "on_secondary",
            "on_secondary_container",
            "on_surface",
            "on_surface_variant",
            "on_tertiary",
            "on_tertiary_container",
            "outline",
            "outline_variant",
            "primary",
            "primary_container",
            "scrim",
            "secondary",
            "secondary_container",
            "shadow",
            "surface",
            "surface_bright",
            "surface_container",
            "surface_container_high",
            "surface_container_highest",
            "surface_container_low",
            "surface_container_lowest",
            "surface_dim",
            "surface_tint",
            "surface_variant",
            "tertiary",
            "tertiary_container",
        ]
        .map(|role| (role.to_owned(), String::from("#123456")))
        .into_iter()
        .chain(
            ["red", "green", "yellow", "blue", "magenta", "cyan"]
                .into_iter()
                .flat_map(|hue| {
                    [
                        format!("kanade_{hue}"),
                        format!("on_kanade_{hue}_container"),
                    ]
                })
                .map(|role| (role, String::from("#654321"))),
        )
        .collect();
        for template in [GTK, QT, KDE, NIRI] {
            let rendered = render(template, &roles).unwrap();
            assert!(generated(&rendered));
            assert!(!rendered.contains("{{"));
        }
        let scheme = Scheme {
            wallpaper: PathBuf::new(),
            dark: roles.clone(),
            light: roles.clone(),
        };
        for tone in [Tone::Dark, Tone::Light] {
            let rendered = render(KITTY, &terminal_roles(&scheme, tone).unwrap()).unwrap();
            assert!(generated(&rendered));
            assert!(!rendered.contains("{{"));
        }
        assert!(render("{{missing}}", &roles).is_err());
    }
}
