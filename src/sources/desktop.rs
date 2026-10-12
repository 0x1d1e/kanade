//! The `.desktop` entries in the XDG data dirs (ADR 0014), with the file id and how the entry
//! launches (ADR 0011). The Dock matches
//! windows and pins to them (`windows`), the Launcher lists them (`apps`), and both start them the
//! one way `launch` says.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::{env, fs};

use super::launch::{Fields, Launch};

// an application's entry
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    // the desktop file id, like `org.kde.dolphin.desktop`
    pub id: String,

    pub name: String,

    // the icon's name in the icon theme, or a path
    pub icon: Option<String>,

    // `Comment`, else `GenericName`, like "Web Browser"
    pub description: Option<String>,

    // `StartupWMClass`: the app id its windows have, when that is not its file id
    pub wm_class: Option<String>,

    // `NoDisplay=true`: no menu lists it, though its windows are still the app's
    pub no_display: bool,

    // how it starts, none for an entry with nothing to run
    pub launch: Option<Launch>,

    // app-specific commands exposed in the Dock's right-click menu
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub id: String,
    pub name: String,
    pub launch: Launch,
}

// the `applications` dirs of the XDG data dirs, the user's first, so their files win
pub fn dirs() -> Vec<PathBuf> {
    let home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| env::var_os("HOME").map(|home| Path::new(&home).join(".local/share")));

    // unset or empty means these two, as the spec says
    let system = env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| String::from("/usr/local/share:/usr/share"));

    home.into_iter()
        .chain(
            system
                .split(':')
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
        )
        .map(|dir| dir.join("applications"))
        .collect()
}

// how deep in `applications` a file may be, so a link loop ends
const DEPTH: usize = 8;

/*
 * every application entry in the dirs, in order: an id in an earlier dir hides the same id in a
 * later one, even when it is `Hidden`, which is how a user deletes a system entry. A file that
 * cannot be read, or is no desktop entry, hides nothing, as the spec skips it
 */
pub fn scan(dirs: &[PathBuf]) -> Vec<Entry> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();

    for dir in dirs {
        let mut files = Vec::new();
        walk(dir, DEPTH, &mut files);
        files.sort();

        for path in files {
            let Some(id) = id(dir, &path) else {
                continue;
            };

            if seen.contains(&id) {
                continue;
            }

            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };

            match read(id.clone(), &path, &text) {
                Read::App(entry) => entries.push(*entry),
                Read::Other => {}
                Read::Invalid => continue,
            }

            seen.insert(id);
        }
    }

    entries
}

// follows links, which is how nix puts files into the profile dirs; a missing dir has no files
fn walk(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };

    for path in read.flatten().map(|entry| entry.path()) {
        if path.is_dir() {
            if depth > 0 {
                walk(&path, depth - 1, files);
            }
        } else {
            files.push(path);
        }
    }
}

// the desktop file id: its path inside `applications`, with `/` turned into `-`
fn id(dir: &Path, path: &Path) -> Option<String> {
    if path.extension()? != "desktop" {
        return None;
    }

    Some(path.strip_prefix(dir).ok()?.to_str()?.replace('/', "-"))
}

// what a `.desktop` file is
#[derive(Debug, PartialEq, Eq)]
enum Read {
    // an application, `NoDisplay` or not
    App(Box<Entry>),

    // an entry, so it hides the same id in a later dir, but no app: `Hidden`, meaning deleted, or of
    // another `Type`
    Other,

    // no desktop entry: no `[Desktop Entry]`, or no `Type` or `Name`
    Invalid,
}

