//! Live Activities from outside (ADR 0042): `kanade activity` lets a script, a build or a download
//! show what it does on the Island. `start` posts a Persistent Ongoing Live Activity, `progress`
//! and `detail` repost it, and `end` or `fail` turn it into a short Transient done or failed form,
//! as a saved recording's. `run` wraps a command in all of it, on the client's side, and `hook`
//! turns an agent's hook events into the same verbs (`hook.rs`, ADR 0058).
//!
//! What comes in is another program's, so it is bounded: at most `MOST` live at once, of which at
//! most `NOTIFIED` are a notification's progress (ADR 0059), ids from a small alphabet that never
//! begins with `NOTIFICATION`, titles and details one line of plain text cut to a length, a verb for
//! an id that is not live refused. A live Activity its program stops talking to is withdrawn after
//! `live.stale` (`config::LIVE`), a notification's after that at most; `run` keeps its own alive
//! while its command runs. A change sooner than
//! `REPOST` after the last post is held and posted as it falls due, one that changes nothing is not
//! posted. Titles and details are never logged.
//!
//! A poster may attach up to `ACTIONS` actions to its live Activity (`actions`, ADR 0110). The shell
//! only draws them as pills and queues a press for the poster, who reads it back (`pressed`, `wait`);
//! it never runs anything for a press. Labels are cut to `LABEL` characters, never logged, and never
//! look like the shell's own controls (`RESERVED`), best effort: invisible characters and edge
//! punctuation are ignored in the comparison, a look-alike in other scripts can still pass.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitCode, ExitStatus};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::MutexExt;
use kanade_runtime::service::Service;
use rustix::process::{Pid, Signal, kill_process};

use super::capture::SHOWN;
use crate::cli::{self, Reply};
use crate::clock;
use crate::config;
use crate::island::activity::{
    Action, Activity, Detail, Ended, Id, Interrupt, Job, Kind, Lifetime, Priority, Scope,
};
use crate::island::service::IslandService;
pub use hook::{Agent, run as hook};

mod hook;

// how many may be live at once; another is refused until one ends
pub const MOST: usize = 8;

// the ids of a notification's progress begin so (ADR 0059); a client's never do
pub const NOTIFICATION: &str = "notification.";

// how many of the live ones may be a notification's, so clients keep at least
// `MOST - NOTIFIED - LAUNCHED`: a new one past it takes the place of the oldest of them
pub const NOTIFIED: usize = 3;

// the ids of an app's progress, from its launcher entry, begin so (ADR 0083); a client's never do
pub const LAUNCHER: &str = "launcher.";

// how many of the live ones may be an app's launcher entry
pub const LAUNCHED: usize = 2;

// the longest id, title and detail, in characters
pub const ID: usize = 64;
pub const TITLE: usize = 80;
pub const DETAIL: usize = 160;

// the most actions a live Activity carries, and the longest label and key of one, in characters
pub const ACTIONS: usize = 3;
pub const LABEL: usize = 24;
pub const KEY: usize = 32;

// the presses waiting for the poster to read them; a newer one drops the oldest
const MAILBOX: usize = 4;

// how long a pressed pill shows as pending without a word from the poster, before it is pressable again
const PENDING: Duration = Duration::from_secs(10);

// what a pending pill says; its key is empty, which no poster's is, so a press of it does nothing
pub const WORKING: &str = "...";

// the labels the shell's own controls carry, which a poster's action never has (compared trimmed,
// without regard to case), so a pill never passes for one of them
const RESERVED: [&str; 5] = ["Cancel", "Now", "Dismiss", "Stop", "Confirm"];

// how often a client asks for a press, and the least between two looks of `run` at its command
const POLL: Duration = Duration::from_millis(250);
const WATCH: Duration = Duration::from_millis(50);

// the longest `wait` may be told to wait, in seconds
const WAIT: u64 = 86_400;

// a progress's full bar, in thousandths
const FULL: u16 = 1000;

// the least between two posts of one live Activity's changes: a client reposting in a loop redraws
// the Island a few times a second, and what it said last is posted when the time is up
const REPOST: Duration = Duration::from_millis(250);

// how often `run` tells the shell its command still runs, well inside the least `live.stale`
const TOUCH: Duration = Duration::from_secs(20);

// what `kanade activity` asks; every text is already one bounded line
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Start {
        id: String,
        title: String,
    },

    // in thousandths
    Progress {
        id: String,
        progress: u16,
    },

    // empty clears it
    Detail {
        id: String,
        detail: String,
    },

    // none keeps the detail it had
    End {
        id: String,
        ended: Ended,
        detail: Option<String>,
    },

    // it still runs, so it is not stale
    Touch {
        id: String,
    },

    // replaces its actions; none clears them
    Actions {
        id: String,
        actions: Vec<Action>,
    },

    // the oldest press waiting, taken: its key, or nothing
    Pressed {
        id: String,
    },

    // the client's own: asks `Pressed` until there is one, or the time is up
    Wait {
        id: String,
        timeout: Option<Duration>,
    },

    List,

    // the client's own: starts it, runs the command, ends it by its exit status
    Run {
        id: String,
        title: String,
        actions: Vec<Action>,
        command: Vec<String>,
    },

    // the client's own: one hook event of an agent on stdin, told as the verbs above
    Hook(Agent),
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        let id = |id: &str| client_id(id).then(|| id.to_owned());
        let title = |words: &[&str]| Some(line(words, TITLE)).filter(|title| !title.is_empty());

        match arguments {
            ["start", key, words @ ..] => Some(Request::Start {
                id: id(key)?,
                title: title(words)?,
            }),
            ["progress", key, amount] => Some(Request::Progress {
                id: id(key)?,
                progress: progress(amount)?,
            }),
            ["detail", key, words @ ..] => Some(Request::Detail {
                id: id(key)?,
                detail: line(words, DETAIL),
            }),
            [verb @ ("end" | "fail"), key, words @ ..] => Some(Request::End {
                id: id(key)?,
                ended: if *verb == "end" {
                    Ended::Done
                } else {
                    Ended::Failed
                },
                detail: (!words.is_empty()).then(|| line(words, DETAIL)),
            }),
            ["touch", key] => Some(Request::Touch { id: id(key)? }),
            ["actions", key, specs @ ..] => Some(Request::Actions {
                id: id(key)?,
                actions: specs
                    .iter()
                    .map(|spec| action(spec))
                    .collect::<Option<_>>()?,
            }),
            ["pressed", key] => Some(Request::Pressed { id: id(key)? }),
            ["wait", key] => Some(Request::Wait {
                id: id(key)?,
                timeout: None,
            }),
            ["wait", key, "--timeout", seconds] => Some(Request::Wait {
                id: id(key)?,
                timeout: Some(Duration::from_secs(
                    seconds
                        .parse::<u64>()
                        .ok()
                        .filter(|seconds| *seconds <= WAIT)?,
                )),
            }),
            ["list"] => Some(Request::List),
            ["run", key, rest @ ..] => {
                let split = rest.iter().position(|word| *word == "--")?;
                let command = &rest[split + 1..];

                if command.is_empty() {
                    return None;
                }

                // the words before `--`: the title, and `--action <key>=<label>` among them
                let (mut words, mut actions) = (Vec::new(), Vec::new());
                let mut before = rest[..split].iter();

                while let Some(word) = before.next() {
                    if *word == "--action" {
                        actions.push(action(before.next()?)?);
                    } else {
                        words.push(*word);
                    }
                }

                Some(Request::Run {
                    id: id(key)?,
                    title: title(&words)?,
                    actions,
                    command: command.iter().map(|word| (*word).to_owned()).collect(),
                })
            }
            ["hook", agent] => Some(Request::Hook(Agent::parse(agent)?)),
            _ => None,
        }
    }
}

