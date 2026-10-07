//! What `kanade <verb>` asks of the island (docs/design.md CLI). Pure: words in, Command out;
//! `crate::cli` says which words reach which of these.

use std::time::Duration;

use super::activity::{Activity, Id, Interrupt, InvalidActivity, Kind, Lifetime, Priority, Scope};
use super::presentation::Surface;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Open(Surface),

    // collapses the focused island when it shows this Surface
    Close(Surface),

    // open, or collapse when the focused island already shows it
    Toggle(Surface),

    Collapse,

    SetDnd(bool),
    ToggleDnd,

    // a fake Activity, for testing what the island does with one
    Post(Activity),
    Withdraw(Id),
}

impl Command {
    // `open|close|toggle`, after the Surface's name
    pub fn surface(surface: Surface, arguments: &[&str]) -> Option<Command> {
        match *arguments {
            ["open"] => Some(Command::Open(surface)),
            ["close"] => Some(Command::Close(surface)),
            ["toggle"] => Some(Command::Toggle(surface)),
            _ => None,
        }
    }

    // `on|off|toggle`, after `dnd`
    pub fn dnd(arguments: &[&str]) -> Option<Command> {
        match *arguments {
            ["on"] => Some(Command::SetDnd(true)),
            ["off"] => Some(Command::SetDnd(false)),
            ["toggle"] => Some(Command::ToggleDnd),
            _ => None,
        }
    }

    /*
     * `post ...|withdraw ...`, after `debug`; a fake Activity whose policy cannot be carried out
     * says why, anything else unknown gets the usage text
     */
    pub fn debug(arguments: &[&str]) -> Result<Command, Unparsed> {
        match *arguments {
            ["post", kind, key, priority, lifetime, scope, interrupt] => {
                let id = Id::new(Kind::parse(kind).ok_or(Unparsed::Usage)?, key);
                let policy = (|| {
                    Some((
                        Priority::parse(priority)?,
                        parse_lifetime(lifetime)?,
                        parse_scope(scope)?,
                        parse_interrupt(interrupt)?,
                    ))
                })();
                let (priority, lifetime, scope, interrupt) = policy.ok_or(Unparsed::Usage)?;

                Activity::new(id, priority, lifetime, scope, interrupt)
                    .map(Command::Post)
                    .map_err(Unparsed::Invalid)
            }
            ["withdraw", kind, key] => Kind::parse(kind)
                .map(|kind| Command::Withdraw(Id::new(kind, key)))
                .ok_or(Unparsed::Usage),
            _ => Err(Unparsed::Usage),
        }
    }

    // the `debug` verbs and what their arguments take
    pub fn debug_usage() -> String {
        let kinds: Vec<&str> = Kind::ALL.into_iter().map(Kind::name).collect();
        let priorities: Vec<&str> = Priority::ALL.into_iter().map(Priority::name).collect();

        format!(
            "debug post <kind> <key> <priority> <lifetime> <scope> <interrupt>
debug withdraw <kind> <key>
  <kind>: {}
  <priority>: {}
  <lifetime>: persistent|<ms>
  <scope>: global|focused-output
  <interrupt>: none|preempt|auto-expand:<ms>",
            kinds.join("|"),
            priorities.join("|")
        )
    }
}

// why arguments make no Command
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unparsed {
    // not a verb, or not its arguments
    Usage,

    // a fake Activity with a policy that cannot be carried out
    Invalid(InvalidActivity),
}

// a Transient's milliseconds, zero included so `Activity::new` is the one to refuse it
fn milliseconds(text: &str) -> Option<Duration> {
    text.parse().ok().map(Duration::from_millis)
}

fn parse_lifetime(text: &str) -> Option<Lifetime> {
    match text {
        "persistent" => Some(Lifetime::Persistent),
        ms => milliseconds(ms).map(Lifetime::Transient),
    }
}

fn parse_scope(text: &str) -> Option<Scope> {
    match text {
        "global" => Some(Scope::Global),
        "focused-output" => Some(Scope::FocusedOutput),
        _ => None,
    }
}

