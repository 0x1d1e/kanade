//! `kanade sddm theme list|info|use` and `kanade sddm enable|disable`: choosing a theme and
//! switching the overlay on and off. What the user can do, they are done here (the settings file, the
//! overlay); what needs root is printed as a command to copy, and Kanade never runs it.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use toml::Value;

use super::class::{self, Class, plain};
use super::{
    Config, OWN, USER, Wanted, Where, apply, cli, config, fail, largest_output, locked,
    make_prefix, modes, owned, plain_target, pointed, pointing, source_of, theme_ok, theme_state,
};

// the most themes listed: an install holds a few dozen
const MOST: usize = 500;

// where a theme is from
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    // Kanade's own, built into the binary
    BuiltIn,

    // a directory of `/usr/share/sddm/themes`
    Installed,

    // only in the overlay: its installed theme is gone
    OverlayOnly,
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Source::BuiltIn => "built in",
            Source::Installed => "installed",
            Source::OverlayOnly => "overlay only",
        }
    }
}

struct Found {
    name: String,
    source: Source,
    class: Class,
}

// every theme Kanade can write the look for: its own, the installed ones, then any left in the overlay
fn found(w: &Where) -> Vec<Found> {
    let mut out: Vec<Found> = vec![Found {
        name: OWN.to_owned(),
        source: Source::BuiltIn,
        class: Class::A(String::from("backdrop")),
    }];

    let names = |dir: PathBuf| -> Vec<String> {
        fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| theme_ok(name))
            .take(MOST)
            .collect()
    };

    for name in names(w.root.join(super::INSTALLED)) {
        // a theme needs its metadata, and Kanade's own is the one built in
        if name != OWN && w.installed(&name).join("metadata.desktop").is_file() {
            out.push(Found {
                class: super::class_of(w, &name),
                name,
                source: Source::Installed,
            });
        }
    }

    for name in names(w.prefix().join("themes")) {
        if !out.iter().any(|found| found.name == name) {
            out.push(Found {
                class: class::classify(&w.overlay(&name), false),
                name,
                source: Source::OverlayOnly,
            });
        }
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));

    out
}

// the theme `sddm.theme` names
fn selected(config: &Config) -> String {
    config.sddm_theme.clone().unwrap_or_else(|| OWN.to_owned())
}

// the theme SDDM's config makes Current through the overlay, if it does
fn read_by_sddm(w: &Where) -> Option<String> {
    let (dir, current) = pointed(w);
    let themes = format!("{}/themes", w.text().ok()?);

    match (dir, current) {
        (Some((dir, _)), Some((current, _))) if dir == themes => Some(current),
        _ => None,
    }
}

fn list_text(w: &Where, config: &Config) -> String {
    let chosen = selected(config);
    let read = read_by_sddm(w);
    let themes = found(w);

    let rows: Vec<[String; 4]> = themes
        .iter()
        .map(|found| {
            let mut state: Vec<String> = Vec::new();

            if found.name == chosen {
                state.push(String::from("selected"));
            }
            if read.as_deref() == Some(found.name.as_str()) {
                state.push(String::from("read by SDDM"));
            }
            if !w.overlay(&found.name).exists() {
                state.push(String::from("not in the overlay"));
            } else if theme_state(w, &found.name) != "current" {
                state.push(String::from("overlay out of date"));
            }

            [
                found.name.clone(),
                found.class.brief(),
                found.source.name().to_owned(),
                state.join(", "),
            ]
        })
        .collect();

    let heads = ["theme", "class", "from", "state"];
    let width = |at: usize| {
        rows.iter()
            .map(|row| row[at].chars().count())
            .chain([heads[at].len()])
            .max()
            .unwrap_or(0)
    };
    let widths = [width(0), width(1), width(2)];

    let mut text = String::new();
    for row in std::iter::once(heads.map(String::from)).chain(rows) {
        let _ = writeln!(
            text,
            "{:<w0$}  {:<w1$}  {:<w2$}  {}",
            row[0],
            row[1],
            row[2],
            row[3],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
        );
    }
    text.push_str(
        "class: A reads a picture from a setting, B draws its own file, C has no picture, D plays a video;\n\
         only A takes the wallpaper (`kanade sddm theme info <name>` says what sync does)\n",
    );

    text
}

pub fn list() -> ExitCode {
    let w = Where::here();

    cli::print(&list_text(&w, &config::get()), ExitCode::SUCCESS)
}

// a metadata.desktop value, as the terminal may show it
fn shown(value: &str) -> String {
    plain(value).chars().take(80).collect()
}

