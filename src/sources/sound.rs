//! Notification sounds (`notifications.sound`): a notification arriving plays a sound through a
//! helper program, `pw-play` or else `canberra-gtk-play`, so Kanade links no audio library.
//!
//! Which sound comes from layers, the first that says something winning (ADR 0118): the matching
//! rule's `sound` (a file or a name, or none), the sender's hints (`sound-file` an absolute path,
//! `sound-name` a freedesktop sound theme name, or `suppress-sound` for none), the sound of the
//! notification's urgency (`notifications.sound_low`, `_normal`, `_critical`),
//! `notifications.sound_file` and last `message-new-instant`. A layer of the user's whose file is
//! missing or whose spelling is no good is skipped with a note on stderr, never an error. The
//! volume is the stream's own (`sound_volume`, or a rule's `volume`), never the system's.
//!
//! One sound plays at a time and a notification arriving meanwhile is silent, as it is when no
//! player is installed. A player runs for at most `LIMIT`, through `setpriv` so the kernel kills it
//! with Kanade. A name is checked and a file must be an existing regular file before either is
//! handed to a player, which gets them as arguments, never through a shell.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use super::icons;
use super::notifications::Urgency;
use super::wake::{self, SETPRIV};
use crate::appearance::child::{self, Interrupt, Ran};

// in the order tried
pub const PLAYERS: [&str; 2] = ["pw-play", "canberra-gtk-play"];

const DEFAULT_NAME: &str = "message-new-instant";

// what silences a configured sound
const OFF: &str = "off";

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

// what one layer of the user's says: nothing, no sound, or this sound
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Choice {
    // says nothing; the next layer decides
    #[default]
    Default,

    // no sound
    Off,

    // a file or a sound theme name, as spelled; checked as it is used
    Sound(String),
}

impl Choice {
    // a configured spelling: unset or empty says nothing, `off` no sound, the rest a file or a name
    pub fn spelled(text: Option<&str>) -> Choice {
        match text.map(str::trim) {
            None | Some("") => Choice::Default,
            Some(text) if text.eq_ignore_ascii_case(OFF) => Choice::Off,
            Some(text) => Choice::Sound(text.to_owned()),
        }
    }

    // a spelling that has to be well formed, as a rule's: an absolute file (or `~/`) or a plain name
    pub fn checked(text: &str) -> Result<Choice, &'static str> {
        let choice = Choice::spelled(Some(text));

        match &choice {
            Choice::Sound(text) if spell(text, Some(Path::new("/"))).is_none() => {
                Err("expected an absolute file path, a sound name or off")
            }
            _ => Ok(choice),
        }
    }
}

// the sounds the config chooses (`notifications.sound_*`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sounds {
    // `sound_file`: the sound of a notification whose sender and urgency say none
    pub file: Option<String>,

    // `sound_low`, `sound_normal`, `sound_critical`
    pub low: Option<String>,
    pub normal: Option<String>,
    pub critical: Option<String>,

    // `sound_volume`, whole percent
    pub volume: u32,
}

impl Default for Sounds {
    fn default() -> Self {
        Sounds {
            file: None,
            low: None,
            normal: None,
            critical: None,
            volume: 100,
        }
    }
}

impl Sounds {
    // the key and the choice of an urgency
    fn of(&self, urgency: Urgency) -> (&'static str, Option<&str>) {
        match urgency {
            Urgency::Low => ("notifications.sound_low", self.low.as_deref()),
            Urgency::Normal => ("notifications.sound_normal", self.normal.as_deref()),
            Urgency::Critical => ("notifications.sound_critical", self.critical.as_deref()),
        }
    }
}

// a notification that rings: what decides its sound
#[derive(Debug, Clone)]
pub struct Ring {
    pub hints: Hints,

    // the urgency it counts as, a rule's included
    pub urgency: Urgency,

    // its rule's `sound`
    pub rule: Choice,

    // its rule's `volume`, percent
    pub volume: Option<u32>,
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

/*
 * the first of `players` that can play the cue, with its arguments: pw-play cannot look up a name
 * the freedesktop theme lacks, which canberra-gtk-play can, in the theme in use
 */
fn playing<'a>(
    players: impl IntoIterator<Item = &'a str>,
    cue: &Cue,
    volume: u32,
    themed: impl Fn(&str) -> Option<PathBuf>,
) -> Option<(&'a str, Vec<OsString>)> {
    players
        .into_iter()
        .find_map(|player| Some((player, arguments(player, cue, volume, &themed)?)))
}

