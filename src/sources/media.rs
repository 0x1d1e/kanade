//! The Media Activity (plan 3, 6.2), from Amane's Media, which polls MPRIS once a second. One
//! Persistent Activity for the active player: Media Priority, so it never interrupts (plan 7).

use std::thread;
use std::time::{Duration, Instant};

use amane::{Media, MediaPlayer, Service};

use crate::island::activity::{Activity, Detail, Id, Kind, Priority, Track};
use crate::island::service::IslandService;

/*
 * how long a paused player keeps its Activity, so a pause to answer the door does not empty the
 * island. MPRIS tells Amane only whether a player plays, so a stopped one counts as paused
 */
const PAUSED: Duration = Duration::from_secs(30);

// Amane's own poll; reading its Media more often would only see the same thing
const POLL: Duration = Duration::from_secs(1);

// the active player as the island shows it
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    // the player's bus name, stable while it stays open
    key: String,

    track: Track,
}

impl Seen {
    fn of(player: &MediaPlayer) -> Seen {
        // a player with nothing to say about the track still names itself
        let title = match player.title() {
            "" => player.identity(),
            title => title,
        };

        Seen {
            key: player.name().to_owned(),
            track: Track {
                title: title.to_owned(),
                artist: player.artist().to_owned(),
                art: art(player.art_url()),
                playing: player.playing(),
            },
        }
    }

    fn activity(&self) -> Activity {
        Activity::persistent(Id::new(Kind::Media, &self.key), Priority::Media)
            .with_detail(Detail::Media(self.track.clone()))
    }
}

#[derive(Debug, PartialEq)]
enum Change {
    Post(Activity),
    Withdraw(Id),
}

// what was posted, remembered so a poll that sees nothing new changes nothing
#[derive(Debug, Default)]
struct Follower {
    posted: Option<Seen>,

    // when the posted player stopped playing
    paused: Option<Instant>,
}

impl Follower {
    /*
     * a player shows from its first play; paused, it stays for PAUSED, then goes until it plays
     * again. A player that closes, or that another one playing replaces, goes at once
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

        if seen.track.playing {
            self.paused = None;
        } else {
            // a player first seen paused never played here, so it does not show
            if self.posted.is_none() {
                return changes;
            }

            let since = *self.paused.get_or_insert(now);

            if now.duration_since(since) >= PAUSED {
                let posted = self.posted.take().expect("checked above");
                changes.push(Change::Withdraw(posted.activity().id().clone()));
                self.paused = None;
                return changes;
            }
        }

        if self.posted.as_ref() != Some(&seen) {
            changes.push(Change::Post(seen.activity()));
            self.posted = Some(seen);
        }

        changes
    }
}

// runs on its own thread for good, reading what Amane's poll last saw
pub fn follow() {
    let mut follower = Follower::default();

    loop {
        // the guard is let go before the island is written
        let seen = Media::read().active().map(Seen::of);
        let now = Instant::now();

        let changes = follower.step(seen, now);

        if !changes.is_empty() {
            let mut island = IslandService::write();

            for change in changes {
                match change {
                    Change::Post(activity) => island.post(activity, now),
                    Change::Withdraw(id) => island.withdraw(&id, now),
                }
            }
        }

        thread::sleep(POLL);
    }
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