fn parse_interrupt(text: &str) -> Option<Interrupt> {
    match text {
        "none" => Some(Interrupt::None),
        "preempt" => Some(Interrupt::Preempt),
        _ => milliseconds(text.strip_prefix("auto-expand:")?).map(Interrupt::AutoExpand),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(arguments: &[&str]) -> Result<Command, Unparsed> {
        Command::debug(arguments)
    }

    fn parse(arguments: &[&str]) -> Option<Command> {
        parsed(arguments).ok()
    }

    #[test]
    fn surface_and_dnd_verbs() {
        let surface = |arguments: &[&str]| Command::surface(Surface::Launcher, arguments);

        assert_eq!(surface(&["open"]), Some(Command::Open(Surface::Launcher)));
        assert_eq!(surface(&["close"]), Some(Command::Close(Surface::Launcher)));
        assert_eq!(
            surface(&["toggle"]),
            Some(Command::Toggle(Surface::Launcher))
        );
        assert_eq!(Command::dnd(&["on"]), Some(Command::SetDnd(true)));
        assert_eq!(Command::dnd(&["off"]), Some(Command::SetDnd(false)));
        assert_eq!(Command::dnd(&["toggle"]), Some(Command::ToggleDnd));

        for arguments in [&[][..], &["Open"], &["open", "eDP-1"], &["collapse"]] {
            assert_eq!(surface(arguments), None, "{arguments:?}");
        }

        for arguments in [&[][..], &["yes"], &["on", "now"]] {
            assert_eq!(Command::dnd(arguments), None, "{arguments:?}");
        }
    }

    #[test]
    fn debug_verbs_post_and_withdraw_fake_activities() {
        let countdown = Id::new(Kind::Timer, "countdown");
        let volume = Id::new(Kind::Volume, "volume");

        let post = |arguments: &[&str]| {
            let mut all = vec!["post"];
            all.extend_from_slice(arguments);
            parse(&all)
        };
        let activity = |id, priority, lifetime, scope, interrupt| {
            Some(Command::Post(
                Activity::new(id, priority, lifetime, scope, interrupt).unwrap(),
            ))
        };

        assert_eq!(
            post(&[
                "timer",
                "countdown",
                "ongoing",
                "persistent",
                "global",
                "none"
            ]),
            activity(
                countdown.clone(),
                Priority::Ongoing,
                Lifetime::Persistent,
                Scope::Global,
                Interrupt::None
            )
        );
        assert_eq!(
            post(&["volume", "volume", "osd", "1200", "focused-output", "none"]),
            activity(
                volume,
                Priority::Osd,
                Lifetime::Transient(Duration::from_millis(1200)),
                Scope::FocusedOutput,
                Interrupt::None
            )
        );
        assert_eq!(
            post(&[
                "media",
                "player",
                "media",
                "persistent",
                "global",
                "auto-expand:4000"
            ]),
            activity(
                Id::new(Kind::Media, "player"),
                Priority::Media,
                Lifetime::Persistent,
                Scope::Global,
                Interrupt::AutoExpand(Duration::from_millis(4000))
            )
        );
        assert_eq!(
            post(&[
                "battery",
                "battery",
                "critical",
                "persistent",
                "global",
                "preempt"
            ]),
            activity(
                Id::new(Kind::Battery, "battery"),
                Priority::Critical,
                Lifetime::Persistent,
                Scope::Global,
                Interrupt::Preempt
            )
        );
        assert_eq!(
            parse(&["withdraw", "timer", "countdown"]),
            Some(Command::Withdraw(countdown))
        );
    }

    #[test]
    fn bad_input_gets_the_usage() {
        for arguments in [
            &[][..],
            &["open", "controls"],
            &["post"],
            &["post", "volume", "volume"],
            &["post", "volume", "volume", "osd"],
            &["post", "volume", "volume", "osd", "1200"],
            &["post", "sound", "volume", "osd", "1200", "global", "none"],
            &["post", "volume", "volume", "loud", "1200", "global", "none"],
            &["post", "volume", "volume", "osd", "-5", "global", "none"],
            &["post", "volume", "volume", "osd", "1s", "global", "none"],
            &[
                "post",
                "volume",
                "volume",
                "osd",
                "1200",
                "everywhere",
                "none",
            ],
            &["post", "volume", "volume", "osd", "1200", "global", "never"],
            &[
                "post",
                "media",
                "m",
                "media",
                "persistent",
                "global",
                "auto-expand",
            ],
            &[
                "post",
                "media",
                "m",
                "media",
                "persistent",
                "global",
                "auto-expand:",
            ],
            &["withdraw", "volume"],
            &["withdraw", "sound", "volume"],
        ] {
            assert_eq!(parsed(arguments), Err(Unparsed::Usage), "{arguments:?}");
        }
    }

    // well formed, but a policy `Activity::new` refuses, says why rather than the usage
    #[test]
    fn a_refused_fake_activity_says_why() {
        assert_eq!(
            parsed(&["post", "volume", "volume", "osd", "0", "global", "none"]),
            Err(Unparsed::Invalid(InvalidActivity::ZeroLifetime))
        );
        assert_eq!(
            parsed(&[
                "post",
                "volume",
                "volume",
                "osd",
                "1200",
                "global",
                "auto-expand:1000"
            ]),
            Err(Unparsed::Invalid(InvalidActivity::AutoExpandWithoutSurface))
        );
    }

    #[test]
    fn debug_usage_names_every_kind_and_priority() {
        assert_eq!(
            Command::debug_usage(),
            "debug post <kind> <key> <priority> <lifetime> <scope> <interrupt>
debug withdraw <kind> <key>
  <kind>: media|notification|volume|brightness|workspace|battery|network|bluetooth|timer|screenshot|recording
  <priority>: passive|media|osd|ongoing|actionable|critical
  <lifetime>: persistent|<ms>
  <scope>: global|focused-output
  <interrupt>: none|preempt|auto-expand:<ms>"
        );
    }
}