fn info_text(w: &Where, config: &Config, name: &str) -> Result<String, String> {
    if !theme_ok(name) {
        return Err(format!("{name}: not a theme's directory name"));
    }
    let themes = found(w);
    let Some(found) = themes.iter().find(|found| found.name == name) else {
        return Err(format!(
            "{name}: no such theme (`kanade sddm theme list` lists them)"
        ));
    };

    let mut text = String::new();
    let _ = writeln!(text, "theme: {name}");

    let installed = w.installed(name);
    match found.source {
        Source::BuiltIn => text.push_str("from: built into this Kanade\n"),
        Source::Installed => {
            let _ = writeln!(text, "from: {}", installed.display());
        }
        Source::OverlayOnly => {
            let _ = writeln!(
                text,
                "from: the overlay only; {} is gone, so `sync` cannot write it",
                installed.display()
            );
        }
    }

    let _ = writeln!(text, "class: {}", found.class.brief());
    let _ = writeln!(text, "sync: {}", found.class.does());

    if found.source == Source::Installed {
        if let Some(meta) = class::read(&installed.join("metadata.desktop")) {
            for (_, key, value) in class::pairs(&meta) {
                if ["Name", "Version", "QtVersion", "MainScript", "Theme-API"]
                    .contains(&key.as_str())
                {
                    let _ = writeln!(text, "{}: {}", shown(&key), shown(&value));
                }
            }
        }

        match modes(&installed, false) {
            Ok(closed) if closed.is_empty() => {
                text.push_str("readable by the sddm user: yes\n");
            }
            Ok(closed) => {
                let _ = writeln!(
                    text,
                    "readable by the sddm user: {} entries are not, as {}",
                    closed.len(),
                    closed[0].display()
                );
            }
            Err(error) => {
                let _ = writeln!(text, "readable by the sddm user: unknown ({error})");
            }
        }
    }

    let overlay = w.overlay(name);
    let _ = writeln!(
        text,
        "overlay: {} ({})",
        overlay.display(),
        if overlay.exists() {
            theme_state(w, name)
        } else {
            String::from("not made yet")
        }
    );
    if overlay.join(USER).exists() {
        let _ = writeln!(
            text,
            "{USER}: written by Kanade, beside the theme's own files"
        );
    }
    let _ = writeln!(
        text,
        "selected: {}",
        if selected(config) == name {
            "yes (sddm.theme)"
        } else {
            "no"
        }
    );
    if found.source != Source::BuiltIn {
        text.push_str(super::OTHER_THEMES);
        text.push('\n');
    }

    Ok(text)
}

pub fn info(name: &str) -> ExitCode {
    let w = Where::here();

    match info_text(&w, &config::get(), name) {
        Ok(text) => cli::print(&text, ExitCode::SUCCESS),
        Err(error) => fail(&error),
    }
}

/*
 * what the user can do for the overlay (write it, when it is theirs), then what SDDM still needs
 * from root: the commands to copy. A missing overlay is said with its command rather than failed,
 * as the user's own part, a key, is already written
 */
fn bring(w: &Where, config: &Config) -> Result<String, String> {
    let mut text = String::new();
    let theme = Wanted::of(config)?.theme;

    if owned(&w.prefix()).is_ok() {
        text.push_str(&locked(w, || apply(w, config, source_of, largest_output))?);
    } else {
        let _ = writeln!(
            text,
            "overlay: {} is not made yet; as root: {}",
            w.text()?,
            make_prefix(&w.text()?)
        );
        text.push_str(
            "then run `kanade sddm sync`, and only then the command for root that points SDDM at it (it does nothing until the theme is there)\n",
        );
    }
    text.push_str(&pointing(w, &theme)?);

    Ok(text)
}

// what the settings file itself holds for `sddm.theme`, whatever the config files over it say
fn settings_theme() -> Option<String> {
    let text = class::read(&config::settings_file()?)?;
    let table = config::settings_table(&text).ok()?;

    table.get("sddm")?.get("theme")?.as_str().map(str::to_owned)
}

// writes `sddm.theme` to the settings file, the last layer, so what it gives is what is in effect;
// the theme's own name is written too, as that is how it wins over a config file's other theme
fn choose(name: &str) -> Result<(Config, String), String> {
    let mut text = String::new();

    crate::settings::set(&["sddm", "theme"], Some(Value::String(name.to_owned())))?;
    let _ = writeln!(text, "sddm.theme = {name} (written to the settings file)");

    Ok((config::layers(true).0, text))
}