pub fn usage() -> String {
    let reserved = RESERVED.join(", ");

    format!(
        "activity start <id> <title>
activity progress <id> <amount>
activity detail <id> [<text>]
activity end|fail <id> [<text>]
activity touch <id>
activity actions <id> [<key>=<label>]...
activity pressed <id>
activity wait <id> [--timeout <seconds>]
activity list
activity run <id> <title> [--action <key>=<label>]... -- <command>...
activity hook claude
  <id>: 1 to {ID} letters, digits, '.', '_' or '-', not starting with '{NOTIFICATION}' or '{LAUNCHER}'; it names one live Activity until it ends
  <title>: what it is, one line, cut to {TITLE} characters
  <amount>: a percent like 40 or 40%, or a fraction like 0.4
  <text>: a line under the title, cut to {DETAIL} characters; detail with none clears it
  actions: shows up to {ACTIONS} buttons on the Activity, replacing those it had; none clears them. \
The shell runs nothing for a press: it queues the press for you to read back, with pressed (the \
oldest key waiting, exit 1 and no output if none) or wait (until one, or the timeout, in {WAIT} \
seconds at most). The actions end with the Activity
  <key>: 1 to {KEY} lowercase letters, digits, '_' or '-'
  <label>: one line, cut to {LABEL} characters, never {reserved}, which are the shell's own
  <command>: run with the Activity live, ended as done on exit status 0 and failed on any other, \
which kanade exits with; an action keyed 'cancel' ends the command with SIGTERM
  hook: reads one Claude Code hook event as JSON on stdin and shows the session as a live Activity \
(`docs/wiki/Agent-hooks.md`); it exits 0 or 1, never 2, which Claude Code takes as a block"
    )
}

// letters, digits, '.', '_' and '-', so an id never carries markup, spaces or control characters
pub(crate) fn valid_id(id: &str) -> bool {
    (1..=ID).contains(&id.len())
        && id
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || matches!(letter, '.' | '_' | '-'))
}

// an id a client may use: a valid one that is not a notification's
fn client_id(id: &str) -> bool {
    valid_id(id) && !id.starts_with(NOTIFICATION) && !id.starts_with(LAUNCHER)
}

// lowercase letters, digits, '_' and '-': what a poster names an action by, and reads a press back as
fn valid_key(key: &str) -> bool {
    (1..=KEY).contains(&key.len())
        && key
            .chars()
            .all(|letter| matches!(letter, 'a'..='z' | '0'..='9' | '_' | '-'))
}

// `<key>=<label>` as an action: the label already one line cut to `LABEL`, which may still be empty
// or reserved, for `check` to refuse with a reason
fn action(spec: &str) -> Option<Action> {
    let (key, label) = spec.split_once('=')?;

    valid_key(key).then(|| Action {
        key: key.to_owned(),
        label: line(&[&without_format(label)], LABEL),
    })
}

// whether `letter` is an invisible format character (Unicode Cf): zero-width, joiners, direction
// marks and the like, which let a label look like another without being it
fn is_format(letter: char) -> bool {
    matches!(letter,
        '\u{ad}' | '\u{600}'..='\u{605}' | '\u{61c}' | '\u{6dd}' | '\u{70f}'
        | '\u{890}'..='\u{891}' | '\u{8e2}' | '\u{180e}' | '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}'
        | '\u{feff}' | '\u{fff9}'..='\u{fffb}' | '\u{110bd}' | '\u{110cd}'
        | '\u{13430}'..='\u{1343f}' | '\u{1bca0}'..='\u{1bca3}' | '\u{1d173}'..='\u{1d17a}'
        | '\u{e0001}' | '\u{e0020}'..='\u{e007f}')
}

fn without_format(text: &str) -> String {
    text.chars().filter(|letter| !is_format(*letter)).collect()
}

// how a label reads to the eye, for the comparison with the shell's own: no format characters, and
// no space, punctuation or symbol around it ("Cancel.", "Cancel ✕")
fn looks_like(label: &str) -> String {
    without_format(label)
        .trim_matches(|letter: char| !letter.is_alphanumeric())
        .to_owned()
}

// whether `actions` are a set the shell shows: few, each with a label of its own that does not pass
// for one of the shell's controls, and keys that differ. The reason otherwise
fn check(actions: &[Action]) -> Result<(), String> {
    if actions.len() > ACTIONS {
        return Err(format!("at most {ACTIONS} actions"));
    }

    for (index, action) in actions.iter().enumerate() {
        let label = action.label.as_str();

        if without_format(label).trim().is_empty() {
            return Err(format!("the action {} has no label", action.key));
        }

        if let Some(reserved) = RESERVED
            .iter()
            .find(|reserved| reserved.eq_ignore_ascii_case(&looks_like(label)))
        {
            return Err(format!(
                "the label {reserved} is the shell's own; the action {} needs another",
                action.key
            ));
        }

        if actions[..index].iter().any(|other| other.key == action.key) {
            return Err(format!("the key {} is used twice", action.key));
        }
    }

    Ok(())
}

/*
 * `words` as one line of at most `most` characters: control characters and those that reorder text
 * become spaces, runs of space one, and a longer line ends in an ellipsis
 */
fn line(words: &[&str], most: usize) -> String {
    let spaced: String = words
        .join(" ")
        .chars()
        .map(|letter| {
            if letter.is_control()
                || matches!(letter, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                ' '
            } else {
                letter
            }
        })
        .collect();
    let line = spaced.split_whitespace().collect::<Vec<_>>().join(" ");

    if line.chars().count() <= most {
        return line;
    }

    line.chars()
        .take(most - 1)
        .chain(std::iter::once('\u{2026}'))
        .collect()
}

/*
 * thousandths done: a whole percent with or without `%` (`40`, `40%`, `42.5%`), or a fraction with
 * a point (`0.4`, `1.0`), from none to all
 */
fn progress(amount: &str) -> Option<u16> {
    let (number, scale) = match amount.strip_suffix('%') {
        Some(percent) => (percent, 10.0),
        None if amount.contains('.') => (amount, 1000.0),
        None => (amount, 10.0),
    };

    // a sign, an exponent or a name like `inf` is no amount a person writes
    if number.is_empty()
        || !number
            .chars()
            .all(|digit| digit.is_ascii_digit() || digit == '.')
    {
        return None;
    }

    let thousandths = number.parse::<f64>().ok()? * scale;

    (0.0..=1000.0)
        .contains(&thousandths)
        .then(|| thousandths.round() as u16)
}

// a live Activity, as the shell keeps it
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    job: Job,

    // when it started, which a notification's oldest is told by
    started: Instant,

    // when its program last said anything of it
    heard: Instant,

    // how long it may go unheard instead of `live.stale`, as a notification asks to show for
    lasts: Option<Duration>,

    // when it was last posted to the Island, and whether a change since waits to be
    posted: Instant,
    held: bool,

    // the actions its program set, and the presses of them it has not read yet, oldest first
    actions: Vec<Action>,
    mailbox: VecDeque<String>,

    // when one was pressed and no word from the program has answered it since
    pending: Option<Instant>,
}

impl Entry {
    // the actions as the Island shows them: a pressed one is "..." until the program answers or
    // `PENDING` passes. Its key is empty, so a press of it does nothing
    fn form(&self, now: Instant) -> Vec<Action> {
        match self.pending {
            Some(at) if now < at + PENDING => vec![Action {
                key: String::new(),
                label: String::from(WORKING),
            }],
            _ => self.actions.clone(),
        }
    }

