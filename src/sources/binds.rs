//! niri's keybinds, read from its config as the Keys Surface lists them: `$NIRI_CONFIG`, else
//! `$XDG_CONFIG_HOME/niri/config.kdl` (else `~/.config/`), else `/etc/niri/config.kdl`, as niri
//! looks for it, with the files it includes. niri writes KDL v1; v2 is read too. Read again only once
//! one of the files changed, so opening the Surface costs a stat per file.
//!
//! Each bind is named by its `hotkey-overlay-title`, else by its action made readable, and grouped
//! by what it acts on. A bind niri's overlay hides (`hotkey-overlay-title=null`) is left out, and a
//! key bound again in a later block or include replaces the earlier bind, as in niri.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use kanade_runtime::MutexExt;
use kdl::{KdlDocument, KdlError, KdlNode, KdlValue};

// what a bind acts on, in the order the Surface lists them
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Kanade,
    Windows,
    Workspaces,
    Other,
}

impl Group {
    pub const ALL: [Group; 4] = [
        Group::Kanade,
        Group::Windows,
        Group::Workspaces,
        Group::Other,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Group::Kanade => "Kanade",
            Group::Windows => "Windows",
            Group::Workspaces => "Workspaces",
            Group::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bind {
    // the modifiers, then the key, each as a keycap reads
    pub keys: Vec<String>,
    pub title: String,
    pub group: Group,
}

// why the binds could not be read: the file, and what went wrong in it
#[derive(Debug, Clone, PartialEq)]
pub struct Unread {
    pub file: PathBuf,
    pub why: String,
}

// the deepest include followed; niri refuses a loop, which this stops too
const DEEPEST: usize = 8;

// the config niri reads, as it looks for it
pub fn config() -> Option<PathBuf> {
    if let Some(path) = env::var_os("NIRI_CONFIG").filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }

    let home = env::var_os("HOME").map(PathBuf::from);
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(|home| home.join(".config")))?;
    let user = base.join("niri/config.kdl");
    let system = PathBuf::from("/etc/niri/config.kdl");

    Some(if !user.exists() && system.exists() {
        system
    } else {
        user
    })
}

// the files read last and when each changed, with what they gave
type Read = (
    Vec<(PathBuf, Option<SystemTime>)>,
    Arc<Result<Vec<Bind>, Unread>>,
);

static READ: Mutex<Option<Read>> = Mutex::new(None);

// the binds in niri's config, read again only once a file it read changed
pub fn binds() -> Arc<Result<Vec<Bind>, Unread>> {
    let Some(path) = config() else {
        return Arc::new(Err(Unread {
            file: PathBuf::from("niri/config.kdl"),
            why: String::from("HOME is not set"),
        }));
    };

    let mut read = READ.locked();

    if let Some((files, binds)) = &*read
        && files.first().map(|(file, _)| file) == Some(&path)
        && files.iter().all(|(file, when)| modified(file) == *when)
    {
        return binds.clone();
    }

    let mut files = Vec::new();
    let binds = Arc::new(load(&path, &mut |file| {
        files.push((file.to_owned(), modified(file)));
        fs::read_to_string(file)
    }));

    *read = Some((files, binds.clone()));

    binds
}

fn modified(file: &Path) -> Option<SystemTime> {
    fs::metadata(file).and_then(|meta| meta.modified()).ok()
}

/*
 * the binds in `path` and what it includes, each file read through `read`; a later bind of the same
 * keys replaces an earlier one where it stood
 */
pub fn load(
    path: &Path,
    read: &mut dyn FnMut(&Path) -> std::io::Result<String>,
) -> Result<Vec<Bind>, Unread> {
    let mut binds: Vec<(String, Option<Bind>)> = Vec::new();
    let mut modifier = None;

    walk(path, false, 0, read, &mut binds, &mut modifier)?;

    // `Mod` is Super unless `input { mod-key }` says otherwise, as niri runs on a TTY
    let modifier = modifier.unwrap_or_else(|| String::from("Super"));

    Ok(binds
        .into_iter()
        .filter_map(|(_, bind)| bind)
        .map(|bind| Bind {
            keys: resolved(&bind.keys, &modifier),
            ..bind
        })
        .collect())
}

