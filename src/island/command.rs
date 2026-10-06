//! The `island` IPC verbs (plan 7). Pure: arguments in, Command out.

use std::time::Duration;

use super::activity::{Activity, Id, Interrupt, InvalidActivity, Kind, Lifetime, Priority, Scope};
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
    // a fake Activity whose policy cannot be carried out says why, anything else unknown gets the usage text
    pub fn parse(arguments: &[String]) -> Result<Command, Unparsed> {
        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();

        if let [
            "debug",
            "post",
            kind,
            key,
            priority,
            lifetime,
            scope,
            interrupt,
        ] = arguments[..]
        {
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

            return Activity::new(id, priority, lifetime, scope, interrupt)
                .map(Command::Post)
                .map_err(Unparsed::Invalid);
        }

        Command::verb(&arguments).ok_or(Unparsed::Usage)
    }

    fn verb(arguments: &[&str]) -> Option<Command> {
        match *arguments {
            ["open", surface] => Surface::parse(surface).map(Command::Open),
            ["toggle", surface] => Surface::parse(surface).map(Command::Toggle),
            ["collapse"] => Some(Command::Collapse),
            ["dnd", "toggle"] => Some(Command::ToggleDnd),
            ["timer", "start", length] => timer(length).map(Command::StartTimer),
            ["timer", "stop"] => Some(Command::StopTimer),
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
       island debug post <kind> <key> <priority> <lifetime> <scope> <interrupt>
       island debug withdraw <kind> <key>
<kind>: {}
<priority>: {}
<lifetime>: persistent|<ms>
<scope>: global|focused-output
<interrupt>: none|transient|preempt|auto-expand:<ms>
<duration>: like 90s, 25m or 1h30m, up to 24h",
            surfaces.join("|"),
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
        "transient" => Some(Interrupt::Transient),
        "preempt" => Some(Interrupt::Preempt),
        _ => milliseconds(text.strip_prefix("auto-expand:")?).map(Interrupt::AutoExpand),
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

    fn parsed(arguments: &[&str]) -> Result<Command, Unparsed> {
        let arguments: Vec<String> = arguments.iter().map(|&argument| argument.into()).collect();

        Command::parse(&arguments)
    }

    fn parse(arguments: &[&str]) -> Option<Command> {
        parsed(arguments).ok()
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
        let countdown = Id::new(Kind::Timer, "countdown");
        let volume = Id::new(Kind::Volume, "volume");

        let post = |arguments: &[&str]| {
            let mut all = vec!["debug", "post"];
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
            post(&[
                "volume",
                "volume",
                "osd",
                "1200",
                "focused-output",
                "transient"
            ]),
            activity(
                volume,
                Priority::Osd,
                Lifetime::Transient(Duration::from_millis(1200)),
                Scope::FocusedOutput,
                Interrupt::Transient
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
            parse(&["debug", "withdraw", "timer", "countdown"]),
            Some(Command::Withdraw(countdown))
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
            &["debug", "post", "volume", "volume", "osd"],
            &["debug", "post", "volume", "volume", "osd", "1200"],
            &[
                "debug", "post", "sound", "volume", "osd", "1200", "global", "none",
            ],
            &[
                "debug", "post", "volume", "volume", "loud", "1200", "global", "none",
            ],
            &[
                "debug", "post", "volume", "volume", "osd", "-5", "global", "none",
            ],
            &[
                "debug", "post", "volume", "volume", "osd", "1s", "global", "none",
            ],
            &[
                "debug",
                "post",
                "volume",
                "volume",
                "osd",
                "1200",
                "everywhere",
                "none",
            ],
            &[
                "debug", "post", "volume", "volume", "osd", "1200", "global", "never",
            ],
            &[
                "debug",
                "post",
                "media",
                "m",
                "media",
                "persistent",
                "global",
                "auto-expand",
            ],
            &[
                "debug",
                "post",
                "media",
                "m",
                "media",
                "persistent",
                "global",
                "auto-expand:",
            ],
            &["debug", "withdraw", "volume"],
            &["debug", "withdraw", "sound", "volume"],
        ] {
            assert_eq!(parsed(arguments), Err(Unparsed::Usage), "{arguments:?}");
        }
    }

    // well formed, but a policy `Activity::new` refuses, says why rather than the usage
    #[test]
    fn a_refused_fake_activity_says_why() {
        assert_eq!(
            parsed(&[
                "debug", "post", "volume", "volume", "osd", "0", "global", "none"
            ]),
            Err(Unparsed::Invalid(InvalidActivity::ZeroLifetime))
        );
        assert_eq!(
            parsed(&[
                "debug",
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
    fn usage_names_every_verb_and_surface() {
        let usage = Command::usage();

        assert_eq!(
            usage,
            "usage: island open|toggle <media|notifications|controls|launcher>
       island collapse
       island dnd toggle
       island timer start <duration>
       island timer stop
       island debug post <kind> <key> <priority> <lifetime> <scope> <interrupt>
       island debug withdraw <kind> <key>
<kind>: media|notification|volume|brightness|workspace|battery|network|bluetooth|timer
<priority>: passive|media|osd|ongoing|actionable|critical
<lifetime>: persistent|<ms>
<scope>: global|focused-output
<interrupt>: none|transient|preempt|auto-expand:<ms>
<duration>: like 90s, 25m or 1h30m, up to 24h"
        );
    }
}
