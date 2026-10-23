//! Synced lyrics for the Media Surface (`media.lyrics`, ADR 0063): the line of the playing track at
//! its MPRIS position, from a `.lrc` file beside the track a local URL names, or also from LRCLIB.
//!
//! Nothing here runs until the Surface is open with the setting on. The media follower then says
//! what it wants (`want`), and one worker thread, started on the first ask, looks it up: the
//! `.lrc` file beside the track a local URL names, read whether it plays or not, else (`lrclib`
//! only, and only while it plays) the cache, else one `GET /api/get` over https for the track's
//! title, artist, album and length. A failed look-up waits, for that track, past a pause or the
//! Surface closing, and the Surface closing drops the lyrics, so nothing is asked for a track no
//! one watches.
//!
//! The cache (`$XDG_CACHE_HOME/kanade/lyrics`) holds an answer per track, named by a hash of its
//! title, artist, album and length, so no file name tells what was played; a miss is kept for
//! `NEGATIVE` only, and at most `CACHE_FILES` files stay. A request that fails waits twice as long
//! after each failure in a row (`Backoff`) or what `Retry-After` says if longer, and two requests
//! are `GAP` apart. A 429 holds every request back, for any track, but not a file beside a track
//! or the cache. The request says `Kanade/<version>` and nothing else of this machine. Neither
//! the lyrics nor the track are logged or shown in `status`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, LazyLock, Mutex, Once};
use std::time::{Duration, Instant, SystemTime};

use kanade_runtime::MutexExt;
use kanade_runtime::queue::Queue;
use ring::digest;
use ureq::Agent;

use super::media::{self, unescape};
use super::body;
use crate::atomic::{self, Mode};
use crate::backoff::Backoff;
use crate::config;
use crate::look::LyricsSource;
use crate::supervise;

// where LRCLIB answers for one track
const ENDPOINT: &str = "https://lrclib.net/api/get";

// under the cache directory
const DIRECTORY: &str = "kanade/lyrics";

// the most a lyrics file or an answer may be
const BODY: u64 = 256 * 1024;

// how long a request may take
const PATIENCE: Duration = Duration::from_secs(15);

// the redirects followed, each of them https too
const REDIRECTS: u32 = 2;

// how long a miss is believed, as lyrics are added to LRCLIB over time
const NEGATIVE: Duration = Duration::from_secs(2 * 60 * 60);

// the most answers kept
const CACHE_FILES: usize = 200;

// between two requests at least
const GAP: Duration = Duration::from_millis(500);

// after a failure: the first wait, and the longest
const RETRY: Duration = Duration::from_secs(30);
const RETRY_LONGEST: Duration = Duration::from_secs(60 * 60);

// the longest `Retry-After` believed
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

// a file of more lines is no song, and a line longer than this is cut
const LINES: usize = 4096;
const LINE_CHARS: usize = 300;

// one timed line
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub at: Duration,
    pub text: String,
}

/*
 * the track to find lyrics for, as the player names it. Not `Debug`, so that it is never printed by
 * accident; `key` is the hash that stands for it everywhere else
 */
#[derive(Clone, PartialEq)]
pub struct Wanted {
    mode: LyricsSource,

    // the track's `mpris:url`, where a `file://` one has a `.lrc` beside it
    url: String,
    title: String,
    artist: String,
    album: String,
    length: Duration,
    key: String,
}

impl Wanted {
    pub fn new(
        mode: LyricsSource,
        url: &str,
        title: &str,
        artist: &str,
        album: &str,
        length: Duration,
    ) -> Self {
        let key = hash(&[url, title, artist, album], length);

        Self {
            mode,
            url: url.to_owned(),
            title: title.trim().to_owned(),
            artist: artist.trim().to_owned(),
            album: album.trim().to_owned(),
            length,
            key,
        }
    }

    // LRCLIB can be asked for this much, and the cache names the answer by it, not by the file
    fn askable(&self) -> bool {
        !self.title.is_empty() && !self.artist.is_empty() && !self.length.is_zero()
    }

    fn cache_name(&self) -> String {
        hash(
            &[
                &self.title.to_lowercase(),
                &self.artist.to_lowercase(),
                &self.album.to_lowercase(),
            ],
            self.length,
        )
    }
}