    // a pending press whose time is up is over; whether it was, so the Island is told
    fn lapse(&mut self, now: Instant) -> bool {
        let over = self.pending.is_some_and(|at| now >= at + PENDING);

        if over {
            self.pending = None;
        }

        over
    }

    // how long it may go unheard: as it asks, but never past `stale`
    fn patience(&self, stale: Duration) -> Duration {
        self.lasts.map_or(stale, |lasts| lasts.min(stale))
    }
}

// the ids a source of this process keeps to, and how many of the live ones they may be
fn share(id: &str) -> Option<(&'static str, usize)> {
    [(NOTIFICATION, NOTIFIED), (LAUNCHER, LAUNCHED)]
        .into_iter()
        .find(|(prefix, _)| id.starts_with(prefix))
}

// the live Activities by id. Pure: time is given, never read
#[derive(Debug, Default)]
pub struct Board {
    live: BTreeMap<String, Entry>,

    // the last start's `Job::serial`; no two starts of an id share one
    serial: u32,
}

impl Board {
    const fn new() -> Board {
        Board {
            live: BTreeMap::new(),
            serial: 0,
        }
    }

    /*
     * a new one, or one live started again from scratch; refused past `MOST`. A new notification's
     * or launcher entry's past its share of them takes the place of the oldest, whose id is returned to be withdrawn
     */
    pub fn start(
        &mut self,
        id: &str,
        title: String,
        now: Instant,
    ) -> Result<(Job, Option<String>), String> {
        let new = !self.live.contains_key(id);

        let evicted = share(id).filter(|_| new).and_then(|(prefix, most)| {
            let theirs = || {
                self.live
                    .iter()
                    .filter(|(other, _)| other.starts_with(prefix))
            };

            (theirs().count() >= most)
                .then(|| theirs().min_by_key(|(_, entry)| entry.started))
                .flatten()
                .map(|(oldest, _)| oldest.clone())
        });

        if new && self.live.len() - usize::from(evicted.is_some()) >= MOST {
            return Err(format!(
                "{MOST} live Activities already; end one before starting another"
            ));
        }

        if let Some(oldest) = &evicted {
            self.live.remove(oldest);
        }

        self.serial = self.serial.wrapping_add(1).max(1);

        let job = Job {
            title,
            serial: self.serial,
            ..Job::default()
        };

        self.live.insert(
            id.to_owned(),
            Entry {
                job: job.clone(),
                started: now,
                heard: now,
                lasts: None,
                posted: now,
                held: false,
                actions: Vec::new(),
                mailbox: VecDeque::new(),
                pending: None,
            },
        );

        Ok((job, evicted))
    }

    /*
     * the live one named `id`, changed by `change` and heard from now. Whether to post its new form
     * now: not when nothing changed, nor within `REPOST` of the last post, which `flush` posts when
     * due. An answer to a press that has not lapsed is a change too
     */
    fn alter(
        &mut self,
        id: &str,
        now: Instant,
        change: impl FnOnce(&mut Entry),
    ) -> Result<bool, String> {
        let entry = self.live.get_mut(id).ok_or_else(|| unknown(id))?;
        let lapsed = entry.lapse(now);
        let before = (entry.job.clone(), entry.form(now));

        change(entry);
        entry.heard = now;

        if !lapsed && (entry.job.clone(), entry.form(now)) == before {
            return Ok(false);
        }

        if now.saturating_duration_since(entry.posted) < REPOST {
            entry.held = true;
            return Ok(false);
        }

        entry.posted = now;
        entry.held = false;

        Ok(true)
    }

    /*
     * the live one named `id`, changed by `change` as its program's word, which answers a press, and
     * heard from now. Its new form to post now, as `alter` has it
     */
    pub fn change(
        &mut self,
        id: &str,
        now: Instant,
        change: impl FnOnce(&mut Job),
    ) -> Result<Option<Job>, String> {
        let post = self.alter(id, now, |entry| {
            entry.pending = None;
            change(&mut entry.job);
        })?;

        Ok(post.then(|| self.live[id].job.clone()))
    }

    // the live one named `id` said it runs; no answer to a press
    pub fn touch(&mut self, id: &str, now: Instant) -> Result<Option<Job>, String> {
        let post = self.alter(id, now, |_| {})?;

        Ok(post.then(|| self.live[id].job.clone()))
    }

    // the live one's actions as the Island shows them, none once it ended or is not live
    pub fn form(&self, id: &str, now: Instant) -> Vec<Action> {
        self.live
            .get(id)
            .map_or_else(Vec::new, |entry| entry.form(now))
    }

    /*
     * its actions replaced by `actions`, which `check` has passed, as its program's word. Its form
     * to post now, as `change` has it
     */
    pub fn set_actions(
        &mut self,
        id: &str,
        actions: Vec<Action>,
        now: Instant,
    ) -> Result<Option<Job>, String> {
        check(&actions)?;

        let post = self.alter(id, now, |entry| {
            entry.pending = None;
            entry.actions = actions;
        })?;

        Ok(post.then(|| self.live[id].job.clone()))
    }

    /*
     * the pill `key` of the start `serial` of `id` pressed: queued for its program, the oldest of
     * `MAILBOX` waiting dropped for it, and shown as pending. Nothing for a press of an earlier
     * start's, of an action it no longer has, or while one is pending. Whether to post at once, as a
     * press is answered in the pill and not held back
     */
    pub fn press(&mut self, id: &str, serial: u32, key: &str, now: Instant) -> bool {
        let Some(entry) = self.live.get_mut(id) else {
            return false;
        };

        // the pill the Island still shows as pending is posted whole by `flush`
        if entry.lapse(now) {
            entry.held = true;
        }

        if entry.job.serial != serial
            || entry.pending.is_some()
            || !entry.actions.iter().any(|action| action.key == key)
        {
            return false;
        }

        if entry.mailbox.len() == MAILBOX {
            entry.mailbox.pop_front();
        }

        entry.mailbox.push_back(key.to_owned());
        entry.pending = Some(now);
        entry.posted = now;
        entry.held = false;

        true
    }

    // the oldest press waiting, taken: its key, or empty when none; its program is heard from
    pub fn pressed(&mut self, id: &str, now: Instant) -> Result<String, String> {
        let entry = self.live.get_mut(id).ok_or_else(|| unknown(id))?;

        entry.heard = now;

        Ok(entry.mailbox.pop_front().unwrap_or_default())
    }

    // the held changes whose time is up, now posted; a pending press whose time is up is one
    pub fn flush(&mut self, now: Instant) -> Vec<(String, Job)> {
        for entry in self.live.values_mut() {
            if entry.lapse(now) {
                entry.held = true;
            }
        }

        self.live
            .iter_mut()
            .filter(|(_, entry)| entry.held && now >= entry.posted + REPOST)
            .map(|(id, entry)| {
                entry.posted = now;
                entry.held = false;

                (id.clone(), entry.job.clone())
            })
            .collect()
    }

    // when the first held change may be posted or pending press lapses, none while there is none
    pub fn held_until(&self) -> Option<Instant> {
        self.live
            .values()
            .flat_map(|entry| {
                [
                    entry.held.then(|| entry.posted + REPOST),
                    entry.pending.map(|at| at + PENDING),
                ]
            })
            .flatten()
            .min()
    }

    // no longer live, done at the full progress it reached, as a finished notification is
    pub fn complete(&mut self, id: &str) -> Result<Job, String> {
        let mut job = self.end(id, Ended::Done, None)?;

        job.progress = Some(FULL);

        Ok(job)
    }

    // no longer live, as it ended; its last form, done or failed
    pub fn end(&mut self, id: &str, ended: Ended, detail: Option<String>) -> Result<Job, String> {
        let mut job = self.live.remove(id).ok_or_else(|| unknown(id))?.job;

        job.ended = Some(ended);

        if let Some(detail) = detail {
            job.detail = detail;
        }

        Ok(job)
    }

