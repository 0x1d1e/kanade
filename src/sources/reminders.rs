//! Calendar reminders (ADR 0043): an event's own alarms, DISPLAY and AUDIO alike, or for a timed
//! event with none `calendar.reminder` minutes before it, each go off as a Reminder Activity that
//! counts down to the event, with Open, which opens the Calendar Surface, and Dismiss.
//!
//! `plan` is pure: the occurrences near now, what already went off and the time in, what goes off
//! now and when the next one does out. `follow` is the monitor around it, on its own thread: it
//! expands the calendars only when they change or a day has passed, keeps what went off in a file
//! under the state directory so a restart never sets one off again, and sleeps on a wall-clock
//! timer armed for the next reminder, the next minute a shown one counts down, or the next
//! expansion; a suspend or a clock set wakes it too, and nothing polls.
//!
//! A key names one reminder of one occurrence by the event's UID, the occurrence's start and the
//! alarm, the default lead by a mark of its own, so a new `calendar.reminder` never sets off again
//! what its old value did.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use chrono::{Days, NaiveDateTime};
use kanade_runtime::{MutexExt, service::Service};
use ring::digest;

use super::calendar::{Calendars, Reminded, Trigger};
use crate::clock::{self, WallTimer};
use crate::island::activity::{
    Action, Activity, Detail, Id, Interrupt, Kind, Lifetime, Priority, Reminder, Scope,
};
use crate::island::command::Command;
use crate::island::presentation::Surface;
use crate::island::service::IslandService;
use crate::{config, supervise};

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

// an event's own reminders past these many, the ones farthest from it, go
pub const PER_EVENT: usize = 5;

// a reminder farther ahead of its event goes, so the calendars are expanded a bounded way ahead
pub const FARTHEST: i64 = 30 * DAY;

// a reminder that went off late, after the shell was off, still goes off this long after its event
// began, or after it went due for one counting to an event's end
pub const LATE: i64 = 5 * MINUTE;

// at most this many reminders show at once; the rest show as one that says how many more
pub const AT_ONCE: usize = 3;

// a reminder stays until its event begins and LATE after, but no longer than this
const HELD: i64 = HOUR;

// how often the calendars are expanded again with none changed, so the window ahead moves on
const REEXPAND: i64 = DAY;

// the key of the Activity that stands for the reminders past AT_ONCE
pub const MORE: &str = "more";

// the Activity's actions
pub const OPEN: &str = "open";
pub const DISMISS: &str = "dismiss";

// where the reminders that went off are kept, under the state directory
const FILE: &str = "kanade/reminders";

// an occurrence near now, as the planner reads it
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    // its event, stable across reads, the same for each of its occurrences
    pub event: String,
    pub title: String,

    // its start and end in seconds since the epoch
    pub start: i64,
    pub end: i64,
    pub all_day: bool,

    // the event's own alarms that show, none for the default lead
    pub triggers: Vec<Trigger>,
}

// one reminder that goes off now
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Due {
    // its occurrence, which its Activity is keyed by
    pub occurrence: String,
    pub title: String,

    // the moment it counts down to, the occurrence's start or, for an alarm on its end, its end
    pub to: i64,
    pub ends: bool,

    // when it was due
    pub due: i64,
}

// what goes off now and when next
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    // the reminders that show, at most AT_ONCE, the soonest first
    pub show: Vec<Due>,

    // the ones past those, which show as one
    pub more: Vec<Due>,

    // every reminder that went off now, those not shown too, with when it can no longer go off
    pub record: Vec<(String, i64)>,

    // the next moment one goes due
    pub next: Option<i64>,
}

// a reminder of an occurrence: when it goes due, what it counts to, and the alarm's part of its key
struct Lead {
    due: i64,
    to: i64,
    ends: bool,
    key: String,
}

/*
 * the reminders of an occurrence: its event's own, or the default lead's for a timed one with
 * none. One farther ahead than FARTHEST goes, and past PER_EVENT those farthest from the event
 */
