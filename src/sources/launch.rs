//! How a desktop entry starts, as the Desktop Entry spec says, for the Dock (#144) and the Launcher
//! (#183): over D-Bus for `DBusActivatable=true`, else `Exec` split into its arguments, its field
//! codes expanded for a launch with no file or URL, run in `Path` and, for `Terminal=true`, in a
//! terminal through `xdg-terminal-exec`. niri runs the command, so the app gets niri's environment,
//! not Kanade's. An activatable app runs `Exec` only when the bus could not deliver `Activate`: one
//! that got it but has not answered may be starting, and a second start would open it twice.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use zbus::blocking::{Connection, connection};
use zbus::zvariant::Value;

use super::json;
use super::niri::{self, Acted};
use super::wake;

// what runs a `Terminal=true` app, as the proposed terminal spec names it
pub const TERMINAL: &str = "xdg-terminal-exec";

const APPLICATION: &str = "org.freedesktop.Application";

// how long an app has to answer `Activate`, as long as libdbus waits by default
const ANSWER: Duration = Duration::from_secs(25);

// the bus's own errors for a call it could not deliver: no such app, or it failed to start one
const UNDELIVERED: [&str; 2] = [
    "org.freedesktop.DBus.Error.ServiceUnknown",
    "org.freedesktop.DBus.Error.NameHasNoOwner",
];
const SPAWN: &str = "org.freedesktop.DBus.Error.Spawn.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    // the bus name to activate, the file id without `.desktop`, for `DBusActivatable=true`
    activate: Option<String>,

    // `Exec`; for an activatable entry, what runs when the bus could not deliver `Activate`
    command: Option<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Command {
    argv: Vec<String>,

    // the arguments as `Exec` writes them, its field codes dropped, for a search to read
    written: String,

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

        // what each code gives when it gives nothing: dropped
        let unexpanded = Fields {
            name: "",
            icon: None,
            file: Path::new(""),
            ..*fields
        };

        let command = fields
            .exec
            .and_then(split)
            .map(|args| {
                let argv: Vec<String> = args.iter().flat_map(|arg| expand(arg, fields)).collect();
                let written: Vec<String> = args
                    .iter()
                    .flat_map(|arg| expand(arg, &unexpanded))
                    .filter(|arg| !arg.is_empty())
                    .collect();

                (argv, written.join(" "))
            })
            .filter(|(argv, _)| !argv.is_empty())
            .map(|(argv, written)| Command {
                argv,
                written,
                dir: fields.path.map(String::from),
                terminal: fields.terminal,
            });

        (activate.is_some() || command.is_some()).then_some(Launch { activate, command })
    }

    // `Exec`'s arguments as written, its field codes dropped; none for an entry that only activates
    pub fn command(&self) -> Option<&str> {
        self.command
            .as_ref()
            .map(|command| command.written.as_str())
    }

    // starts it; blocks on the bus and niri, so off the view thread
    pub fn run(&self) -> Result<(), String> {
        self.start(
            |name| match connection::Builder::session()
                .and_then(|bus| bus.method_timeout(ANSWER).build())
            {
                Ok(bus) => activate(&bus, name),
                Err(error) => Activation::Undelivered(format!("no session bus: {error}")),
            },
            Command::run,
        )
    }

    // activates it when it is activatable, running `Exec` only when that was not delivered
    fn start(
        &self,
        activate: impl FnOnce(&str) -> Activation,
        spawn: impl FnOnce(&Command) -> Result<(), String>,
    ) -> Result<(), String> {
        let activated = self.activate.as_deref().map(|name| (name, activate(name)));

        match (activated, &self.command) {
            (Some((_, Activation::Done)), _) => Ok(()),
            (Some((name, Activation::Unknown(why))), _) => Err(format!(
                "activating {name}: {why}; it may still start, so Exec does not run"
            )),
            (Some((name, Activation::Undelivered(why))), None) => {
                Err(format!("activating {name}: {why}"))
            }
            (Some((_, Activation::Undelivered(_))) | None, Some(command)) => spawn(command),
            (None, None) => Err(String::from("nothing to launch")),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Activation {
    Done,

    // the bus says no app got `Activate`, so starting it another way starts it once
    Undelivered(String),

    // an app may have got it: no answer in time, the connection lost, or the app's own error
    Unknown(String),
}

// `org.freedesktop.Application.Activate`, which D-Bus starts the app for
fn activate(bus: &Connection, name: &str) -> Activation {
    let path = format!("/{}", name.replace('.', "/").replace('-', "_"));

    match bus.call_method(
        Some(name),
        path.as_str(),
        Some(APPLICATION),
        "Activate",
        &(HashMap::<&str, Value>::new(),),
    ) {
        Ok(_) => Activation::Done,
        Err(zbus::Error::MethodError(error, _, _))
            if UNDELIVERED.contains(&error.as_str()) || error.starts_with(SPAWN) =>
        {
            Activation::Undelivered(error.to_string())
        }
        Err(error) => Activation::Unknown(error.to_string()),
    }
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
    fn an_activatable_entry_activates_with_exec_in_reserve() {
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

    /*
     * an activatable app on a socket pair, with no bus between, that answers `Activate` as `answer`
     * says, or never; gives the client and whether the app got `Activate`
     */
    fn app(answer: Option<&'static str>) -> (Connection, std::sync::mpsc::Receiver<String>) {
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let (got, activated) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let app = connection::Builder::async_io_unix_stream(theirs)
                .server(zbus::Guid::generate())
                .unwrap()
                .p2p()
                .build()
                .unwrap();

            // until the client hangs up
            for message in zbus::blocking::MessageIterator::from(&app).map_while(Result::ok) {
                let header = message.header();

                if header.member().is_some_and(|member| member == "Activate") {
                    got.send(header.path().unwrap().to_string()).unwrap();

                    if let Some(error) = answer {
                        app.reply_error(&header, error, &("no",)).unwrap();
                    }
                }
            }
        });

        // a reply has time to come on a loaded machine; only one that never comes is waited out
        let wait = if answer.is_some() {
            Duration::from_secs(5)
        } else {
            Duration::from_millis(200)
        };
        let bus = connection::Builder::async_io_unix_stream(ours)
            .p2p()
            .method_timeout(wait)
            .build()
            .unwrap();

        (bus, activated)
    }

    fn launch_with_exec() -> Launch {
        Launch::of(&Fields {
            dbus_activatable: true,
            ..fields("app")
        })
        .unwrap()
    }

    #[test]
    fn an_app_that_got_activate_but_never_answers_is_not_started_again() {
        let (bus, activated) = app(None);
        let mut spawned = false;

        let started = launch_with_exec().start(
            |name| activate(&bus, name),
            |_| {
                spawned = true;
                Ok(())
            },
        );

        assert_eq!(activated.try_recv().as_deref(), Ok("/org/example/App"));
        assert!(started.is_err_and(|error| error.contains("Exec does not run")));
        assert!(!spawned);
    }

    #[test]
    fn exec_runs_only_when_the_bus_could_not_deliver_activate() {
        let outcome = |answer| {
            let (bus, _activated) = app(Some(answer));
            let mut spawned = false;
            let started = launch_with_exec().start(
                |name| activate(&bus, name),
                |_| {
                    spawned = true;
                    Ok(())
                },
            );

            (started.is_ok(), spawned)
        };

        assert_eq!(
            outcome("org.freedesktop.DBus.Error.ServiceUnknown"),
            (true, true)
        );
        assert_eq!(
            outcome("org.freedesktop.DBus.Error.Spawn.ChildExited"),
            (true, true)
        );

        // the app's own error: it got `Activate`, so it may be on its way
        assert_eq!(outcome("org.example.App.Error.Busy"), (false, false));
        assert_eq!(
            outcome("org.freedesktop.DBus.Error.NoReply"),
            (false, false)
        );

        // nothing to fall back to
        let (bus, _activated) = app(Some("org.freedesktop.DBus.Error.ServiceUnknown"));
        let bare = Launch {
            command: None,
            ..launch_with_exec()
        };
        assert!(bare.start(|name| activate(&bus, name), |_| Ok(())).is_err());
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