    // those not heard from for `stale`, taken out
    pub fn expire(&mut self, now: Instant, stale: Duration) -> Vec<String> {
        let gone: Vec<String> = self
            .live
            .iter()
            .filter(|(_, entry)| {
                now.saturating_duration_since(entry.heard) >= entry.patience(stale)
            })
            .map(|(id, _)| id.clone())
            .collect();

        for id in &gone {
            self.live.remove(id);
        }

        gone
    }

    // when the next goes stale, none while nothing is live
    pub fn due(&self, stale: Duration) -> Option<Instant> {
        self.live
            .values()
            .map(|entry| entry.heard + entry.patience(stale))
            .min()
    }

    // a line each, by id: the id, how far along, the title, tab-separated
    pub fn list(&self) -> Vec<String> {
        self.live
            .iter()
            .map(|(id, entry)| {
                let progress = entry.job.progress.map_or_else(
                    || String::from("-"),
                    |thousandths| format!("{}%", f32::from(thousandths) / 10.0),
                );

                format!("{id}\t{progress}\t{}", entry.job.title)
            })
            .collect()
    }
}

// how a refusal for an id that is not live begins, which `hook` tells from any other
const NOT_LIVE: &str = "no live Activity named ";

fn unknown(id: &str) -> String {
    format!("{NOT_LIVE}{id}; start it first")
}

static BOARD: Mutex<Board> = Mutex::new(Board::new());

// wakes the thread that withdraws stale ones when what is live changes
static CHANGED: Condvar = Condvar::new();

// a request, on the draw thread; nothing is said of what it shows, but for `pressed`, which says the key
pub fn request(request: Request) -> Result<String, String> {
    let now = Instant::now();
    let mut board = BOARD.locked();

    let (id, job) = match request {
        Request::Start { id, title } => {
            let (job, _) = board.start(&id, title, now)?;
            (id, Some(job))
        }
        Request::Progress { id, progress } => {
            let job = board.change(&id, now, |job| job.progress = Some(progress))?;
            (id, job)
        }
        Request::Detail { id, detail } => {
            let job = board.change(&id, now, |job| job.detail = detail)?;
            (id, job)
        }
        Request::End { id, ended, detail } => {
            let job = board.end(&id, ended, detail)?;
            (id, Some(job))
        }
        Request::Touch { id } => {
            let job = board.touch(&id, now)?;
            (id, job)
        }
        Request::Actions { id, actions } => {
            let job = board.set_actions(&id, actions, now)?;
            (id, job)
        }
        Request::Pressed { id } => return board.pressed(&id, now),
        Request::List => return Ok(board.list().join("\n")),
        Request::Run { .. } | Request::Wait { .. } | Request::Hook(_) => {
            return Err(String::from(
                "`activity run`, `wait` and `hook` run in the client",
            ));
        }
    };

    // under the lock, so the thread withdrawing a stale one never comes between
    if let Some(job) = job {
        let actions = board.form(&id, now);

        IslandService::write().post(activity(&id, job, actions), now);
    }

    // it may hold a change, or end the one the thread waits on
    CHANGED.notify_one();

    Ok(String::new())
}

/*
 * a press of the pill `key` of an Activity's, on the draw thread (ADR 0110): `subject` is the id and
 * the start the pill was drawn for, `<id>#<serial>`. It is queued for the program that posted the
 * Activity, which reads it back; nothing is run for it
 */
pub fn act(key: &str, subject: &str) {
    let Some((id, serial)) = subject
        .rsplit_once('#')
        .and_then(|(id, serial)| Some((id, serial.parse::<u32>().ok()?)))
    else {
        return;
    };

    // a pending pill has no key to press
    if key.is_empty() {
        return;
    }

    let now = Instant::now();
    let mut board = BOARD.locked();

    if board.press(id, serial, key, now) {
        let job = board.live.get(id).map(|entry| entry.job.clone());

        if let Some(job) = job {
            let actions = board.form(id, now);

            IslandService::write().post(activity(id, job, actions), now);
        }
    }

    // the thread lapses the pending press when it is due
    CHANGED.notify_one();
}

/*
 * a progress from this process rather than a client, a notification's `value` hint (ADR 0059):
 * starts the live Activity `id` unless it is live, then shows `title`, `detail` and `percent`.
 * Unheard from for `lasts`, else `live.stale`, and never longer, it is withdrawn. Refused past `MOST`,
 * as a client is; past `NOTIFIED` of them the oldest is withdrawn for it
 */
pub fn show(
    id: &str,
    title: &str,
    detail: &str,
    percent: u8,
    lasts: Option<Duration>,
) -> Result<(), String> {
    let now = Instant::now();
    let title = line(&[title], TITLE);
    let detail = line(&[detail], DETAIL);
    let mut board = BOARD.locked();

    let fresh = !board.live.contains_key(id);

    if fresh {
        let (_, evicted) = board.start(id, title.clone(), now)?;

        if let Some(oldest) = evicted {
            IslandService::write().withdraw(&Id::new(Kind::Live, oldest), now);
        }
    }

    let changed = board.change(id, now, |job| {
        job.title = title;
        job.detail = detail;
        job.progress = Some(u16::from(percent) * 10);
    })?;

    let entry = board.live.get_mut(id);

    // a new one is posted whole at once, not held back as a change within `REPOST` of its start
    let job = entry.map(|entry| {
        entry.lasts = lasts;
        entry.held = entry.held && !fresh;

        entry.job.clone()
    });

    if let Some(job) = job.filter(|_| fresh || changed.is_some()) {
        IslandService::write().post(activity(id, job, Vec::new()), now);
    }

    CHANGED.notify_one();

    Ok(())
}

// ends the live Activity `id` as done at 100%, if it is live: a progress that jumps to its end
// would otherwise end showing the last step it was told
pub fn finish(id: &str) {
    let now = Instant::now();
    let mut board = BOARD.locked();

    if let Ok(job) = board.complete(id) {
        IslandService::write().post(activity(id, job, Vec::new()), now);
        CHANGED.notify_one();
    }
}

// takes the live Activity `id` away at once, with no ended form, if it is live
pub fn withdraw(id: &str) {
    let now = Instant::now();
    let mut board = BOARD.locked();

    if board.live.remove(id).is_some() {
        IslandService::write().withdraw(&Id::new(Kind::Live, id), now);
        CHANGED.notify_one();
    }
}

