//! `kanade sddm theme list|info|use` and `kanade sddm enable|disable`: choosing a theme and
//! switching the overlay on and off. What the user can do, they are done here (the settings file, the
//! overlay); what needs root is printed as a command to copy, and Kanade never runs it.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use toml::Value;

use super::class::{self, Class, plain};
use super::{
    Config, OWN, USER, Wanted, Where, apply, cli, config, fail, largest_output, locked,
    make_prefix, modes, owned, plain_target, pointed, pointing, root_owned_read_only, source_of,
    theme_ok, theme_state,
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
    // before anything is built from it or shown: an argument that is not a theme's name is said so,
    // not echoed, as it may hold an escape sequence
    if !theme_ok(name) {
        return Err(String::from(
            "not a theme's directory name: letters, digits, _ . and - only",
        ));
    }
    let themes = found(w);
    let Some(found) = themes.iter().find(|found| found.name == name) else {
        return Err(format!(
            "{}: no such theme (`kanade sddm theme list` lists them)",
            plain(name)
        ));
    };

    let mut text = String::new();
    let _ = writeln!(text, "theme: {}", plain(name));

    let installed = w.installed(name);
    match found.source {
        Source::BuiltIn => text.push_str("from: built into this Kanade\n"),
        Source::Installed => {
            let _ = writeln!(text, "from: {}", plain(&installed.display().to_string()));
        }
        Source::OverlayOnly => {
            let _ = writeln!(
                text,
                "from: the overlay only; {} is gone, so `sync` cannot write it",
                plain(&installed.display().to_string())
            );
        }
    }

    let _ = writeln!(text, "class: {}", found.class.brief());
    let _ = writeln!(text, "sync: {}", found.class.does());

    if found.source == Source::Installed {
        if let Some(meta) = class::text(&installed.join("metadata.desktop")) {
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
                    plain(&closed[0].display().to_string())
                );
            }
            Err(error) => {
                let _ = writeln!(
                    text,
                    "readable by the sddm user: unknown ({})",
                    plain(&error.to_string())
                );
            }
        }
    }

    let overlay = w.overlay(name);
    let _ = writeln!(
        text,
        "overlay: {} ({})",
        plain(&overlay.display().to_string()),
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
    let theme = Wanted::of(config)?.theme;
    if theme != OWN {
        return Err(String::from(
            "only the root-installed kanade theme can be selected safely",
        ));
    }

    let prefix = w.text()?;
    let executable_ready = root_owned_read_only(&w.root, &w.prefix()).is_ok()
        && root_owned_read_only(&w.root, &w.prefix().join("themes")).is_ok()
        && root_owned_read_only(&w.root, &w.overlay(OWN)).is_ok()
        && w.overlay(OWN).join("metadata.desktop").is_file();
    let data_ready = owned(&w.data()).is_ok();
    let mut text = String::new();

    if !executable_ready || !data_ready {
        let make = make_prefix(&prefix);
        if !data_ready || root_owned_read_only(&w.root, &w.prefix()).is_err() {
            let _ = writeln!(text, "as root: {make}");
        }
        if !executable_ready {
            text.push_str("as root: kanade sddm setup\n");
        }
        text.push_str("then run `kanade sddm sync`\n");
    } else {
        text.push_str(&locked(w, || apply(w, config, source_of, largest_output))?);
    }
    text.push_str(&pointing(w, &theme)?);

    Ok(text)
}

// what the settings file itself holds for `sddm.theme`, whatever the config files over it say
fn settings_theme() -> Option<String> {
    let text = class::text(&config::settings_file()?)?;
    let table = config::settings_table(&text).ok()?;

    table.get("sddm")?.get("theme")?.as_str().map(str::to_owned)
}

// writes `sddm.theme` to the settings file, the last layer, so what it gives is what is in effect;
// the theme's own name is written too, as that is how it wins over a config file's other theme
fn choose(name: &str) -> Result<(Config, String), String> {
    let mut text = String::new();

    crate::settings::set(&["sddm", "theme"], Some(Value::String(name.to_owned())))?;
    let _ = writeln!(
        text,
        "sddm.theme = {} (written to the settings file)",
        plain(name)
    );

    Ok((config::layers(true).0, text))
}

/*
 * `None` means no file existed, so rollback removes the path. Unsnapshotable files are errors, never
 * `None`, or rollback could delete a real picture.
 */