fn walk(
    path: &Path,
    optional: bool,
    depth: usize,
    read: &mut dyn FnMut(&Path) -> std::io::Result<String>,
    binds: &mut Vec<(String, Option<Bind>)>,
    modifier: &mut Option<String>,
) -> Result<(), Unread> {
    let unread = |why: String| Unread {
        file: path.to_owned(),
        why,
    };

    if depth > DEEPEST {
        return Err(unread(String::from("includes itself, or too deeply")));
    }

    let text = match read(path) {
        Ok(text) => text,
        Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(unread(String::from("not found")));
        }
        Err(error) => return Err(unread(error.to_string())),
    };

    let document = parse(&text).map_err(unread)?;

    for node in document.nodes() {
        match node.name().value() {
            "binds" => {
                for bind in node.children().map(KdlDocument::nodes).unwrap_or_default() {
                    let key = same(bind.name().value());
                    let bind = read_bind(bind);

                    match binds.iter_mut().find(|(earlier, _)| *earlier == key) {
                        Some((_, earlier)) => *earlier = bind,
                        None => binds.push((key, bind)),
                    }
                }
            }
            "include" => {
                let Some(file) = node.get(0).and_then(KdlValue::as_string) else {
                    continue;
                };
                let optional = node
                    .get("optional")
                    .and_then(KdlValue::as_bool)
                    .unwrap_or(false);

                walk(
                    &beside(path, file),
                    optional,
                    depth + 1,
                    read,
                    binds,
                    modifier,
                )?;
            }
            "input" => {
                let set = node
                    .children()
                    .and_then(|input| input.get("mod-key"))
                    .and_then(|key| key.get(0))
                    .and_then(KdlValue::as_string);

                if let Some(set) = set {
                    *modifier = Some(set.to_owned());
                }
            }
            _ => {}
        }
    }

    Ok(())
}

// KDL v1, as niri writes it, else v2; the error says the line of v1's first problem
fn parse(text: &str) -> Result<KdlDocument, String> {
    KdlDocument::parse_v1(text)
        .or_else(|v1| KdlDocument::parse_v2(text).map_err(|_| v1))
        .map_err(|error| problem(text, &error))
}

fn problem(text: &str, error: &KdlError) -> String {
    let Some(first) = error.diagnostics.first() else {
        return String::from("not valid KDL");
    };

    let at = first.span.offset().min(text.len());
    let line = text.as_bytes()[..at]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1;
    let what = first
        .message
        .as_deref()
        .or(first.label.as_deref())
        .unwrap_or("not valid KDL");

    format!("line {line}: {what}")
}

// an include's path: `~/` from home, else relative to the file including it
fn beside(path: &Path, file: &str) -> PathBuf {
    if let Some(rest) = file.strip_prefix("~/")
        && let Some(home) = env::var_os("HOME")
    {
        return Path::new(&home).join(rest);
    }

    path.parent().unwrap_or(Path::new("")).join(file)
}

/*
 * one bind, none when niri's overlay hides it or it has no action. Its title is the overlay's, else
 * its action read out; its group is what the action acts on
 */
fn read_bind(node: &KdlNode) -> Option<Bind> {
    let action = node.children()?.nodes().first()?;
    let (group, readable) = readable(action);

    let title = match node
        .entry("hotkey-overlay-title")
        .map(|entry| entry.value())
    {
        Some(KdlValue::Null) => return None,
        Some(value) => value.as_string().map(str::to_owned).unwrap_or(readable),
        None => readable,
    };

    Some(Bind {
        keys: keys(node.name().value()),
        title,
        group,
    })
}