fn leads(candidate: &Candidate, lead: Option<i64>) -> Vec<Lead> {
    let (start, end) = (candidate.start, candidate.end);
    let mut leads: Vec<Lead> = if candidate.triggers.is_empty() {
        lead.filter(|_| !candidate.all_day)
            .map(|lead| Lead {
                due: start - lead,
                to: start,
                ends: false,
                key: String::from("d"),
            })
            .into_iter()
            .collect()
    } else {
        candidate
            .triggers
            .iter()
            .map(|&trigger| match trigger {
                Trigger::Start(seconds) => Lead {
                    due: start.saturating_add(seconds),
                    to: start,
                    ends: false,
                    key: format!("s{seconds}"),
                },
                Trigger::End(seconds) => Lead {
                    due: end.saturating_add(seconds),
                    to: end,
                    ends: true,
                    key: format!("e{seconds}"),
                },
                Trigger::At(at) => Lead {
                    due: at,
                    to: start,
                    ends: false,
                    key: format!("a{at}"),
                },
            })
            .collect()
    };

    leads.retain(|lead| lead.to - lead.due <= FARTHEST);
    // nearest the event first, one per moment
    leads.sort_by(|a, b| b.due.cmp(&a.due).then_with(|| a.key.cmp(&b.key)));
    leads.dedup_by_key(|lead| lead.due);
    leads.truncate(PER_EVENT);

    leads
}

/*
 * what goes off at `now`, in seconds since the epoch, of the reminders of `candidates` not in
 * `fired`, `lead` the default lead in seconds. One goes off from when it is due while its event has
 * not begun, or LATE after, so one missed while the shell was off still goes off before its event.
 * Of an occurrence's reminders due at once only the nearest the event shows, the rest go off
 * unseen; past AT_ONCE occurrences, the later ones show as one
 */
pub fn plan(candidates: &[Candidate], fired: &Fired, lead: Option<i64>, now: i64) -> Plan {
    let mut plan = Plan::default();
    let mut due: BTreeMap<String, Due> = BTreeMap::new();

    for candidate in candidates {
        let occurrence = format!("{}|{}", candidate.event, candidate.start);

        for lead in leads(candidate, lead) {
            let key = format!("{occurrence}|{}", lead.key);
            let until = lead.to.max(lead.due).saturating_add(LATE);

            if fired.contains(&key) || now >= until {
                continue;
            }
            if lead.due > now {
                plan.next = Some(plan.next.map_or(lead.due, |next| next.min(lead.due)));
                continue;
            }

            plan.record.push((key, until));

            let nearer = due
                .get(&occurrence)
                .is_none_or(|shown| lead.due > shown.due);
            if nearer {
                due.insert(
                    occurrence.clone(),
                    Due {
                        occurrence: occurrence.clone(),
                        title: candidate.title.clone(),
                        to: lead.to,
                        ends: lead.ends,
                        due: lead.due,
                    },
                );
            }
        }
    }

    let mut due: Vec<Due> = due.into_values().collect();
    due.sort_by(|a, b| (a.to, &a.title).cmp(&(b.to, &b.title)));

    plan.more = due.split_off(due.len().min(AT_ONCE));
    plan.show = due;
    plan
}

/*
 * the reminders that went off, each with the moment after which it could not go off again, so
 * one is forgotten only then (`prune`); kept as a file of `<moment>\t<key>` lines
 */
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fired(BTreeMap<String, i64>);

impl Fired {
    // a line that is not one is left out, so a damaged file costs only its own lines
    pub fn parse(text: &str) -> Fired {
        Fired(
            text.lines()
                .filter_map(|line| {
                    let (until, key) = line.split_once('\t')?;

                    Some((key.to_owned(), until.parse().ok()?))
                })
                .filter(|(key, _)| !key.is_empty())
                .collect(),
        )
    }

    pub fn text(&self) -> String {
        self.0
            .iter()
            .map(|(key, until)| format!("{until}\t{key}\n"))
            .collect()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub fn insert(&mut self, key: String, until: i64) {
        self.0.insert(key, until);
    }

    // forgets the ones that can no longer go off at `now`; whether any went
    pub fn prune(&mut self, now: i64) -> bool {
        let before = self.0.len();
        self.0.retain(|_, until| *until > now);

        self.0.len() != before
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

// the reminders that went off, as the file says, none for none or one unreadable
pub fn load(path: &Path) -> Fired {
    match fs::read_to_string(path) {
        Ok(text) => Fired::parse(&text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Fired::default(),
        Err(error) => {
            eprintln!("kanade: reminders: {}: {error}", path.display());
            Fired::default()
        }
    }
}

/*
 * writes them whole beside the file, then over it, so a crash leaves the old file or the new one,
 * never part of either. Only the owner reads it, as it says when the user's events are
 */
pub fn save(path: &Path, fired: &Fired) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }

    let part = path.with_extension("part");
    let written = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&part)
        .and_then(|mut file| {
            file.write_all(fired.text().as_bytes())?;
            file.sync_all()
        });

    match written.and_then(|()| fs::rename(&part, path)) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&part);
            Err(error)
        }
    }
}

