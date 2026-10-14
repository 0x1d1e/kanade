//! Notification sounds (`notifications.sound`): a notification arriving plays the sound its sender
//! asked for through a helper program, `pw-play` or else `canberra-gtk-play`, so Kanade links no
//! audio library. The sender asks with the `sound-file` hint (an absolute path) or `sound-name` (a
//! freedesktop sound theme name, like `message-new-instant`, which is also the default), or with
//! `suppress-sound` for none.
//!
//! One sound plays at a time and a notification arriving meanwhile is silent, as it is when no
//! player is installed. A player runs for at most `LIMIT`, through `setpriv` so the kernel kills it
//! with Kanade. A name is checked and a file must be an existing regular file before either is
//! handed to a player, which gets them as arguments, never through a shell.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use super::icons;
use super::wake::{self, SETPRIV};
use crate::appearance::child::{self, Interrupt, Ran};

// in the order tried
pub const PLAYERS: [&str; 2] = ["pw-play", "canberra-gtk-play"];

const DEFAULT_NAME: &str = "message-new-instant";

// the longest a sound plays, a player that hangs is killed then
const LIMIT: Duration = Duration::from_secs(5);

const NAME_LENGTH: usize = 64;

// what a notification's sender asked for in its hints; none of it is trusted
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hints {
    // `suppress-sound`
    pub suppress: bool,

    // `sound-file`, empty without
    pub file: String,

    // `sound-name`, empty without
    pub name: String,
}

#[derive(Debug, PartialEq, Eq)]
enum Cue {
    File(PathBuf),
    Name(String),
}

// the first of `PLAYERS` on the PATH
pub fn player() -> Option<&'static str> {
    PLAYERS.into_iter().find(|player| wake::found(player))
}

static PLAYING: AtomicBool = AtomicBool::new(false);

// frees the one place when the sound ends, however it does
struct Playing;

impl Drop for Playing {
    fn drop(&mut self) {
        PLAYING.store(false, Ordering::Release);
    }
}

// plays the sound the hints ask for unless one plays already or there is no player; never blocks
pub fn play(hints: &Hints) {
    let Some(player) = player() else {
        return;
    };
    let Some(cue) = cue(hints, regular) else {
        return;
    };
    let Some(arguments) = arguments(player, &cue, themed) else {
        return;
    };

    if PLAYING.swap(true, Ordering::AcqRel) {
        return;
    }

    let playing = Playing;

    let started = thread::Builder::new()
        .name(String::from("kanade-sound"))
        .spawn(move || {
            let _playing = playing;

            run(player, &arguments);
        });

    if let Err(error) = started {
        eprintln!("kanade: notification sound: could not start a thread: {error}");
    }
}

fn run(player: &str, arguments: &[OsString]) {
    let mut command = if wake::guards() {
        let mut command = Command::new(SETPRIV);

        command.args(["--pdeathsig", "KILL", player]);
        command
    } else {
        Command::new(player)
    };

    command.args(arguments).stdin(Stdio::null());

    if let Ran::Done(Err(why)) = child::run(command, player, LIMIT, || None::<Interrupt<()>>) {
        eprintln!("kanade: notification sound: {why}");
    }
}

// what to play: the file if it is a usable one, else the name, else the default; none if suppressed
fn cue(hints: &Hints, regular: impl Fn(&Path) -> bool) -> Option<Cue> {
    if hints.suppress {
        return None;
    }

    let file = Path::new(&hints.file);

    if !hints.file.is_empty() && file.is_absolute() && regular(file) {
        return Some(Cue::File(file.to_owned()));
    }

    match hints.name.as_str() {
        "" if hints.file.is_empty() => Some(Cue::Name(String::from(DEFAULT_NAME))),
        name if valid(name) => Some(Cue::Name(name.to_owned())),
        _ => None,
    }
}