// the arguments an action takes, as text, its properties left out
fn arguments(action: &KdlNode) -> Vec<String> {
    action
        .entries()
        .iter()
        .filter(|entry| entry.name().is_none())
        .filter_map(|entry| match entry.value() {
            KdlValue::String(text) => Some(text.clone()),
            KdlValue::Integer(number) => Some(number.to_string()),
            KdlValue::Float(number) => Some(number.to_string()),
            KdlValue::Bool(value) => Some(value.to_string()),
            KdlValue::Null => None,
        })
        .collect()
}

// what an action acts on and what it does in words: a Kanade verb, a program run, or niri's action
fn readable(action: &KdlNode) -> (Group, String) {
    let name = action.name().value();
    let arguments = arguments(action);

    let command: Vec<String> = match name {
        "spawn" => arguments.clone(),
        "spawn-sh" => arguments
            .first()
            .map(|line| line.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    if let Some(program) = command.first() {
        let program = Path::new(program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        if program == "kanade" && command.len() > 1 {
            return (Group::Kanade, sentence(&command[1..].join(" ")));
        }

        let rest = command[1..].join(" ");

        return (
            Group::Other,
            format!(
                "Run {program}{}{rest}",
                if rest.is_empty() { "" } else { " " }
            ),
        );
    }

    let group = if name.starts_with("screenshot") {
        Group::Other
    } else if name.contains("workspace") {
        Group::Workspaces
    } else if [
        "window",
        "column",
        "floating",
        "fullscreen",
        "maximize",
        "consume",
        "expel",
    ]
    .iter()
    .any(|word| name.contains(word))
    {
        Group::Windows
    } else {
        Group::Other
    };

    let mut words = name.replace('-', " ");

    for argument in arguments {
        words.push(' ');
        words.push_str(&argument);
    }

    (group, sentence(&words))
}

// `text` with its first letter capital
fn sentence(text: &str) -> String {
    let mut letters = text.chars();

    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

// the modifiers niri takes, each once whatever its case or alias, in the order they are shown
const MODIFIERS: &[(&[&str], &str)] = &[
    (&["mod"], "Mod"),
    (&["super", "win"], "Super"),
    (&["ctrl", "control"], "Ctrl"),
    (&["alt"], "Alt"),
    (&["shift"], "Shift"),
    (&["iso_level3_shift", "isolevel3shift", "mod5"], "AltGr"),
    (&["iso_level5_shift", "isolevel5shift", "mod3"], "Level5"),
];

// a bind's keys, compared as niri does: modifiers in any order or case, the key in any case
fn same(combination: &str) -> String {
    keys(combination).join("+").to_lowercase()
}

// the keycaps of a bind like `Mod+Shift+Left`: its modifiers in one order, then its key
fn keys(combination: &str) -> Vec<String> {
    let mut parts: Vec<&str> = combination.split('+').collect();
    let key = parts.pop().unwrap_or_default();

    let mut caps: Vec<String> = MODIFIERS
        .iter()
        .filter(|(names, _)| {
            parts
                .iter()
                .any(|part| names.contains(&part.to_lowercase().as_str()))
        })
        .map(|(_, shown)| (*shown).to_owned())
        .collect();

    caps.push(cap(key));

    caps
}

// `keys` with `Mod` as the modifier niri takes it for, the modifiers again in one order, each once
fn resolved(keys: &[String], modifier: &str) -> Vec<String> {
    let Some((key, modifiers)) = keys.split_last() else {
        return Vec::new();
    };

    let parts: Vec<String> = modifiers
        .iter()
        .map(|part| match part.as_str() {
            "Mod" => modifier.to_owned(),
            other => other.to_owned(),
        })
        .chain([String::from("_")])
        .collect();

    let mut resolved = self::keys(&parts.join("+"));
    resolved.pop();
    resolved.push(key.clone());

    resolved
}

// a key as its keycap reads: arrows as arrows, a letter capital, the wheel and touchpad in words
fn cap(key: &str) -> String {
    let named = match key.to_lowercase().as_str() {
        "left" => "\u{2190}",
        "right" => "\u{2192}",
        "up" => "\u{2191}",
        "down" => "\u{2193}",
        "comma" => ",",
        "period" => ".",
        "slash" => "/",
        "backslash" => "\\",
        "semicolon" => ";",
        "apostrophe" => "'",
        "grave" => "`",
        "minus" => "-",
        "equal" => "=",
        "plus" => "+",
        "bracketleft" => "[",
        "bracketright" => "]",
        "space" => "Space",
        "return" => "Enter",
        "escape" => "Esc",
        "tab" => "Tab",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "page_up" | "prior" => "Page Up",
        "page_down" | "next" => "Page Down",
        "home" => "Home",
        "end" => "End",
        "print" => "Print",
        "wheelscrollup" => "Wheel up",
        "wheelscrolldown" => "Wheel down",
        "wheelscrollleft" => "Wheel left",
        "wheelscrollright" => "Wheel right",
        "touchpadscrollup" => "Touchpad up",
        "touchpadscrolldown" => "Touchpad down",
        "touchpadscrollleft" => "Touchpad left",
        "touchpadscrollright" => "Touchpad right",
        "xf86audioraisevolume" => "Volume up",
        "xf86audiolowervolume" => "Volume down",
        "xf86audiomute" => "Mute",
        "xf86audiomicmute" => "Mic mute",
        "xf86audioplay" => "Play",
        "xf86audiopause" => "Pause",
        "xf86audiostop" => "Stop",
        "xf86audioprev" => "Previous",
        "xf86audionext" => "Next",
        "xf86monbrightnessup" => "Brightness up",
        "xf86monbrightnessdown" => "Brightness down",
        _ => "",
    };

    if !named.is_empty() {
        return named.to_owned();
    }

    if key.chars().count() == 1 {
        return key.to_uppercase();
    }

    // `XF86AudioRaiseVolume` as `Audio Raise Volume`, `Caps_Lock` as `Caps Lock`
    let key = key.strip_prefix("XF86").unwrap_or(key).replace('_', " ");
    let mut words = String::new();
    let mut previous: Option<char> = None;

    for letter in key.chars() {
        if letter.is_uppercase() && previous.is_some_and(char::is_lowercase) {
            words.push(' ');
        }

        words.push(letter);
        previous = Some(letter);
    }

    words
}

/*
 * the keys Kanade's own Surfaces take, as `docs/wiki/Keyboard.md` lists them in its table: where,
 * and what the keys there do. Read from the page, so the two never differ
 */
pub fn surface_keys() -> Vec<(String, String)> {
    table(include_str!("../../docs/wiki/Keyboard.md"))
}

// the rows of the first `| Where | Keys |` table in `page`, without its markup
fn table(page: &str) -> Vec<(String, String)> {
    page.lines()
        .skip_while(|line| !line.starts_with("| Where | Keys |"))
        .skip(2)
        .take_while(|line| line.starts_with('|'))
        .filter_map(|line| {
            let line = line.replace("\\|", "\u{0}").replace('`', "");
            let mut cells = line
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().replace('\u{0}', "|"));

            let place = cells.next().filter(|cell| !cell.is_empty())?;
            let keys = cells.next().filter(|cell| !cell.is_empty())?;

            Some((place, keys))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io;

    fn files(pairs: &[(&str, &str)]) -> HashMap<PathBuf, String> {
        pairs
            .iter()
            .map(|(path, text)| (PathBuf::from(path), (*text).to_owned()))
            .collect()
    }

    fn read(files: &HashMap<PathBuf, String>, path: &str) -> Result<Vec<Bind>, Unread> {
        load(Path::new(path), &mut |file| {
            files
                .get(file)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        })
    }

    fn titles(binds: &[Bind]) -> Vec<&str> {
        binds.iter().map(|bind| bind.title.as_str()).collect()
    }

    #[test]
    fn binds_are_named_by_their_title_else_their_action_and_grouped() {
        let config = files(&[(
            "/c/config.kdl",
            r#"
            input { keyboard { xkb { layout "us"; }; }; }
            binds {
                Mod+Alt+V hotkey-overlay-title="Island: Clipboard" { spawn "kanade" "clipboard" "toggle"; }
                Mod+L { spawn-sh "kanade lock"; }
                Mod+T { spawn "kitty"; }
                Mod+Shift+Left { focus-monitor-left; }
                Mod+Ctrl+Left { move-column-left; }
                Mod+shift+1 { move-column-to-workspace 1; }
                Mod+Minus { set-column-width "-10%"; }
                Mod+Shift+S { screenshot show-pointer=false; }
                Mod+O hotkey-overlay-title=null { toggle-overview; }
                Mod+X { }
            }
            "#,
        )]);

        let binds = read(&config, "/c/config.kdl").unwrap();

        assert_eq!(
            titles(&binds),
            [
                "Island: Clipboard",
                "Lock",
                "Run kitty",
                "Focus monitor left",
                "Move column left",
                "Move column to workspace 1",
                "Set column width -10%",
                "Screenshot",
            ]
        );
        assert_eq!(
            binds.iter().map(|bind| bind.group).collect::<Vec<_>>(),
            [
                Group::Kanade,
                Group::Kanade,
                Group::Other,
                Group::Other,
                Group::Windows,
                Group::Workspaces,
                Group::Windows,
                Group::Other,
            ]
        );
        assert_eq!(binds[0].keys, ["Super", "Alt", "V"]);
        assert_eq!(binds[3].keys, ["Super", "Shift", "\u{2190}"]);
    }

    #[test]
    fn includes_are_read_beside_their_file_and_a_later_bind_replaces_one() {
        let config = files(&[
            (
                "/c/config.kdl",
                r#"
                binds {
                    Mod+T { spawn "kitty"; }
                    Mod+Q { close-window; }
                }
                include "more/binds.kdl"
                include optional=true "absent.kdl"
                "#,
            ),
            (
                "/c/more/binds.kdl",
                r#"binds { MOD+t { spawn "foot"; }; Mod+E { spawn "nautilus"; }; }; input { mod-key "Alt"; }"#,
            ),
        ]);

        let binds = read(&config, "/c/config.kdl").unwrap();

        assert_eq!(titles(&binds), ["Run foot", "Close window", "Run nautilus"]);
        assert_eq!(binds[0].keys, ["Alt", "T"]);
    }

    #[test]
    fn a_missing_include_or_a_broken_file_is_named() {
        let missing = files(&[("/c/config.kdl", r#"include "gone.kdl""#)]);

        assert_eq!(
            read(&missing, "/c/config.kdl"),
            Err(Unread {
                file: PathBuf::from("/c/gone.kdl"),
                why: String::from("not found"),
            })
        );

        let broken = files(&[("/c/config.kdl", "binds {\n    Mod+T { spawn \"kitty\"; }\n")]);
        let unread = read(&broken, "/c/config.kdl").unwrap_err();

        assert_eq!(unread.file, PathBuf::from("/c/config.kdl"));
        assert!(unread.why.starts_with("line "), "{}", unread.why);

        let looped = files(&[("/c/config.kdl", r#"include "config.kdl""#)]);

        assert_eq!(
            read(&looped, "/c/config.kdl").unwrap_err().file,
            PathBuf::from("/c/config.kdl")
        );
    }

    #[test]
    fn kdl_v2_is_read_too() {
        let config = files(&[(
            "/c/config.kdl",
            r#"
            binds { Mod+W hotkey-overlay-title=#null { close-window; }; Mod+Q { close-window; }; }
            include optional=#true "absent.kdl"
            "#,
        )]);

        assert_eq!(
            titles(&read(&config, "/c/config.kdl").unwrap()),
            ["Close window"]
        );
    }

    #[test]
    fn the_keyboard_page_lists_every_surface_s_keys() {
        let keys = surface_keys();

        assert!(keys.iter().any(|(place, _)| place == "Any Surface"));
        assert!(keys.iter().any(|(place, _)| place == "Keys"));
        assert!(keys.iter().all(|(_, keys)| !keys.contains('`')));
    }
}