// the `live` Module's thread: withdraws each live Activity once it is stale and posts the changes
// held back; asleep while none is live or held
pub fn follow() {
    let mut board = BOARD.locked();

    loop {
        let stale = config::get().live_stale;
        let now = Instant::now();
        let gone = board.expire(now, stale);
        let held = board.flush(now);

        if !gone.is_empty() || !held.is_empty() {
            let mut island = IslandService::write();

            for id in gone {
                island.withdraw(&Id::new(Kind::Live, id), now);
            }

            for (id, job) in held {
                let actions = board.form(&id, now);

                island.post(activity(&id, job, actions), now);
            }
        }

        board = match [board.due(stale), board.held_until()]
            .into_iter()
            .flatten()
            .min()
        {
            Some(due) => {
                CHANGED
                    .wait_timeout(board, due.saturating_duration_since(now))
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
            None => CHANGED.wait(board).unwrap_or_else(PoisonError::into_inner),
        };
    }
}

/*
 * live: Ongoing like a timer, so over Media and a Satellite beside a higher primary, on every
 * island. Ended: shown a while where the user looks, as a saved recording is
 */
fn activity(id: &str, job: Job, actions: Vec<Action>) -> Activity {
    let (priority, lifetime, scope) = match job.ended {
        None => (Priority::Ongoing, Lifetime::Persistent, Scope::Global),
        Some(_) => (
            Priority::Actionable,
            Lifetime::Transient(SHOWN),
            Scope::FocusedOutput,
        ),
    };

    Activity::new(
        Id::new(Kind::Live, id),
        priority,
        lifetime,
        scope,
        Interrupt::None,
    )
    .expect("an Activity that does not auto-expand and lasts a while is valid")
    .with_actions(actions)
    .with_detail(Detail::Live(job))
}

// the id of niri's failed config load; a space is outside a client's alphabet, so no client has it
const NIRI_CONFIG: &str = "niri config";

/*
 * niri says its config failed to load (`niri::on_config_failed`, ADR 0071): the failed form of a
 * Live Activity, shown a while on the focused output. Nothing is live, so there is nothing to end
 * or to withdraw
 */
pub fn niri_config_failed() {
    let job = Job {
        title: String::from("niri config"),
        detail: String::from("run niri validate"),
        progress: None,
        ended: Some(Ended::Failed),
        serial: 0,
    };

    IslandService::write().post(activity(NIRI_CONFIG, job, Vec::new()), Instant::now());
}

// the id of a picked color; a space is outside a client's alphabet, so no client has it
const COLOR: &str = "picked color";

/*
 * a color picked on the screen (`capture::color`) and copied: its hex, shown as the done form of a
 * Live Activity a while on the focused output. Nothing is live, so there is nothing to end
 */
pub fn color_picked(hex: &str) {
    let job = Job {
        title: hex.to_owned(),
        detail: String::from("copied"),
        progress: None,
        ended: Some(Ended::Done),
        serial: 0,
    };

    IslandService::write().post(activity(COLOR, job, Vec::new()), Instant::now());
}

// the id of a gesture's outcome; a space is outside a client's alphabet
const GESTURE: &str = "gesture outcome";

/*
 * a gesture that ran nothing (`gesture`, ADR 0107): not recognized, or its verb failed. The
 * failed form of a Live Activity a while on the focused output. `detail` is a verb's refusal,
 * kept to a line
 */
pub fn gesture(title: &str, detail: &str) {
    let job = Job {
        title: title.to_owned(),
        detail: detail
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(60)
            .collect(),
        progress: None,
        ended: Some(Ended::Failed),
        serial: 0,
    };

    IslandService::write().post(activity(GESTURE, job, Vec::new()), Instant::now());
}

// the id of the first-start health notice; a space is outside a client's alphabet
const HEALTH: &str = "health notice";

/*
 * the first start finds `waiting` checks that need attention (`settings::spawn_notice`, ADR 0088):
 * the failed form of a Live Activity a while on the focused output, naming where to look
 */
pub fn health_attention(waiting: usize) {
    let job = Job {
        title: match waiting {
            1 => String::from("1 check needs attention"),
            _ => format!("{waiting} checks need attention"),
        },
        detail: String::from("open Settings, Health"),
        progress: None,
        ended: Some(Ended::Failed),
        serial: 0,
    };

    IslandService::write().post(activity(HEALTH, job, Vec::new()), Instant::now());
}

// the key of the action `run` hears: its press ends the command with SIGTERM
const CANCEL: &str = "cancel";

/*
 * `kanade activity run`: starts the live Activity, runs `command` and ends it by how the command
 * ended, exiting as it did; 127 when it cannot start. The command runs without a shell to tell, as
 * it is what the user asked for; Ctrl+C reaches only it, so its end is still told. `actions` are
 * shown with it (ADR 0110); a press of `cancel` is the only one `run` hears, and it sends SIGTERM to
 * the command. `run` reads presses only for it, so it is the only action it takes with it: another
 * would be read, and lost, by `run` before its own reader came for it. 2 for actions it refuses
 */
pub fn run(id: &str, title: &str, actions: &[Action], command: &[String]) -> ExitCode {
    if let Err(why) = check(actions) {
        complain(&format!("kanade: {why}"));
        return ExitCode::from(2);
    }

    let cancel = actions.iter().any(|action| action.key == CANCEL);

    if cancel && actions.len() > 1 {
        complain(&format!(
            "kanade: with the action {CANCEL}, `run` takes no other: it would read their presses first"
        ));
        return ExitCode::from(2);
    }

    let tell = |words: &[&str]| -> Result<(), String> {
        match cli::call(
            &words
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>(),
        ) {
            Ok(Reply::Done(_)) => Ok(()),
            Ok(Reply::Refused(why) | Reply::Unknown(why)) | Err(why) => Err(why),
        }
    };

    let live = match tell(&["activity", "start", id, title]) {
        Ok(()) => true,
        Err(why) => {
            complain(&format!(
                "kanade: {why}; running it without a live Activity"
            ));
            false
        }
    };

    // the shell shows them while it runs; they end with the Activity
    let shown = live && !actions.is_empty() && {
        let mut words = vec![
            String::from("activity"),
            String::from("actions"),
            id.to_owned(),
        ];

        words.extend(
            actions
                .iter()
                .map(|action| format!("{}={}", action.key, action.label)),
        );

        match cli::call(&words) {
            Ok(Reply::Done(_)) => true,
            Ok(Reply::Refused(why) | Reply::Unknown(why)) | Err(why) => {
                complain(&format!("kanade: {why}; running it without its actions"));
                false
            }
        }
    };
    let cancel = cancel && shown;

    clock::ignore_sigint();

    let (program, arguments) = command.split_first().expect("parse asks for a command");
    let spawned = clock::default_sigint(Command::new(program).args(arguments)).spawn();

    let (code, ended, detail) = match spawned {
        Err(error) => {
            complain(&format!("kanade: {program}: {error}"));
            (127, Ended::Failed, Some(String::from("Could not start")))
        }
        Ok(mut child) => {
            let (stop, stopped) = mpsc::channel::<()>();
            let id_kept = id.to_owned();

            let keeper = live.then(|| {
                thread::spawn(move || {
                    while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(TOUCH) {
                        // a shell gone or restarted meanwhile only loses the Activity
                        let _ = tell(&["activity", "touch", &id_kept]);
                    }
                })
            });

            let status = if cancel {
                listen(&mut child, id)
            } else {
                child.wait()
            };

            drop(stop);
            if let Some(keeper) = keeper {
                let _ = keeper.join();
            }

            match status {
                Ok(status) if status.success() => (0, Ended::Done, None),
                Ok(status) => match (status.code(), status.signal()) {
                    (Some(code), _) => (
                        u8::try_from(code).unwrap_or(1),
                        Ended::Failed,
                        Some(format!("Exited with {code}")),
                    ),
                    (None, Some(signal)) => (
                        u8::try_from(128 + signal).unwrap_or(255),
                        Ended::Failed,
                        Some(format!("Ended by signal {signal}")),
                    ),
                    (None, None) => (1, Ended::Failed, None),
                },
                Err(error) => {
                    complain(&format!("kanade: {program}: {error}"));
                    (1, Ended::Failed, None)
                }
            }
        }
    };

    if live {
        let verb = match ended {
            Ended::Done => "end",
            Ended::Failed => "fail",
        };
        let mut words = vec!["activity", verb, id];
        words.extend(detail.as_deref());

        if let Err(why) = tell(&words) {
            complain(&format!("kanade: {why}"));
        }
    }

    ExitCode::from(code)
}

/*
 * waits for `child`, ending it with SIGTERM at each press of `cancel` read, so a command that ignores
 * the first can be asked again. The shell is asked every `POLL` for as long as the child runs, a
 * call that fails for now (`Lapse::for_good` false) just tried again; none once the shell has
 * refused or is gone
 */
fn listen(child: &mut Child, id: &str) -> io::Result<ExitStatus> {
    let pid = Pid::from_raw(i32::try_from(child.id()).unwrap_or(0));
    let (mut asking, mut asked) = (true, Instant::now());

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }

        if asking && asked.elapsed() >= POLL {
            match press_of(id) {
                Ok(Some(key)) if key == CANCEL => {
                    if let Some(pid) = pid {
                        // the child is not reaped yet, so its pid is still its own
                        let _ = kill_process(pid, Signal::TERM);
                    }
                }
                Ok(_) => {}
                Err(lapse) => asking = !lapse.for_good,
            }

            asked = Instant::now();
        }

        thread::sleep(WATCH);
    }
}