fn read(id: String, file: &Path, text: &str) -> Read {
    #[derive(Clone)]
    enum Group {
        Main,
        Action(String),
    }

    let mut main = BTreeMap::<&str, &str>::new();
    let mut actions = BTreeMap::<String, BTreeMap<&str, &str>>::new();
    let mut current = None;
    let mut has_main = false;

    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            current = if line == "[Desktop Entry]" {
                has_main = true;
                Some(Group::Main)
            } else {
                line.strip_prefix("[Desktop Action ")
                    .and_then(|group| group.strip_suffix(']'))
                    .filter(|id| !id.is_empty())
                    .map(|id| Group::Action(id.to_owned()))
            };
        } else if let Some((key, value)) = line.split_once('=') {
            let (key, value) = (key.trim(), value.trim());

            match current.as_ref() {
                Some(Group::Main) => {
                    // the first wins, as the spec forbids a key twice
                    main.entry(key).or_insert(value);
                }
                Some(Group::Action(id)) => {
                    actions
                        .entry(id.clone())
                        .or_default()
                        .entry(key)
                        .or_insert(value);
                }
                None => {}
            }
        }
    }

    let field = |fields: &BTreeMap<&str, &str>, key: &str| -> Option<String> {
        fields
            .get(key)
            .copied()
            .filter(|value| !value.is_empty())
            .map(unescape)
    };

    if !has_main {
        return Read::Invalid;
    }

    // `Hidden` alone deletes, with no `Type` or `Name`
    if main.get("Hidden") == Some(&"true") {
        return Read::Other;
    }

    let (Some(kind), Some(app_name)) = (field(&main, "Type"), field(&main, "Name")) else {
        return Read::Invalid;
    };

    if kind != "Application" {
        return Read::Other;
    }

    let icon = field(&main, "Icon");
    let flag = |key| main.get(key) == Some(&"true");
    let path = field(&main, "Path");
    let terminal = flag("Terminal");
    let dbus_activatable = flag("DBusActivatable");
    let launch = Launch::of(&Fields {
        id: &id,
        name: &app_name,
        icon: icon.as_deref(),
        file,
        exec: field(&main, "Exec").as_deref(),
        path: path.as_deref(),
        terminal,
        dbus_activatable,
    });

    let mut seen = HashSet::new();
    let actions = field(&main, "Actions")
        .into_iter()
        .flat_map(|actions| actions.split(';').map(str::to_owned).collect::<Vec<_>>())
        .filter(|action| !action.is_empty() && seen.insert(action.clone()))
        .filter_map(|action_id| {
            let fields = actions.get(&action_id)?;
            let name = field(fields, "Name")?;
            let exec = field(fields, "Exec");
            let icon = field(fields, "Icon");
            let launch = Launch::of(&Fields {
                id: &id,
                name: &app_name,
                icon: icon.as_deref(),
                file,
                exec: exec.as_deref(),
                path: path.as_deref(),
                terminal,
                dbus_activatable,
            })?;

            Some(Action {
                id: action_id,
                name,
                launch,
            })
        })
        .collect();

    Read::App(Box::new(Entry {
        id,
        name: app_name,
        icon,
        description: field(&main, "Comment").or_else(|| field(&main, "GenericName")),
        wm_class: field(&main, "StartupWMClass"),
        no_display: flag("NoDisplay"),
        launch,
        actions,
    }))
}

// a string value as written, with the spec's escapes decoded; another escape stays as it is
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();

    while let Some(char) = chars.next() {
        if char != '\\' {
            out.push(char);
            continue;
        }

        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }

    out
}

// applications dirs on disk, for the tests of what reads them
#[cfg(test)]
pub mod fixture {
    use std::path::{Path, PathBuf};
    use std::{env, fs};

    pub const APP: &str = "[Desktop Entry]\nType=Application\nName=";