static PLAYING: AtomicBool = AtomicBool::new(false);

// frees the one place when the sound ends, however it does
struct Playing;

impl Drop for Playing {
    fn drop(&mut self) {
        PLAYING.store(false, Ordering::Release);
    }
}

// plays the sound the layers choose unless one plays already or there is no player; never blocks
pub fn play(ring: &Ring, sounds: &Sounds) {
    let volume = ring.volume.unwrap_or(sounds.volume).min(100);

    if volume == 0 {
        return;
    }

    let home = env::var_os("HOME").map(PathBuf::from);
    let (cue, notes) = cue(ring, sounds, home.as_deref(), regular, resolvable);

    for note in notes {
        eprintln!("kanade: notification sound: {note}");
    }

    let Some(cue) = cue else {
        return;
    };
    let installed = PLAYERS.into_iter().filter(|player| wake::found(player));
    let Some((player, arguments)) = playing(installed, &cue, volume, themed) else {
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

// what of the chosen sounds cannot be used, for `kanade doctor`
pub fn unusable(sounds: &Sounds) -> Vec<String> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let mut notes = Vec::new();

    for (label, text) in [
        ("notifications.sound_file", sounds.file.as_deref()),
        sounds.of(Urgency::Low),
        sounds.of(Urgency::Normal),
        sounds.of(Urgency::Critical),
    ] {
        let choice = Choice::spelled(text);

        layer(
            &choice,
            label,
            home.as_deref(),
            regular,
            resolvable,
            &mut notes,
        );
    }

    notes
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

// what one layer gives
enum Layer {
    // nothing, or nothing usable: the next layer decides
    Next,

    // no sound
    Silent,

    Cue(Cue),
}

/*
 * a spelling as a cue: `~/` is the home directory, any other path must be absolute, and what has
 * no `/` is a plain name
 */
fn spell(text: &str, home: Option<&Path>) -> Option<Cue> {
    if let Some(rest) = text.strip_prefix("~/") {
        let home = home.filter(|home| home.is_absolute())?;

        return Some(Cue::File(home.join(rest)));
    }

    if text.contains('/') {
        return Path::new(text)
            .is_absolute()
            .then(|| Cue::File(PathBuf::from(text)));
    }

    valid(text).then(|| Cue::Name(text.to_owned()))
}

// a layer of the user's; a sound it cannot use (a missing file, a name `known` lacks) is noted under `label` and left to the next layer
fn layer(
    choice: &Choice,
    label: &str,
    home: Option<&Path>,
    regular: impl Fn(&Path) -> bool,
    known: impl Fn(&str) -> bool,
    notes: &mut Vec<String>,
) -> Layer {
    let text = match choice {
        Choice::Default => return Layer::Next,
        Choice::Off => return Layer::Silent,
        Choice::Sound(text) => text,
    };

    match spell(text, home) {
        Some(Cue::File(file)) if !regular(&file) => {
            notes.push(format!("{label}: {text} is not a readable file, skipped"));

            Layer::Next
        }
        Some(Cue::Name(name)) if !known(&name) => {
            notes.push(format!(
                "{label}: {text} is not a sound the installed player can find, skipped"
            ));

            Layer::Next
        }
        Some(cue) => Layer::Cue(cue),
        None => {
            notes.push(format!(
                "{label}: {text} is not an absolute file path or a plain sound name, skipped"
            ));

            Layer::Next
        }
    }
}

// what the sender's hints give
enum Sender {
    // asks for nothing
    Absent,

    // `suppress-sound`, or a hint that is no good
    Silent,

    Cue(Cue),
}

// a usable file, else a name
fn sender(hints: &Hints, regular: impl Fn(&Path) -> bool) -> Sender {
    if hints.suppress {
        return Sender::Silent;
    }

    let file = Path::new(&hints.file);

    if !hints.file.is_empty() && file.is_absolute() && regular(file) {
        return Sender::Cue(Cue::File(file.to_owned()));
    }

    match hints.name.as_str() {
        "" if hints.file.is_empty() => Sender::Absent,
        name if valid(name) => Sender::Cue(Cue::Name(name.to_owned())),
        _ => Sender::Silent,
    }
}

/*
 * what to play, with notes of the chosen sounds that could not be used: the rule's sound, then the
 * sender's hints (`suppress-sound` silences all below), the sound of the urgency, `sound_file`, the
 * default. A rule's sound is the one thing that beats `suppress-sound`.
 */
fn cue(
    ring: &Ring,
    sounds: &Sounds,
    home: Option<&Path>,
    regular: impl Fn(&Path) -> bool + Copy,
    known: impl Fn(&str) -> bool + Copy,
) -> (Option<Cue>, Vec<String>) {
    let mut notes = Vec::new();

    match layer(
        &ring.rule,
        "a rule's sound",
        home,
        regular,
        known,
        &mut notes,
    ) {
        Layer::Silent => return (None, notes),
        Layer::Cue(cue) => return (Some(cue), notes),
        Layer::Next => {}
    }

    match sender(&ring.hints, regular) {
        Sender::Silent => return (None, notes),
        Sender::Cue(cue) => return (Some(cue), notes),
        Sender::Absent => {}
    }

    let (urgency, text) = sounds.of(ring.urgency);

    for (label, text) in [
        (urgency, text),
        ("notifications.sound_file", sounds.file.as_deref()),
    ] {
        match layer(
            &Choice::spelled(text),
            label,
            home,
            regular,
            known,
            &mut notes,
        ) {
            Layer::Silent => return (None, notes),
            Layer::Cue(cue) => return (Some(cue), notes),
            Layer::Next => {}
        }
    }

    (Some(Cue::Name(String::from(DEFAULT_NAME))), notes)
}

/*
 * whether an installed player can find a name: pw-play only in the freedesktop theme, canberra in
 * the theme in use, so it is not looked up here
 */
fn resolvable(name: &str) -> bool {
    wake::found("canberra-gtk-play") || themed(name).is_some()
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

/*
 * the player's arguments, none when it cannot play the cue; `themed` finds a name's file. A volume
 * under 100 % is the stream's own, never the system's: pw-play takes it linear, canberra in dB
 */
fn arguments(
    player: &str,
    cue: &Cue,
    volume: u32,
    themed: impl Fn(&str) -> Option<PathBuf>,
) -> Option<Vec<OsString>> {
    let mut arguments: Vec<OsString> = Vec::new();
    let canberra = player == "canberra-gtk-play";

    if volume < 100 {
        let linear = f64::from(volume) / 100.0;

        if canberra {
            arguments.extend(["-V".into(), format!("{:.1}", 20.0 * linear.log10()).into()]);
        } else {
            arguments.push(format!("--volume={linear:.2}").into());
        }
    }

    match (canberra, cue) {
        (true, Cue::File(file)) => arguments.extend(["-f".into(), file.into()]),
        (true, Cue::Name(name)) => arguments.extend(["-i".into(), name.into()]),
        (false, Cue::File(file)) => arguments.push(file.into()),
        (false, Cue::Name(name)) => arguments.push(themed(name)?.into()),
    }

    Some(arguments)
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

    fn ring(hints: Hints) -> Ring {
        Ring {
            hints,
            urgency: Urgency::Normal,
            rule: Choice::Default,
            volume: None,
        }
    }

    fn name(name: &str) -> Option<Cue> {
        Some(Cue::Name(name.to_owned()))
    }

    fn file(file: &str) -> Option<Cue> {
        Some(Cue::File(file.into()))
    }

    // the cue for a ring and sounds, `regular` saying whether every file exists
    fn chosen(ring: &Ring, sounds: &Sounds, regular: bool) -> Option<Cue> {
        cue(
            ring,
            sounds,
            Some(Path::new("/home/k")),
            |_| regular,
            |_| true,
        )
        .0
    }

    fn sounds(file: &str, low: &str, normal: &str, critical: &str) -> Sounds {
        let some = |text: &str| (!text.is_empty()).then(|| text.to_owned());

        Sounds {
            file: some(file),
            low: some(low),
            normal: some(normal),
            critical: some(critical),
            volume: 100,
        }
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
            assert_eq!(
                chosen(&ring(hints("", bad)), &Sounds::default(), true),
                None,
                "{bad}"
            );
        }

        assert_eq!(
            chosen(
                &ring(hints("", "message-new-instant")),
                &Sounds::default(),
                true
            ),
            name("message-new-instant")
        );
    }

    #[test]
    fn a_hinted_file_must_be_absolute_and_a_regular_file() {
        let plays =
            |path: &str, regular| chosen(&ring(hints(path, "")), &Sounds::default(), regular);

        assert_eq!(plays("/a/b.oga", true), file("/a/b.oga"));
        assert_eq!(plays("/a/b.oga", false), None);
        assert_eq!(plays("b.oga", true), None);
        assert_eq!(plays("-v", true), None);
        assert_eq!(plays("file:///a/b.oga", true), None);
    }

    // a file wins over a name, a file that is no good falls back to the name, and none given plays
    // the default
    #[test]
    fn the_hinted_file_comes_first_then_the_name_then_the_default() {
        let none = Sounds::default();

        assert_eq!(
            chosen(&ring(hints("/a.oga", "bell")), &none, true),
            file("/a.oga")
        );
        assert_eq!(
            chosen(&ring(hints("/a.oga", "bell")), &none, false),
            name("bell")
        );
        assert_eq!(
            chosen(&ring(hints("", "")), &none, true),
            name(DEFAULT_NAME)
        );
    }

    // rule, then the sender, then the urgency, then `sound_file`, then the default
    #[test]
    fn the_layers_come_in_order() {
        let all = sounds("/file.oga", "low", "/normal.oga", "critical");
        let mut ring = ring(hints("", ""));

        assert_eq!(chosen(&ring, &all, true), file("/normal.oga"));

        ring.urgency = Urgency::Low;
        assert_eq!(chosen(&ring, &all, true), name("low"));
        ring.urgency = Urgency::Critical;
        assert_eq!(chosen(&ring, &all, true), name("critical"));

        // an urgency with no sound of its own takes `sound_file`, then the default
        let some = sounds("/file.oga", "", "", "");
        assert_eq!(chosen(&ring, &some, true), file("/file.oga"));
        assert_eq!(chosen(&ring, &Sounds::default(), true), name(DEFAULT_NAME));

        // the sender's hint beats the urgency's, the rule's beats the sender's
        ring.hints = hints("", "bell");
        assert_eq!(chosen(&ring, &all, true), name("bell"));
        ring.rule = Choice::Sound(String::from("~/rule.oga"));
        assert_eq!(chosen(&ring, &all, true), file("/home/k/rule.oga"));
    }

    #[test]
    fn off_is_silent_where_it_is_said() {
        let off = |text: &str| Choice::spelled(Some(text));
        let mut ring = ring(hints("", ""));

        assert_eq!(off("OFF"), Choice::Off);
        assert_eq!(off(" "), Choice::Default);

        // the urgency's off silences it and the layers below, and no other urgency
        let quiet = sounds("/file.oga", "off", "", "");
        ring.urgency = Urgency::Low;
        assert_eq!(chosen(&ring, &quiet, true), None);
        ring.urgency = Urgency::Normal;
        assert_eq!(chosen(&ring, &quiet, true), file("/file.oga"));

        // `sound_file = "off"` silences what has no sound of its own, never a sender's
        let nothing = sounds("off", "", "", "");
        assert_eq!(chosen(&ring, &nothing, true), None);
        ring.hints = hints("", "bell");
        assert_eq!(chosen(&ring, &nothing, true), name("bell"));

        ring.rule = Choice::Off;
        assert_eq!(chosen(&ring, &Sounds::default(), true), None);
    }

    // `suppress-sound` silences the urgency's sound, `sound_file` and the default, not a rule's own sound
    #[test]
    fn suppress_sound_silences_all_but_a_rules_sound() {
        let all = sounds("/file.oga", "", "bell", "");
        let mut ring = ring(Hints {
            suppress: true,
            ..hints("/a.oga", "bell")
        });

        assert_eq!(chosen(&ring, &all, true), None);

        ring.rule = Choice::Sound(String::from("/rule.oga"));
        assert_eq!(chosen(&ring, &all, true), file("/rule.oga"));
    }

    // a configured file that is gone, or spelled badly, is noted and the next layer plays, never an error
    #[test]
    fn a_chosen_sound_that_is_no_good_falls_through_with_a_note() {
        let all = sounds("/gone.oga", "", "-v", "");
        let ring = ring(hints("", ""));
        let (played, notes) = cue(&ring, &all, None, |_| false, |_| true);

        assert_eq!(played, name(DEFAULT_NAME));
        assert_eq!(notes.len(), 2);
        assert!(
            notes[0].starts_with("notifications.sound_normal: -v"),
            "{notes:?}"
        );
        assert!(
            notes[1].contains("/gone.oga is not a readable file"),
            "{notes:?}"
        );

        // `~/` needs a home directory
        let tilde = sounds("~/a.oga", "", "", "");
        assert_eq!(cue(&ring, &tilde, None, |_| true, |_| true).1.len(), 1);
    }

    // with only pw-play, a configured name the theme lacks is a missing file: noted, the next layer plays
    #[test]
    fn a_name_no_player_can_find_falls_through_with_a_note() {
        let mut ring = ring(hints("", ""));
        let all = sounds("/file.oga", "", "chime", "");
        let known = |name: &str| name != "chime" && name != "gong";

        let (played, notes) = cue(&ring, &all, None, |_| true, known);
        assert_eq!(played, file("/file.oga"));
        assert_eq!(notes.len(), 1);
        assert!(
            notes[0].starts_with("notifications.sound_normal: chime"),
            "{notes:?}"
        );

        // a rule's name too, then nothing else is configured: the default
        ring.rule = Choice::Sound(String::from("gong"));
        let (played, notes) = cue(&ring, &Sounds::default(), None, |_| true, known);
        assert_eq!(played, name(DEFAULT_NAME));
        assert!(notes[0].starts_with("a rule's sound: gong"), "{notes:?}");

        // a name the theme has is kept
        ring.rule = Choice::Sound(String::from("bell"));
        assert_eq!(cue(&ring, &all, None, |_| true, known).0, name("bell"));
    }

    #[test]
    fn a_rule_sound_must_be_well_formed() {
        assert_eq!(Choice::checked("off"), Ok(Choice::Off));
        assert_eq!(
            Choice::checked("/a/b.oga"),
            Ok(Choice::Sound(String::from("/a/b.oga")))
        );
        assert_eq!(
            Choice::checked("~/b.oga"),
            Ok(Choice::Sound(String::from("~/b.oga")))
        );
        assert_eq!(
            Choice::checked("bell"),
            Ok(Choice::Sound(String::from("bell")))
        );

        for bad in ["b/c.oga", "-v", "a b", "$(x)", "../x"] {
            assert!(Choice::checked(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn each_player_gets_a_cue_in_its_own_way() {
        let themed = |name: &str| Some(PathBuf::from(format!("/sounds/{name}.oga")));
        let file = Cue::File("/a.oga".into());
        let bell = Cue::Name(String::from("bell"));
        let args = |player, cue, volume| arguments(player, cue, volume, themed);

        assert_eq!(args("pw-play", &file, 100), Some(vec!["/a.oga".into()]));
        assert_eq!(
            args("pw-play", &bell, 100),
            Some(vec!["/sounds/bell.oga".into()])
        );
        assert_eq!(
            args("canberra-gtk-play", &file, 100),
            Some(vec!["-f".into(), "/a.oga".into()])
        );
        assert_eq!(
            args("canberra-gtk-play", &bell, 100),
            Some(vec!["-i".into(), "bell".into()])
        );
        assert_eq!(arguments("pw-play", &bell, 100, |_| None), None);
    }

    // the volume is the stream's: pw-play's linear, canberra's in dB, and full volume passes none
    #[test]
    fn a_volume_is_the_streams_own() {
        let file = Cue::File("/a.oga".into());
        let args = |player, volume| arguments(player, &file, volume, |_| None);

        assert_eq!(
            args("pw-play", 50),
            Some(vec!["--volume=0.50".into(), "/a.oga".into()])
        );
        assert_eq!(
            args("canberra-gtk-play", 50),
            Some(vec![
                "-V".into(),
                "-6.0".into(),
                "-f".into(),
                "/a.oga".into()
            ])
        );
        assert_eq!(args("pw-play", 100), Some(vec!["/a.oga".into()]));
    }

    // with both players installed and no theme file for a name, pw-play could not play it and the
    // notification stayed silent
    #[test]
    fn a_name_pw_play_cannot_find_goes_to_canberra() {
        let bell = Cue::Name(String::from("bell"));
        let none = |_: &str| None;

        assert_eq!(
            playing(PLAYERS, &bell, 100, none),
            Some(("canberra-gtk-play", vec!["-i".into(), "bell".into()]))
        );
        assert_eq!(playing(["pw-play"], &bell, 100, none), None);
        assert_eq!(
            playing(PLAYERS, &bell, 100, |_| Some(PathBuf::from("/bell.oga"))),
            Some(("pw-play", vec!["/bell.oga".into()]))
        );
    }

    // a path from a hint never reaches a shell: it is one argument, however it is spelled
    #[test]
    fn a_path_stays_one_argument() {
        let file = Cue::File("/a b/$(x);y.oga".into());

        assert_eq!(
            arguments("pw-play", &file, 100, |_| None),
            Some(vec!["/a b/$(x);y.oga".into()])
        );
    }
}
