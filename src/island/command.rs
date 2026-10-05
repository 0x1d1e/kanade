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

    // the one timer, restarted when it runs; the timer's source runs these, not the island
    StartTimer(Duration),
    StopTimer,

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
            ["timer", "start", length] => timer(length).map(Command::StartTimer),
            ["timer", "stop"] => Some(Command::StopTimer),
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
       island timer start <duration>
       island timer stop
       island debug post <kind> <key> <priority> [<ms>]
       island debug withdraw <kind> <key>
<kind>: {}
<priority>: {}
<duration>: like 90s, 25m or 1h30m, up to 24h
<ms>: a Transient's lifetime, Persistent without",
            surfaces.join("|"),
            kinds.join("|"),
            priorities.join("|")
        )
    }
}

// the longest timer, so a Satellite's few characters always fit it
const LONGEST: Duration = Duration::from_secs(24 * 60 * 60);

// hours, minutes and seconds, each at most once and in that order, like 1h30m; never zero
fn timer(text: &str) -> Option<Duration> {
    const UNITS: [(char, u64); 3] = [('h', 3600), ('m', 60), ('s', 1)];

    let mut units = UNITS.iter();
    let mut rest = text;
    let mut seconds: u64 = 0;

    while !rest.is_empty() {
        let digits = rest.find(|c: char| !c.is_ascii_digit())?;
        let number: u64 = rest[..digits].parse().ok()?;
        let unit = rest[digits..].chars().next()?;
        let &(_, scale) = units.find(|&&(name, _)| name == unit)?;

        seconds = seconds.checked_add(number.checked_mul(scale)?)?;
        rest = &rest[digits + unit.len_utf8()..];
    }

    Some(Duration::from_secs(seconds)).filter(|length| !length.is_zero() && *length <= LONGEST)
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
        assert_eq!(parse(&["timer", "stop"]), Some(Command::StopTimer));
    }

    #[test]
    fn timer_lengths_read_in_hours_minutes_and_seconds() {
        let start = |length: &str| match parse(&["timer", "start", length]) {
            Some(Command::StartTimer(length)) => Some(length.as_secs()),
            _ => None,
        };

        assert_eq!(start("90s"), Some(90));
        assert_eq!(start("25m"), Some(1500));
        assert_eq!(start("1h30m"), Some(5400));
        assert_eq!(start("1h0m5s"), Some(3605));
        assert_eq!(start("24h"), Some(86_400));
        assert_eq!(start("0h1s"), Some(1));

        for length in [
            "",
            "25",
            "m",
            "0s",
            "0h0m",
            "24h1s",
            "25h",
            "1m1h",
            "1m1m",
            "1.5m",
            "-5m",
            "5 m",
            "5min",
            "5M",
            "18446744073709551615h",
        ] {
            assert_eq!(start(length), None, "{length:?}");
        }
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
            &["timer"],
            &["timer", "start"],
            &["timer", "start", "5m", "eDP-1"],
            &["timer", "stop", "now"],
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
       island timer start <duration>
       island timer stop
       island debug post <kind> <key> <priority> [<ms>]
       island debug withdraw <kind> <key>
<kind>: media|notification|volume|brightness|workspace|battery|network|bluetooth|screen-cast|timer|privacy
<priority>: passive|media|osd|ongoing|actionable|critical
<duration>: like 90s, 25m or 1h30m, up to 24h
<ms>: a Transient's lifetime, Persistent without"
        );
    }
}
