//! The Media Activity (plan 3, 6.2): one Persistent Activity for the active MPRIS player, Media
//! Priority, so it never interrupts (plan 7).
//!
//! Follows MPRIS signals rather than Amane's Media, whose 1 Hz poll would wake Kanade every
//! second to learn nothing: a player announces every change the island shows, only the position
//! goes unannounced. So nothing here runs until a player changes, or a pause runs out.
//!
//! While the Media Surface is open it also feeds its `Playback`: every player, the one it shows,
//! and that one's position, asked once a second (plan 7). Its controls come back here as
//! `Control`s, so the bus is only ever called from this thread.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use amane::{Argument, Bus, Color, Palette, Service, Signal, Value};

use super::playback::{self, Deck, Playback, Timeline};
use crate::island::activity::{Activity, Detail, Id, Kind, Priority, Track};
use crate::island::presentation::Surface;
use crate::island::service::IslandService;

// how long a paused player keeps its Activity, so a pause to answer the door does not empty the island
const PAUSED: Duration = Duration::from_secs(30);

const PATH: &str = "/org/mpris/MediaPlayer2";

const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

// where a player keeps its own name, like "Spotify"
const ROOT: &str = "org.mpris.MediaPlayer2";

// every player owns a bus name starting with this
const PREFIX: &str = "org.mpris.MediaPlayer2.";

// not a player, it mirrors the active one, so it would show that player twice
const PROXY: &str = "org.mpris.MediaPlayer2.playerctld";

const DBUS: &str = "org.freedesktop.DBus";

const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

// plan 7: the shown player's position is asked this often at least while the Surface is open
const POLL: Duration = Duration::from_secs(1);

/*
 * a tick asks just after the shown second should change, so the player has moved past it too, and
 * never sooner than this after the last, so a player that lags behind is not asked in a burst
 */
const PAST: Duration = Duration::from_millis(20);
const GAP: Duration = Duration::from_millis(200);

// colors read from the artwork, enough for its most vivid to be among them
const COLORS: usize = 8;

// set by the Media Surface when it draws, cleared here once it closed
static WATCHED: AtomicBool = AtomicBool::new(false);

// where the Surface's watch and controls reach the follower, once it runs
static EVENTS: OnceLock<Sender<Event>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq)]
enum Status {
    Playing,
    Paused,

    // no track loaded, or none it means to go on with
    Stopped,
}

// one open player as MPRIS last told it
#[derive(Debug, Clone, PartialEq)]
struct Player {
    // the unique name like :1.42 its signals come from
    owner: String,

    // like "Spotify", read once, as a player keeps it
    identity: String,

    title: String,
    artist: String,
    art_url: String,
    status: Status,

    timeline: Timeline,
    can_play: bool,
    can_previous: bool,
    can_next: bool,
}

impl Player {
    /*
     * from the Player interface's properties, read `at`; anything missing reads as empty, stopped,
     * or unable. MPRIS gives times in microseconds
     */
    fn of(owner: &str, identity: &str, properties: &Value, at: Instant) -> Player {
        let metadata = properties.get("Metadata");
        let micros = |value: &Value| Duration::from_micros(value.number().max(0.0) as u64);
        let able = |property| properties.get(property).bool();

        let status = match properties.get("PlaybackStatus").text() {
            "Playing" => Status::Playing,
            "Paused" => Status::Paused,
            _ => Status::Stopped,
        };

        Player {
            owner: owner.to_owned(),
            identity: identity.to_owned(),
            title: metadata.get("xesam:title").text().to_owned(),

            // MPRIS lists every artist, the island has room for one
            artist: metadata
                .get("xesam:artist")
                .list()
                .first()
                .map_or_else(String::new, |artist| artist.text().to_owned()),

            art_url: metadata.get("mpris:artUrl").text().to_owned(),
            status,

            timeline: Timeline {
                position: micros(properties.get("Position")),
                at,
                length: micros(metadata.get("mpris:length")),

                // a player without Rate plays at normal speed
                rate: match (status, properties.get("Rate")) {
                    (Status::Playing, Value::Number(rate)) => *rate,
                    (Status::Playing, _) => 1.0,
                    _ => 0.0,
                },
            },

            // CanControl false means none of the others hold, whatever they say
            can_play: able("CanControl") && (able("CanPlay") || able("CanPause")),
            can_previous: able("CanControl") && able("CanGoPrevious"),
            can_next: able("CanControl") && able("CanGoNext"),
        }
    }
}

