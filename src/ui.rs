//! `kanade ui` (ADR 0095): the windows' semantic tree as JSON, and acts on a node by name. The tree
//! is the runtime's (`kanade_runtime::ui`), read off the input targets the views already build; this
//! is the CLI's words, the lock and mode checks, and the JSON.
//!
//! What a script or agent may read is `ui.describe` (ADR 0096): the default says roles, names and
//! states, and `<redacted>` for any label or value a user or the world wrote. Nothing is asked of
//! the runtime while the session is locked or the mode is off, and the Module off asks nothing.

use std::time::Duration;

use kanade_runtime::IpcReply;
use kanade_runtime::service::Service;
use kanade_runtime::ui::{self, Act, Answer, Mode, Node, Value, Window};
use serde_json::{Value as Json, json};

use crate::cli::Reply;
use crate::config;
use crate::island::command::Unparsed;
use crate::island::presentation::Surface;
use crate::island::service::IslandService;
use crate::lock;
use crate::look::UiDescribe;

pub const MODULE: &str = "ui";

// how long `wait` looks for a node when it is not told, and the longest it may
const WAIT: Duration = Duration::from_secs(5);
const WAIT_MOST: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Describe {
        window: Option<String>,
    },
    Act {
        name: String,
        window: Option<String>,
        act: Act,
    },

    // answers at once whether the node is there; the CLI asks again until it is or time is up
    Wait {
        name: String,
        window: Option<String>,
        timeout: Duration,
    },
}

pub fn usage() -> String {
    String::from(
        "ui describe [--window <window>]
ui press|focus <name> [--window <window>]
ui type <name> <text> [--window <window>]
ui key <name> <key> [--window <window>]
ui scroll <name> <lines> [--window <window>]
ui wait <name> [--window <window>] [--timeout <seconds>]
  <name>: a node's id as `ui describe` gives it, like dock/app, or the end of one when only one fits
  <window>: a window's name, like kanade, kanade-dock or dock
  <key>: enter, escape, tab, backspace, space, up, down, left, right, home, end, delete, pageup, pagedown or a character, after any of ctrl+, alt+, shift+, super+
  <lines>: wheel lines, positive down
  <seconds>: how long wait looks for the node, 5 by default",
    )
}

pub fn parse(words: &[&str]) -> Result<crate::cli::Call, Unparsed> {
    let request = match words {
        ["describe", rest @ ..] => {
            let (window, None) = options(rest)? else {
                return Err(Unparsed::Usage);
            };

            Request::Describe { window }
        }
        ["press", name, rest @ ..] => acting(name, rest, Act::Press)?,
        ["focus", name, rest @ ..] => acting(name, rest, Act::Focus)?,
        ["type", name, text, rest @ ..] => acting(name, rest, Act::Type((*text).to_owned()))?,
        ["key", name, key, rest @ ..] => acting(name, rest, Act::Key((*key).to_owned()))?,
        ["scroll", name, lines, rest @ ..] => {
            let lines = lines.parse().map_err(|_| Unparsed::Usage)?;

            acting(name, rest, Act::Scroll(lines))?
        }
        ["wait", name, rest @ ..] => {
            let (window, timeout) = options(rest)?;

            Request::Wait {
                name: (*name).to_owned(),
                window,
                timeout: timeout.unwrap_or(WAIT),
            }
        }
        _ => return Err(Unparsed::Usage),
    };

    Ok(crate::cli::Call::Ui(request))
}

fn acting(name: &str, rest: &[&str], act: Act) -> Result<Request, Unparsed> {
    let (window, None) = options(rest)? else {
        return Err(Unparsed::Usage);
    };

    Ok(Request::Act {
        name: name.to_owned(),
        window,
        act,
    })
}

// `--window <name>` and `--timeout <seconds>`, in either order
fn options(words: &[&str]) -> Result<(Option<String>, Option<Duration>), Unparsed> {
    let mut window = None;
    let mut timeout = None;
    let mut words = words.iter();

    while let Some(word) = words.next() {
        match (*word, words.next()) {
            ("--window", Some(name)) if window.is_none() => window = Some((*name).to_owned()),
            ("--timeout", Some(seconds)) if timeout.is_none() => {
                let seconds: f64 = seconds.parse().map_err(|_| Unparsed::Usage)?;

                if !seconds.is_finite() || seconds < 0.0 {
                    return Err(Unparsed::Usage);
                }

                timeout = Some(Duration::from_secs_f64(seconds).min(WAIT_MOST));
            }
            _ => return Err(Unparsed::Usage),
        }
    }

    Ok((window, timeout))
}

// the mode `ui.describe` names, in effect for every window as it next draws
pub fn reread() {
    ui::set_mode(match config::get().ui_describe {
        UiDescribe::Structure => Mode::Structure,
        UiDescribe::Content => Mode::Content,
        UiDescribe::Off => Mode::Off,
    });
}