fn use_text(w: &Where, name: &str) -> Result<String, String> {
    if !theme_ok(name) {
        return Err(format!("{name}: not a theme's directory name"));
    }
    if !found(w)
        .iter()
        .any(|found| found.name == name && found.source != Source::OverlayOnly)
    {
        return Err(format!(
            "{name}: not an installed SDDM theme (`kanade sddm theme list` lists them)"
        ));
    }

    // the setting is written first, as the overlay is built from the files' answer; a failure to
    // build it puts the settings file's own value back exactly, so a refused theme is not left chosen
    let before = settings_theme();
    let (config, mut text) = choose(name)?;

    match bring(w, &config) {
        Ok(brought) => text.push_str(&brought),
        Err(error) => {
            let put_back =
                crate::settings::set(&["sddm", "theme"], before.clone().map(Value::String));

            return Err(match put_back {
                Ok(()) => format!("{error} (sddm.theme in the settings file is as it was)"),
                Err(again) => format!("{error} (and sddm.theme could not be put back: {again})"),
            });
        }
    }

    Ok(text)
}

pub fn use_theme(name: &str) -> ExitCode {
    let w = Where::here();

    match use_text(&w, name) {
        Ok(text) => cli::print(&text, ExitCode::SUCCESS),
        Err(error) => fail(&error),
    }
}

fn enable_text(w: &Where, config: &Config) -> Result<String, String> {
    bring(w, config)
}

pub fn enable() -> ExitCode {
    let w = Where::here();

    match enable_text(&w, &config::get()) {
        Ok(text) => cli::print(&text, ExitCode::SUCCESS),
        Err(error) => fail(&error),
    }
}

/*
 * what the user can do to stop the login screen using the overlay: remove the picture (a copy of
 * the desktop), and stop following the wallpaper. The overlay's themes stay, as they hold nothing
 * private; root removes the file that points SDDM at them
 */
fn disable_text(w: &Where, config: &Config) -> Result<String, String> {
    let mut text = String::new();

    // first, so a write of the picture that is waiting on the lock finds it off and writes nothing
    if config.sddm_sync {
        crate::settings::set(&["sddm", "sync"], Some(Value::Boolean(false)))?;
        text.push_str("sddm.sync = false (written to the settings file)\n");
    }

    if owned(&w.prefix()).is_ok() {
        let shown = w.picture();

        locked(w, || {
            plain_target(&shown)?;
            match fs::remove_file(&shown) {
                Ok(()) => {
                    let _ = writeln!(text, "picture: removed {}", shown.display());
                    Ok(())
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!("{}: {error}", shown.display())),
            }
        })?;
    }

    let ours = w.ours();
    if fs::symlink_metadata(&ours).is_ok() {
        let _ = writeln!(
            text,
            "as root (Kanade never runs these), so SDDM stops reading the overlay:\n  rm {}",
            ours.display()
        );
    } else {
        let _ = writeln!(
            text,
            "{} is not there: SDDM does not read the overlay through it",
            ours.display()
        );
    }
    let _ = writeln!(
        text,
        "to remove the overlay as well: as root `rm -r {}`",
        w.text()?
    );

    Ok(text)
}

pub fn disable() -> ExitCode {
    let w = Where::here();

    match disable_text(&w, &config::get()) {
        Ok(text) => cli::print(&text, ExitCode::SUCCESS),
        Err(error) => fail(&error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("kanade-sddm-cmd-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        root
    }

    // what a person is shown to pick from: only directories of a plain name that hold a theme
    #[test]
    fn the_list_holds_installed_themes_with_metadata_and_plain_names_only() {
        let dir = root("list");
        let w = Where::under(dir.clone());
        let themes = dir.join(super::super::INSTALLED);

        for (name, files) in [
            (
                "breeze",
                &[
                    ("metadata.desktop", ""),
                    ("theme.conf", "[General]\nbackground=a.png\n"),
                ][..],
            ),
            ("fixed", &[("metadata.desktop", ""), ("bg.png", "")]),
            ("no metadata", &[("Main.qml", "")]),
            ("nometa", &[("Main.qml", "")]),
            ("evil\u{1b}[2J", &[("metadata.desktop", "")]),
            ("half.new", &[("metadata.desktop", "")]),
        ] {
            for (file, text) in files {
                fs::create_dir_all(themes.join(name)).unwrap();
                fs::write(themes.join(name).join(file), text).unwrap();
            }
        }

        let listed: Vec<(String, Source, char)> = found(&w)
            .into_iter()
            .map(|found| {
                (
                    found.name,
                    found.source,
                    found.class.brief().chars().next().unwrap(),
                )
            })
            .collect();

        assert_eq!(
            listed,
            [
                ("breeze".into(), Source::Installed, 'A'),
                ("fixed".into(), Source::Installed, 'B'),
                ("kanade".into(), Source::BuiltIn, 'A'),
            ]
        );
        assert!(list_text(&w, &Config::default()).contains("kanade"));

        // a name no directory has is refused before any path is built from it
        for bad in ["../x", "a/b", "", "evil\u{1b}[2J", "x.new"] {
            assert!(info_text(&w, &Config::default(), bad).is_err(), "{bad}");
            assert!(use_text(&w, bad).is_err(), "{bad}");
        }

        fs::remove_dir_all(dir).unwrap();
    }
}
