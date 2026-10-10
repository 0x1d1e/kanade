//! The file an icon name stands for, for the tray's items and the Dock's and Launcher's apps: an
//! absolute path as it is, else the item's own folder, then the user's icon theme, the themes it
//! inherits and hicolor, then the loose pixmaps. An svg beats every png, a bigger png a smaller
//! one. This looks up only the names given, each once a run, so a theme switched while Kanade runs
//! shows after a restart.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex, OnceLock, PoisonError};

// how deep an item's own folder is searched: a theme in it, like hicolor/22x22/apps
const DEPTH: usize = 3;

// what each name was found as, by the item's own folder
type Found = HashMap<(String, Option<PathBuf>), Option<PathBuf>>;

static FOUND: LazyLock<Mutex<Found>> = LazyLock::new(Mutex::default);

// the folders under each item's own folder, walked once
static OWN: LazyLock<Mutex<HashMap<PathBuf, Vec<PathBuf>>>> = LazyLock::new(Mutex::default);

// every folder of the themes that may hold an icon, nearest theme first
static FOLDERS: OnceLock<Vec<Vec<PathBuf>>> = OnceLock::new();

pub fn find(name: &str, own: Option<&Path>) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }

    if name.starts_with('/') {
        return Some(PathBuf::from(name)).filter(|path| path.is_file());
    }

    let key = (name.to_owned(), own.map(Path::to_path_buf));

    if let Some(found) = FOUND
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return found.clone();
    }

    let found = look(name, own);

    FOUND
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, found.clone());

    found
}

fn look(name: &str, own: Option<&Path>) -> Option<PathBuf> {
    // some give the file's name rather than the icon's
    let name = name
        .strip_suffix(".png")
        .or_else(|| name.strip_suffix(".svg"))
        .unwrap_or(name);

    if let Some(own) = own {
        let folders = OWN
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(own.to_path_buf())
            .or_insert_with(|| {
                let mut folders = Vec::new();
                below(own, DEPTH, &mut folders);
                folders
            })
            .clone();

        if let Some(found) = best(name, &folders) {
            return Some(found);
        }
    }

    let themes = FOLDERS.get_or_init(|| {
        let roots = roots();

        chain(&roots)
            .iter()
            .map(|theme| {
                let mut folders = Vec::new();

                for root in &roots {
                    below(&root.join(theme), 2, &mut folders);
                }

                folders
            })
            .collect()
    });

    themes
        .iter()
        .find_map(|folders| best(name, folders))
        .or_else(|| {
            let pixmaps: Vec<PathBuf> = data_dirs()
                .into_iter()
                .map(|dir| dir.join("pixmaps"))
                .collect();

            best(name, &pixmaps)
        })
}

// the best file named `name` in any of `folders`
fn best(name: &str, folders: &[PathBuf]) -> Option<PathBuf> {
    folders
        .iter()
        .flat_map(|folder| {
            ["svg", "png"].map(|extension| folder.join(format!("{name}.{extension}")))
        })
        .filter(|path| path.is_file())
        .max_by_key(|path| score(path))
}

// `folder` and the folders under it, `depth` levels down
fn below(folder: &Path, depth: usize, folders: &mut Vec<PathBuf>) {
    if !folder.is_dir() {
        return;
    }

    folders.push(folder.to_path_buf());

    if depth == 0 {
        return;
    }

    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };

    let mut inner: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();

    // the same order every run, so a tie always picks the same file
    inner.sort();

    for folder in inner {
        below(&folder, depth - 1, folders);
    }
}

/*
 * an svg stays sharp at any size, so it beats every png; among them the bigger drawing wins, the
 * size its folder names, like 48x48 or 48, scalable counting as 512
 */
fn score(path: &Path) -> u32 {
    let svg = path.extension().is_some_and(|extension| extension == "svg");

    let size = path
        .components()
        .filter_map(|part| part.as_os_str().to_str())
        .find_map(|part| {
            if part == "scalable" {
                return Some(512);
            }

            part.split('@')
                .next()
                .and_then(|part| {
                    part.split_once('x')
                        .map_or(Some(part), |(width, _)| Some(width))
                })
                .and_then(|width| width.parse().ok())
        })
        .unwrap_or(0);

    if svg { 10_000 + size } else { 1 + size }
}

// where themes live, the user's own first
fn roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = data_dirs()
        .into_iter()
        .map(|dir| dir.join("icons"))
        .collect();

    if let Some(home) = env::var_os("HOME") {
        roots.insert(0, PathBuf::from(home).join(".icons"));
    }

    roots
}

// the user's theme, each it inherits, nearest first, then hicolor, where programs put their own
fn chain(roots: &[PathBuf]) -> Vec<String> {
    let mut themes: Vec<String> = theme().into_iter().collect();
    let mut next = 0;

    while next < themes.len() {
        for parent in parents(&themes[next], roots) {
            if !themes.contains(&parent) {
                themes.push(parent);
            }
        }

        next += 1;
    }

    if !themes.iter().any(|theme| theme == "hicolor") {
        themes.push(String::from("hicolor"));
    }

    themes
}

// what a theme's index.theme names in its Inherits line, from the first root that has it
fn parents(theme: &str, roots: &[PathBuf]) -> Vec<String> {
    let Some(index) = roots
        .iter()
        .find_map(|root| fs::read_to_string(root.join(theme).join("index.theme")).ok())
    else {
        return Vec::new();
    };

    index
        .lines()
        .find_map(|line| line.strip_prefix("Inherits="))
        .map(|list| {
            list.split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

// the theme gtk is set to, which most desktops share, else where gnome and gtk 4 keep it
fn theme() -> Option<String> {
    let config = match env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(env::var_os("HOME")?).join(".config"),
    };

    let settings = fs::read_to_string(config.join("gtk-3.0/settings.ini")).unwrap_or_default();

    let set = settings.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;

        (key.trim() == "gtk-icon-theme-name").then(|| value.trim().to_owned())
    });

    set.or_else(|| {
        let output = Command::new("dconf")
            .args(["read", "/org/gnome/desktop/interface/icon-theme"])
            .stderr(Stdio::null())
            .output()
            .ok()?;

        let theme = String::from_utf8(output.stdout).ok()?;
        let theme = theme.trim().trim_matches('\'');

        (!theme.is_empty()).then(|| theme.to_owned())
    })
    .filter(|theme| !theme.is_empty())
}

// the user's data folder, then the system's, as the XDG spec has them
fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    match env::var_os("XDG_DATA_HOME") {
        Some(dir) if !dir.is_empty() => dirs.push(PathBuf::from(dir)),
        _ => {
            if let Some(home) = env::var_os("HOME") {
                dirs.push(PathBuf::from(home).join(".local/share"));
            }
        }
    }

    let system = env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| String::from("/usr/local/share:/usr/share"));

    dirs.extend(
        system
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from),
    );

    dirs
}