fn snapshot_picture(picture: &Path) -> Result<Option<Vec<u8>>, String> {
    let shown = picture.display();

    match fs::symlink_metadata(picture) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{shown}: {error}")),
        Ok(meta) if !meta.is_file() => Err(format!(
            "{shown} is not a plain file, so Kanade cannot keep it as it is for a failed theme change; \
             remove it, or make it a file of at most {} MiB, and try again",
            16
        )),
        Ok(meta) if meta.len() > 16 << 20 => Err(format!(
            "{shown} is over {} MiB, so Kanade cannot keep it as it is for a failed theme change; \
             make it smaller, or remove it, and try again",
            16
        )),
        Ok(_) => fs::read(picture)
            .map(Some)
            .map_err(|error| format!("{shown}: {error}")),
    }
}

fn restore_picture(picture: &Path, saved: Option<&[u8]>) -> Result<(), String> {
    match saved {
        Some(bytes) => {
            if fs::read(picture).is_ok_and(|current| current == bytes) {
                return Ok(());
            }
            super::put_picture(picture, bytes)
        }
        // only ever reached when `snapshot_picture` found nothing at the path, so what is there now
        // is the change's own
        None => {
            if fs::symlink_metadata(picture).is_err() {
                return Ok(());
            }
            super::plain_target(picture)?;
            match fs::remove_file(picture) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!(
                    "{}: {error}",
                    plain(&picture.display().to_string())
                )),
            }
        }
    }
}

