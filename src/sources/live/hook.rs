//! `kanade activity hook <agent>` (ADR 0058): one hook event of a coding agent, as JSON on stdin,
//! told to the shell as the `activity` verbs, so a session shows as a live Activity: a prompt starts
//! it, each tool touches it with the tool's name, a wait for the user says so, the end ends it.
//! Only Claude Code's hooks are read, as its documentation gives them (`docs/wiki/Agent-hooks.md`).
//!
//! It runs in the client, inside the agent's own loop, so it is bounded: a document is read to a
//! limit, the shell is waited for `PATIENCE` in all, and the exit status is 0 or 1, never 2, which Claude
//! Code takes as a block. No shell, or the `live` Module off, is no error: the agent runs on
//! machines where Kanade does not. What the document says is never logged; the title and the
//! detail are cut as any other, by the shell.

use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use ring::digest;
use serde::Deserialize;

use super::{DETAIL, ID, NOT_LIVE, TITLE, line, valid_id};
use crate::cli::{self, Reply};

// the longest document read; a prompt or a reply may be long, the rest is a few hundred bytes
const LIMIT: u64 = 8 * 1024 * 1024;

// how long the shell may take over all the verbs of one hook, so a stuck one never holds the agent
// for long: a failure that starts the Activity first says three, within this together
const PATIENCE: Duration = Duration::from_secs(2);

// the agents whose hooks are read
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
}

impl Agent {
    pub fn parse(name: &str) -> Option<Agent> {
        (name == "claude").then_some(Agent::Claude)
    }

    // what its ids begin with, so two agents' sessions and a script's ids never meet
    fn prefix(self) -> &'static str {
        match self {
            Agent::Claude => "claude-",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
        }
    }
}

// the fields of a Claude Code hook event this reads; any other is ignored
#[derive(Debug, Deserialize)]
struct Event {
    hook_event_name: String,
    session_id: String,
    cwd: Option<String>,
    tool_name: Option<String>,
    notification_type: Option<String>,
    error: Option<String>,
}

