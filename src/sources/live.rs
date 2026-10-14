//! Live Activities from outside (ADR 0042): `kanade activity` lets a script, a build or a download
//! show what it does on the Island. `start` posts a Persistent Ongoing Live Activity, `progress`
//! and `detail` repost it, and `end` or `fail` turn it into a short Transient done or failed form,
//! as a saved recording's. `run` wraps a command in all of it, on the client's side, and `hook`
//! turns an agent's hook events into the same verbs (`hook.rs`, ADR 0058).
//!
//! What comes in is another program's, so it is bounded: at most `MOST` live at once, ids from a
//! small alphabet, titles and details one line of plain text cut to a length, a verb for an id that
//! is not live refused. A live Activity its program stops talking to is withdrawn after `live.stale`
//! (`config::LIVE`); `run` keeps its own alive while its command runs. A change sooner than
//! `REPOST` after the last post is held and posted as it falls due, one that changes nothing is not
//! posted. Titles and details are never logged.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitCode};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::MutexExt;
use kanade_runtime::service::Service;

use super::capture::SHOWN;
use crate::cli::{self, Reply};
use crate::clock;
use crate::config;
use crate::island::activity::{
    Activity, Detail, Ended, Id, Interrupt, Job, Kind, Lifetime, Priority, Scope,
};
use crate::island::service::IslandService;
pub use hook::{Agent, run as hook};

mod hook;

// how many may be live at once; another is refused until one ends
pub const MOST: usize = 8;

// the longest id, title and detail, in characters
pub const ID: usize = 64;
pub const TITLE: usize = 80;
pub const DETAIL: usize = 160;

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

    List,

    // the client's own: starts it, runs the command, ends it by its exit status
    Run {
        id: String,
        title: String,
        command: Vec<String>,
    },

    // the client's own: one hook event of an agent on stdin, told as the verbs above
    Hook(Agent),
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        let id = |id: &str| valid_id(id).then(|| id.to_owned());
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
            ["list"] => Some(Request::List),
            ["run", key, rest @ ..] => {
                let split = rest.iter().position(|word| *word == "--")?;
                let command = &rest[split + 1..];

                if command.is_empty() {
                    return None;
                }

                Some(Request::Run {
                    id: id(key)?,
                    title: title(&rest[..split])?,
                    command: command.iter().map(|word| (*word).to_owned()).collect(),
                })
            }
            ["hook", agent] => Some(Request::Hook(Agent::parse(agent)?)),
            _ => None,
        }
    }
}