fn use_text(w: &Where, name: &str) -> Result<String, String> {
    // validated before it is echoed or a path is built from it; then the one theme that may be
    // selected, so a refused name cannot be mistaken for one Kanade simply does not support
    if !theme_ok(name) {
        return Err(String::from(
            "not a theme's directory name: letters, digits, _ . and - only",
        ));
    }
    if name != super::OWN {
        return Err(String::from(
            "only the root-installed kanade theme can be selected safely",
        ));
    }
    if !found(w)
        .iter()
        .any(|found| found.name == name && found.source != Source::OverlayOnly)
    {
        return Err(format!(
            "{}: not an installed SDDM theme (`kanade sddm theme list` lists them)",
            plain(name)
        ));
    }

    // the picture is kept before anything is written, and a picture Kanade cannot keep stops the
    // change here: a rollback that treated a file it could not snapshot as "no picture" would
    // remove the user's file
    let before_theme = settings_theme();
    let before_picture = snapshot_picture(&w.picture())
        .map_err(|error| format!("{error}; nothing was changed, sddm.theme is as it was"))?;
    // the setting is written first, as the overlay is built from the files' answer; a failure to
    // build it puts the settings file's own value and any modified picture back exactly
    let (config, mut text) = choose(name)?;

    match bring(w, &config) {
        Ok(brought) => text.push_str(&brought),
        Err(error) => {
            let put_theme =
                crate::settings::set(&["sddm", "theme"], before_theme.clone().map(Value::String));
            let put_picture = restore_picture(&w.picture(), before_picture.as_deref());

            let theme_msg = match put_theme {
                Ok(()) => "sddm.theme in the settings file is as it was",
                Err(again) => {
                    return Err(format!(
                        "{error} (and sddm.theme could not be put back: {again})"
                    ));
                }
            };
            let picture_msg = match put_picture {
                Ok(()) => "the picture is as it was",
                Err(again) => {
                    return Err(format!(
                        "{error} ({theme_msg}; and the picture could not be put back: {again})"
                    ));
                }
            };

            return Err(format!("{error} ({theme_msg}; {picture_msg})"));
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

    if owned(&w.data()).is_ok() {
        let shown = w.picture();

        locked(w, || {
            plain_target(&shown)?;
            match fs::remove_file(&shown) {
                Ok(()) => {
                    let _ = writeln!(
                        text,
                        "picture: removed {}",
                        plain(&shown.display().to_string())
                    );
                    Ok(())
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!("{}: {error}", plain(&shown.display().to_string()))),
            }
        })?;
    }

    let ours = w.ours();
    if fs::symlink_metadata(&ours).is_ok() {
        let _ = writeln!(
            text,
            "as root (Kanade never runs these), so SDDM stops reading the overlay:\n  rm {}",
            plain(&ours.display().to_string())
        );
    } else {
        let _ = writeln!(
            text,
            "{} is not there: SDDM does not read the overlay through it",
            plain(&ours.display().to_string())
        );
    }
    let _ = writeln!(
        text,
        "to remove the root-owned themes as root: `rm -r {}`",
        plain(&w.prefix().join("themes").display().to_string())
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
    use std::os::unix::fs::PermissionsExt;

    fn root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("kanade-sddm-cmd-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        root
    }

    // what a person is shown to pick from: only directories of a plain name that hold a theme
    #[test]
    fn enable_prints_prerequisites_before_sync_when_data_is_missing() {
        let dir = root("enable-data");
        let w = Where::under(dir.clone());
        super::super::install_own(&w).unwrap();
        let output = bring(&w, &Config::default()).unwrap();

        let make = output.find("install -d -o").unwrap();
        let sync = output.find("kanade sddm sync").unwrap();
        assert!(make < sync, "{output}");
        assert!(!output.contains("picture:"), "{output}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn enable_prints_setup_before_sync_when_theme_is_missing() {
        let dir = root("enable-theme");
        let w = Where::under(dir.clone());
        fs::create_dir_all(w.data()).unwrap();
        fs::set_permissions(w.data(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = bring(&w, &Config::default()).unwrap();

        let setup = output.find("kanade sddm setup").unwrap();
        let sync = output.find("kanade sddm sync").unwrap();
        assert!(setup < sync, "{output}");
        assert!(!output.contains("picture:"), "{output}");
        fs::remove_dir_all(dir).unwrap();
    }

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
            let info_err = info_text(&w, &Config::default(), bad).unwrap_err();
            assert!(!info_err.contains('\u{1b}'), "{info_err}");
            assert_eq!(
                info_err,
                "not a theme's directory name: letters, digits, _ . and - only"
            );
            let use_err = use_text(&w, bad).unwrap_err();
            assert!(!use_err.contains('\u{1b}'), "{use_err}");
            assert_eq!(
                use_err,
                "not a theme's directory name: letters, digits, _ . and - only"
            );
        }

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn info_strips_control_characters_from_file_paths() {
        let dir = root("control-info");
        let w = Where::under(dir.clone());
        let themes = dir.join(super::super::INSTALLED);
        let theme = themes.join("breeze");
        fs::create_dir_all(&theme).unwrap();
        fs::write(theme.join("metadata.desktop"), "").unwrap();
        let bad = theme.join("bad\u{1b}[31m.png");
        fs::write(&bad, "").unwrap();
        fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o600)).unwrap();

        let info = info_text(&w, &Config::default(), "breeze").unwrap();
        assert!(!info.contains('\u{1b}'), "{info}");

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn restore_picture_restores_prior_bytes_or_removes_unwanted_file() {
        let dir = root("restore-pic");
        let w = Where::under(dir.clone());
        let pic = w.picture();
        fs::create_dir_all(pic.parent().unwrap()).unwrap();

        // 1. Snapshot nonexistent, then something creates pic, restore removes it
        let snap = snapshot_picture(&pic).unwrap();
        assert_eq!(snap, None);
        fs::write(&pic, b"new picture").unwrap();
        restore_picture(&pic, snap.as_deref()).unwrap();
        assert!(!pic.exists());

        // 2. Snapshot existing picture, then pic is changed/removed, restore puts it back
        fs::write(&pic, b"original picture").unwrap();
        let snap = snapshot_picture(&pic).unwrap();
        assert_eq!(snap, Some(b"original picture".to_vec()));
        fs::write(&pic, b"changed picture").unwrap();
        restore_picture(&pic, snap.as_deref()).unwrap();
        assert_eq!(fs::read(&pic).unwrap(), b"original picture");

        fs::remove_file(&pic).unwrap();
        restore_picture(&pic, snap.as_deref()).unwrap();
        assert_eq!(fs::read(&pic).unwrap(), b"original picture");

        // 3. A picture Kanade cannot snapshot is an error, not "no picture": a rollback must not
        // remove the file it could not read
        let real = pic.with_file_name("real.png");
        fs::write(&real, b"the user's own").unwrap();
        fs::remove_file(&pic).unwrap();
        std::os::unix::fs::symlink(&real, &pic).unwrap();
        let error = snapshot_picture(&pic).unwrap_err();
        assert!(error.contains("not a plain file"), "{error}");
        assert!(pic.exists() && real.exists(), "the link is left as it was");
        fs::remove_file(&pic).unwrap();

        // a directory where the picture goes is refused the same way
        fs::create_dir(&pic).unwrap();
        assert!(snapshot_picture(&pic).is_err());
        assert!(pic.is_dir());
        fs::remove_dir(&pic).unwrap();

        // over the cap is refused too, rather than taken as none
        let big = vec![b'a'; (16 << 20) + 1];
        fs::write(&pic, &big).unwrap();
        let error = snapshot_picture(&pic).unwrap_err();
        assert!(error.contains("MiB"), "{error}");
        assert_eq!(fs::read(&pic).unwrap().len(), big.len());

        fs::remove_dir_all(dir).unwrap();
    }
}