// what `kanade reminders` asks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Dismiss,
    Status,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            ["dismiss"] => Some(Request::Dismiss),
            ["status"] => Some(Request::Status),
            _ => None,
        }
    }
}

enum Event {
    // the calendars were read again, or the config with them
    Changed,

    // the timer went off, or the clock was set
    Timer,

    // the reminder of this Activity key, none for every one, goes
    Dismiss(Option<String>),
}

// where the rest of the shell reaches the thread, set before IPC can take a command
static EVENTS: OnceLock<Sender<Event>> = OnceLock::new();

// what `status` says, as the thread last left it
static STATUS: Mutex<Status> = Mutex::new(Status {
    shown: 0,
    next: None,
    fired: 0,
    problem: None,
});

struct Status {
    shown: usize,
    next: Option<i64>,
    fired: usize,
    problem: Option<String>,
}

fn send(event: Event) {
    if let Some(events) = EVENTS.get() {
        let _ = events.send(event);
    }
}

// the calendars changed, which a watch on them says
pub fn changed() {
    send(Event::Changed);
}

pub fn request(request: Request) -> Result<String, String> {
    match request {
        Request::Dismiss => {
            send(Event::Dismiss(None));
            Ok(String::new())
        }
        Request::Status => {
            let status = STATUS.locked();
            let next = match status.next.and_then(clock::local_time) {
                Some(at) => format!("next at {}", at.format("%Y-%m-%d %H:%M")),
                None => String::from("none due in the next 30 days"),
            };
            let mut text = format!(
                "{} shown, {next}, {} remembered as gone off",
                status.shown, status.fired
            );
            if let Some(problem) = &status.problem {
                text.push_str(&format!("\n{problem}"));
            }

            Ok(text)
        }
    }
}

/*
 * runs an action of the reminder keyed `key`, from its pill on the draw thread: Open opens the
 * Calendar Surface there and then, so it is open before the click is done, and both take the
 * reminder away
 */
pub fn act(action: &str, key: &str) {
    if action == OPEN {
        let effect = IslandService::read().resolve(Command::Open(Surface::Calendar));

        if let Ok(Some(effect)) = effect {
            IslandService::write().apply(effect, Instant::now());
        }
    }

    if action == OPEN || action == DISMISS {
        send(Event::Dismiss(Some(key.to_owned())));
    }
}

// called before IPC runs, so no command finds the thread missing
pub fn spawn() {
    let (sender, events) = mpsc::channel();
    let _ = EVENTS.set(sender.clone());

    let timer = match WallTimer::new() {
        Ok(timer) => Arc::new(timer),
        Err(error) => {
            let why = format!("no reminders, as there is no timer ({error})");

            eprintln!("kanade: {why}");
            supervise::stopped("calendar-reminders", why);
            return;
        }
    };

    let waited = timer.clone();
    supervise::spawn("reminder-timer", move || wait(&waited, &sender));
    supervise::spawn("calendar-reminders", move || follow(&events, &timer));
}

// the timer's own thread, which only passes its wakes on
fn wait(timer: &WallTimer, events: &Sender<Event>) {
    loop {
        if let Err(error) = timer.wait() {
            let why = format!("lost the reminders' timer ({error})");

            eprintln!("kanade: {why}");
            supervise::stopped("calendar-reminders", why);
            return;
        }
        if events.send(Event::Timer).is_err() {
            return;
        }
    }
}

// a reminder on the island, and what it last said
#[derive(Debug, Clone)]
struct Shown {
    key: String,
    title: String,
    to: i64,
    ends: bool,

    // when it goes on its own
    until: i64,

    // the titles a MORE stands for
    of: Vec<String>,

