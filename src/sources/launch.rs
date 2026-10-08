//! How a desktop entry starts, as the Desktop Entry spec says, for the Dock (#144): over D-Bus for
//! `DBusActivatable=true`, else `Exec` split into its arguments, its field codes expanded for a
//! launch with no file or URL, run in `Path` and, for `Terminal=true`, in a terminal through
//! `xdg-terminal-exec`. niri runs the command, so the app gets niri's environment, not Kanade's.

use std::collections::HashMap;
use std::path::Path;

use zbus::blocking::Connection;
use zbus::zvariant::Value;

use super::json;
use super::niri::{self, Acted};
use super::wake;

// what runs a `Terminal=true` app, as the proposed terminal spec names it
pub const TERMINAL: &str = "xdg-terminal-exec";

const APPLICATION: &str = "org.freedesktop.Application";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    // the bus name to activate, the file id without `.desktop`, for `DBusActivatable=true`
    activate: Option<String>,

    // `Exec`; for an activatable entry, what runs when activation fails
    command: Option<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Command {
    argv: Vec<String>,

    // `Path`: the working directory
    dir: Option<String>,

    terminal: bool,
}

// what of a `[Desktop Entry]` launching reads, its string values unescaped
pub struct Fields<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub icon: Option<&'a str>,

    // where the entry was read from, for `%k`
    pub file: &'a Path,

    pub exec: Option<&'a str>,
    pub path: Option<&'a str>,
    pub terminal: bool,
    pub dbus_activatable: bool,
}

impl Launch {
    // none for an entry that cannot start: no `Exec` that parses, nor a bus name to activate
    pub fn of(fields: &Fields) -> Option<Launch> {
        let activate = fields
            .dbus_activatable
            .then(|| fields.id.strip_suffix(".desktop"))
            .flatten()
            .filter(|name| bus_name(name))
            .map(String::from);

        let command = fields
            .exec
            .and_then(split)
            .map(|args| args.iter().flat_map(|arg| expand(arg, fields)).collect())
            .filter(|argv: &Vec<String>| !argv.is_empty())
            .map(|argv| Command {
                argv,
                dir: fields.path.map(String::from),
                terminal: fields.terminal,
            });

        (activate.is_some() || command.is_some()).then_some(Launch { activate, command })
    }

    /*
     * starts it, activating it first when it is activatable and running `Exec` when that fails;
     * blocks on the bus and niri, so off the view thread
     */
    pub fn run(&self) -> Result<(), String> {
        let activated = self.activate.as_deref().map(activate);

        match (activated, &self.command) {
            (Some(Ok(())), _) => Ok(()),
            (Some(Err(error)), None) => Err(error),
            (Some(Err(_)) | None, Some(command)) => command.run(),
            (None, None) => Err(String::from("nothing to launch")),
        }
    }
}

// `org.freedesktop.Application.Activate`, which D-Bus starts the app for
fn activate(name: &str) -> Result<(), String> {
    let path = format!("/{}", name.replace('.', "/").replace('-', "_"));

    Connection::session()
        .and_then(|bus| {
            bus.call_method(
                Some(name),
                path.as_str(),
                Some(APPLICATION),
                "Activate",
                &(HashMap::<&str, Value>::new(),),
            )
        })
        .map(drop)
        .map_err(|error| format!("activating {name}: {error}"))
}

impl Command {
    fn run(&self) -> Result<(), String> {
        if self.terminal && !wake::found(TERMINAL) {
            return Err(format!(
                "{TERMINAL} is missing, so a terminal app cannot start"
            ));
        }

        if let Some(dir) = &self.dir
            && !Path::new(dir).is_dir()
        {
            return Err(format!("its Path {dir} is no directory"));
        }

        let action = format!(
            r#"{{"Action":{{"SpawnSh":{{"command":{}}}}}}}"#,
            json::quote(&self.shell())
        );

        match niri::act(&action) {
            Ok(Acted::Done) => Ok(()),
            Ok(Acted::Unknown(why)) => Err(format!("niri gave no clear answer: {why}")),
            Err(error) => Err(error.to_string()),
        }
    }