    // a fresh dir under the temp dir, gone first if a run before left it
    pub fn temp(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("kanade-desktop-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub fn write(dir: &Path, file: &str, text: &str) {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{APP, temp, write};
    use super::*;

    const FILE: &str = "/usr/share/applications/a.desktop";

    #[test]
    fn a_desktop_file_reads_its_main_section() {
        let text = "\
# comment
[Desktop Entry]
Type=Application
Name=Files
Name[de]=Dateien
Icon = org.gnome.Nautilus
StartupWMClass=
GenericName=File Manager
Comment=Access and organize files
Exec=nautilus --new-window

[Desktop Action new-window]
Name=New Window
StartupWMClass=other
";

        assert_eq!(
            read(
                String::from("org.gnome.Nautilus.desktop"),
                Path::new(FILE),
                text
            ),
            Read::App(Box::new(Entry {
                id: String::from("org.gnome.Nautilus.desktop"),
                name: String::from("Files"),
                icon: Some(String::from("org.gnome.Nautilus")),
                description: Some(String::from("Access and organize files")),
                wm_class: None,
                no_display: false,
                launch: Launch::of(&Fields {
                    id: "org.gnome.Nautilus.desktop",
                    name: "Files",
                    icon: Some("org.gnome.Nautilus"),
                    file: Path::new(FILE),
                    exec: Some("nautilus --new-window"),
                    path: None,
                    terminal: false,
                    dbus_activatable: false,
                }),
                actions: Vec::new(),
            }))
        );
    }

    #[test]
    fn the_description_is_the_comment_else_the_generic_name() {
        let description = |lines: &str| {
            let Read::App(entry) = read(
                String::from("a.desktop"),
                Path::new(FILE),
                &format!("{APP}A\n{lines}"),
            ) else {
                panic!("an app");
            };

            entry.description
        };

        assert_eq!(
            description("GenericName=Browser\nComment=Browse").as_deref(),
            Some("Browse")
        );
        assert_eq!(
            description("GenericName=Browser\nComment=").as_deref(),
            Some("Browser")
        );
        assert_eq!(description(""), None);
    }

    #[test]
    fn desktop_actions_follow_the_declared_order_and_use_their_exec() {
        let text = "\
[Desktop Entry]
Type=Application
Name=Browser
Exec=browser
Path=/opt/browser
Terminal=true
Actions=NewWindow;Private;Missing;NewWindow;

[Desktop Action Private]
Name=New Private Window
Exec=browser --private %c

[Desktop Action NewWindow]
Name=New Window
Exec=browser --new-window

[Desktop Action Missing]
Name=Missing Exec
";
        let Read::App(entry) = read(String::from("browser.desktop"), Path::new(FILE), text) else {
            panic!("an app");
        };

        assert_eq!(
            entry
                .actions
                .iter()
                .map(|action| (action.id.as_str(), action.name.as_str()))
                .collect::<Vec<_>>(),
            [
                ("NewWindow", "New Window"),
                ("Private", "New Private Window")
            ]
        );
        assert_eq!(
            entry.actions[0].launch.command(),
            Some("browser --new-window")
        );
        assert_eq!(entry.actions[1].launch.command(), Some("browser --private"));
        assert_eq!(
            entry.actions[0].launch.shell_line().as_deref(),
            Some("cd -- '/opt/browser' && exec 'xdg-terminal-exec' 'browser' '--new-window'")
        );
    }

    #[test]
    fn dbus_activatable_actions_without_exec_are_kept() {
        let text = "\
[Desktop Entry]
Type=Application
Name=Browser
Exec=browser
DBusActivatable=true
Actions=NewWindow;

[Desktop Action NewWindow]
Name=New Window
";
        let Read::App(entry) = read(
            String::from("org.example.Browser.desktop"),
            Path::new(FILE),
            text,
        ) else {
            panic!("an app");
        };

        assert_eq!(entry.actions.len(), 1);
        assert_eq!(entry.actions[0].id, "NewWindow");
        assert_eq!(entry.actions[0].name, "New Window");
        assert_eq!(entry.actions[0].launch.command(), None);
    }

    #[test]
    fn launching_reads_exec_path_terminal_and_activation() {
        let launch = |lines: &str| {
            let Read::App(entry) = read(
                String::from("org.example.App.desktop"),
                Path::new(FILE),
                &format!("{APP}A\nIcon=a\n{lines}"),
            ) else {
                panic!("an app");
            };

            entry.launch
        };
        let expected = |exec, path, terminal, dbus_activatable| {
            Launch::of(&Fields {
                id: "org.example.App.desktop",
                name: "A",
                icon: Some("a"),
                file: Path::new(FILE),
                exec,
                path,
                terminal,
                dbus_activatable,
            })
        };

        assert_eq!(
            launch("Exec=a\\sb %c %k\nPath=/srv\nTerminal=true"),
            expected(Some("a b %c %k"), Some("/srv"), true, false)
        );
        assert_eq!(
            launch("DBusActivatable=true"),
            expected(None, None, false, true)
        );
        assert_eq!(launch("Exec=%U"), None);
        assert_eq!(launch(""), None);
    }

    // the spec's escapes in a string value: `\s`, `\n`, `\t`, `\r`, `\\`; any other stays as written
    #[test]
    fn string_values_decode_their_escapes() {
        let text = "[Desktop Entry]\nType=Application\nName=Foo\\sBar\\t\\\\s\\q\nIcon=foo\\\\bar\nStartupWMClass=a\\sb\\";
        let Read::App(entry) = read(String::from("a.desktop"), Path::new(FILE), text) else {
            panic!("an app");
        };

        assert_eq!(entry.name, "Foo Bar\t\\s\\q");
        assert_eq!(entry.icon.as_deref(), Some("foo\\bar"));
        assert_eq!(entry.wm_class.as_deref(), Some("a b\\"));
    }

    #[test]
    fn only_applications_that_are_not_hidden_are_entries() {
        let read = |text: &str| read(String::from("a.desktop"), Path::new(FILE), text);
        let app = |extra: &str| {
            read(&format!(
                "[Desktop Entry]\nType=Application\nName=A\n{extra}"
            ))
        };

        assert!(matches!(app(""), Read::App(entry) if !entry.no_display));
        assert!(matches!(app("NoDisplay=true"), Read::App(entry) if entry.no_display));
        assert!(matches!(app("Hidden=false"), Read::App(_)));
        assert_eq!(app("Hidden=true"), Read::Other);
        assert_eq!(read("[Desktop Entry]\nHidden=true"), Read::Other);
        assert_eq!(read("[Desktop Entry]\nType=Link\nName=A"), Read::Other);

        assert_eq!(read("[Desktop Entry]\nType=Application"), Read::Invalid);
        assert_eq!(read("[Desktop Entry]\nName=A"), Read::Invalid);
        assert_eq!(read("Type=Application\nName=A"), Read::Invalid);
        assert_eq!(read("Hidden=true"), Read::Invalid);
        assert_eq!(read(""), Read::Invalid);
    }

    #[test]
    fn the_scan_reads_ids_in_order_and_the_user_hides_the_system() {
        let root = temp("scan");
        let (user, system) = (root.join("user"), root.join("system"));

        write(&user, "editor.desktop", &format!("{APP}Mine"));
        write(&user, "gone.desktop", &format!("{APP}Gone\nHidden=true"));
        write(&system, "editor.desktop", &format!("{APP}Theirs"));
        write(&system, "gone.desktop", &format!("{APP}Gone"));
        write(&system, "kde/dolphin.desktop", &format!("{APP}Dolphin"));
        write(&system, "a.desktop", &format!("{APP}A"));
        write(&system, "notes.txt", &format!("{APP}Notes"));

        let read: Vec<(String, String)> = scan(&[user, system, root.join("missing")])
            .into_iter()
            .map(|entry| (entry.id, entry.name))
            .collect();

        fs::remove_dir_all(&root).unwrap();

        assert_eq!(
            read,
            [
                (String::from("editor.desktop"), String::from("Mine")),
                (String::from("a.desktop"), String::from("A")),
                (String::from("kde-dolphin.desktop"), String::from("Dolphin")),
            ]
        );
    }

    // a user's file that cannot be read, or is no desktop entry, leaves the id to the system's
    #[test]
    fn an_unreadable_or_invalid_user_entry_falls_back_to_the_system() {
        let root = temp("fallback");
        let (user, system) = (root.join("user"), root.join("system"));

        fs::create_dir_all(&user).unwrap();
        std::os::unix::fs::symlink(root.join("nowhere"), user.join("firefox.desktop")).unwrap();
        write(&user, "editor.desktop", "not a desktop entry");
        write(
            &user,
            "nameless.desktop",
            "[Desktop Entry]\nType=Application",
        );
        write(&system, "firefox.desktop", &format!("{APP}Firefox"));
        write(&system, "editor.desktop", &format!("{APP}Editor"));
        write(&system, "nameless.desktop", &format!("{APP}Named"));

        let read: Vec<String> = scan(&[user, system])
            .into_iter()
            .map(|entry| entry.name)
            .collect();

        fs::remove_dir_all(&root).unwrap();

        assert_eq!(read, ["Editor", "Firefox", "Named"]);
    }
}
