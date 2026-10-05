//! The `island` IPC verbs (plan 7). Pure: arguments in, Command out.

use super::presentation::Surface;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Open(Surface),

    // open, or collapse when the focused island already shows it
    Toggle(Surface),

    Collapse,
}

impl Command {
    // none for anything else, which gets the usage text
    pub fn parse(arguments: &[String]) -> Option<Command> {
        match arguments {
            [verb, surface] if verb == "open" => Surface::parse(surface).map(Command::Open),
            [verb, surface] if verb == "toggle" => Surface::parse(surface).map(Command::Toggle),
            [verb] if verb == "collapse" => Some(Command::Collapse),
            _ => None,
        }
    }

    pub fn usage() -> String {
        let surfaces: Vec<&str> = Surface::ALL.into_iter().map(Surface::name).collect();

        format!(
            "usage: island open|toggle <{}> | island collapse",
            surfaces.join("|")
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
            &["dnd", "toggle"],
        ] {
            assert_eq!(parse(arguments), None, "{arguments:?}");
        }
    }

    #[test]
    fn usage_names_every_verb_and_surface() {
        let usage = Command::usage();

        assert_eq!(
            usage,
            "usage: island open|toggle <media|notifications|controls|launcher> | island collapse"
        );
    }
}