    // a shell line, since niri's `Spawn` takes no working directory
    fn shell(&self) -> String {
        let terminal = self.terminal.then_some(TERMINAL);
        let argv = terminal
            .into_iter()
            .chain(self.argv.iter().map(String::as_str))
            .map(quoted)
            .collect::<Vec<_>>()
            .join(" ");

        match &self.dir {
            Some(dir) => format!("cd -- {} && exec {argv}", quoted(dir)),
            None => format!("exec {argv}"),
        }
    }
}

fn quoted(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/*
 * `Exec`'s arguments: split at unquoted blanks, `"` quoting with `\` escaping `"`, `` ` ``, `$`
 * and `\` inside; leniently, as GLib does, `'` quotes too and `\` escapes any character outside.
 * None for a quote left open
 */
fn split(exec: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut arg = String::new();

    // whether `arg` has begun, so `""` is an argument
    let mut begun = false;
    let mut chars = exec.chars();

    while let Some(char) = chars.next() {
        match char {
            ' ' | '\t' | '\n' => {
                if begun {
                    args.push(std::mem::take(&mut arg));
                    begun = false;
                }
            }
            '"' => {
                begun = true;

                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            escaped @ ('"' | '`' | '$' | '\\') => arg.push(escaped),
                            other => {
                                arg.push('\\');
                                arg.push(other);
                            }
                        },
                        other => arg.push(other),
                    }
                }
            }
            '\'' => {
                begun = true;

                loop {
                    match chars.next()? {
                        '\'' => break,
                        other => arg.push(other),
                    }
                }
            }
            '\\' => {
                begun = true;
                arg.push(chars.next()?);
            }
            other => {
                begun = true;
                arg.push(other);
            }
        }
    }

    if begun {
        args.push(arg);
    }

    Some(args)
}

/*
 * one argument with its field codes expanded for a launch without files: a file or URL code alone
 * drops the argument, `%i` alone is `--icon` and the icon, `%c` the name, `%k` the entry's file,
 * `%%` a `%`; a code inside another argument with nothing to give, or one the spec deprecates or
 * does not know, is dropped
 */
fn expand(arg: &str, fields: &Fields) -> Vec<String> {
    match arg {
        "%f" | "%F" | "%u" | "%U" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => Vec::new(),
        "%i" => fields
            .icon
            .map(|icon| vec![String::from("--icon"), icon.to_owned()])
            .unwrap_or_default(),
        _ => {
            let mut out = String::with_capacity(arg.len());
            let mut chars = arg.chars();

            while let Some(char) = chars.next() {
                if char != '%' {
                    out.push(char);
                    continue;
                }

                match chars.next() {
                    Some('%') => out.push('%'),
                    Some('c') => out.push_str(fields.name),
                    Some('k') => out.push_str(&fields.file.to_string_lossy()),
                    _ => {}
                }
            }

            vec![out]
        }
    }
}

