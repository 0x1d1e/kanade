//! Focused time per app (ADR 0138). Off, the Module starts nothing, so this file is never read
//! and no focus callback is registered. On, niri's existing window events feed a worker, which
//! writes the private state file and posts the limit Transient. The draw thread only reads the
//! Service.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use chrono::{Days, NaiveDate};
use kanade_runtime::{MutexExt, service::Service};

use super::niri::{self, AppHeard, AppWindow};
use crate::atomic::{self, Mode};
use crate::clock;
use crate::config;
use crate::island::activity::{Activity, Detail, Id, Interrupt, Kind, Lifetime, Priority, Scope};
use crate::island::service::IslandService;
use crate::supervise;

// kept days, and apps in one day; past that the oldest day and the smallest app go
const DAYS: usize = 7;
const APPS: usize = 48;

// an app id and a title, cut so a window cannot grow the file
const ID: usize = 128;
const TITLE: usize = 80;

// a file larger than this is ignored, not trusted
const FILE_MOST: usize = 256 * 1024;

const FILE: &str = "kanade/screen-time";
const UNKNOWN: &str = "unknown";

// the limit notice; no app id and no title
const NOTICE: &str = "Reached today's limit";

// how long the notice shows
const SHOWN: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Win {
    id: u64,
    app_id: String,
    title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Open {
    id: String,
    title: Option<String>,
    since: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AppTime {
    id: String,
    seconds: u64,
    title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Day {
    date: NaiveDate,
    apps: Vec<AppTime>,
}

// today's list, as the Surface reads it. Titles are absent unless the switch is on
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub seconds: u64,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScreenTime {
    pub total: u64,
    pub rows: Vec<Row>,

    // the open interval, so a frame can add what has passed since the last event
    pub open_id: Option<String>,
    pub open_since: Option<i64>,
    pub open_title: Option<String>,
}

impl Service for ScreenTime {
    fn new() -> Self {
        ScreenTime::default()
    }

    fn listen() {}
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Ledger {
    days: Vec<Day>,
    windows: BTreeMap<u64, Win>,
    open: Option<Open>,

    // the window niri last said was focused, so a later change of its app id follows it
    focused: Option<u64>,
    fired: Option<NaiveDate>,
    dirty: bool,
}

struct Inbox {
    notes: Mutex<Vec<AppHeard>>,
    wake: Condvar,
}

static INBOX: OnceLock<Inbox> = OnceLock::new();

// registered only from `start`, which the catalog calls only while the Module is on
pub fn start() {
    INBOX.get_or_init(|| Inbox {
        notes: Mutex::new(Vec::new()),
        wake: Condvar::new(),
    });
    niri::on_apps(hear);
    supervise::spawn("screen-time", follow);
}

pub fn reload() {
    if let Some(inbox) = INBOX.get() {
        inbox.wake.notify_one();
    }
}

fn hear(event: AppHeard) {
    let Some(inbox) = INBOX.get() else {
        return;
    };

    inbox.notes.locked().push(event);
    inbox.wake.notify_one();
}

fn follow() {
    let mut ledger = path()
        .as_deref()
        .and_then(read_file)
        .map(|bytes| Ledger::decode(&bytes))
        .unwrap_or_default();
    let inbox = INBOX.get().expect("start sets the inbox");

    loop {
        let titles = config::get().screen_time_titles;
        let limit = limit_secs();

        if !titles {
            ledger.drop_titles();
        }

        let now = clock::unix_seconds();
        let until = ledger.due(now, limit, local_date);
        let notes = take(inbox, until);
        let now = clock::unix_seconds();
        let mut fire = false;

        if notes.is_empty() {
            fire |= ledger.account(now, limit, local_date);
        } else {
            for note in notes {
                fire |= ledger.note(note, now, titles, limit, local_date);
            }
        }

        publish(&ledger, now, titles);

        if ledger.dirty {
            if let Some(path) = path() {
                save(&path, &ledger, titles);
            }
            ledger.dirty = false;
        }

        if fire {
            post_limit();
        }
    }
}

fn take(inbox: &Inbox, until: Option<i64>) -> Vec<AppHeard> {
    let mut notes = inbox.notes.locked();

    loop {
        if !notes.is_empty() {
            return std::mem::take(&mut *notes);
        }

        let now = clock::unix_seconds();

        match until {
            Some(until) if until <= now => return Vec::new(),
            Some(until) => {
                let left = Duration::from_secs((until - now) as u64);
                let (guard, _) = inbox
                    .wake
                    .wait_timeout(notes, left)
                    .unwrap_or_else(PoisonError::into_inner);

                notes = guard;

                // a timeout or a reload, which queues nothing, rereads the switches
                if notes.is_empty() {
                    return Vec::new();
                }
            }
            None => {
                notes = inbox
                    .wake
                    .wait(notes)
                    .unwrap_or_else(PoisonError::into_inner);

                if notes.is_empty() {
                    return Vec::new();
                }
            }
        }
    }
}

fn limit_secs() -> u64 {
    config::get().screen_time_limit.as_secs()
}

fn local_date(seconds: i64) -> NaiveDate {
    clock::local_time(seconds)
        .map(|time| time.date())
        .unwrap_or_else(|| utc_date(seconds))
}

fn utc_date(seconds: i64) -> NaiveDate {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default();
    let days = seconds.div_euclid(86_400);

    if days >= 0 {
        epoch
            .checked_add_days(Days::new(days as u64))
            .unwrap_or(epoch)
    } else {
        epoch
            .checked_sub_days(Days::new(days.unsigned_abs()))
            .unwrap_or(epoch)
    }
}

fn path() -> Option<PathBuf> {
    config::base(
        "XDG_STATE_HOME",
        ".local/state",
        std::env::var("HOME").ok().as_deref(),
    )
    .map(|dir| dir.join(FILE))
}

fn read_file(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let mut bytes = Vec::new();

    file.by_ref()
        .take(FILE_MOST as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;

    (bytes.len() <= FILE_MOST).then_some(bytes)
}

fn save(path: &Path, ledger: &Ledger, titles: bool) {
    if let Err(error) = atomic::write(path, ledger.encode(titles).as_bytes(), Mode::Private) {
        eprintln!("kanade: screen time: not written ({error})");
    }
}

fn publish(ledger: &Ledger, now: i64, titles: bool) {
    let day = local_date(now);
    let next = ledger.snapshot(day, titles);
    let mut service = ScreenTime::write();

    if *service == next {
        service.quiet();
    } else {
        *service = next;
    }
}

fn post_limit() {
    let activity = Activity::new(
        Id::new(Kind::ScreenTime, "limit"),
        Priority::Feedback,
        Lifetime::Transient(SHOWN),
        Scope::FocusedOutput,
        Interrupt::None,
    )
    .expect("the notice lasts")
    .with_detail(Detail::ScreenTime(String::from(NOTICE)));

    IslandService::write().post(activity, Instant::now());
}

fn app_id(raw: Option<&str>) -> String {
    let Some(raw) = raw else {
        return String::from(UNKNOWN);
    };
    let id: String = raw
        .chars()
        .filter(|character| !character.is_control() && *character != '\t')
        .take(ID)
        .collect();

    if id.is_empty() {
        String::from(UNKNOWN)
    } else {
        id
    }
}

fn title_of(raw: Option<&str>, titles: bool) -> Option<String> {
    if !titles {
        return None;
    }

    let title: String = raw?
        .chars()
        .filter(|character| !character.is_control() && *character != '\t')
        .take(TITLE)
        .collect();
    let title = title.trim();

    (!title.is_empty()).then(|| title.to_owned())
}

fn win_of(window: &AppWindow, titles: bool) -> Win {
    Win {
        id: window.id,
        app_id: app_id(window.app_id.as_deref()),
        title: title_of(window.title.as_deref(), titles),
    }
}

impl Ledger {
    fn note(
        &mut self,
        heard: AppHeard,
        now: i64,
        titles: bool,
        limit: u64,
        date_of: impl Fn(i64) -> NaiveDate,
    ) -> bool {
        let fired = self.account(now, limit, &date_of);

        match heard {
            AppHeard::All(windows) => {
                let focused = windows
                    .iter()
                    .find(|window| window.focused)
                    .map(|window| window.id);

                self.windows = windows
                    .iter()
                    .map(|window| (window.id, win_of(window, titles)))
                    .collect();
                self.focused = focused;
                self.focus(focused, now, &date_of);
            }
            AppHeard::Opened(window) => {
                let id = window.id;

                if window.focused {
                    self.focused = Some(id);
                }
                self.windows.insert(id, win_of(&window, titles));
                if window.focused || self.focused == Some(id) {
                    self.focus(self.focused, now, &date_of);
                }
            }
            AppHeard::Closed(id) => {
                self.windows.remove(&id);
                if self.focused == Some(id) {
                    self.focused = None;
                    self.focus(None, now, &date_of);
                }
            }
            AppHeard::Focused(id) => {
                self.focused = id;
                self.focus(id, now, &date_of);
            }
            AppHeard::Lost => {
                self.windows.clear();
                self.focused = None;
                self.open = None;
                self.dirty = true;
            }
        }

        if !titles {
            self.drop_titles();
        }

        fired
    }

    // the focused window's app, or none when no window has the focus
    fn focus(&mut self, id: Option<u64>, now: i64, date_of: &impl Fn(i64) -> NaiveDate) {
        let next = id.and_then(|id| self.windows.get(&id)).cloned();

        match next {
            Some(win) if self.open.as_ref().is_some_and(|open| open.id == win.app_id) => {
                if win.title.is_some() {
                    if let Some(open) = &mut self.open {
                        open.title = win.title.clone();
                    }
                    self.remember_title(&win.app_id, win.title.as_deref(), date_of(now));
                }
            }
            Some(win) => {
                self.open = Some(Open {
                    id: win.app_id,
                    title: win.title,
                    since: now,
                });
                self.dirty = true;
            }
            None => {
                if self.open.take().is_some() {
                    self.dirty = true;
                }
            }
        }
    }

    fn remember_title(&mut self, id: &str, title: Option<&str>, day: NaiveDate) {
        let Some(title) = title else {
            return;
        };
        if let Some(found) = self.days.iter_mut().find(|found| found.date == day)
            && let Some(app) = found.apps.iter_mut().find(|app| app.id == id)
        {
            app.title = Some(title.to_owned());
            self.dirty = true;
        }
    }

    fn drop_titles(&mut self) {
        let mut dropped = false;

        for day in &mut self.days {
            for app in &mut day.apps {
                dropped |= app.title.take().is_some();
            }
        }
        if let Some(open) = &mut self.open {
            dropped |= open.title.take().is_some();
        }
        for win in self.windows.values_mut() {
            dropped |= win.title.take().is_some();
        }
        self.dirty |= dropped;
    }

    // close the open interval up to `now`, splitting on local midnights. Whether the limit crossed
    fn account(&mut self, now: i64, limit: u64, date_of: impl Fn(i64) -> NaiveDate) -> bool {
        let Some(open) = self.open.clone() else {
            return false;
        };

        if now <= open.since {
            return false;
        }

        let mut at = open.since;
        let mut fired = false;

        while at < now {
            let day = date_of(at);
            let end = now.min(next_midnight(at, day, &date_of));

            if end <= at {
                break;
            }

            let before = self.seconds(day);
            self.add(day, &open.id, open.title.as_deref(), (end - at) as u64);
            fired |= self.cross(day, before, self.seconds(day), limit);
            at = end;
        }

        self.open = Some(Open { since: now, ..open });
        self.dirty = true;
        fired
    }

    fn add(&mut self, day: NaiveDate, id: &str, title: Option<&str>, seconds: u64) {
        if seconds == 0 {
            return;
        }

        let day = self.day(day);

        if let Some(app) = day.apps.iter_mut().find(|app| app.id == id) {
            app.seconds = app.seconds.saturating_add(seconds);
            if title.is_some() {
                app.title = title.map(str::to_owned);
            }
        } else {
            day.apps.push(AppTime {
                id: id.to_owned(),
                seconds,
                title: title.map(str::to_owned),
            });
        }

        if day.apps.len() > APPS {
            let keep = id.to_owned();
            let drop_at = day
                .apps
                .iter()
                .enumerate()
                .filter(|(_, app)| app.id != keep)
                .min_by_key(|(_, app)| app.seconds)
                .map(|(index, _)| index);

            if let Some(index) = drop_at {
                day.apps.remove(index);
            }
        }

        self.days.sort_by_key(|day| day.date);
        while self.days.len() > DAYS {
            self.days.remove(0);
        }
    }

    fn day(&mut self, date: NaiveDate) -> &mut Day {
        if let Some(index) = self.days.iter().position(|day| day.date == date) {
            return &mut self.days[index];
        }

        self.days.push(Day {
            date,
            apps: Vec::new(),
        });
        self.days.last_mut().expect("just pushed")
    }

    fn seconds(&self, day: NaiveDate) -> u64 {
        self.days
            .iter()
            .find(|known| known.date == day)
            .map(|day| day.apps.iter().map(|app| app.seconds).sum())
            .unwrap_or(0)
    }

    fn cross(&mut self, day: NaiveDate, before: u64, after: u64, limit: u64) -> bool {
        if limit == 0 || self.fired == Some(day) || before >= limit || after < limit {
            return false;
        }

        self.fired = Some(day);
        self.dirty = true;
        true
    }

    // when the open interval will cross the limit, if it will, and not after it already has
    fn due(&self, now: i64, limit: u64, date_of: impl Fn(i64) -> NaiveDate) -> Option<i64> {
        let open = self.open.as_ref()?;

        let day = date_of(now);
        let midnight = next_midnight(now, day, &date_of);

        if limit == 0 || self.fired == Some(day) {
            return Some(midnight);
        }

        let closed = self.seconds(day);
        if closed >= limit {
            return Some(midnight);
        }

        let at = open.since.saturating_add((limit - closed) as i64);

        Some(at.min(midnight))
    }

    fn snapshot(&self, day: NaiveDate, titles: bool) -> ScreenTime {
        let mut rows: Vec<Row> = self
            .days
            .iter()
            .find(|known| known.date == day)
            .map(|day| {
                day.apps
                    .iter()
                    .map(|app| Row {
                        id: app.id.clone(),
                        seconds: app.seconds,
                        title: titles.then(|| app.title.clone()).flatten(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        rows.sort_by(|one, other| other.seconds.cmp(&one.seconds).then(one.id.cmp(&other.id)));

        ScreenTime {
            total: rows.iter().map(|row| row.seconds).sum(),
            open_id: self.open.as_ref().map(|open| open.id.clone()),
            open_since: self.open.as_ref().map(|open| open.since),
            open_title: titles
                .then(|| self.open.as_ref().and_then(|open| open.title.clone()))
                .flatten(),
            rows,
        }
    }

    fn encode(&self, titles: bool) -> String {
        let mut out = String::from("1\n");

        for day in &self.days {
            out.push_str(&format!("D {}\n", day.date.format("%Y-%m-%d")));

            for app in &day.apps {
                out.push_str(&format!("A {} {}\n", app.seconds, app.id));

                if titles && let Some(title) = &app.title {
                    out.push_str(&format!("T {}\t{title}\n", app.id));
                }
            }
        }

        if let Some(fired) = self.fired {
            out.push_str(&format!("F {}\n", fired.format("%Y-%m-%d")));
        }

        out
    }

    fn decode(bytes: &[u8]) -> Ledger {
        if bytes.len() > FILE_MOST {
            return Ledger::default();
        }

        let text = String::from_utf8_lossy(bytes);
        let mut lines = text.lines();

        if lines.next() != Some("1") {
            return Ledger::default();
        }

        let mut ledger = Ledger::default();
        let mut day: Option<NaiveDate> = None;

        for line in lines {
            let mut parts = line.splitn(2, ' ');
            match (parts.next(), parts.next()) {
                (Some("D"), Some(date)) => {
                    day = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok();
                    if let Some(date) = day {
                        ledger.days.push(Day {
                            date,
                            apps: Vec::new(),
                        });
                    }
                }
                (Some("A"), Some(rest)) if day.is_some() => {
                    let mut parts = rest.splitn(2, ' ');
                    let seconds = parts.next().and_then(|text| text.parse().ok()).unwrap_or(0);
                    let id = app_id(parts.next());

                    if seconds > 0
                        && let Some(day) = ledger.days.last_mut()
                    {
                        day.apps.push(AppTime {
                            id,
                            seconds,
                            title: None,
                        });
                    }
                }
                (Some("T"), Some(rest)) => {
                    let Some((id, title)) = rest.split_once('\t') else {
                        continue;
                    };
                    let id = app_id(Some(id));
                    let title = title_of(Some(title), true);

                    if let Some(day) = ledger.days.last_mut()
                        && let Some(app) = day.apps.iter_mut().find(|app| app.id == id)
                    {
                        app.title = title;
                    }
                }
                (Some("F"), Some(date)) => {
                    ledger.fired = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok();
                }
                _ => {}
            }
        }

        ledger.days.sort_by_key(|day| day.date);
        while ledger.days.len() > DAYS {
            ledger.days.remove(0);
        }
        for day in &mut ledger.days {
            day.apps.sort_by_key(|app| std::cmp::Reverse(app.seconds));
            day.apps.truncate(APPS);
        }

        ledger
    }
}

fn next_midnight(at: i64, day: NaiveDate, date_of: &impl Fn(i64) -> NaiveDate) -> i64 {
    let mut lo = at + 1;
    let mut hi = at.saturating_add(26 * 3600);

    if date_of(hi) == day {
        return hi;
    }

    while lo < hi {
        let mid = lo + (hi - lo) / 2;

        if date_of(mid) == day {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }

    lo
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn day(year: i32, month: u32, date: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, date).unwrap()
    }

    fn utc(seconds: i64) -> NaiveDate {
        utc_date(seconds)
    }

    fn at(year: i32, month: u32, date: u32, hour: u32, min: u32) -> i64 {
        let days = (day(year, month, date) - day(1970, 1, 1)).num_days();

        days * 86_400 + i64::from(hour) * 3600 + i64::from(min) * 60
    }

    fn window(id: u64, app: &str, title: Option<&str>, focused: bool) -> AppWindow {
        AppWindow {
            id,
            app_id: Some(app.to_owned()),
            title: title.map(str::to_owned),
            focused,
        }
    }

    #[test]
    fn a_focus_change_moves_time_to_the_new_app() {
        let mut ledger = Ledger::default();
        let start = at(2026, 8, 14, 10, 0);

        ledger.note(
            AppHeard::All(vec![
                window(1, "kitty", None, true),
                window(2, "firefox", None, false),
            ]),
            start,
            false,
            0,
            utc,
        );
        ledger.note(AppHeard::Focused(Some(1)), start, false, 0, utc);
        ledger.note(AppHeard::Focused(Some(2)), start + 30, false, 0, utc);

        let snap = ledger.snapshot(utc(start), false);

        assert_eq!(snap.rows[0].id, "kitty");
        assert_eq!(snap.rows[0].seconds, 30);
        assert_eq!(snap.open_id.as_deref(), Some("firefox"));
        assert!(snap.rows.iter().all(|row| row.title.is_none()));
    }

    #[test]
    fn time_splits_on_the_local_day() {
        let mut ledger = Ledger::default();
        let late = at(2026, 8, 14, 23, 0);
        let early = at(2026, 8, 15, 1, 0);

        ledger.note(
            AppHeard::Opened(window(1, "kitty", None, true)),
            late,
            false,
            0,
            utc,
        );
        ledger.note(AppHeard::Focused(Some(1)), late, false, 0, utc);
        ledger.account(early, 0, utc);

        assert_eq!(ledger.seconds(day(2026, 8, 14)), 3600);
        assert_eq!(ledger.seconds(day(2026, 8, 15)), 3600);
        assert_eq!(ledger.days.len(), 2);
    }

    #[test]
    fn the_limit_fires_once_when_the_total_crosses_it() {
        let mut ledger = Ledger::default();
        let start = at(2026, 8, 14, 10, 0);
        let limit = 60;

        ledger.note(
            AppHeard::Opened(window(1, "kitty", None, true)),
            start,
            false,
            limit,
            utc,
        );

        assert!(!ledger.account(start + 59, limit, utc));
        assert!(ledger.account(start + 61, limit, utc));
        assert!(!ledger.account(start + 120, limit, utc));
        assert_eq!(ledger.fired, Some(day(2026, 8, 14)));

        let next = at(2026, 8, 15, 10, 0);
        ledger.note(AppHeard::Focused(None), start + 120, false, limit, utc);
        ledger.note(AppHeard::Focused(Some(1)), next, false, limit, utc);
        assert!(!ledger.account(next + 59, limit, utc));
        assert!(ledger.account(next + 61, limit, utc));
    }

    #[test]
    fn titles_are_kept_only_while_the_switch_is_on() {
        let mut ledger = Ledger::default();
        let start = at(2026, 8, 14, 10, 0);

        ledger.note(
            AppHeard::Opened(window(1, "kitty", Some("secret title"), true)),
            start,
            false,
            0,
            utc,
        );
        ledger.note(AppHeard::Focused(Some(1)), start, false, 0, utc);
        ledger.account(start + 5, 0, utc);

        assert!(ledger.encode(false).contains("kitty"));
        assert!(!ledger.encode(false).contains("secret"));
        assert!(
            ledger
                .snapshot(utc(start), false)
                .rows
                .iter()
                .all(|row| row.title.is_none())
        );

        ledger.note(
            AppHeard::Opened(window(1, "kitty", Some("secret title"), true)),
            start + 5,
            true,
            0,
            utc,
        );
        ledger.account(start + 10, 0, utc);

        assert!(ledger.encode(true).contains("secret title"));
        assert!(!ledger.encode(false).contains("secret"));
    }

    #[test]
    fn ids_and_the_file_are_bounded() {
        let long = "a".repeat(200);
        let messy = String::from("ki\ntty\u{0007}");

        assert_eq!(app_id(Some(&long)).chars().count(), ID);
        assert_eq!(app_id(Some(&messy)), "kitty");
        assert_eq!(app_id(Some("")), UNKNOWN);
        assert_eq!(
            title_of(Some("  hi\nthere  "), true).as_deref(),
            Some("hithere")
        );
        assert_eq!(title_of(Some("secret"), false), None);

        let mut ledger = Ledger::default();
        let today = day(2026, 8, 14);

        for index in 0..APPS + 3 {
            ledger.add(today, &format!("app-{index}"), None, index as u64 + 1);
        }
        assert_eq!(ledger.days[0].apps.len(), APPS);

        for index in 0..DAYS + 2 {
            ledger.add(day(2026, 8, 1 + index as u32), "kitty", None, 1);
        }
        assert_eq!(ledger.days.len(), DAYS);
        assert_eq!(ledger.days[0].date, day(2026, 8, 4));

        let encoded = ledger.encode(false);
        let back = Ledger::decode(encoded.as_bytes());
        assert_eq!(back.days.len(), DAYS);

        let huge = vec![b'1'; FILE_MOST + 1];
        assert_eq!(Ledger::decode(&huge), Ledger::default());
    }

    #[test]
    fn the_state_file_is_private_and_round_trips() {
        let mut ledger = Ledger::default();
        ledger.add(day(2026, 8, 14), "kitty", Some("nope"), 12);
        ledger.fired = Some(day(2026, 8, 14));

        let dir = std::env::temp_dir().join(format!("kanade-screen-time-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("screen-time");

        save(&path, &ledger, false);

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let back = Ledger::decode(&std::fs::read(&path).unwrap());
        assert_eq!(back.seconds(day(2026, 8, 14)), 12);
        assert_eq!(back.fired, Some(day(2026, 8, 14)));
        assert!(
            back.days
                .iter()
                .all(|day| day.apps.iter().all(|app| app.title.is_none()))
        );
        assert!(!String::from_utf8_lossy(&std::fs::read(&path).unwrap()).contains("nope"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_notice_names_no_app() {
        assert_eq!(NOTICE, "Reached today's limit");
        assert!(!NOTICE.contains("kitty"));
    }

    #[test]
    fn screen_time_is_off_by_default() {
        assert!(config::Config::default().off("screen-time"));
    }
}