    said: Option<Reminder>,
}

// the monitor's state, kept across a restart of its loop
struct Monitor {
    file: Option<PathBuf>,
    fired: Fired,

    // the occurrences near now, the Calendars generation they are of and when they were expanded
    candidates: Vec<Candidate>,
    expanded: Option<(u64, i64)>,

    shown: Vec<Shown>,

    // why what went off could not be kept, if it could not
    problem: Option<String>,
}

fn follow(events: &Receiver<Event>, timer: &WallTimer) {
    let file = config::base(
        "XDG_STATE_HOME",
        ".local/state",
        std::env::var("HOME").ok().as_deref(),
    )
    .map(|dir| dir.join(FILE));
    let mut monitor = Monitor {
        fired: file.as_deref().map(load).unwrap_or_default(),
        file,
        candidates: Vec::new(),
        expanded: None,
        shown: Vec::new(),
        problem: None,
    };

    supervise::run("calendar-reminders", || {
        loop {
            let now = seconds();
            let next = monitor.step(now).unwrap_or(now + REEXPAND);

            if let Err(error) = timer.set(next) {
                eprintln!("kanade: reminders: cannot arm the timer ({error})");
            }

            match events.recv() {
                Ok(Event::Changed | Event::Timer) => {}
                Ok(Event::Dismiss(key)) => monitor.dismiss(key.as_deref()),
                Err(_) => return,
            }

            // whatever came meanwhile is folded into the next step
            while let Ok(event) = events.try_recv() {
                if let Event::Dismiss(key) = event {
                    monitor.dismiss(key.as_deref());
                }
            }
        }
    });
}

impl Monitor {
    // expands if need be, sets off what is due and redraws what shows; the next moment to wake at
    fn step(&mut self, now: i64) -> Option<i64> {
        let calendars = Calendars::read().clone();
        let stale = self.expanded.is_none_or(|(generation, at)| {
            generation != calendars.generation || now >= at + REEXPAND || now < at
        });
        if stale {
            let range = (now - DAY, now + FARTHEST + REEXPAND);

            self.candidates = calendars
                .reminded(range)
                .into_iter()
                .map(candidate)
                .collect();
            self.expanded = Some((calendars.generation, now));
        }

        let lead = Some(i64::from(config::get().reminder) * MINUTE).filter(|&lead| lead > 0);
        let mut changed = self.fired.prune(now);
        let plan = plan(&self.candidates, &self.fired, lead, now);

        for (key, until) in &plan.record {
            self.fired.insert(key.clone(), *until);
            changed = true;
        }
        // a failure to keep them is said until a save works again
        if changed && let Some(file) = &self.file {
            self.problem = save(file, &self.fired).err().map(|error| {
                let problem = format!("cannot keep what went off: {}: {error}", file.display());
                eprintln!("kanade: reminders: {problem}");
                problem
            });
        }

        // the ones already showing count toward AT_ONCE, so a later due joins MORE past it
        let mut more: Vec<&Due> = plan.more.iter().collect();
        for due in &plan.show {
            let until = (due.to + LATE).min(now + HELD).max(now + LATE);

            self.shown.retain(|shown| shown.key != due.occurrence);
            if self.shown.iter().filter(|shown| shown.key != MORE).count() >= AT_ONCE {
                more.push(due);
                continue;
            }
            self.shown.push(Shown {
                key: due.occurrence.clone(),
                title: due.title.clone(),
                to: due.to,
                ends: due.ends,
                until,
                of: Vec::new(),
                said: None,
            });
        }
        if !more.is_empty() {
            let until = more
                .iter()
                .map(|due| (due.to + LATE).min(now + HELD).max(now + LATE))
                .max()
                .unwrap_or(now + LATE);
            let mut of: Vec<String> = self
                .shown
                .iter()
                .find(|shown| shown.key == MORE)
                .map(|shown| shown.of.clone())
                .unwrap_or_default();
            of.extend(more.iter().map(|due| due.title.clone()));

            self.shown.retain(|shown| shown.key != MORE);
            self.shown.push(Shown {
                key: String::from(MORE),
                title: String::new(),
                to: more.iter().map(|due| due.to).min().unwrap_or(now),
                ends: false,
                until,
                of,
                said: None,
            });
        }

        // gone on their own
        let gone: Vec<String> = self
            .shown
            .iter()
            .filter(|shown| now >= shown.until)
            .map(|shown| shown.key.clone())
            .collect();
        for key in &gone {
            withdraw(key);
        }
        self.shown.retain(|shown| now < shown.until);

        // a countdown that reads otherwise now is posted again, in place
        for shown in &mut self.shown {
            let reads = reads(shown, now);

            if shown.said.as_ref() != Some(&reads) {
                IslandService::write().post(activity(&shown.key, reads.clone()), Instant::now());
                shown.said = Some(reads);
            }
        }

        let next = self
            .shown
            .iter()
            .flat_map(|shown| [Some(shown.until), turns(shown.to, now)])
            .flatten()
            .chain(plan.next)
            .chain(self.expanded.map(|(_, at)| at + REEXPAND))
            .min();

        *STATUS.locked() = Status {
            shown: self.shown.len(),
            next: plan.next,
            fired: self.fired.len(),
            problem: self.problem.clone(),
        };

        next
    }