pub fn status() -> Vec<String> {
    vec![format!("ui: {}", config::get().ui_describe.name())]
}

fn refusal(request: &Request) -> Option<&'static str> {
    // the lock screen is the one place nothing may be read or pressed from outside
    if lock::holds() {
        return Some("the session is locked");
    }

    if let Request::Act { name, .. } = request
        && (IslandService::read().surface() == Some(Surface::Auth)
            || name == "authentication"
            || name.starts_with("authentication/"))
    {
        return Some("polkit authentication is active");
    }

    None
}

// asks the runtime, which answers from the windows as they were last drawn, on this thread
pub fn request(request: Request, reply: IpcReply) {
    if let Some(why) = refusal(&request) {
        reply.send(&Reply::Refused(String::from(why)).encode());

        return;
    }

    let waiting = matches!(request, Request::Wait { .. });
    let asked = match request {
        Request::Describe { window } => ui::Request::Describe { window },
        Request::Act { name, window, act } => ui::Request::Act { window, name, act },
        Request::Wait { name, window, .. } => ui::Request::Find { window, name },
    };
    ui::ask(asked, move |answer| {
        let reply_to = match answer {
            Ok(Answer::Windows(windows)) => Reply::Done(describe(&windows)),
            Ok(Answer::Present(there)) if waiting => {
                Reply::Done(String::from(if there { "present" } else { "absent" }))
            }
            Ok(Answer::Present(_)) => Reply::Done(String::new()),
            Ok(Answer::Done(said)) => Reply::Done(said),
            Err(why) => Reply::Refused(why),
        };

        reply.send(&reply_to.encode());
    });
}

fn describe(windows: &[Window]) -> String {
    let windows: Vec<Json> = windows.iter().map(window).collect();

    serde_json::to_string_pretty(&json!({ "windows": windows })).unwrap_or_default()
}

fn window(window: &Window) -> Json {
    let nodes: Vec<Json> = window
        .nodes
        .iter()
        .map(|node| self::node(node, &window.nodes))
        .collect();

    json!({
        "name": window.name,
        "size": [window.width, window.height],
        "scale": window.scale,
        "nodes": nodes,
    })
}

// shaped as an accessibility node is: an id, a parent, a role, a label, a value, states and bounds
fn node(node: &Node, nodes: &[Node]) -> Json {
    let mut said = json!({
        "id": node.id,
        "parent": node.parent.and_then(|parent| nodes.get(parent)).map(|parent| &parent.id),
        "role": node.role.name(),
        "bounds": node.bounds,
    });

    let fields = said.as_object_mut().expect("an object");

    if let Some(label) = &node.label {
        fields.insert(String::from("label"), json!(label));
    }

    match &node.value {
        Some(Value::Level(level)) => fields.insert(String::from("value"), json!(level)),
        Some(Value::Text(text)) => fields.insert(String::from("value"), json!(text)),
        None => None,
    };

    for (state, on) in [
        ("focused", node.focused),
        ("pressed", node.pressed),
        ("disabled", node.disabled),
        ("selected", node.selected),
        ("secret", node.secret),
    ] {
        if on {
            fields.insert(String::from(state), json!(true));
        }
    }

    if let Some(checked) = node.checked {
        fields.insert(String::from("checked"), json!(checked));
    }

    said
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use kanade_runtime::MutexExt;

    use super::*;

    struct CloseAuth;

    impl Drop for CloseAuth {
        fn drop(&mut self) {
            let now = Instant::now();
            let mut island = IslandService::write();
            island.displace(Surface::Auth, now);
            island.set_niri(None, false, now);
        }
    }

    #[test]
    fn act_is_refused_while_auth_is_open_or_targeting_authentication_nodes() {
        let _island = crate::island::service::ISLAND_TURN.locked();
        let _cleanup = CloseAuth;

        let act = |name: &str| Request::Act {
            name: String::from(name),
            window: None,
            act: Act::Press,
        };

        assert_eq!(refusal(&act("controls/wifi")), None);
        assert_eq!(
            refusal(&act("authentication")),
            Some("polkit authentication is active")
        );
        assert_eq!(
            refusal(&act("authentication/cancel")),
            Some("polkit authentication is active")
        );

        let now = Instant::now();
        {
            let mut island = IslandService::write();
            island.set_niri(Some(String::from("eDP-1")), false, now);
            island.open("eDP-1", Surface::Auth, now);
            assert_eq!(island.surface(), Some(Surface::Auth));
        }

        for target in [
            "controls/wifi",
            "cancel",
            "authenticate",
            "authentication/password",
        ] {
            assert_eq!(
                refusal(&act(target)),
                Some("polkit authentication is active"),
                "{target}"
            );
        }
        assert_eq!(refusal(&Request::Describe { window: None }), None);
    }
}