// every open player by bus name, sorted so the choice among them is stable
#[derive(Debug, Default)]
struct Players {
    open: BTreeMap<String, Player>,

    // the bus name shown last
    active: Option<String>,
}

impl Players {
    // the names a signal's sender owns; usually one
    fn owned_by(&self, owner: &str) -> Vec<String> {
        self.open
            .iter()
            .filter(|(_, player)| player.owner == owner)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /*
     * the one playing, or the one shown last, or the first; as Amane's Media chooses, so a paused
     * player stays active and the island does not jump to another one
     */
    fn choose(&mut self) -> Option<Seen> {
        let playing = self
            .open
            .iter()
            .find(|(_, player)| player.status == Status::Playing);

        let kept = || {
            let active = self.active.as_deref()?;
            self.open.get_key_value(active)
        };

        let (name, player) = playing
            .or_else(kept)
            .or_else(|| self.open.first_key_value())?;

        let seen = Seen::of(name, player);
        self.active = Some(name.clone());

        Some(seen)
    }
}

// the active player as the island shows it
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    // the player's bus name, stable while it stays open
    key: String,

    status: Status,
    track: Track,
}

impl Seen {
    fn of(name: &str, player: &Player) -> Seen {
        // a player with nothing to say about the track still names itself
        let title = match player.title.as_str() {
            "" => &player.identity,
            title => title,
        };

        Seen {
            key: name.to_owned(),
            status: player.status,
            track: Track {
                title: title.to_owned(),
                artist: player.artist.clone(),
                art: art(&player.art_url),
                playing: player.status == Status::Playing,
            },
        }
    }

    fn activity(&self) -> Activity {
        Activity::persistent(Id::new(Kind::Media, &self.key), Priority::Media)
            .with_detail(Detail::Media(self.track.clone()))
    }
}

// what the bus says, as far as the island cares
#[derive(Debug, PartialEq)]
enum Event {
    // a player opened, or closed when it has no owner
    Owner { name: String, owner: Option<String> },

    // some owner's properties changed, which ones is read back in full
    Changed { owner: String },

    // the Media Surface opened, so what it shows is wanted now
    Watch,

    // pressed on the Media Surface
    Control(Control),
}

// what the Media Surface asks of a player, by its bus name
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Toggle(String),
    Previous(String),
    Next(String),

    // show this player until the Surface closes; the Activity keeps following the playing one
    Select(String),
}

impl Event {
    // NameOwnerChanged: name, old owner, new owner, empty for none
    fn owner(arguments: &[Value]) -> Option<Event> {
        let [name, _, owner] = arguments else {
            return None;
        };

        let name = name.text();
        let owner = owner.text();

        is_player(name).then(|| Event::Owner {
            name: name.to_owned(),
            owner: (!owner.is_empty()).then(|| owner.to_owned()),
        })
    }

    /*
     * PropertiesChanged: interface, changed, invalidated. Only the Player interface on the MPRIS
     * path counts; the rest of the session bus says nothing about media
     */
    fn changed(sender: &str, path: &str, arguments: &[Value]) -> Option<Event> {
        let interface = arguments.first().map(Value::text);

        (path == PATH && interface == Some(PLAYER)).then(|| Event::Changed {
            owner: sender.to_owned(),
        })
    }

    // Seeked: the one jump in position a player announces, read back like any change
    fn seeked(sender: &str, path: &str) -> Option<Event> {
        (path == PATH).then(|| Event::Changed {
            owner: sender.to_owned(),
        })
    }
}

fn is_player(name: &str) -> bool {
    name.starts_with(PREFIX) && name != PROXY
}

#[derive(Debug, PartialEq)]
enum Change {
    Post(Activity),
    Withdraw(Id),
}

// what was posted, remembered so an event that changes nothing shown posts nothing
#[derive(Debug, Default)]
struct Follower {
    posted: Option<Seen>,

    // when the posted player paused
    paused: Option<Instant>,
}