    fn dismiss(&mut self, key: Option<&str>) {
        self.shown.retain(|shown| {
            let goes = key.is_none_or(|key| key == shown.key);
            if goes {
                withdraw(&shown.key);
            }

            !goes
        });
    }
}

// the planner's reading of an occurrence; its event by a digest of its UID, which the file keeps
fn candidate(reminded: Reminded) -> Candidate {
    let hash = digest::digest(&digest::SHA256, reminded.uid.as_bytes());
    let event: String = hash.as_ref()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();

    Candidate {
        event,
        title: reminded.occurrence.title,
        start: reminded.occurrence.span.0,
        end: reminded.occurrence.span.1,
        all_day: reminded.occurrence.all_day,
        triggers: reminded.triggers,
    }
}

fn seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

// the next moment a countdown to `to` reads otherwise, none once it reads now
fn turns(to: i64, now: i64) -> Option<i64> {
    let minutes = minutes_left(to, now);

    (minutes > 0).then(|| to - (minutes - 1) * MINUTE)
}

// whole minutes until `to`, rounded up, so a reminder never says less time than is left
fn minutes_left(to: i64, now: i64) -> i64 {
    let left = to - now;

    if left <= 0 {
        0
    } else {
        (left + MINUTE - 1) / MINUTE
    }
}

// how far off `to` is, at length and in short
pub fn countdown(to: i64, now: i64) -> (String, String) {
    let minutes = minutes_left(to, now);
    let (hours, rest) = (minutes / 60, minutes % 60);
    let days = (minutes + 24 * 60 - 1) / (24 * 60);

    match minutes {
        0 => (String::from("now"), String::from("now")),
        1..60 => (format!("in {minutes} min"), format!("{minutes}m")),
        60..1440 if rest == 0 => (format!("in {hours} h"), format!("{hours}h")),
        60..1440 => (format!("in {hours} h {rest} min"), format!("{hours}h")),
        _ if days == 1 => (String::from("in 1 day"), String::from("1d")),
        _ => (format!("in {days} days"), format!("{days}d")),
    }
}

// what a shown reminder says at `now`
fn reads(shown: &Shown, now: i64) -> Reminder {
    if shown.key == MORE {
        let count = shown.of.len();

        return Reminder {
            title: if count == 1 {
                String::from("1 more reminder")
            } else {
                format!("{count} more reminders")
            },
            when: shown.of.join(", "),
            short: format!("+{count}"),
        };
    }

    let (left, short) = countdown(shown.to, now);
    let at = clock::local_time(shown.to)
        .map(|at| when(at, clock::local_time(now)))
        .unwrap_or_default();
    let when = match shown.ends {
        true => format!("Ends {at}, {left}"),
        false => format!("{at}, {left}"),
    };

    Reminder {
        title: if shown.title.is_empty() {
            String::from("Event")
        } else {
            shown.title.clone()
        },
        when,
        short,
    }
}

// the time of day, with the day before it when that is not today
fn when(at: NaiveDateTime, now: Option<NaiveDateTime>) -> String {
    let time = clock::time(at.time(), config::get().clock);
    let today = now.map(|now| now.date());

    match today {
        Some(today) if at.date() == today => time,
        Some(today) if Some(at.date()) == today.checked_add_days(Days::new(1)) => {
            format!("Tomorrow {time}")
        }
        _ => format!("{} {time}", at.format("%a %-d %b")),
    }
}