// what an event does to its session's Activity
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Start,

    // only while it is live: a late event never brings an ended session back
    Detail(String),
    End,

    // shown even when it was not live, as a failure is worth seeing
    Fail(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Plan {
    id: String,
    title: String,
    action: Action,
}

// what an event says to do, none when it needs nothing shown; an error for one that is no event of
// the agent's or has no session
fn plan(agent: Agent, document: &[u8]) -> Result<Option<Plan>, String> {
    let event: Event = serde_json::from_slice(document)
        .map_err(|_| format!("not a {} hook event on stdin", agent.name()))?;

    let action = match event.hook_event_name.as_str() {
        "UserPromptSubmit" => Action::Start,
        // cut here, before it goes over the socket, as the shell cuts it again
        "PreToolUse" => Action::Detail(line(
            &[event.tool_name.as_deref().unwrap_or_default()],
            DETAIL,
        )),
        "PermissionRequest" => Action::Detail(String::from(WAITING)),
        // an idle prompt comes after the turn ended, and the rest say nothing of waiting
        "Notification" => match event.notification_type.as_deref() {
            None
            | Some(
                "permission_prompt"
                | "elicitation_dialog"
                | "elicitation_url_dialog"
                | "agent_needs_input",
            ) => Action::Detail(String::from(WAITING)),
            Some(_) => return Ok(None),
        },
        "Stop" | "SessionEnd" => Action::End,
        "StopFailure" => Action::Fail(failure(event.error.as_deref())),
        _ => {
            return Err(format!(
                "not an event `activity hook` shows for {}",
                agent.name()
            ));
        }
    };

    if event.session_id.is_empty() {
        return Err(String::from("the hook event has no session_id"));
    }

    let project = event
        .cwd
        .as_deref()
        .and_then(|cwd| cwd.trim_end_matches('/').rsplit('/').next())
        .map(|name| line(&[name], TITLE))
        .filter(|name| !name.is_empty());

    Ok(Some(Plan {
        id: id(agent, &event.session_id),
        title: match project {
            Some(project) => line(&[&format!("{}: {project}", agent.name())], TITLE),
            None => String::from(agent.name()),
        },
        action,
    }))
}

// what a session waits for the user with
const WAITING: &str = "Waiting for you";

// an error type like `rate_limit` as a line: `Rate limit`
fn failure(error: Option<&str>) -> String {
    let text = line(&[&error.unwrap_or("failed").replace('_', " ")], DETAIL);
    let mut letters = text.chars();

    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

/*
 * the id of a session: its own id after the agent's prefix when that is a valid id, else a digest of
 * it, so any session id gets an id of its own and one that is not safe never reaches the Island
 */
fn id(agent: Agent, session: &str) -> String {
    let own = format!("{}{session}", agent.prefix());

    if valid_id(&own) {
        return own;
    }

    let hash = digest::digest(&digest::SHA256, session.as_bytes());
    let id: String = hash.as_ref()[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();

    debug_assert!(agent.prefix().len() + id.len() <= ID);

    format!("{}{id}", agent.prefix())
}

// how a verb was taken
enum Told {
    Done,
    NotLive,

    // no shell, or `live` off: nothing to show on, and no fault of the agent's
    Absent,
}

// `plan` carried out through `ask`, which says the words to the shell
fn carry_out(
    plan: &Plan,
    ask: &mut impl FnMut(&[&str]) -> Result<Reply, String>,
) -> Result<(), String> {
    let mut say = |words: &[&str]| -> Result<Told, String> {
        match ask(words) {
            Ok(Reply::Done(_)) => Ok(Told::Done),
            Err(why) if why == cli::NO_SHELL => Ok(Told::Absent),
            Ok(Reply::Refused(why)) if why == cli::module_off("live") => Ok(Told::Absent),
            Ok(Reply::Refused(why)) if why.starts_with(NOT_LIVE) => Ok(Told::NotLive),
            Ok(Reply::Refused(why) | Reply::Unknown(why)) | Err(why) => Err(why),
        }
    };
    let id = plan.id.as_str();

    match &plan.action {
        Action::Start => say(&["activity", "start", id, &plan.title]).map(drop),
        // no text clears the line, and an empty argument would be no word
        Action::Detail(text) => say(&["activity", "detail", id]
            .into_iter()
            .chain((!text.is_empty()).then_some(text.as_str()))
            .collect::<Vec<_>>())
        .map(drop),
        Action::End => say(&["activity", "end", id]).map(drop),
        Action::Fail(text) => match say(&["activity", "fail", id, text])? {
            Told::NotLive => {
                say(&["activity", "start", id, &plan.title])?;
                say(&["activity", "fail", id, text]).map(drop)
            }
            Told::Done | Told::Absent => Ok(()),
        },
    }
}

// the document on `input`, whole, or an error for one over `LIMIT`
fn read(input: impl Read) -> io::Result<Vec<u8>> {
    let mut document = Vec::new();

    input.take(LIMIT + 1).read_to_end(&mut document)?;

    if document.len() as u64 > LIMIT {
        return Err(io::Error::other("the hook event is too large"));
    }

    Ok(document)
}

// `kanade activity hook`: 0 when it is told or needs no telling, 1 with a line on stderr otherwise
pub fn run(agent: Agent) -> ExitCode {
    let deadline = Instant::now() + PATIENCE;
    let told = read(io::stdin().lock())
        .map_err(|error| format!("stdin: {error}"))
        .and_then(|document| plan(agent, &document))
        .and_then(|plan| match plan {
            Some(plan) => carry_out(&plan, &mut |words| {
                let words: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();

                cli::call_within(&words, deadline.saturating_duration_since(Instant::now()))
            }),
            None => Ok(()),
        });

    match told {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            drop(writeln!(io::stderr().lock(), "kanade: {why}"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned(json: &str) -> Result<Option<Plan>, String> {
        plan(Agent::Claude, json.as_bytes())
    }

    fn action(json: &str) -> Option<Action> {
        planned(json).expect("an event").map(|plan| plan.action)
    }

    #[test]
    fn events_map_onto_the_verbs() {
        let event = |name: &str, more: &str| {
            format!(
                r#"{{"session_id":"abc-123","cwd":"/home/k/kanade","hook_event_name":"{name}"{more}}}"#
            )
        };

        let start = planned(&event("UserPromptSubmit", r#","prompt":"secret""#))
            .expect("an event")
            .expect("a plan");

        assert_eq!(start.id, "claude-abc-123");
        assert_eq!(start.title, "Claude Code: kanade");
        assert_eq!(start.action, Action::Start);

        assert_eq!(
            action(&event(
                "PreToolUse",
                r#","tool_name":"Bash","tool_input":{}"#
            )),
            Some(Action::Detail(String::from("Bash")))
        );
        assert_eq!(
            action(&event("PermissionRequest", r#","tool_name":"Bash""#)),
            Some(Action::Detail(String::from(WAITING)))
        );
        assert_eq!(
            action(&event(
                "Notification",
                r#","notification_type":"permission_prompt""#
            )),
            Some(Action::Detail(String::from(WAITING)))
        );
        assert_eq!(
            action(&event(
                "Notification",
                r#","notification_type":"idle_prompt""#
            )),
            None
        );
        assert_eq!(action(&event("Stop", "")), Some(Action::End));
        assert_eq!(action(&event("SessionEnd", "")), Some(Action::End));
        assert_eq!(
            action(&event("StopFailure", r#","error":"rate_limit""#)),
            Some(Action::Fail(String::from("Rate limit")))
        );
    }

    #[test]
    fn what_is_no_event_is_an_error_and_a_session_id_always_makes_a_valid_id() {
        for bad in [
            "",
            "not json",
            "[]",
            r#"{"hook_event_name":"Stop"}"#,
            r#"{"hook_event_name":"Stop","session_id":""}"#,
            r#"{"hook_event_name":"SubagentStop","session_id":"a"}"#,
            r#"{"hook_event_name":7,"session_id":"a"}"#,
        ] {
            assert!(planned(bad).is_err(), "{bad}");
        }

        let valid = id(Agent::Claude, &"x".repeat(ID));
        let odd = id(Agent::Claude, "has space/é");

        for id in [&valid, &odd, &id(Agent::Claude, "")] {
            assert!(valid_id(id), "{id}");
        }

        assert_ne!(valid, odd);
        assert_eq!(id(Agent::Claude, "has space/é"), odd);
    }

    #[test]
    fn a_late_event_never_brings_a_session_back_but_a_failure_is_shown() {
        let plan = |action| Plan {
            id: String::from("claude-a"),
            title: String::from("Claude Code"),
            action,
        };
        let not_live = || {
            Ok(Reply::Refused(format!(
                "{NOT_LIVE}claude-a; start it first"
            )))
        };

        // a tool's detail or the end for a session no longer live is told no more
        for action in [Action::Detail(String::from("Bash")), Action::End] {
            let mut said = Vec::new();
            let told = carry_out(&plan(action), &mut |words| {
                said.push(words.join(" "));
                not_live()
            });

            assert_eq!(told, Ok(()));
            assert_eq!(said.len(), 1);
            assert!(!said[0].contains("start"));
        }

        // a failure of one that was not live starts it first
        let mut said = Vec::new();
        let told = carry_out(
            &plan(Action::Fail(String::from("Rate limit"))),
            &mut |words| {
                said.push(words.join(" "));

                if said.len() == 1 {
                    not_live()
                } else {
                    Ok(Reply::Done(String::new()))
                }
            },
        );

        assert_eq!(told, Ok(()));
        assert_eq!(
            said,
            [
                "activity fail claude-a Rate limit",
                "activity start claude-a Claude Code",
                "activity fail claude-a Rate limit"
            ]
        );

        // no shell, or the Module off, is no error; another refusal is
        for (reply, ok) in [
            (Err(String::from(cli::NO_SHELL)), true),
            (Ok(Reply::Refused(cli::module_off("live"))), true),
            (
                Ok(Reply::Refused(String::from("8 live Activities already"))),
                false,
            ),
            (Ok(Reply::Unknown(String::from("late"))), false),
        ] {
            let told = carry_out(&plan(Action::Start), &mut |_| reply.clone());

            assert_eq!(told.is_ok(), ok, "{reply:?}");
        }
    }

    #[test]
    fn a_document_is_read_to_a_limit() {
        assert_eq!(read(&b"{}"[..]).expect("small").len(), 2);
        assert!(read(io::repeat(b' ')).is_err());
    }
}