impl Follower {
    /*
     * a player shows from its first play; paused, it stays for PAUSED, then goes until it plays
     * again. A player that stops, closes, or that another one playing replaces, goes at once
     */
    fn step(&mut self, seen: Option<Seen>, now: Instant) -> Vec<Change> {
        let mut changes = Vec::new();

        let same = |posted: &Seen| seen.as_ref().is_some_and(|seen| seen.key == posted.key);

        if let Some(posted) = self.posted.take_if(|posted| !same(posted)) {
            changes.push(Change::Withdraw(posted.activity().id().clone()));
            self.paused = None;
        }

        let Some(seen) = seen else {
            return changes;
        };

        match seen.status {
            Status::Playing => self.paused = None,

            // a player first seen paused never played here, so it does not show
            Status::Paused if self.posted.is_none() => return changes,

            Status::Paused => {
                let since = *self.paused.get_or_insert(now);

                if now.duration_since(since) >= PAUSED {
                    return self.withdraw(changes);
                }
            }

            Status::Stopped => return self.withdraw(changes),
        }

        if self.posted.as_ref() != Some(&seen) {
            changes.push(Change::Post(seen.activity()));
            self.posted = Some(seen);
        }

        changes
    }

    // when a paused Activity runs out, the one moment step() has to run without an event
    fn deadline(&self) -> Option<Instant> {
        self.paused.map(|since| since + PAUSED)
    }

    fn withdraw(&mut self, mut changes: Vec<Change>) -> Vec<Change> {
        if let Some(posted) = self.posted.take() {
            changes.push(Change::Withdraw(posted.activity().id().clone()));
        }

        self.paused = None;
        changes
    }
}

// the Media Surface's side of the follower, idle while the Surface is closed
#[derive(Debug, Default)]
struct Watcher {
    // the player chosen on the Surface, until it closes
    selected: Option<String>,

    // the artwork last read, and the accent it gave
    accent: Option<(String, Option<Color>)>,

    // what Playback holds, so a tick that changes nothing does not write
    published: Option<Playback>,

    // when to ask the shown player where it is next
    tick: Option<Instant>,
}

impl Watcher {
    // the player chosen on the Surface while it is open, else the one the Activity follows
    fn shown<'a>(&self, players: &'a Players) -> Option<(&'a String, &'a Player)> {
        let open = |name: &Option<String>| players.open.get_key_value(name.as_deref()?);

        open(&self.selected)
            .or_else(|| open(&players.active))
            .or_else(|| players.open.first_key_value())
    }

    // writes what the open Surface shows when it changed, and times the next tick
    fn show(&mut self, players: &Players, now: Instant) {
        if !watching() {
            self.selected = None;
            self.tick = None;
            return;
        }

        let deck = self.shown(players).map(|(name, player)| {
            let seen = Seen::of(name, player);
            let accent = seen.track.art.as_deref().and_then(|art| self.accent(art));

            Deck {
                name: name.clone(),
                track: seen.track,
                timeline: player.timeline,
                accent,
                can_play: player.can_play,
                can_previous: player.can_previous,
                can_next: player.can_next,
            }
        });

        // a paused player is still asked, it may be moved without playing
        self.tick = deck.as_ref().map(|deck| {
            deck.timeline.next_second(now).map_or(now + POLL, |next| {
                (next + PAST).clamp(now + GAP, now + POLL)
            })
        });

        let playback = Playback {
            players: playback::choices(
                players
                    .open
                    .iter()
                    .map(|(name, player)| (name.clone(), player.identity.clone())),
            ),
            shown: deck,
        };

        if self.published.as_ref() != Some(&playback) {
            *Playback::write() = playback.clone();
            self.published = Some(playback);
        }
    }

    // read once per artwork; a file not there yet is read again next time
    fn accent(&mut self, art: &str) -> Option<Color> {
        if let Some((read, accent)) = &self.accent
            && read == art
        {
            return *accent;
        }

        let mut palette = Palette::default();
        palette.open(art, COLORS);

        let accent = playback::accent(palette.colors());

        if !palette.colors().is_empty() {
            self.accent = Some((art.to_owned(), accent));
        }

        accent
    }

    fn due(&self, now: Instant) -> bool {
        self.tick.is_some_and(|tick| tick <= now)
    }
}

/*
 * the Media Surface calls this as it draws, so the follower feeds it from then until it closes;
 * only the first call after a close wakes the follower
 */
pub fn watch() {
    if !WATCHED.swap(true, Ordering::Relaxed) {
        send(Event::Watch);
    }
}

// a press on the Media Surface; the bus call happens on the follower, never in the view
pub fn control(control: Control) {
    send(Event::Control(control));
}

