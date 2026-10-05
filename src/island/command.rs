//! The `island` IPC verbs (plan 7). Pure: arguments in, Command out.

use std::time::Duration;

use super::activity::{Activity, Id, Kind, Priority};
use super::presentation::Surface;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Open(Surface),

    // open, or collapse when the focused island already shows it
    Toggle(Surface),

    Collapse,

    ToggleDnd,

    // a fake Activity, until the sources post real ones (#21-#33)
    Post(Activity),
    Withdraw(Id),
}

impl Command {
    // none for anything else, which gets the usage text
    pub fn parse(arguments: &[String]) -> Option<Command> {
        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();

        match arguments[..] {
            ["open", surface] => Surface::parse(surface).map(Command::Open),
            ["toggle", surface] => Surface::parse(surface).map(Command::Toggle),
            ["collapse"] => Some(Command::Collapse),
            ["dnd", "toggle"] => Some(Command::ToggleDnd),
            ["debug", "post", kind, key, priority] => {
                let id = Id::new(Kind::parse(kind)?, key);

                Some(Command::Post(Activity::persistent(
                    id,
                    Priority::parse(priority)?,
                )))
            }
            ["debug", "post", kind, key, priority, ms] => {
                let id = Id::new(Kind::parse(kind)?, key);
                let duration = Duration::from_millis(ms.parse().ok().filter(|&ms| ms > 0)?);

                Some(Command::Post(Activity::transient(
                    id,
                    Priority::parse(priority)?,
                    duration,
                )))
            }
            ["debug", "withdraw", kind, key] => {
                Some(Command::Withdraw(Id::new(Kind::parse(kind)?, key)))
            }
            _ => None,
        }
    }

    pub fn usage() -> String {
        let surfaces: Vec<&str> = Surface::ALL.into_iter().map(Surface::name).collect();
        let kinds: Vec<&str> = Kind::ALL.into_iter().map(Kind::name).collect();
        let priorities: Vec<&str> = Priority::ALL.into_iter().map(Priority::name).collect();

        format!(
            "usage: island open|toggle <{}>
       island collapse
       island dnd toggle
       island debug post <kind> <key> <priority> [<ms>]
       island debug withdraw <kind> <key>
<kind>: {}
<priority>: {}
<ms>: a Transient's lifetime, Persistent without",
            surfaces.join("|"),
            kinds.join("|"),
            priorities.join("|")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Option<Command> {
        let arguments: Vec<String> = arguments.iter().map(|&argument| argument.into()).collect();

        Command::parse(&arguments)
    }

    #[test]
    fn verbs() {
        assert_eq!(
            parse(&["open", "controls"]),
            Some(Command::Open(Surface::Controls))
        );
        assert_eq!(
            parse(&["toggle", "launcher"]),
            Some(Command::Toggle(Surface::Launcher))
        );
        assert_eq!(parse(&["collapse"]), Some(Command::Collapse));
        assert_eq!(parse(&["dnd", "toggle"]), Some(Command::ToggleDnd));
    }

    #[test]
    fn debug_verbs_post_and_withdraw_fake_activities() {
        let cast = Id::new(Kind::ScreenCast, "cast");
        let volume = Id::new(Kind::Volume, "volume");

        assert_eq!(
            parse(&["debug", "post", "screen-cast", "cast", "ongoing"]),
            Some(Command::Post(Activity::persistent(
                cast.clone(),
                Priority::Ongoing
            )))
        );
        assert_eq!(
            parse(&["debug", "post", "volume", "volume", "osd", "1200"]),
            Some(Command::Post(Activity::transient(
                volume,
                Priority::Osd,
                Duration::from_millis(1200)
            )))
        );
        assert_eq!(
            parse(&["debug", "withdraw", "screen-cast", "cast"]),
            Some(Command::Withdraw(cast))
        );
    }

    #[test]
    fn bad_input_gets_the_usage() {
        for arguments in [
            &[][..],
            &["open"],
            &["open", "settings"],
            &["toggle"],
            &["collapse", "eDP-1"],
            &["open", "controls", "eDP-1"],
            &["Open", "controls"],
            &["dnd"],
            &["dnd", "on"],
            &["debug", "post", "volume", "volume"],
            &["debug", "post", "sound", "volume", "osd"],
            &["debug", "post", "volume", "volume", "loud"],
            &["debug", "post", "volume", "volume", "osd", "0"],
            &["debug", "post", "volume", "volume", "osd", "-5"],
            &["debug", "post", "volume", "volume", "osd", "1s"],
            &["debug", "withdraw", "volume"],
            &["debug", "withdraw", "sound", "volume"],
        ] {
            assert_eq!(parse(arguments), None, "{arguments:?}");
        }
    }

    #[test]
    fn usage_names_every_verb_and_surface() {
        let usage = Command::usage();

        assert_eq!(
            usage,
            "usage: island open|toggle <media|notifications|controls|launcher>
       island collapse
       island dnd toggle
       island debug post <kind> <key> <priority> [<ms>]
       island debug withdraw <kind> <key>
<kind>: media|notification|volume|brightness|workspace|battery|network|bluetooth|screen-cast|timer|privacy
<priority>: passive|media|osd|ongoing|actionable|critical
<ms>: a Transient's lifetime, Persistent without"
        );
    }
}