fn id(key: &str) -> Id {
    Id::new(Kind::Reminder, key)
}

/*
 * Ongoing and Persistent, as a timer is: something on its way the user keeps in view, a Satellite
 * when another Activity outranks it, never pushing in front of a Surface the user opened
 */
fn activity(key: &str, reminder: Reminder) -> Activity {
    Activity::new(
        id(key),
        Priority::Ongoing,
        Lifetime::Persistent,
        Scope::Global,
        Interrupt::None,
    )
    .expect("a Persistent that does not auto-expand is valid")
    .with_actions(vec![
        Action {
            key: String::from(OPEN),
            label: String::from("Open"),
        },
        Action {
            key: String::from(DISMISS),
            label: String::from("Dismiss"),
        },
    ])
    .with_detail(Detail::Reminder(reminder))
}

fn withdraw(key: &str) {
    IslandService::write().withdraw(&id(key), Instant::now());
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn event(name: &str, start: i64, triggers: &[Trigger]) -> Candidate {
        Candidate {
            event: name.to_owned(),
            title: name.to_owned(),
            start,
            end: start + HOUR,
            all_day: false,
            triggers: triggers.to_vec(),
        }
    }

    fn keep(fired: &mut Fired, plan: &Plan) {
        for (key, until) in &plan.record {
            fired.insert(key.clone(), *until);
        }
    }

    fn shown(plan: &Plan) -> Vec<(&str, i64)> {
        plan.show
            .iter()
            .map(|due| (due.title.as_str(), due.due))
            .collect()
    }

    #[test]
    fn a_reminder_goes_off_once_when_due() {
        let events = [event(
            "standup",
            NOW + 15 * MINUTE,
            &[Trigger::Start(-15 * MINUTE)],
        )];
        let mut fired = Fired::default();

        let early = plan(&events, &fired, None, NOW - 1);
        assert!(early.show.is_empty());
        assert_eq!(early.next, Some(NOW));

        let due = plan(&events, &fired, None, NOW);
        assert_eq!(shown(&due), [("standup", NOW)]);
        keep(&mut fired, &due);

        // nor again, after a restart reads the file back
        let fired = Fired::parse(&fired.text());
        assert!(plan(&events, &fired, None, NOW + 1).show.is_empty());
    }

    #[test]
    fn an_alarm_counts_from_the_end_or_an_absolute_moment() {
        let start = NOW + 10 * MINUTE;
        let events = [event(
            "review",
            start,
            &[Trigger::End(-5 * MINUTE), Trigger::At(NOW)],
        )];

        let now = plan(&events, &Fired::default(), None, NOW);
        assert_eq!(shown(&now), [("review", NOW)]);
        assert!(!now.show[0].ends);
        assert_eq!(now.next, Some(start + HOUR - 5 * MINUTE));

        let mut fired = Fired::default();
        keep(&mut fired, &now);
        let ending = plan(&events, &fired, None, start + HOUR - 5 * MINUTE);
        assert!(ending.show[0].ends);
        assert_eq!(ending.show[0].to, start + HOUR);
    }

    #[test]
    fn the_default_lead_is_for_timed_events_without_alarms() {
        let start = NOW + 10 * MINUTE;
        let mut all_day = event("holiday", start, &[]);
        all_day.all_day = true;
        let events = [
            event("plain", start, &[]),
            event("own", start, &[Trigger::Start(-5 * MINUTE)]),
            all_day,
        ];

        assert!(plan(&events, &Fired::default(), None, NOW).show.is_empty());

        let with_lead = plan(&events, &Fired::default(), Some(10 * MINUTE), NOW);
        assert_eq!(shown(&with_lead), [("plain", NOW)]);
        assert_eq!(with_lead.next, Some(start - 5 * MINUTE));
    }

    #[test]
    fn a_new_default_lead_never_sets_off_its_reminder_again() {
        let events = [event("plain", NOW + 10 * MINUTE, &[])];
        let mut fired = Fired::default();

        let first = plan(&events, &fired, Some(10 * MINUTE), NOW);
        keep(&mut fired, &first);
        assert!(
            plan(&events, &fired, Some(5 * MINUTE), NOW + 5 * MINUTE)
                .show
                .is_empty()
        );
    }

    #[test]
    fn several_due_at_once_show_the_one_nearest_the_event() {
        let start = NOW + 5 * MINUTE;
        let events = [event(
            "flight",
            start,
            &[
                Trigger::Start(-DAY),
                Trigger::Start(-HOUR),
                Trigger::Start(-10 * MINUTE),
            ],
        )];

        let plan = plan(&events, &Fired::default(), None, NOW);
        assert_eq!(shown(&plan), [("flight", start - 10 * MINUTE)]);
        assert_eq!(plan.record.len(), 3);
    }

    #[test]
    fn an_event_keeps_at_most_its_nearest_reminders() {
        let start = NOW + 10 * DAY;
        let triggers: Vec<Trigger> = (1..=8).map(|day| Trigger::Start(-day * DAY)).collect();
        let events = [event("trip", start, &triggers)];

        // the 8 days before are past and go off unseen, nearest the event kept
        let plan = plan(&events, &Fired::default(), None, NOW);
        assert!(plan.show.is_empty());
        assert_eq!(plan.next, Some(start - 5 * DAY));
        let mut counted = Fired::default();
        let mut now = NOW;
        let mut went = 0;
        while let Some(next) = super::plan(&events, &counted, None, now).next {
            now = next;
            let at = super::plan(&events, &counted, None, now);
            went += at.show.len();
            keep(&mut counted, &at);
        }
        assert_eq!(went, PER_EVENT);
    }

    #[test]
    fn one_too_far_ahead_goes() {
        let events = [event(
            "far",
            NOW + 40 * DAY,
            &[Trigger::Start(-35 * DAY), Trigger::At(NOW + DAY)],
        )];

        let plan = plan(&events, &Fired::default(), None, NOW);
        assert_eq!(plan.next, None);
    }

    #[test]
    fn a_missed_reminder_goes_off_until_its_event_is_late() {
        let start = NOW;
        let events = [event("missed", start, &[Trigger::Start(-HOUR)])];

        // the shell was off when it went due: it goes off as the shell starts, before the event
        assert_eq!(
            shown(&plan(&events, &Fired::default(), None, start - MINUTE)),
            [("missed", start - HOUR)]
        );
        assert_eq!(
            plan(&events, &Fired::default(), None, start + LATE - 1)
                .show
                .len(),
            1
        );
        assert!(
            plan(&events, &Fired::default(), None, start + LATE)
                .show
                .is_empty()
        );
    }

    #[test]
    fn past_a_few_at_once_the_rest_show_as_one() {
        let events: Vec<Candidate> = (0..5)
            .map(|index| {
                event(
                    &format!("event {index}"),
                    NOW + (index + 1) * MINUTE,
                    &[Trigger::Start(-HOUR)],
                )
            })
            .collect();

        let plan = plan(&events, &Fired::default(), None, NOW);
        assert_eq!(plan.show.len(), AT_ONCE);
        assert_eq!(plan.show[0].title, "event 0");
        assert_eq!(plan.more.len(), 2);
        assert_eq!(plan.record.len(), 5);
    }

    #[test]
    fn a_fired_set_forgets_only_what_cannot_go_off_again() {
        let mut fired = Fired::parse("100\ta|1|d\nnot a line\n300\tb|2|s-60\n\t\nx\tc\n");
        assert_eq!(fired.len(), 2);

        assert!(fired.prune(100));
        assert!(!fired.contains("a|1|d"));
        assert!(fired.contains("b|2|s-60"));
        assert!(!fired.prune(299));
    }

    #[test]
    fn a_fired_set_is_saved_whole_and_read_back() {
        let dir = std::env::temp_dir().join(format!("kanade-reminders-{}", std::process::id()));
        let path = dir.join("state/reminders");
        let mut fired = Fired::default();
        fired.insert(String::from("a|1|d"), 10);
        fired.insert(String::from("b|2|e-300"), 20);

        save(&path, &fired).unwrap();
        assert_eq!(load(&path), fired);
        assert!(!path.with_extension("part").exists());

        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        assert_eq!(load(&dir.join("missing")), Fired::default());
        fs::remove_dir_all(&dir).unwrap();
    }
}