fn send(event: Event) {
    if let Some(events) = EVENTS.get() {
        let _ = events.send(event);
    }
}

// watched, and still open; a closed Surface stops being watched until it draws again
fn watching() -> bool {
    if !WATCHED.load(Ordering::Relaxed) {
        return false;
    }

    let open = IslandService::read().surface() == Some(Surface::Media);

    if !open {
        WATCHED.store(false, Ordering::Relaxed);
    }

    open
}

/*
 * runs on its own thread for good, blocked until the bus has news or a pause runs out. Without a
 * session bus it ends at once, and Media never shows
 */
pub fn follow() {
    let bus = Bus::session();

    // watched before the first look, so a player opening in between is not missed
    let (send, events) = mpsc::channel();

    forward(
        bus.signals(DBUS, "NameOwnerChanged"),
        send.clone(),
        |signal| Event::owner(signal.arguments()),
    );
    forward(
        bus.signals(PROPERTIES, "PropertiesChanged"),
        send.clone(),
        |signal| Event::changed(signal.sender(), signal.path(), signal.arguments()),
    );
    forward(bus.signals(PLAYER, "Seeked"), send.clone(), |signal| {
        Event::seeked(signal.sender(), signal.path())
    });

    let _ = EVENTS.set(send);

    let mut players = Players::default();

    for name in names(bus) {
        if let Some(owner) = owner(bus, &name) {
            players.open.insert(name.clone(), read(bus, &name, &owner));
        }
    }

    let mut follower = Follower::default();
    let mut watcher = Watcher::default();

    loop {
        let now = Instant::now();

        post(follower.step(players.choose(), now), now);
        watcher.show(&players, now);

        let deadline = [follower.deadline(), watcher.tick]
            .into_iter()
            .flatten()
            .min();

        let event = match deadline {
            Some(deadline) => events.recv_timeout(deadline.saturating_duration_since(now)),
            None => events.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };

        match event {
            Ok(Event::Owner { name, owner: None }) => {
                players.open.remove(&name);
            }
            Ok(Event::Owner {
                name,
                owner: Some(owner),
            }) => {
                let player = read(bus, &name, &owner);
                players.open.insert(name, player);
            }
            Ok(Event::Changed { owner }) => {
                for name in players.owned_by(&owner) {
                    refresh(bus, &mut players, &name);
                }
            }
            Ok(Event::Watch) => {}
            Ok(Event::Control(control)) => {
                let (method, name) = match control {
                    Control::Toggle(name) => ("PlayPause", name),
                    Control::Previous(name) => ("Previous", name),
                    Control::Next(name) => ("Next", name),
                    Control::Select(name) => {
                        watcher.selected = Some(name);
                        continue;
                    }
                };

                // the player announces what changed, read back here too for an answer this frame
                bus.call(&name, PATH, PLAYER, method, &[]);
                refresh(bus, &mut players, &name);
            }
            Err(RecvTimeoutError::Timeout) => {
                if watcher.due(Instant::now())
                    && let Some((name, _)) = watcher.shown(&players)
                {
                    let name = name.clone();
                    refresh(bus, &mut players, &name);
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                eprintln!("kanade: session bus unreachable, no Media Activity");
                post(follower.step(None, now), now);
                return;
            }
        }
    }
}

// each signal stream blocks, so each gets a thread that hands what counts to the follower
fn forward(
    signals: impl Iterator<Item = Signal> + Send + 'static,
    events: Sender<Event>,
    event: impl Fn(&Signal) -> Option<Event> + Send + 'static,
) {
    thread::spawn(move || {
        for event in signals.filter_map(|signal| event(&signal)) {
            if events.send(event).is_err() {
                return;
            }
        }
    });
}

fn post(changes: Vec<Change>, now: Instant) {
    if changes.is_empty() {
        return;
    }

    let mut island = IslandService::write();

    for change in changes {
        match change {
            Change::Post(activity) => island.post(activity, now),
            Change::Withdraw(id) => island.withdraw(&id, now),
        }
    }
}

// every player open now
fn names(bus: Bus) -> Vec<String> {
    let names = bus.call(DBUS, "/org/freedesktop/DBus", DBUS, "ListNames", &[]);

    names
        .list()
        .iter()
        .map(Value::text)
        .filter(|name| is_player(name))
        .map(String::from)
        .collect()
}

// none when the player closed before it was asked
fn owner(bus: Bus, name: &str) -> Option<String> {
    let owner = bus.call(
        DBUS,
        "/org/freedesktop/DBus",
        DBUS,
        "GetNameOwner",
        &[Argument::from(name)],
    );

    Some(owner.text().to_owned()).filter(|owner| !owner.is_empty())
}

fn read(bus: Bus, name: &str, owner: &str) -> Player {
    let identity = get_all(bus, name, ROOT);

    Player::of(
        owner,
        identity.get("Identity").text(),
        &get_all(bus, name, PLAYER),
        Instant::now(),
    )
}

// an open player read again, keeping who it is
fn refresh(bus: Bus, players: &mut Players, name: &str) {
    let Some(player) = players.open.get(name) else {
        return;
    };

    let properties = get_all(bus, name, PLAYER);
    let player = Player::of(&player.owner, &player.identity, &properties, Instant::now());

    players.open.insert(name.to_owned(), player);
}

// every property of an interface in one call, as a map
fn get_all(bus: Bus, name: &str, interface: &str) -> Value {
    bus.call(
        name,
        PATH,
        PROPERTIES,
        "GetAll",
        &[Argument::from(interface)],
    )
}

// only a local file can be drawn; a web link, like Spotify's, falls back to no art
fn art(url: &str) -> Option<String> {
    let path = url.strip_prefix("file://")?;

    // a file:// link may carry a host, which can only be this machine
    let path = path.strip_prefix("localhost").unwrap_or(path);

    path.starts_with('/').then(|| unescape(path))?
}

// a link escapes bytes as %XX; one that does not decode to UTF-8 is not a path Amane can open
fn unescape(path: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(path.len());
    let mut rest = path.as_bytes();

    while let Some((&byte, after)) = rest.split_first() {
        let hex = after
            .get(..2)
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());

        match (byte, hex) {
            (b'%', Some(decoded)) => {
                bytes.push(decoded);
                rest = &after[2..];
            }
            _ => {
                bytes.push(byte);
                rest = after;
            }
        }
    }

    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(key: &str, title: &str, playing: bool) -> Seen {
        Seen {
            key: key.to_owned(),
            status: if playing {
                Status::Playing
            } else {
                Status::Paused
            },
            track: Track {
                title: title.to_owned(),
                playing,
                ..Track::default()
            },
        }
    }

    fn id(key: &str) -> Id {
        Id::new(Kind::Media, key)
    }

    fn secs(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    #[test]
    fn playing_posts_once() {
        let mut follower = Follower::default();
        let now = Instant::now();
        let mpv = seen("mpv", "Song", true);

        assert_eq!(
            follower.step(Some(mpv.clone()), now),
            [Change::Post(mpv.activity())]
        );

        // the position moving is not a change the island shows
        assert_eq!(follower.step(Some(mpv.clone()), now + secs(1)), []);
    }

    #[test]
    fn media_never_interrupts() {
        let activity = seen("mpv", "Song", true).activity();

        assert_eq!(
            activity.interrupt(),
            crate::island::activity::Interrupt::Never
        );
        assert_eq!(activity.priority(), Priority::Media);
    }

    #[test]
    fn a_new_track_reposts() {
        let mut follower = Follower::default();
        let now = Instant::now();

        follower.step(Some(seen("mpv", "One", true)), now);

        let two = seen("mpv", "Two", true);

        assert_eq!(
            follower.step(Some(two.clone()), now + secs(1)),
            [Change::Post(two.activity())]
        );
    }

    #[test]
    fn a_pause_shows_then_withdraws_after_the_grace() {
        let mut follower = Follower::default();
        let now = Instant::now();
        let paused = seen("mpv", "Song", false);

        follower.step(Some(seen("mpv", "Song", true)), now);

        assert_eq!(
            follower.step(Some(paused.clone()), now + secs(1)),
            [Change::Post(paused.activity())]
        );
        assert_eq!(
            follower.step(Some(paused.clone()), now + secs(1) + PAUSED - secs(1)),
            []
        );
        assert_eq!(
            follower.step(Some(paused.clone()), now + secs(1) + PAUSED),
            [Change::Withdraw(id("mpv"))]
        );

        // withdrawn for good until it plays again
        assert_eq!(follower.step(Some(paused), now + secs(2) + PAUSED), []);

        let playing = seen("mpv", "Song", true);

        assert_eq!(
            follower.step(Some(playing.clone()), now + secs(3) + PAUSED),
            [Change::Post(playing.activity())]
        );
    }

    #[test]
    fn playing_again_restarts_the_grace() {
        let mut follower = Follower::default();
        let now = Instant::now();

        follower.step(Some(seen("mpv", "Song", true)), now);
        follower.step(Some(seen("mpv", "Song", false)), now + secs(1));
        follower.step(Some(seen("mpv", "Song", true)), now + secs(20));
        follower.step(Some(seen("mpv", "Song", false)), now + secs(21));

        // 50s after the first pause, but only 30 into the second
        assert_eq!(
            follower.step(Some(seen("mpv", "Song", false)), now + secs(50)),
            []
        );
        assert_eq!(
            follower.step(Some(seen("mpv", "Song", false)), now + secs(51)),
            [Change::Withdraw(id("mpv"))]
        );
    }

    #[test]
    fn a_player_first_seen_paused_does_not_show() {
        let mut follower = Follower::default();

        assert_eq!(
            follower.step(Some(seen("mpv", "Song", false)), Instant::now()),
            []
        );
    }

    #[test]
    fn a_closed_player_withdraws_at_once() {
        let mut follower = Follower::default();
        let now = Instant::now();

        follower.step(Some(seen("mpv", "Song", true)), now);

        assert_eq!(
            follower.step(None, now + secs(1)),
            [Change::Withdraw(id("mpv"))]
        );
        assert_eq!(follower.step(None, now + secs(2)), []);
    }

    #[test]
    fn another_player_replaces_the_first() {
        let mut follower = Follower::default();
        let now = Instant::now();
        let spotify = seen("spotify", "Other", true);

        follower.step(Some(seen("mpv", "Song", true)), now);

        assert_eq!(
            follower.step(Some(spotify.clone()), now + secs(1)),
            [
                Change::Withdraw(id("mpv")),
                Change::Post(spotify.activity())
            ]
        );

        // the first closing leaves a paused one active, which never played here
        assert_eq!(
            follower.step(Some(seen("vlc", "Paused", false)), now + secs(2)),
            [Change::Withdraw(id("spotify"))]
        );
    }

    fn stopped(key: &str) -> Seen {
        Seen {
            status: Status::Stopped,
            ..seen(key, "Song", false)
        }
    }

    fn text(text: &str) -> Value {
        Value::Text(text.to_owned())
    }

    fn map<const N: usize>(entries: [(&str, Value); N]) -> Value {
        Value::Map(entries.map(|(key, value)| (key.to_owned(), value)).into())
    }

    fn player(owner: &str, title: &str, status: &str) -> Player {
        let properties = map([
            ("PlaybackStatus", text(status)),
            ("Metadata", map([("xesam:title", text(title))])),
        ]);

        Player::of(owner, "mpv", &properties, Instant::now())
    }

    #[test]
    fn a_stop_withdraws_at_once() {
        let mut follower = Follower::default();
        let now = Instant::now();

        follower.step(Some(seen("mpv", "Song", true)), now);

        assert_eq!(
            follower.step(Some(stopped("mpv")), now + secs(1)),
            [Change::Withdraw(id("mpv"))]
        );
        assert_eq!(follower.step(Some(stopped("mpv")), now + secs(2)), []);
        assert_eq!(follower.deadline(), None);
    }

    #[test]
    fn only_a_pause_waits_for_time() {
        let mut follower = Follower::default();
        let now = Instant::now();

        follower.step(Some(seen("mpv", "Song", true)), now);
        assert_eq!(follower.deadline(), None);

        follower.step(Some(seen("mpv", "Song", false)), now + secs(1));
        assert_eq!(follower.deadline(), Some(now + secs(1) + PAUSED));

        follower.step(Some(seen("mpv", "Song", false)), now + secs(1) + PAUSED);
        assert_eq!(follower.deadline(), None);
    }

    #[test]
    fn a_player_reads_from_its_properties() {
        let properties = map([
            ("PlaybackStatus", text("Playing")),
            (
                "Metadata",
                map([
                    ("xesam:title", text("Song")),
                    ("xesam:artist", Value::List(vec![text("One"), text("Two")])),
                    ("mpris:artUrl", text("file:///tmp/a.png")),
                ]),
            ),
        ]);

        let seen = Seen::of(
            "org.mpris.MediaPlayer2.mpv",
            &Player::of(":1.4", "mpv", &properties, Instant::now()),
        );

        assert_eq!(seen.status, Status::Playing);
        assert_eq!(
            seen.track,
            Track {
                title: String::from("Song"),
                artist: String::from("One"),
                art: Some(String::from("/tmp/a.png")),
                playing: true,
            }
        );
    }

    #[test]
    fn a_player_with_nothing_to_say_names_itself() {
        let seen = Seen::of(
            "org.mpris.MediaPlayer2.mpv",
            &Player::of(":1.4", "mpv", &Value::Nothing, Instant::now()),
        );

        assert_eq!(seen.status, Status::Stopped);
        assert_eq!(seen.track.title, "mpv");
        assert_eq!(seen.track.artist, "");
        assert_eq!(seen.track.art, None);
    }

    #[test]
    fn the_playing_player_is_active_else_the_one_shown() {
        let mut players = Players::default();

        players
            .open
            .insert(String::from("a"), player(":1.1", "A", "Paused"));
        players
            .open
            .insert(String::from("b"), player(":1.2", "B", "Playing"));
        assert_eq!(players.choose().unwrap().key, "b");

        // b pausing does not hand the island to a
        players
            .open
            .insert(String::from("b"), player(":1.2", "B", "Paused"));
        assert_eq!(players.choose().unwrap().key, "b");

        players
            .open
            .insert(String::from("a"), player(":1.1", "A", "Playing"));
        assert_eq!(players.choose().unwrap().key, "a");

        players.open.clear();
        assert_eq!(players.choose(), None);
    }

    #[test]
    fn the_first_player_is_active_when_none_plays() {
        let mut players = Players::default();

        players
            .open
            .insert(String::from("b"), player(":1.2", "B", "Paused"));
        players
            .open
            .insert(String::from("a"), player(":1.1", "A", "Stopped"));

        assert_eq!(players.choose().unwrap().key, "a");
        assert_eq!(players.owned_by(":1.2"), ["b"]);
        assert!(players.owned_by(":1.9").is_empty());
    }

    #[test]
    fn players_open_and_close_by_name() {
        let opened = [text("org.mpris.MediaPlayer2.mpv"), text(""), text(":1.4")];
        let closed = [text("org.mpris.MediaPlayer2.mpv"), text(":1.4"), text("")];

        assert_eq!(
            Event::owner(&opened),
            Some(Event::Owner {
                name: String::from("org.mpris.MediaPlayer2.mpv"),
                owner: Some(String::from(":1.4")),
            })
        );
        assert_eq!(
            Event::owner(&closed),
            Some(Event::Owner {
                name: String::from("org.mpris.MediaPlayer2.mpv"),
                owner: None,
            })
        );

        // not players
        assert_eq!(
            Event::owner(&[
                text("org.mpris.MediaPlayer2.playerctld"),
                text(""),
                text(":1.5")
            ]),
            None
        );
        assert_eq!(
            Event::owner(&[
                text("org.freedesktop.Notifications"),
                text(""),
                text(":1.6")
            ]),
            None
        );
        assert_eq!(Event::owner(&[]), None);
    }

    #[test]
    fn only_player_properties_count() {
        let changed = [text(PLAYER), map([]), Value::List(Vec::new())];

        assert_eq!(
            Event::changed(":1.4", PATH, &changed),
            Some(Event::Changed {
                owner: String::from(":1.4")
            })
        );
        assert_eq!(Event::changed(":1.4", "/org/other", &changed), None);
        assert_eq!(Event::changed(":1.4", PATH, &[text(ROOT)]), None);
        assert_eq!(Event::changed(":1.4", PATH, &[]), None);
    }

    #[test]
    fn only_local_art_draws() {
        assert_eq!(
            art("file:///tmp/cover%20art.jpg").as_deref(),
            Some("/tmp/cover art.jpg")
        );
        assert_eq!(
            art("file://localhost/tmp/a.png").as_deref(),
            Some("/tmp/a.png")
        );
        assert_eq!(art("https://i.scdn.co/image/ab67"), None);
        assert_eq!(art(""), None);
        assert_eq!(art("file://host/tmp/a.png"), None);

        // a stray % is kept, an escape that is not UTF-8 is no path
        assert_eq!(
            art("file:///tmp/100%.png").as_deref(),
            Some("/tmp/100%.png")
        );
        assert_eq!(art("file:///tmp/%FF.png"), None);
    }
}