// why a call for a press failed, and whether asking again could not help: the shell refused (the
// Activity is not live, or not the poster's) or is gone. A timeout or a full queue is for now
#[derive(Debug, PartialEq)]
struct Lapse {
    why: String,
    for_good: bool,
}

fn read(reply: Result<Reply, String>) -> Result<Option<String>, Lapse> {
    match reply {
        Ok(Reply::Done(key)) => Ok((!key.is_empty()).then_some(key)),
        Ok(Reply::Refused(why)) => Err(Lapse {
            why,
            for_good: true,
        }),
        Ok(Reply::Unknown(why)) => Err(Lapse {
            why,
            for_good: false,
        }),
        Err(why) => Err(Lapse {
            for_good: why == cli::NO_SHELL,
            why,
        }),
    }
}

// one `activity pressed` of the shell: the key waiting, or none
fn press_of(id: &str) -> Result<Option<String>, Lapse> {
    let words = [
        String::from("activity"),
        String::from("pressed"),
        id.to_owned(),
    ];

    read(cli::call(&words))
}

// `kanade activity pressed`: prints the oldest press waiting and exits 0, else exits 1, silent
pub fn pressed(id: &str) -> ExitCode {
    match press_of(id) {
        Ok(Some(key)) => cli::print(&format!("{key}\n"), ExitCode::SUCCESS),
        Ok(None) => ExitCode::FAILURE,
        Err(lapse) => {
            complain(&format!("kanade: {}", lapse.why));
            ExitCode::FAILURE
        }
    }
}

// `kanade activity wait`: asks for a press every `POLL` until there is one, to print and exit 0, or
// the timeout passes, exit 1. The shell answers each ask at once and holds nothing between them; the
// client asks about four times a second, a call that fails for now tried again, and one that is
// refused for good or finds no shell exit 1
pub fn wait(id: &str, timeout: Option<Duration>) -> ExitCode {
    let until = timeout.map(|timeout| Instant::now() + timeout);

    loop {
        match press_of(id) {
            Ok(Some(key)) => return cli::print(&format!("{key}\n"), ExitCode::SUCCESS),
            Ok(None) => {}
            Err(lapse) if lapse.for_good => {
                complain(&format!("kanade: {}", lapse.why));
                return ExitCode::FAILURE;
            }
            Err(_) => {}
        }

        if until.is_some_and(|until| Instant::now() >= until) {
            complain(&format!("kanade: timed out waiting for a press on {id}"));
            return ExitCode::FAILURE;
        }

        thread::sleep(POLL);
    }
}