pub fn usage() -> String {
    format!(
        "activity start <id> <title>
activity progress <id> <amount>
activity detail <id> [<text>]
activity end|fail <id> [<text>]
activity touch <id>
activity list
activity run <id> <title> -- <command>...
activity hook claude
  <id>: 1 to {ID} letters, digits, '.', '_' or '-'; it names one live Activity until it ends
  <title>: what it is, one line, cut to {TITLE} characters
  <amount>: a percent like 40 or 40%, or a fraction like 0.4
  <text>: a line under the title, cut to {DETAIL} characters; detail with none clears it
  <command>: run with the Activity live, ended as done on exit status 0 and failed on any other, \
which kanade exits with
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

    // when its program last said anything of it
    heard: Instant,

    // how long it may go unheard instead of `live.stale`, as a notification asks to show for
    lasts: Option<Duration>,

    // when it was last posted to the Island, and whether a change since waits to be
    posted: Instant,
    held: bool,
}

// the live Activities by id. Pure: time is given, never read
#[derive(Debug, Default)]
pub struct Board {
    live: BTreeMap<String, Entry>,
}

impl Board {
    const fn new() -> Board {
        Board {
            live: BTreeMap::new(),
        }
    }

    // a new one, or one live started again from scratch; refused past `MOST`
    pub fn start(&mut self, id: &str, title: String, now: Instant) -> Result<Job, String> {
        if !self.live.contains_key(id) && self.live.len() >= MOST {
            return Err(format!(
                "{MOST} live Activities already; end one before starting another"
            ));
        }

        let job = Job {
            title,
            ..Job::default()
        };

        self.live.insert(
            id.to_owned(),
            Entry {
                job: job.clone(),
                heard: now,
                lasts: None,
                posted: now,
                held: false,
            },
        );

        Ok(job)
    }

    /*
     * the live one named `id`, changed by `change` and heard from now. Its new form to post now: none
     * when nothing changed, and none within `REPOST` of the last post, which `flush` posts when due
     */
    pub fn change(
        &mut self,
        id: &str,
        now: Instant,
        change: impl FnOnce(&mut Job),
    ) -> Result<Option<Job>, String> {
        let entry = self.live.get_mut(id).ok_or_else(|| unknown(id))?;
        let before = entry.job.clone();

        change(&mut entry.job);
        entry.heard = now;

        if entry.job == before {
            return Ok(None);
        }

        if now.saturating_duration_since(entry.posted) < REPOST {
            entry.held = true;
            return Ok(None);
        }

        entry.posted = now;
        entry.held = false;

        Ok(Some(entry.job.clone()))
    }

    // the held changes whose time is up, now posted
    pub fn flush(&mut self, now: Instant) -> Vec<(String, Job)> {
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

    // when the first held change may be posted, none while nothing is held
    pub fn held_until(&self) -> Option<Instant> {
        self.live
            .values()
            .filter(|entry| entry.held)
            .map(|entry| entry.posted + REPOST)
            .min()
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
                now.saturating_duration_since(entry.heard) >= entry.lasts.unwrap_or(stale)
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
            .map(|entry| entry.heard + entry.lasts.unwrap_or(stale))
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

// a request, on the draw thread; nothing is said of what it shows
pub fn request(request: Request) -> Result<String, String> {
    let now = Instant::now();
    let mut board = BOARD.locked();

    let (id, job) = match request {
        Request::Start { id, title } => {
            let job = board.start(&id, title, now)?;
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
            let job = board.change(&id, now, |_| {})?;
            (id, job)
        }
        Request::List => return Ok(board.list().join("\n")),
        Request::Run { .. } | Request::Hook(_) => {
            return Err(String::from("`activity run` and `hook` run in the client"));
        }
    };

    // under the lock, so the thread withdrawing a stale one never comes between
    if let Some(job) = job {
        IslandService::write().post(activity(&id, job), now);
    }

    // it may hold a change, or end the one the thread waits on
    CHANGED.notify_one();

    Ok(String::new())
}

/*
 * a progress from this process rather than a client, a notification's `value` hint (ADR 0059):
 * starts the live Activity `id` unless it is live, then shows `title`, `detail` and `percent`.
 * Unheard from for `lasts`, else `live.stale`, it is withdrawn. Refused past `MOST`, as a client is
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
        board.start(id, title.clone(), now)?;
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
        IslandService::write().post(activity(id, job), now);
    }

    CHANGED.notify_one();

    Ok(())
}

// ends the live Activity `id` as done, if it is live
pub fn finish(id: &str) {
    let _ = request(Request::End {
        id: id.to_owned(),
        ended: Ended::Done,
        detail: None,
    });
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
                island.post(activity(&id, job), now);
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
fn activity(id: &str, job: Job) -> Activity {
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
    .with_detail(Detail::Live(job))
}

/*
 * `kanade activity run`: starts the live Activity, runs `command` and ends it by how the command
 * ended, exiting as it did; 127 when it cannot start. The command runs without a shell to tell, as
 * it is what the user asked for; Ctrl+C reaches only it, so its end is still told
 */
pub fn run(id: &str, title: &str, command: &[String]) -> ExitCode {
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

            let status = child.wait();

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
                command: vec![String::from("make"), String::from("-j8")],
            })
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
}