// a plain theme name, never a path or something a player takes as an option
fn valid(name: &str) -> bool {
    (1..=NAME_LENGTH).contains(&name.len())
        && !name.starts_with(['-', '.'])
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

fn regular(file: &Path) -> bool {
    fs::metadata(file).is_ok_and(|file| file.is_file())
}

// the player's arguments, none when it cannot play the cue; `themed` finds a name's file
fn arguments(
    player: &str,
    cue: &Cue,
    themed: impl Fn(&str) -> Option<PathBuf>,
) -> Option<Vec<OsString>> {
    match (player, cue) {
        ("canberra-gtk-play", Cue::File(file)) => Some(vec!["-f".into(), file.into()]),
        ("canberra-gtk-play", Cue::Name(name)) => Some(vec!["-i".into(), name.into()]),
        (_, Cue::File(file)) => Some(vec![file.into()]),
        (_, Cue::Name(name)) => themed(name).map(|file| vec![file.into()]),
    }
}

// the file of a name in the freedesktop sound theme, which pw-play cannot look up itself
fn themed(name: &str) -> Option<PathBuf> {
    let directory = Path::new("sounds/freedesktop/stereo");

    icons::data_dirs().into_iter().find_map(|data| {
        ["oga", "ogg", "wav"]
            .iter()
            .map(|extension| data.join(directory).join(format!("{name}.{extension}")))
            .find(|file| regular(file))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hints(file: &str, name: &str) -> Hints {
        Hints {
            suppress: false,
            file: file.to_owned(),
            name: name.to_owned(),
        }
    }

    fn name(name: &str) -> Option<Cue> {
        Some(Cue::Name(name.to_owned()))
    }

    // a sender's hints reach a player as arguments, so nothing but a plain name or a real file does
    #[test]
    fn a_name_that_is_not_a_plain_theme_name_plays_nothing() {
        for bad in [
            "../etc/passwd",
            "a/b",
            "-v",
            ".hidden",
            "a b",
            "a;b",
            "$(x)",
            "tab\t",
            &"a".repeat(NAME_LENGTH + 1),
        ] {
            assert_eq!(cue(&hints("", bad), |_| true), None, "{bad}");
        }

        assert_eq!(
            cue(&hints("", "message-new-instant"), |_| true),
            name("message-new-instant")
        );
    }

    #[test]
    fn a_file_must_be_absolute_and_a_regular_file() {
        let file = |file: &str, regular: bool| cue(&hints(file, ""), |_| regular);

        assert_eq!(file("/a/b.oga", true), Some(Cue::File("/a/b.oga".into())));
        assert_eq!(file("/a/b.oga", false), None);
        assert_eq!(file("b.oga", true), None);
        assert_eq!(file("-v", true), None);
        assert_eq!(file("file:///a/b.oga", true), None);
    }

    // a file wins over a name, a file that is no good falls back to the name, and none given plays
    // the default
    #[test]
    fn the_file_comes_first_then_the_name_then_the_default() {
        assert_eq!(
            cue(&hints("/a.oga", "bell"), |_| true),
            Some(Cue::File("/a.oga".into()))
        );
        assert_eq!(cue(&hints("/a.oga", "bell"), |_| false), name("bell"));
        assert_eq!(cue(&hints("", ""), |_| true), name(DEFAULT_NAME));
    }

    #[test]
    fn suppress_sound_plays_nothing() {
        let hints = Hints {
            suppress: true,
            ..hints("/a.oga", "bell")
        };

        assert_eq!(cue(&hints, |_| true), None);
    }

    #[test]
    fn each_player_gets_a_cue_in_its_own_way() {
        let themed = |name: &str| Some(PathBuf::from(format!("/sounds/{name}.oga")));
        let file = Cue::File("/a.oga".into());
        let bell = Cue::Name(String::from("bell"));
        let args = |player, cue| arguments(player, cue, themed);

        assert_eq!(args("pw-play", &file), Some(vec!["/a.oga".into()]));
        assert_eq!(
            args("pw-play", &bell),
            Some(vec!["/sounds/bell.oga".into()])
        );
        assert_eq!(
            args("canberra-gtk-play", &file),
            Some(vec!["-f".into(), "/a.oga".into()])
        );
        assert_eq!(
            args("canberra-gtk-play", &bell),
            Some(vec!["-i".into(), "bell".into()])
        );
        assert_eq!(arguments("pw-play", &bell, |_| None), None);
    }

    // a path from a hint never reaches a shell: it is one argument, however it is spelled
    #[test]
    fn a_path_stays_one_argument() {
        let file = Cue::File("/a b/$(x);y.oga".into());

        assert_eq!(
            arguments("pw-play", &file, |_| None),
            Some(vec!["/a b/$(x);y.oga".into()])
        );
    }
}