fn complain(text: &str) {
    drop(writeln!(io::stderr().lock(), "{text}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_take_a_small_alphabet_and_a_length() {
        for id in ["build", "dl.42", "a_b-c", &"x".repeat(ID)] {
            assert!(valid_id(id), "{id}");
        }

        for id in ["", "two words", "a/b", "é", "a\nb", &"x".repeat(ID + 1)] {
            assert!(!valid_id(id), "{id:?}");
        }
    }

    #[test]
    fn text_from_outside_is_one_bounded_line() {
        assert_eq!(line(&["make", " all"], TITLE), "make all");
        assert_eq!(line(&["a\nb\tc\u{1b}[31m"], TITLE), "a b c [31m");
        assert_eq!(line(&["\u{202e}evil"], TITLE), "evil");
        assert_eq!(line(&[], DETAIL), "");

        let long = line(&[&"é".repeat(TITLE + 5)], TITLE);

        assert_eq!(long.chars().count(), TITLE);
        assert!(long.ends_with('\u{2026}'));
    }

    #[test]
    fn only_a_refusal_or_no_shell_ends_the_asking_for_presses() {
        let lapsed = |reply| read(reply).expect_err("fails").for_good;

        assert!(lapsed(Ok(Reply::Refused(unknown("a")))));
        assert!(lapsed(Err(String::from(cli::NO_SHELL))));
        assert!(!lapsed(Err(String::from(
            "the shell did not take the call within 5s"
        ))));
        assert!(!lapsed(Ok(Reply::Unknown(String::from("?")))));
        assert_eq!(read(Ok(Reply::Done(String::new()))), Ok(None));
        assert_eq!(
            read(Ok(Reply::Done(String::from("yes")))),
            Ok(Some(String::from("yes")))
        );
    }

    #[test]
    fn an_amount_is_a_percent_or_a_fraction() {
        assert_eq!(progress("40"), Some(400));
        assert_eq!(progress("40%"), Some(400));
        assert_eq!(progress("42.5%"), Some(425));
        assert_eq!(progress("0.4"), Some(400));
        assert_eq!(progress("1.0"), Some(1000));
        assert_eq!(progress("1"), Some(10));
        assert_eq!(progress("100"), Some(1000));

        for amount in [
            "", "%", "-1", "101", "1.5", "1e2", "inf", "NaN", "40 %", "0x10",
        ] {
            assert_eq!(progress(amount), None, "{amount}");
        }
    }

    #[test]
    fn verbs_parse_with_their_bounds() {
        assert_eq!(
            Request::parse(&["start", "build", "Building", "kanade"]),
            Some(Request::Start {
                id: String::from("build"),
                title: String::from("Building kanade"),
            })
        );
        assert_eq!(Request::parse(&["start", "build"]), None);
        assert_eq!(Request::parse(&["start", "build", "\n"]), None);
        assert_eq!(Request::parse(&["start", "no id", "x"]), None);
        assert_eq!(Request::parse(&["progress", "build", "half"]), None);
        assert_eq!(
            Request::parse(&["fail", "build"]),
            Some(Request::End {
                id: String::from("build"),
                ended: Ended::Failed,
                detail: None,
            })
        );
        assert_eq!(
            Request::parse(&["run", "b", "Build", "--", "make", "-j8"]),
            Some(Request::Run {
                id: String::from("b"),
                title: String::from("Build"),
                actions: Vec::new(),
                command: vec![String::from("make"), String::from("-j8")],
            })
        );
        assert_eq!(
            Request::parse(&[
                "run",
                "b",
                "Build",
                "--action",
                "cancel=Abort",
                "--",
                "make"
            ]),
            Some(Request::Run {
                id: String::from("b"),
                title: String::from("Build"),
                actions: vec![pill("cancel", "Abort")],
                command: vec![String::from("make")],
            })
        );
        assert_eq!(
            Request::parse(&["run", "b", "Build", "--action", "--", "make"]),
            None
        );
        assert_eq!(
            Request::parse(&["run", "b", "Build", "--action", "No Key", "--", "make"]),
            None
        );
        assert_eq!(Request::parse(&["run", "b", "Build", "make"]), None);
        assert_eq!(Request::parse(&["run", "b", "Build", "--"]), None);
        assert_eq!(Request::parse(&["run", "b", "--", "make"]), None);
    }

    #[test]
    fn only_so_many_are_live_and_only_live_ones_change() {
        let now = Instant::now();
        let mut board = Board::new();

        for index in 0..MOST {
            assert!(
                board
                    .start(&index.to_string(), String::from("t"), now)
                    .is_ok()
            );
        }

        assert!(board.start("one more", String::from("t"), now).is_err());

        // starting one live again is no new one
        assert!(board.start("0", String::from("again"), now).is_ok());

        assert!(board.change("gone", now, |_| {}).is_err());
        assert!(board.end("gone", Ended::Done, None).is_err());

        let ended = board.end("1", Ended::Failed, None).expect("live");

        assert_eq!(ended.ended, Some(Ended::Failed));
        assert!(board.end("1", Ended::Done, None).is_err());
        assert!(board.start("one more", String::from("t"), now).is_ok());
    }

    #[test]
    fn notifications_keep_to_their_share_and_a_client_cannot_use_their_ids() {
        let start = Instant::now();
        let mut board = Board::new();
        let notified = |index: usize| format!("{NOTIFICATION}app.{index}");

        // each a second later than the one before, the oldest first
        for index in 0..NOTIFIED {
            let at = start + Duration::from_secs(index as u64);

            assert_eq!(
                board
                    .start(&notified(index), String::from("t"), at)
                    .map(|s| s.1),
                Ok(None)
            );
        }

        // one more takes the oldest's place
        let later = start + Duration::from_secs(10);
        let (_, evicted) = board
            .start(&notified(9), String::from("t"), later)
            .expect("room");

        assert_eq!(evicted, Some(notified(0)));

        // so the clients always have what is left
        for index in 0..MOST - NOTIFIED {
            assert!(
                board
                    .start(&format!("c{index}"), String::from("t"), later)
                    .is_ok()
            );
        }

        assert!(board.start("c-more", String::from("t"), later).is_err());
        assert!(board.start(&notified(10), String::from("t"), later).is_ok());
        assert_eq!(board.live.len(), MOST);

        // one asked to last for ever lasts `stale` at most; a client's has no other bound
        board.live.get_mut(&notified(9)).expect("live").lasts = Some(Duration::MAX);
        let stale = Duration::from_secs(60);

        assert_eq!(
            board.due(stale),
            Some(start + Duration::from_secs(2) + stale)
        );

        // nor the launcher entries' (ADR 0083), which keep to their share too
        let launched = |index: usize| format!("{LAUNCHER}app.{index}");
        let mut board = Board::new();

        for index in 0..LAUNCHED {
            let at = start + Duration::from_secs(index as u64);

            assert_eq!(
                board
                    .start(&launched(index), String::from("t"), at)
                    .map(|s| s.1),
                Ok(None)
            );
        }

        assert_eq!(
            board
                .start(&launched(9), String::from("t"), later)
                .map(|s| s.1),
            Ok(Some(launched(0)))
        );

        for id in [
            "notification.x.1",
            "notification.",
            "notification.a",
            "launcher.a.desktop",
            "launcher.",
        ] {
            assert_eq!(Request::parse(&["start", id, "t"]), None, "{id}");
            assert_eq!(Request::parse(&["end", id]), None, "{id}");
        }
    }

    #[test]
    fn a_completed_progress_ends_at_the_full_bar_not_the_last_step() {
        let now = Instant::now();
        let mut board = Board::new();

        board.start("a", String::from("t"), now).expect("room");
        board
            .change("a", now, |job| job.progress = Some(800))
            .expect("live");

        let job = board.complete("a").expect("live");

        assert_eq!((job.progress, job.ended), (Some(1000), Some(Ended::Done)));
        assert!(board.complete("a").is_err());
    }

    #[test]
    fn an_end_keeps_the_last_detail_unless_it_says_another() {
        let now = Instant::now();
        let mut board = Board::new();

        board.start("a", String::from("t"), now).expect("room");
        board
            .change("a", now, |job| job.detail = String::from("step 3"))
            .expect("live");

        assert_eq!(
            board.end("a", Ended::Done, None).expect("live").detail,
            "step 3"
        );

        board.start("a", String::from("t"), now).expect("room");

        assert_eq!(board.end("a", Ended::Done, None).expect("live").detail, "");
    }

    #[test]
    fn a_change_within_the_repost_time_is_held_and_one_that_changes_nothing_is_not_posted() {
        let start = Instant::now();
        let mut board = Board::new();
        let progress = |thousandths| move |job: &mut Job| job.progress = Some(thousandths);

        board.start("a", String::from("t"), start).expect("room");

        // soon after the start: held, and nothing to post until the time is up
        let soon = start + REPOST / 5;

        assert_eq!(board.change("a", soon, progress(100)), Ok(None));
        assert_eq!(board.change("a", soon, progress(200)), Ok(None));
        assert_eq!(board.held_until(), Some(start + REPOST));
        assert!(board.flush(soon).is_empty());

        // the newest of the held is what is posted
        let due = board.flush(start + REPOST);

        assert_eq!(due.len(), 1);
        assert_eq!(due[0].1.progress, Some(200));
        assert_eq!(board.held_until(), None);

        // a repost of what shows is not posted, and one after the time is posted at once
        let later = start + REPOST * 3;

        assert_eq!(board.change("a", later, progress(200)), Ok(None));
        assert_eq!(board.held_until(), None);
        assert!(matches!(
            board.change("a", later, progress(300)),
            Ok(Some(_))
        ));
    }

    #[test]
    fn one_not_heard_from_goes_stale_and_a_word_keeps_it() {
        let start = Instant::now();
        let stale = Duration::from_secs(60);
        let mut board = Board::new();

        assert_eq!(board.due(stale), None);

        board
            .start("quiet", String::from("t"), start)
            .expect("room");
        board
            .start("talking", String::from("t"), start)
            .expect("room");

        assert_eq!(board.due(stale), Some(start + stale));

        let later = start + Duration::from_secs(40);

        board.change("talking", later, |_| {}).expect("live");

        assert!(
            board
                .expire(start + Duration::from_secs(59), stale)
                .is_empty()
        );
        assert_eq!(board.expire(start + stale, stale), ["quiet"]);
        assert_eq!(board.due(stale), Some(later + stale));
        assert!(board.change("quiet", later, |_| {}).is_err());
        assert_eq!(board.expire(later + stale, stale), ["talking"]);
        assert_eq!(board.due(stale), None);
    }

    fn pill(key: &str, label: &str) -> Action {
        Action {
            key: key.to_owned(),
            label: label.to_owned(),
        }
    }

    // a board with `a` live and pillable, started at `now`
    fn pilled(now: Instant) -> (Board, u32) {
        let mut board = Board::new();
        let (job, _) = board.start("a", String::from("t"), now).expect("room");

        board
            .set_actions("a", vec![pill("yes", "Yes"), pill("no", "No")], now)
            .expect("live");

        (board, job.serial)
    }

    #[test]
    fn actions_are_few_short_one_line_and_never_the_shells_own() {
        let parsed = Request::parse(&["actions", "a", "ok=Allow", "stop=Stop it"]);

        assert_eq!(
            parsed,
            Some(Request::Actions {
                id: String::from("a"),
                actions: vec![pill("ok", "Allow"), pill("stop", "Stop it")],
            })
        );
        assert_eq!(
            Request::parse(&["actions", "a"]),
            Some(Request::Actions {
                id: String::from("a"),
                actions: Vec::new(),
            })
        );

        // keys keep to their alphabet and length; a spec without a label separator is none
        for spec in [
            "Ok=x",
            "o k=x",
            "=x",
            "ok",
            "é=x",
            &format!("{}=x", "k".repeat(KEY + 1)),
        ] {
            assert_eq!(Request::parse(&["actions", "a", spec]), None, "{spec}");
        }

        assert!(Request::parse(&["actions", "a", &format!("{}=x", "k".repeat(KEY))]).is_some());

        // the label is one line cut to its length
        let Some(Request::Actions { actions, .. }) = Request::parse(&[
            "actions",
            "a",
            &format!("k={}\n\u{202e}b", "x".repeat(LABEL * 2)),
        ]) else {
            panic!("parses");
        };

        assert_eq!(actions[0].label.chars().count(), LABEL);
        assert!(actions[0].label.ends_with('\u{2026}'));

        assert!(check(&[pill("a", "A"), pill("b", "B"), pill("c", "C")]).is_ok());
        assert!(
            check(&[
                pill("a", "A"),
                pill("b", "B"),
                pill("c", "C"),
                pill("d", "D")
            ])
            .is_err()
        );
        assert!(check(&[pill("a", "A"), pill("a", "B")]).is_err());
        assert!(check(&[pill("a", "")]).is_err());

        // invisible characters and trailing marks do not make a label another
        for label in [
            "Can\u{200b}cel",
            "Cancel\u{feff}",
            "\u{2060}Stop",
            "Cancel.",
            "Dismiss\u{200e}!",
            "Now \u{2026}",
            "Confirm \u{2715}",
        ] {
            let spec = format!("k={label}");
            let Some(Request::Actions { actions, .. }) = Request::parse(&["actions", "a", &spec])
            else {
                panic!("parses {label:?}");
            };

            assert!(check(&actions).is_err(), "{label:?}");
        }

        // a zero-width character alone is no label, and one inside a word is not shown
        assert_eq!(action("k=\u{200b}"), Some(pill("k", "")));
        assert_eq!(action("k=A\u{200d}B"), Some(pill("k", "AB")));
        assert!(check(&[pill("k", "\u{200b}")]).is_err());

        for label in ["Cancel", "cancel", " NOW ", "Dismiss", "stop", "Confirm"] {
            let spec = format!("k={label}");
            let Some(Request::Actions { actions, .. }) = Request::parse(&["actions", "a", &spec])
            else {
                panic!("parses {label}");
            };

            assert!(check(&actions).is_err(), "{label}");
        }
    }

    #[test]
    fn a_press_is_queued_once_for_its_poster_and_the_oldest_four_are_kept() {
        let now = Instant::now();
        let (mut board, serial) = pilled(now);

        assert!(board.press("a", serial, "yes", now));
        assert_eq!(board.pressed("a", now), Ok(String::from("yes")));
        assert_eq!(board.pressed("a", now), Ok(String::new()));
        assert!(board.pressed("gone", now).is_err());

        // a pending pill takes no more, until the poster answers
        board
            .change("a", now, |job| job.detail = String::from("x"))
            .expect("live");
        assert!(board.press("a", serial, "yes", now));
        assert!(!board.press("a", serial, "no", now));
        board.pressed("a", now).expect("live");

        // six presses, each answered, none read: the four newest wait, oldest first
        let keys = ["yes", "no", "no", "yes", "no", "yes"];

        for (step, key) in keys.iter().enumerate() {
            let at = now + Duration::from_secs(step as u64);

            board
                .change("a", at, |job| job.detail = step.to_string())
                .expect("live");
            assert!(board.press("a", serial, key, at));
        }

        let read: Vec<String> =
            std::iter::from_fn(|| board.pressed("a", now).ok().filter(|key| !key.is_empty()))
                .collect();

        assert_eq!(read, keys[keys.len() - MAILBOX..]);
    }

    #[test]
    fn a_press_of_an_earlier_start_or_a_removed_action_does_nothing() {
        let now = Instant::now();
        let (mut board, first) = pilled(now);

        board.start("a", String::from("again"), now).expect("room");
        board
            .set_actions("a", vec![pill("yes", "Yes")], now)
            .expect("live");

        assert!(!board.press("a", first, "yes", now));
        assert_eq!(board.pressed("a", now), Ok(String::new()));

        let second = board.live["a"].job.serial;

        assert_ne!(first, second);
        assert!(!board.press("a", second, "no", now));
        assert!(!board.press("a", second, "", now));
        assert!(!board.press("other", second, "yes", now));
        assert!(board.press("a", second, "yes", now));
    }

    #[test]
    fn what_ends_or_withdraws_an_activity_takes_its_actions_and_presses() {
        let now = Instant::now();
        let (mut board, serial) = pilled(now);

        assert!(board.press("a", serial, "yes", now));

        let job = board.end("a", Ended::Done, None).expect("live");

        assert_eq!(job.ended, Some(Ended::Done));
        assert!(board.form("a", now).is_empty());
        assert!(!board.press("a", serial, "yes", now));
        assert!(board.pressed("a", now).is_err());

        // started again, nothing of the earlier one is left, not even its press
        board.start("a", String::from("t"), now).expect("room");
        assert_eq!(board.pressed("a", now), Ok(String::new()));
        assert!(board.form("a", now).is_empty());

        // gone stale, the same
        let (mut board, serial) = pilled(now);
        let stale = Duration::from_secs(60);

        assert_eq!(board.expire(now + stale, stale), ["a"]);
        assert!(!board.press("a", serial, "yes", now + stale));
        assert!(board.form("a", now + stale).is_empty());
    }

    #[test]
    fn a_pressed_pill_is_pending_until_its_poster_speaks_or_the_time_given_is_up() {
        let now = Instant::now();
        let (mut board, serial) = pilled(now);
        let pressed = now + Duration::from_secs(1);

        assert!(board.press("a", serial, "yes", pressed));
        assert_eq!(board.form("a", pressed), [pill("", WORKING)]);
        assert_eq!(board.held_until(), Some(pressed + PENDING));

        // reading the press, or a touch, is no word about it
        board.pressed("a", pressed).expect("live");
        board.touch("a", pressed).expect("live");
        assert_eq!(board.form("a", pressed), [pill("", WORKING)]);

        // time passing re-enables it, and the thread posts that
        let over = pressed + PENDING;

        assert_eq!(board.form("a", over - Duration::from_millis(1)).len(), 1);
        assert_eq!(board.form("a", over).len(), 2);

        let flushed = board.flush(over);

        assert_eq!(flushed.len(), 1);
        assert_eq!(board.held_until(), None);
        assert!(board.live["a"].pending.is_none());

        // each word of the poster's answers it, and posts the pill whole again
        type Word = fn(&mut Board, Instant) -> Result<Option<Job>, String>;

        let words: [Word; 3] = [
            |board, at| board.change("a", at, |job| job.detail = String::from("done")),
            |board, at| board.change("a", at, |job| job.progress = Some(500)),
            |board, at| board.set_actions("a", vec![pill("go", "Go")], at),
        ];

        for word in words {
            let (mut board, serial) = pilled(now);
            let at = now + Duration::from_secs(5);

            assert!(board.press("a", serial, "yes", at));

            let answered = at + Duration::from_secs(1);

            assert!(word(&mut board, answered).expect("live").is_some());
            assert!(board.live["a"].pending.is_none());
            assert_ne!(board.form("a", answered), [pill("", WORKING)]);
        }

        // an answer that changes nothing visible still shows the pill again
        let (mut board, serial) = pilled(now);
        let at = now + Duration::from_secs(5);

        board.press("a", serial, "yes", at);

        assert!(
            board
                .change("a", at + Duration::from_secs(1), |_| {})
                .expect("live")
                .is_some()
        );
        assert_eq!(board.form("a", at).len(), 2);
    }

    #[test]
    fn a_gesture_never_runs_an_activity_verb() {
        for verb in [
            "activity actions a ok=Ok",
            "activity pressed a",
            "activity wait a",
            "activity run a t --action cancel=Cancel -- true",
        ] {
            assert!(crate::gesture::check(verb).is_err(), "{verb}");
        }
    }
}