// a hash of the parts, hex, as a name that tells nothing of the track
fn hash(parts: &[&str], length: Duration) -> String {
    let mut context = digest::Context::new(&digest::SHA256);

    for part in parts {
        context.update(part.as_bytes());
        context.update(&[0]);
    }
    context.update(&length.as_secs().to_le_bytes());

    context.finish().as_ref()[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// the lyrics found, for the track whose key this is
struct Loaded {
    key: String,
    lines: Arc<[Line]>,
}

static CURRENT: Mutex<Option<Loaded>> = Mutex::new(None);

// how the last look-up ended, for `status`; no word of it names a track
static OUTCOME: Mutex<&'static str> = Mutex::new("nothing looked up yet");

// what the follower asked of the worker
enum Message {
    // a track plays and its lyrics are wanted
    Want(Box<Wanted>),

    // it stopped playing: what was found stays, and a retry that waits goes on waiting
    Pause,

    // nobody watches: forget it all
    Stop,
}

// the worker's queue, which outlives a restart of it
static QUEUE: LazyLock<Queue<Message>> = LazyLock::new(Queue::new);

// what was asked last, so a follower that asks every tick sends only a change
static ASKED: Mutex<Option<(Wanted, bool)>> = Mutex::new(None);

/*
 * the track that is shown, and whether it plays. Lyrics are looked up while it plays; paused, none
 * is, what was found stays on screen, and a retry that waits goes on waiting
 */
pub fn want(wanted: &Wanted, playing: bool) {
    let mut asked = ASKED.locked();

    if asked
        .as_ref()
        .is_some_and(|(last, last_playing)| last == wanted && *last_playing == playing)
    {
        return;
    }

    static STARTED: Once = Once::new();
    STARTED.call_once(|| supervise::spawn("lyrics", follow));

    QUEUE.send(if playing {
        Message::Want(Box::new(wanted.clone()))
    } else {
        Message::Pause
    });
    *asked = Some((wanted.clone(), playing));
}

// nothing is watched, or lyrics are off: the lyrics found are forgotten
pub fn stop() {
    let mut asked = ASKED.locked();

    if asked.take().is_some() {
        QUEUE.send(Message::Stop);
    }
}

// what the Surface shows now
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Shown {
    // none before the first line, in a gap, or while nothing is found
    pub text: Option<String>,

    // how long until the next line, if there is one
    pub until: Option<Duration>,
}

// the line of `wanted` at `position`
pub fn line(wanted: &Wanted, position: Duration) -> Shown {
    let current = CURRENT.locked();

    match current.as_ref() {
        Some(loaded) if loaded.key == wanted.key => at(&loaded.lines, position),
        _ => Shown::default(),
    }
}

fn at(lines: &[Line], position: Duration) -> Shown {
    let passed = lines.partition_point(|line| line.at <= position);

    Shown {
        text: passed
            .checked_sub(1)
            .map(|index| lines[index].text.clone())
            .filter(|text| !text.is_empty()),
        until: lines
            .get(passed)
            .map(|next| next.at.saturating_sub(position)),
    }
}

// the line `kanade status` shows, none while lyrics are off
pub fn status() -> Vec<String> {
    let mode = config::get().lyrics;

    if mode == LyricsSource::Off {
        return Vec::new();
    }

    vec![format!(
        "lyrics: {}, last look-up: {}",
        mode.name(),
        OUTCOME.locked()
    )]
}

// where answers are kept
fn directory() -> Option<PathBuf> {
    config::base(
        "XDG_CACHE_HOME",
        ".cache",
        std::env::var("HOME").ok().as_deref(),
    )
    .map(|dir| dir.join(DIRECTORY))
}

// the HTTP client; `secure` false only in tests, which talk to a server on this machine
fn agent(secure: bool) -> Agent {
    Agent::config_builder()
        .https_only(secure)
        .max_redirects(REDIRECTS)
        .timeout_global(Some(PATIENCE))
        .http_status_as_error(false)
        .user_agent(concat!("Kanade/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/*
 * the timed lines of an LRC file: `[mm:ss.xx]text`, several stamps to a line, `[offset:ms]`
 * honored, other tags and the stamps inside a line ignored. Lines come out in time order
 */
pub fn parse(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut offset = 0i64;

    for raw in text.trim_start_matches('\u{feff}').lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();

        while let Some(tag) = rest.strip_prefix('[') {
            let Some((tag, after)) = tag.split_once(']') else {
                break;
            };

            if let Some(at) = stamp(tag) {
                stamps.push(at);
            } else if let Some(value) = tag.strip_prefix("offset:") {
                offset = value.trim().parse().unwrap_or(0);
            }
            rest = after;
        }

        if stamps.is_empty() {
            continue;
        }

        let text = words(rest);

        for at in stamps {
            if lines.len() < LINES {
                lines.push((at, text.clone()));
            }
        }
    }

    lines.sort_by_key(|(at, _)| *at);

    lines
        .into_iter()
        .map(|(at, text)| Line {
            // a positive offset shows every line earlier
            at: Duration::from_millis(u64::try_from(at.saturating_sub(offset).max(0)).unwrap_or(0)),
            text,
        })
        .collect()
}

// whether `letter` is an invisible format character (Unicode Cf): zero-width, joiners, direction
// marks and the like, which could reorder or hide what a line says
fn is_format(letter: char) -> bool {
    matches!(letter,
        '\u{ad}' | '\u{600}'..='\u{605}' | '\u{61c}' | '\u{6dd}' | '\u{70f}'
        | '\u{890}'..='\u{891}' | '\u{8e2}' | '\u{180e}' | '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}'
        | '\u{feff}' | '\u{fff9}'..='\u{fffb}' | '\u{110bd}' | '\u{110cd}'
        | '\u{13430}'..='\u{1343f}' | '\u{1bca0}'..='\u{1bca3}' | '\u{1d173}'..='\u{1d17a}'
        | '\u{e0001}' | '\u{e0020}'..='\u{e007f}')
}

/*
 * the text of a line without its word-by-word stamps, like `<00:12.30>`, without control or format
 * characters, which a server's answer could otherwise use to reorder or hide it, and cut to a sane
 * length
 */
fn words(text: &str) -> String {
    let mut plain = String::new();
    let mut rest = text;

    while let Some((before, after)) = rest.split_once('<') {
        match after.split_once('>') {
            Some((tag, after)) if stamp(tag).is_some() => {
                plain.push_str(before);
                rest = after;
            }
            _ => {
                plain.push_str(before);
                plain.push('<');
                rest = after;
            }
        }
    }
    plain.push_str(rest);

    plain
        .trim()
        .chars()
        .filter(|letter| !letter.is_control() && !is_format(*letter))
        .take(LINE_CHARS)
        .collect()
}

// `mm:ss`, `mm:ss.x`, `mm:ss.xx` or `mm:ss.xxx` (also `:` before the fraction), in milliseconds
fn stamp(tag: &str) -> Option<i64> {
    let (minutes, rest) = tag.split_once(':')?;
    let (seconds, fraction) = match rest.split_once(['.', ':']) {
        Some((seconds, fraction)) => (seconds, fraction),
        None => (rest, ""),
    };

    let digits = |text: &str, most: usize| {
        (!text.is_empty() && text.len() <= most && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse::<i64>().ok())
            .flatten()
    };

    let minutes = digits(minutes, 3)?;
    let seconds = digits(seconds, 2)?;
    let millis = if fraction.is_empty() {
        0
    } else {
        let value = digits(fraction, 3)?;

        value * 10i64.pow(3 - fraction.len() as u32)
    };

    Some((minutes * 60 + seconds) * 1000 + millis)
}

// the `.lrc` named like a local track and beside it, if there is one
fn beside(url: &str) -> Option<String> {
    let path = media::file_path(url)?;
    let path = Path::new(&path).with_extension("lrc");

    read_file(&path)
}

// a regular file of sane size, as text
fn read_file(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;

    if !meta.is_file() || meta.len() > BODY {
        return None;
    }

    fs::read_to_string(path).ok()
}

// the outcome of a 429, which holds every track back, not only the one asked
const RATE_LIMITED: &str = "rate limited";

// what asking LRCLIB came to
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Found(String),

    // no synced lyrics for the track, to be believed for `NEGATIVE`
    Missing,

    // nothing learned: try again after this long at least
    Later(Duration, &'static str),
}

// one request, as LRCLIB's docs describe `/api/get`
fn fetch(agent: &Agent, endpoint: &str, wanted: &Wanted) -> Answer {
    let seconds = wanted.length.as_secs_f64().round() as u64;

    let response = agent
        .get(endpoint)
        .query("track_name", &wanted.title)
        .query("artist_name", &wanted.artist)
        .query("album_name", &wanted.album)
        .query("duration", seconds.to_string())
        .header("Accept", "application/json")
        .call();

    let mut response = match response {
        Ok(response) => response,
        Err(_) => return Answer::Later(Duration::ZERO, "offline"),
    };

    match response.status().as_u16() {
        200 => {}
        // not there, or not a question it takes: the same question gets the same answer
        400 | 404 => return Answer::Missing,
        429 => {
            let after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
                .map_or(Duration::ZERO, Duration::from_secs);

            return Answer::Later(after.min(RETRY_AFTER), RATE_LIMITED);
        }
        _ => return Answer::Later(Duration::ZERO, "server error"),
    }

    let Ok(body) = body::text(response.body_mut(), BODY) else {
        return Answer::Later(Duration::ZERO, "bad answer");
    };

    // not JSON is a captive portal or a broken server, not a track without lyrics
    let Ok(answer) = serde_json::from_str::<serde_json::Value>(&body) else {
        return Answer::Later(Duration::ZERO, "bad answer");
    };

    match answer
        .get("syncedLyrics")
        .and_then(|synced| synced.as_str())
    {
        Some(text) if !parse(text).is_empty() => Answer::Found(text.to_owned()),
        _ => Answer::Missing,
    }
}

// an answer kept in the cache
#[derive(Debug, PartialEq, Eq)]
enum Cached {
    Found(String),
    Missing,
}

struct Cache {
    dir: PathBuf,
}

impl Cache {
    fn read(&self, name: &str) -> Option<Cached> {
        if let Some(text) = read_file(&self.dir.join(format!("{name}.lrc"))) {
            return Some(Cached::Found(text));
        }

        let modified = fs::metadata(self.dir.join(format!("{name}.none")))
            .ok()?
            .modified()
            .ok()?;

        // a miss from the future, by a clock that was wrong, is not believed
        let age = SystemTime::now().duration_since(modified).ok()?;

        (age < NEGATIVE).then_some(Cached::Missing)
    }

    fn write(&self, name: &str, answer: &Cached) {
        let (file, other, bytes) = match answer {
            Cached::Found(text) => ("lrc", "none", text.as_bytes()),
            Cached::Missing => ("none", "lrc", &[][..]),
        };

        let written = atomic::write(
            &self.dir.join(format!("{name}.{file}")),
            bytes,
            Mode::Private,
        );

        match written {
            Ok(()) => {
                drop(fs::remove_file(self.dir.join(format!("{name}.{other}"))));
                self.prune();
            }
            Err(error) => eprintln!("kanade: lyrics: cannot write the cache: {}", error.kind()),
        }
    }

    // expired misses go, then the oldest answers past `CACHE_FILES`; a `.part` a killed write left
    // between staging and renaming goes too, by the same rule every write of the cache sweeps with
    fn prune(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };

        let mut kept = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();

            if let Some(target) = staged(&path) {
                atomic::sweep(&target);
                continue;
            }

            let extension = path.extension().and_then(|extension| extension.to_str());

            if !matches!(extension, Some("lrc" | "none")) {
                continue;
            }

            let modified = entry.metadata().and_then(|meta| meta.modified()).ok();
            let age = modified.and_then(|modified| SystemTime::now().duration_since(modified).ok());

            if extension == Some("none") && age.is_none_or(|age| age >= NEGATIVE) {
                drop(fs::remove_file(&path));
            } else {
                kept.push((modified.unwrap_or(SystemTime::UNIX_EPOCH), path));
            }
        }

        if kept.len() > CACHE_FILES {
            kept.sort();

            for (_, path) in kept.drain(..kept.len() - CACHE_FILES) {
                drop(fs::remove_file(path));
            }
        }
    }
}

/*
 * the file a staged write in the cache is for: the `.{name}.{pid}.{n}.part` a killed write leaves
 * between staging and renaming, as `atomic` names it, gives `{name}`. None for anything else
 */
fn staged(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix('.')?.strip_suffix(".part")?;
    let (rest, number) = rest.rsplit_once('.')?;
    let (target, pid) = rest.rsplit_once('.')?;

    let numbered = number.parse::<u64>().is_ok() && pid.parse::<u32>().is_ok();
    let named = !target.is_empty() && !target.starts_with('.');

    (numbered && named).then(|| path.with_file_name(target))
}

struct Worker {
    agent: Agent,
    endpoint: String,
    cache: Option<Cache>,

    // the track asked for last, and whether it plays
    current: Option<Wanted>,
    playing: bool,

    // its lyrics are in `CURRENT`, or known not to exist
    resolved: bool,

    // when to try a failed look-up again
    retry: Option<Instant>,

    // the failed track and when to ask for it again, kept past a Stop, so closing the Surface or
    // a skip back does not skip a `Retry-After`
    penalty: Option<(String, Instant)>,

    // when LRCLIB may be asked again after it limited us: it is the server's, so every track waits
    cooldown: Option<Instant>,
    backoff: Backoff,
    last_request: Option<Instant>,
}

impl Worker {
    fn new(agent: Agent, endpoint: &str, dir: Option<PathBuf>) -> Self {
        Self {
            agent,
            endpoint: endpoint.to_owned(),
            cache: dir.map(|dir| Cache { dir }),
            current: None,
            playing: false,
            resolved: false,
            retry: None,
            penalty: None,
            cooldown: None,
            backoff: Backoff::new(RETRY, RETRY_LONGEST),
            last_request: None,
        }
    }

    fn apply(&mut self, message: Message) {
        match message {
            Message::Want(wanted) => {
                if self.current.as_ref() != Some(&*wanted) {
                    self.retry = self
                        .penalty
                        .as_ref()
                        .filter(|(key, until)| {
                            *key == wanted.key
                                && wanted.mode == LyricsSource::Lrclib
                                && *until > Instant::now()
                        })
                        .map(|(_, until)| *until);
                    self.current = Some(*wanted);
                    self.resolved = false;
                    self.backoff.reset();
                }

                self.playing = true;
            }
            Message::Pause => self.playing = false,
            Message::Stop => {
                self.current = None;
                self.playing = false;
                self.resolved = false;
                self.retry = None;
                self.backoff.reset();
                *CURRENT.locked() = None;
            }
        }
    }

    fn due(&self) -> bool {
        self.playing && !self.resolved && self.retry.is_none() && self.current.is_some()
    }

    // finds the lyrics of the current track and hands them to the follower
    fn resolve(&mut self) {
        let Some(wanted) = self.current.clone() else {
            return;
        };

        let (found, outcome) = match self.look_up(&wanted) {
            Ok(found) => found,
            Err((wait, outcome)) => {
                let now = Instant::now();

                // held back by the cooldown, which is no failure of this track's
                let held = self.cooldown.is_some_and(|until| until > now);
                let until = now
                    + if held {
                        wait
                    } else {
                        wait.max(self.backoff.failed())
                    };

                self.retry = Some(until);
                self.penalty = Some((wanted.key, until));

                if outcome == RATE_LIMITED {
                    self.cooldown = Some(until);
                }

                *OUTCOME.locked() = outcome;
                return;
            }
        };

        self.backoff.reset();

        if self
            .penalty
            .as_ref()
            .is_some_and(|(key, _)| *key == wanted.key)
        {
            self.penalty = None;
        }

        self.resolved = true;
        *OUTCOME.locked() = outcome;

        *CURRENT.locked() = Some(Loaded {
            key: wanted.key,
            lines: found.into(),
        });
        media::lyrics_ready();
    }

    // the lines, or how long to wait before asking again and why
    fn look_up(
        &mut self,
        wanted: &Wanted,
    ) -> Result<(Vec<Line>, &'static str), (Duration, &'static str)> {
        let beside = beside(&wanted.url).map(|text| parse(&text));

        if let Some(lines) = beside.filter(|lines| !lines.is_empty()) {
            return Ok((lines, "found beside the track"));
        }

        if wanted.mode != LyricsSource::Lrclib || !wanted.askable() {
            return Ok((Vec::new(), "none found"));
        }

        let name = wanted.cache_name();

        match self.cache.as_ref().and_then(|cache| cache.read(&name)) {
            Some(Cached::Found(text)) => return Ok((parse(&text), "found in the cache")),
            Some(Cached::Missing) => return Ok((Vec::new(), "none found, remembered")),
            None => {}
        }

        // the files and the cache above answer through a rate limit; only a request waits for it
        if let Some(wait) = self
            .cooldown
            .map(|until| until.saturating_duration_since(Instant::now()))
            .filter(|wait| !wait.is_zero())
        {
            return Err((wait, RATE_LIMITED));
        }

        if let Some(last) = self.last_request {
            std::thread::sleep(GAP.saturating_sub(last.elapsed()));
        }
        self.last_request = Some(Instant::now());

        let answer = fetch(&self.agent, &self.endpoint, wanted);

        let cached = match &answer {
            Answer::Found(text) => Some(Cached::Found(text.clone())),
            Answer::Missing => Some(Cached::Missing),
            Answer::Later(..) => None,
        };

        if let (Some(cache), Some(cached)) = (&self.cache, &cached) {
            cache.write(&name, cached);
        }

        match answer {
            Answer::Found(text) => Ok((parse(&text), "found on LRCLIB")),
            Answer::Missing => Ok((Vec::new(), "none found")),
            Answer::Later(wait, outcome) => Err((wait, outcome)),
        }
    }

    // until the queue closes
    fn run(&mut self, queue: &Receiver<Message>) {
        loop {
            let message = match self.retry {
                Some(due) => queue.recv_timeout(due.saturating_duration_since(Instant::now())),
                None => queue.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };

            match message {
                Ok(message) => {
                    self.apply(message);

                    // a skip through many tracks is looked up once, for the last
                    while let Ok(message) = queue.try_recv() {
                        self.apply(message);
                    }
                }
                Err(RecvTimeoutError::Timeout) => self.retry = None,
                Err(RecvTimeoutError::Disconnected) => return,
            }

            if self.due() {
                self.resolve();
            }
        }
    }
}

// looks lyrics up on its own thread for as long as Kanade runs
fn follow() {
    let queue = QUEUE.receiver();

    Worker::new(agent(true), ENDPOINT, directory()).run(&queue);
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    fn wanted(mode: LyricsSource, url: &str) -> Wanted {
        Wanted::new(
            mode,
            url,
            "Song",
            "Artist",
            "Album",
            Duration::from_secs(200),
        )
    }

    fn seconds(line: &Line) -> f64 {
        line.at.as_secs_f64()
    }

    #[test]
    fn stamps_tags_and_offsets_of_an_lrc_file() {
        let lines = parse(
            "\u{feff}[ar:Someone]\r\n[ti:A song]\r\n[offset:+500]\r\n\
             [00:12.34]first\r\n[00:05.5][01:00]twice\r\n[00:20.123]<00:20.12>word <not a time> by word\r\n\
             [00:30.00]\r\nnot a lyric\r\n[bad:stamp]ignored\r\n",
        );

        let seen: Vec<(f64, &str)> = lines
            .iter()
            .map(|line| (seconds(line), line.text.as_str()))
            .collect();

        assert_eq!(
            seen,
            [
                (5.0, "twice"),
                (11.84, "first"),
                (19.623, "word <not a time> by word"),
                (29.5, ""),
                (59.5, "twice"),
            ]
        );
    }

    // a line is drawn as it reads: no control or bidi character reorders or hides it
    #[test]
    fn a_line_keeps_no_control_or_format_character() {
        let line = &parse("[00:01.00]a\u{202e}b\u{1b}[31m c\u{200b}d\n")[0].text;

        assert_eq!(line, "ab[31m cd");
    }

    #[test]
    fn malformed_stamps_and_huge_files_are_bounded() {
        for bad in [
            "[1:2:3:4]x",
            "[aa:bb]x",
            "[00:61.1234]x",
            "[0000:01]x",
            "[00:]x",
            "[:01]x",
        ] {
            assert!(parse(bad).is_empty(), "{bad:?}");
        }

        let long = format!("[00:01.00]{}", "a".repeat(5000));
        assert_eq!(parse(&long)[0].text.chars().count(), LINE_CHARS);

        let many = "[00:01.00]x\n".repeat(LINES * 2);
        assert_eq!(parse(&many).len(), LINES);
    }

    #[test]
    fn the_line_is_the_last_one_passed() {
        let lines = parse("[00:10.00]a\n[00:20.00]\n[00:30.00]c\n");
        let at = |secs: u64| at(&lines, Duration::from_secs(secs));

        assert_eq!(at(5).text, None);
        assert_eq!(at(5).until, Some(Duration::from_secs(5)));
        assert_eq!(at(10).text.as_deref(), Some("a"));
        assert_eq!(at(19).until, Some(Duration::from_secs(1)));
        // a gap shows nothing
        assert_eq!(at(25).text, None);
        assert_eq!(at(31).text.as_deref(), Some("c"));
        assert_eq!(at(31).until, None);
    }

    #[test]
    fn the_cache_name_hides_the_track_and_ignores_the_file() {
        let a = wanted(LyricsSource::Lrclib, "file:///music/a.flac");
        let b = wanted(LyricsSource::Lrclib, "file:///other/a.flac");

        assert_eq!(a.cache_name(), b.cache_name());
        assert_ne!(a.key, b.key);
        assert_eq!(a.cache_name().len(), 32);
        assert!(!a.cache_name().contains("song"));
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanade-lyrics-{name}-{}", std::process::id()));

        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap();

        dir
    }

    #[test]
    fn a_lrc_beside_a_local_track_is_read_and_a_miss_expires() {
        let dir = scratch("beside");
        fs::write(dir.join("my song.lrc"), "[00:01.00]hi\n").unwrap();

        let url = format!("file://{}/my%20song.flac", dir.display());
        assert_eq!(beside(&url).as_deref(), Some("[00:01.00]hi\n"));
        assert_eq!(beside("https://example.com/a.flac"), None);

        let cache = Cache {
            dir: dir.join("cache"),
        };
        assert_eq!(cache.read("n"), None);

        cache.write("n", &Cached::Missing);
        assert_eq!(cache.read("n"), Some(Cached::Missing));

        // a miss older than NEGATIVE is asked again
        let old = SystemTime::now() - NEGATIVE - Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(dir.join("cache/n.none"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(cache.read("n"), None);

        cache.write("n", &Cached::Found(String::from("[00:01.00]x")));
        assert_eq!(
            cache.read("n"),
            Some(Cached::Found(String::from("[00:01.00]x")))
        );
        assert!(!dir.join("cache/n.none").exists());

        drop(fs::remove_dir_all(&dir));
    }

    // a write killed between staging and renaming leaves a hidden part; the next write sweeps it,
    // along with this one, whose process is gone
    #[test]
    fn a_part_of_a_killed_write_does_not_stay_in_the_cache() {
        let dir = scratch("part");
        let cache = Cache { dir: dir.clone() };

        // a child, reaped, so its number is certainly not a live process's
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();

        let gone = dir.join(format!(".n.lrc.{dead}.0.part"));
        let live = dir.join(format!(".n.lrc.{}.0.part", std::process::id()));
        let unrelated = dir.join(".not-a-part");
        for part in [&gone, &live, &unrelated] {
            fs::write(part, b"half a file").unwrap();
        }

        cache.write("n", &Cached::Found(String::from("[00:01.00]x")));

        assert!(!gone.exists(), "the dead process's part is swept");
        assert!(live.exists(), "this process's own part is left alone");
        assert!(unrelated.exists(), "a name that is no part is left alone");

        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn the_cache_keeps_at_most_its_bound_and_drops_the_oldest() {
        let dir = scratch("bound");
        let cache = Cache { dir: dir.clone() };

        for index in 0..CACHE_FILES + 5 {
            let path = dir.join(format!("{index:04}.lrc"));
            fs::write(&path, "[00:01.00]x").unwrap();

            let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + index as u64);
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(at)
                .unwrap();
        }
        fs::write(dir.join("unrelated.txt"), "x").unwrap();

        cache.prune();

        let left = fs::read_dir(&dir).unwrap().count();
        assert_eq!(left, CACHE_FILES + 1);
        assert!(!dir.join("0000.lrc").exists());
        assert!(dir.join("unrelated.txt").exists());

        drop(fs::remove_dir_all(&dir));
    }

    // answers each connection with the next of `answers`, and tells what it was asked
    fn serve(answers: Vec<Vec<u8>>) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/api/get", listener.local_addr().unwrap());

        let asked = thread::spawn(move || {
            let mut asked = Vec::new();

            for answer in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = vec![0; 4096];
                let read = stream.read(&mut request).unwrap();

                asked.push(String::from_utf8_lossy(&request[..read]).into_owned());
                let _ = stream.write_all(&answer);
            }

            asked
        });

        (url, asked)
    }

    fn answer(status: &str, headers: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    #[test]
    fn lrclib_answers_become_lyrics_a_miss_or_a_wait() {
        let (url, asked) = serve(vec![
            answer(
                "200 OK",
                "",
                r#"{"id":1,"instrumental":false,"plainLyrics":"x","syncedLyrics":"[00:01.00]hi"}"#,
            ),
            answer("200 OK", "", r#"{"instrumental":true,"syncedLyrics":null}"#),
            answer("200 OK", "", r#"{"syncedLyrics":"no stamps here"}"#),
            answer("404 Not Found", "", r#"{"statusCode":404}"#),
            answer("429 Too Many Requests", "Retry-After: 90\r\n", ""),
            answer("429 Too Many Requests", "Retry-After: 999999\r\n", ""),
            answer("503 Service Unavailable", "", ""),
            answer("200 OK", "", "not json"),
        ]);
        let agent = agent(false);
        let track = Wanted::new(
            LyricsSource::Lrclib,
            "",
            "Sö ng&x",
            "Artist",
            "Album",
            Duration::from_millis(200_400),
        );

        assert_eq!(
            fetch(&agent, &url, &track),
            Answer::Found(String::from("[00:01.00]hi"))
        );
        assert_eq!(fetch(&agent, &url, &track), Answer::Missing);
        assert_eq!(fetch(&agent, &url, &track), Answer::Missing);
        assert_eq!(fetch(&agent, &url, &track), Answer::Missing);
        assert_eq!(
            fetch(&agent, &url, &track),
            Answer::Later(Duration::from_secs(90), "rate limited")
        );
        assert_eq!(
            fetch(&agent, &url, &track),
            Answer::Later(RETRY_AFTER, "rate limited")
        );
        assert_eq!(
            fetch(&agent, &url, &track),
            Answer::Later(Duration::ZERO, "server error")
        );
        assert_eq!(
            fetch(&agent, &url, &track),
            Answer::Later(Duration::ZERO, "bad answer")
        );

        let asked = asked.join().unwrap();
        let first = asked[0].to_lowercase();

        assert!(first.starts_with(
            "get /api/get?track_name=s%c3%b6%20ng%26x&artist_name=artist&album_name=album&duration=200 "
        ), "{first}");
        assert!(first.contains(concat!(
            "user-agent: kanade/",
            env!("CARGO_PKG_VERSION"),
            "\r\n"
        )));
    }

    #[test]
    fn lyrics_are_asked_for_over_https_only() {
        let track = wanted(LyricsSource::Lrclib, "");

        // the default agent refuses a plain address, redirects included
        assert_eq!(
            fetch(&agent(true), "http://127.0.0.1:9/api/get", &track),
            Answer::Later(Duration::ZERO, "offline")
        );
    }

    #[test]
    fn a_worker_finds_beside_then_cache_then_asks_once() {
        let dir = scratch("worker");
        let (url, asked) = serve(vec![answer(
            "200 OK",
            "",
            r#"{"syncedLyrics":"[00:02.00]net"}"#,
        )]);

        let mut worker = Worker::new(agent(false), &url, Some(dir.join("cache")));
        let track = wanted(LyricsSource::Lrclib, "");

        worker.apply(Message::Want(Box::new(track.clone())));
        assert!(worker.due());
        worker.resolve();

        assert!(worker.resolved);
        assert_eq!(
            line(&track, Duration::from_secs(3)).text.as_deref(),
            Some("net")
        );

        // another run with no server: the cache answers
        let mut again = Worker::new(agent(false), "http://127.0.0.1:9/", Some(dir.join("cache")));
        again.apply(Message::Want(Box::new(track.clone())));
        again.resolve();
        assert!(again.resolved);
        assert_eq!(asked.join().unwrap().len(), 1);

        // `local` never asks
        let local = wanted(LyricsSource::Local, "");
        again.apply(Message::Want(Box::new(local.clone())));
        again.resolve();
        assert!(again.resolved);
        assert_eq!(line(&local, Duration::from_secs(3)).text, None);

        again.apply(Message::Stop);
        assert_eq!(line(&track, Duration::from_secs(3)), Shown::default());

        // a wait is for its track: it outlasts a Stop, and another track's success keeps it
        let other = Wanted::new(
            LyricsSource::Lrclib,
            "",
            "Other",
            "Artist",
            "Album",
            Duration::from_secs(100),
        );
        let until = Instant::now() + Duration::from_secs(60);
        again.penalty = Some((track.key.clone(), until));
        again.apply(Message::Want(Box::new(other)));
        assert_eq!(again.retry, None);
        again.apply(Message::Stop);
        again.apply(Message::Want(Box::new(track.clone())));
        assert_eq!(again.retry, Some(until));
        assert!(!again.due());

        // a 429 is LRCLIB's: another track's request waits for it, but its cache answers at once
        let third = Wanted::new(
            LyricsSource::Lrclib,
            "",
            "Third",
            "Artist",
            "Album",
            Duration::from_secs(150),
        );
        let cooled = Instant::now() + Duration::from_secs(60);
        let mut held = Worker::new(agent(false), "http://127.0.0.1:9/", Some(dir.join("cache")));

        held.cooldown = Some(cooled);
        held.apply(Message::Want(Box::new(third.clone())));
        assert!(held.due());
        held.resolve();
        assert!(!held.resolved);
        assert!(held.retry.is_some_and(|until| until >= cooled));
        assert_eq!(held.backoff.failed(), RETRY);

        held.apply(Message::Want(Box::new(track.clone())));
        held.retry = None;
        held.resolve();
        assert!(held.resolved);

        drop(fs::remove_dir_all(&dir));
    }
}