// a well-known bus name, which the spec requires of an activatable entry's id
fn bus_name(name: &str) -> bool {
    let elements: Vec<&str> = name.split('.').collect();

    name.len() <= 255
        && elements.len() >= 2
        && elements.iter().all(|element| {
            element
                .chars()
                .next()
                .is_some_and(|first| !first.is_ascii_digit())
                && element
                    .chars()
                    .all(|char| char.is_ascii_alphanumeric() || char == '_' || char == '-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(exec: &str) -> Fields<'_> {
        Fields {
            id: "org.example.App.desktop",
            name: "Example App",
            icon: Some("example"),
            file: Path::new("/usr/share/applications/org.example.App.desktop"),
            exec: Some(exec),
            path: None,
            terminal: false,
            dbus_activatable: false,
        }
    }

    fn argv(exec: &str) -> Option<Vec<String>> {
        Launch::of(&fields(exec)).and_then(|launch| launch.command.map(|command| command.argv))
    }

    fn strings(args: &[&str]) -> Option<Vec<String>> {
        Some(args.iter().map(|arg| String::from(*arg)).collect())
    }

    #[test]
    fn file_and_url_codes_alone_drop_their_argument() {
        assert_eq!(argv("firefox %u"), strings(&["firefox"]));
        assert_eq!(
            argv("code --new-window %F"),
            strings(&["code", "--new-window"])
        );
        assert_eq!(argv("app --file=%f"), strings(&["app", "--file="]));
        assert_eq!(argv("%U"), None);
    }

    #[test]
    fn name_icon_and_location_codes_expand() {
        assert_eq!(
            argv("app --name %c"),
            strings(&["app", "--name", "Example App"])
        );
        assert_eq!(argv("app %i"), strings(&["app", "--icon", "example"]));
        assert_eq!(
            argv("app --entry=%k"),
            strings(&[
                "app",
                "--entry=/usr/share/applications/org.example.App.desktop"
            ])
        );

        let iconless = Fields {
            icon: None,
            ..fields("app %i")
        };
        assert_eq!(
            Launch::of(&iconless).and_then(|launch| launch.command.map(|command| command.argv)),
            strings(&["app"])
        );
    }

    #[test]
    fn percent_signs_and_quotes_stay_literal() {
        assert_eq!(argv("printf 100%%"), strings(&["printf", "100%"]));
        assert_eq!(
            argv(r#"sh -c "echo \"\$HOME\" \\ \`x\`" ''"#),
            strings(&["sh", "-c", r#"echo "$HOME" \ `x`"#, ""])
        );
        assert_eq!(
            argv("'/opt/My App/bin' a\\ b"),
            strings(&["/opt/My App/bin", "a b"])
        );
        assert_eq!(argv(r#"app "unclosed"#), None);
    }

    #[test]
    fn path_and_terminal_shape_the_shell_line() {
        let command = |path, terminal| {
            Launch::of(&Fields {
                path,
                terminal,
                ..fields("htop --tree 'it''s'")
            })
            .and_then(|launch| launch.command)
            .map(|command| command.shell())
        };

        assert_eq!(
            command(None, false).as_deref(),
            Some("exec 'htop' '--tree' 'its'")
        );
        assert_eq!(
            command(Some("/srv/my dir"), true).as_deref(),
            Some("cd -- '/srv/my dir' && exec 'xdg-terminal-exec' 'htop' '--tree' 'its'")
        );
        assert_eq!(quoted("it's"), r"'it'\''s'");
    }

    #[test]
    fn an_activatable_entry_activates_with_exec_as_a_fallback() {
        let activatable = |id, exec| {
            Launch::of(&Fields {
                id,
                exec,
                dbus_activatable: true,
                ..fields("")
            })
        };

        assert_eq!(
            activatable("org.example.App.desktop", None),
            Some(Launch {
                activate: Some(String::from("org.example.App")),
                command: None,
            })
        );
        assert_eq!(
            activatable("org.example.App.desktop", Some("app"))
                .and_then(|launch| launch.command)
                .map(|command| command.argv),
            strings(&["app"])
        );

        // not a bus name, so not activatable
        assert_eq!(activatable("app.desktop", None), None);
        assert_eq!(activatable("org.9example.App.desktop", None), None);
        assert_eq!(
            activatable("org.example.app-name.desktop", None).and_then(|launch| launch.activate),
            Some(String::from("org.example.app-name"))
        );
    }

    #[test]
    fn an_entry_with_nothing_to_run_cannot_launch() {
        assert_eq!(
            Launch::of(&Fields {
                exec: None,
                ..fields("")
            }),
            None
        );
        assert_eq!(argv("   "), None);
    }
}
