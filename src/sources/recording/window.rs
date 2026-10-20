//! A window as the thing recorded (ADR 0117): the ScreenCast session niri gives for it, the PipeWire
//! node it casts on, and the sessions Kanade must not leave behind. niri takes no prompt for a
//! window, and keeps a session until it is stopped, even when the program that made it is gone, so
//! every session Kanade makes is written down (the ledger, a file of its own) until it is confirmed
//! gone, and one a dead Kanade left is stopped by the next, and only such a one: a live Kanade's
//! sessions are not left to it.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::MutexExt;

use crate::atomic::{self, Mode};
use crate::bus::{Argument, Bus};
use crate::sources::json::{self, Json};
use crate::sources::niri;

const SERVICE: &str = "org.gnome.Mutter.ScreenCast";
const PATH: &str = "/org/gnome/Mutter/ScreenCast";
const MANAGER: &str = "org.gnome.Mutter.ScreenCast";
const SESSION: &str = "org.gnome.Mutter.ScreenCast.Session";

// how long niri has to cast a window, polled this often, before the window is taken to be gone: a
// session for a window niri does not have is answered all the same, and never cast
const CASTING: Duration = Duration::from_secs(3);
const LOOK: Duration = Duration::from_millis(50);

// a window as niri lists it
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub app: Option<String>,
}

/*
 * the window `id`, or the focused workspace's active window for none (as the screenshot takes it, and
 * whether or not a window has the keyboard focus), as niri lists it now
 */
pub fn find(id: Option<u64>) -> Result<Found, String> {
    let id = match id {
        Some(id) => id,
        None => super::super::capture::window()
            .map_err(|error| format!("niri: {error}"))?
            .ok_or("no window is focused to record")?,
    };
    let windows = niri::ask("\"Windows\"").map_err(|error| format!("niri: {error}"))?;

    listed(&windows, id).ok_or_else(|| format!("window {id} is not open"))
}

// whether niri lists the window `id`; none when niri cannot be asked
pub fn open(id: u64) -> Option<bool> {
    niri::ask("\"Windows\"")
        .ok()
        .map(|windows| listed(&windows, id).is_some())
}

fn listed(reply: &Json, id: u64) -> Option<Found> {
    let window = json::array(reply.get("Windows")?)?
        .iter()
        .find(|window| window.get("id").and_then(Json::as_u64) == Some(id))?;
    let size = window.get("layout")?.get("window_size")?.as_array()?;
    let side = |index: usize| size.get(index)?.as_u64()?.try_into().ok();

    Some(Found {
        id,
        width: side(0)?,
        height: side(1)?,
        app: window
            .get("app_id")
            .and_then(Json::as_str)
            .map(String::from),
    })
}

/*
 * a session niri holds for a window, cast on a PipeWire node. Stopped by `stop` only: dropping it
 * leaves it, so what holds one must end it on every path
 */
#[derive(Debug)]
pub struct Cast {
    path: String,
    session: u64,
    pub node: u64,
}

impl Cast {
    /*
     * asks niri to cast the window `id`: no prompt. niri answers a window it does not have as it
     * does one it does, so only the lack of a cast within `CASTING` says the window is gone. Blocks,
     * so never under a lock
     */
    pub fn open(id: u64) -> Result<Cast, String> {
        let bus = Bus::session();
        let path = bus
            .call(
                SERVICE,
                PATH,
                MANAGER,
                "CreateSession",
                &[Argument::Map(BTreeMap::new())],
            )
            .text()
            .to_owned();
        let Some(session) = session_of(&path) else {
            if path.is_empty() {
                return Err(String::from(
                    "niri has no screen cast service to record a window",
                ));
            }

            // a session was made but not named a number: stopped by its path, though not written down
            eprintln!("capture: stopping the unnamed screen cast session {path}");
            bus.call(SERVICE, &path, SESSION, "Stop", &[]);

            return Err(String::from(
                "niri gave a screen cast session that cannot be followed",
            ));
        };

        // from here on the session exists, and is written down before anything can fail
        remember(session);

        let cast = Cast {
            path,
            session,
            node: 0,
        };
        let window = Argument::Map(BTreeMap::from([(
            String::from("window-id"),
            Argument::from(id),
        )]));

        if bus
            .call(SERVICE, &cast.path, SESSION, "RecordWindow", &[window])
            .text()
            .is_empty()
        {
            cast.stop();
            return Err(format!("niri would not cast window {id}"));
        }

        bus.call(SERVICE, &cast.path, SESSION, "Start", &[]);

        let until = Instant::now() + CASTING;

        loop {
            let found = niri::ask("\"Casts\"")
                .map(|casts| node(&casts, session))
                .map_err(|error| format!("niri: {error}"));

            match found {
                Ok(Some(node)) => return Ok(Cast { node, ..cast }),
                Err(why) => {
                    cast.stop();
                    return Err(why);
                }
                Ok(None) if Instant::now() < until => thread::sleep(LOOK),
                Ok(None) => {
                    cast.stop();

                    return Err(match open(id) {
                        Some(true) => format!("niri did not cast window {id}"),
                        _ => format!("window {id} is gone"),
                    });
                }
            }
        }
    }

    /*
     * ends the session, which a window that closed ended already. Forgotten only once niri no longer
     * casts it (asked again by its own action if the bus did not end it), so one that stays is kept
     * for a later Kanade to stop. Whether it is gone
     */
    pub fn stop(&self) -> bool {
        Bus::session().call(SERVICE, &self.path, SESSION, "Stop", &[]);

        let gone = || {
            niri::ask("\"Casts\"")
                .map(|casts| !casts_hold(&casts, self.session))
                .unwrap_or(false)
        };
        let mut ended = gone();

        if !ended && niri::stop(self.session).is_ok() {
            ended = gone();
        }

        if ended {
            forget(self.session);
        } else {
            eprintln!("capture: the window cast {} may still be up", self.session);
        }

        ended
    }
}

// the session's number: the path is `/org/gnome/Mutter/ScreenCast/Session/u<number>`
fn session_of(path: &str) -> Option<u64> {
    path.strip_prefix(PATH)?
        .strip_prefix("/Session/u")?
        .parse()
        .ok()
}

// the PipeWire node of the cast of `session` in niri's casts; none while it has none
fn node(casts: &Json, session: u64) -> Option<u64> {
    json::array(casts.get("Casts")?)?
        .iter()
        .find(|cast| cast.get("session_id").and_then(Json::as_u64) == Some(session))?
        .get("pw_node_id")?
        .as_u64()
}

// whether niri lists a cast of `session`
fn casts_hold(casts: &Json, session: u64) -> bool {
    json::array(casts.get("Casts").unwrap_or(&Json::Null)).is_some_and(|casts| {
        casts
            .iter()
            .any(|cast| cast.get("session_id").and_then(Json::as_u64) == Some(session))
    })
}

/*
 * the sessions among `remembered` that niri still casts, and that are Kanade's to stop: a cast of a
 * window with no program behind it (a D-Bus session has no pid). A cast of an output or of any pid,
 * and one nobody wrote down, is another's
 */
fn leftovers(remembered: &[u64], casts: &Json) -> Vec<u64> {
    json::array(casts.get("Casts").unwrap_or(&Json::Null))
        .unwrap_or_default()
        .iter()
        .filter(|cast| {
            cast.get("target")
                .is_some_and(|target| target.get("Window").is_some())
                && cast.get("pid").is_none_or(Json::is_null)
        })
        .filter_map(|cast| cast.get("session_id")?.as_u64())
        .filter(|session| remembered.contains(session))
        .collect()
}

/*
 * the sessions an earlier Kanade made and did not stop, as the ledgers of the Kanades that are gone
 * say, and the ones this pid's own ledger holds from a Kanade that had this pid before, stopped; a
 * ledger of a Kanade that runs is left alone. Off the caller's thread, as niri is asked. A session
 * niri still casts after the attempt, or whose niri could not be asked, stays in a ledger for the
 * next start
 */
pub fn reclaim() {
    let Some(dir) = ledger_dir() else {
        return;
    };
    let gone = stale(&dir, process::id(), alive);
    let own = inherit(&dir, process::id());

    if gone.is_empty() && own.is_empty() {
        return;
    }

    let spawned = thread::Builder::new()
        .name(String::from("capture-reclaim"))
        .spawn(move || {
            let casts = niri::ask("\"Casts\"");
            let niri = niri_socket();

            for file in gone {
                let text = fs::read_to_string(&file).unwrap_or_default();
                let (ours, kept): (Vec<_>, Vec<_>) = entries(&text)
                    .into_iter()
                    .partition(|(socket, _)| *socket == niri);
                let ours: Vec<u64> = ours.into_iter().map(|(_, session)| session).collect();
                let kept = end(&ours, &casts, &niri, kept);

                keep(&file, &kept);
            }

            if !own.is_empty() {
                let (ours, kept): (Vec<_>, Vec<_>) =
                    own.into_iter().partition(|(socket, _)| *socket == niri);
                let ours: Vec<u64> = ours.into_iter().map(|(_, session)| session).collect();
                let kept = end(&ours, &casts, &niri, kept);

                amend(|entries| entries.extend(kept));
            }
        });

    if let Err(error) = spawned {
        eprintln!("capture: cannot look for window casts an earlier run left: {error}");
    }
}

/*
 * the sessions among `ours` that niri still casts stopped, and every one that could not be, for the
 * ledger: what `kept` already held is kept with them
 */
fn end<E>(
    ours: &[u64],
    casts: &Result<Json, E>,
    niri: &str,
    mut kept: Vec<(String, u64)>,
) -> Vec<(String, u64)> {
    match casts {
        Ok(casts) => {
            for session in leftovers(ours, casts) {
                eprintln!("capture: stopping the window cast {session} an earlier run left");

                if !matches!(niri::stop(session), Ok(niri::Acted::Done)) {
                    eprintln!("capture: cannot stop the cast {session}");
                    kept.push((niri.to_owned(), session));
                }
            }
        }
        // not known to be gone: tried again at the next start
        Err(_) => kept.extend(ours.iter().map(|session| (niri.to_owned(), *session))),
    }

    kept
}

/*
 * this Kanade's own ledger, read and cleared: a pid is reused, so a ledger named for the pid this
 * run has may be a Kanade's that died with it, and its casts are this run's to end rather than
 * another's to show
 */
fn inherit(dir: &Path, own: u32) -> Vec<(String, u64)> {
    let file = dir.join(format!("{LEDGERS}.{own}"));
    let _held = LEDGER.locked();
    let inherited = fs::read_to_string(&file)
        .map(|text| entries(&text))
        .unwrap_or_default();

    if !inherited.is_empty() {
        eprintln!(
            "capture: {} window casts an earlier Kanade with this pid made are taken over",
            inherited.len()
        );
        keep(&file, &[]);
    }

    inherited
}

/*
 * the sessions made and not yet confirmed gone, in a file of this Kanade's own under the runtime
 * directory, which is the user's alone and goes with the login: `window-casts.<pid>`, so no two
 * Kanades change one file, and a file whose pid runs is a Kanade's that lives. Each is written with the
 * niri it was made for: niri numbers its sessions from 1, so a number alone could be another niri's
 * cast. Held across a change of the file
 */
static LEDGER: Mutex<()> = Mutex::new(());

const LEDGERS: &str = "window-casts";

fn ledger_dir() -> Option<PathBuf> {
    let runtime = env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty())?;

    Some(PathBuf::from(runtime).join("kanade"))
}

fn ledger_file() -> Option<PathBuf> {
    Some(ledger_dir()?.join(format!("{LEDGERS}.{}", process::id())))
}

fn niri_socket() -> String {
    env::var("NIRI_SOCKET").unwrap_or_default()
}

/*
 * the ledgers in `dir` of Kanades that are gone: one named for a pid other than `own` that `alive`
 * denies, and the shared file of an earlier version, which no owner is known for. A ledger of a
 * Kanade that lives is never among them
 */
fn stale(dir: &Path, own: u32, alive: impl Fn(u32) -> bool) -> Vec<PathBuf> {
    let Ok(listing) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut files: Vec<PathBuf> = listing
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return false;
            };

            if name == LEDGERS {
                return true;
            }

            name.strip_prefix(LEDGERS)
                .and_then(|rest| rest.strip_prefix('.'))
                .and_then(|pid| pid.parse::<u32>().ok())
                .is_some_and(|pid| pid != own && !alive(pid))
        })
        .map(|entry| entry.path())
        .collect();

    files.sort();
    files
}

/*
 * whether the Kanade `pid` runs: a process of that number with the name this one has, so a number
 * another program took since is not taken for it
 */
fn alive(pid: u32) -> bool {
    let name = |pid: u32| fs::read_to_string(format!("/proc/{pid}/comm")).ok();

    match (name(pid), name(process::id())) {
        (Some(theirs), Some(ours)) => theirs == ours,
        _ => false,
    }
}

// every entry of this Kanade's ledger, whichever niri it is for; call it holding `LEDGER`
fn entries_now() -> Vec<(String, u64)> {
    ledger_file()
        .and_then(|file| fs::read_to_string(file).ok())
        .map(|text| entries(&text))
        .unwrap_or_default()
}

// the ledger at `file` holds `entries`; none left, no file
fn keep(file: &Path, entries: &[(String, u64)]) {
    let written = if entries.is_empty() {
        fs::remove_file(file).or_else(|error| match error.kind() {
            io::ErrorKind::NotFound => Ok(()),
            _ => Err(error),
        })
    } else {
        atomic::write(file, ledger(entries).as_bytes(), Mode::Private)
    };

    if let Err(error) = written {
        eprintln!("capture: cannot keep {}: {error}", file.display());
    }
}

// `change`s this Kanade's entries and writes them
fn amend(change: impl FnOnce(&mut Vec<(String, u64)>)) {
    let _held = LEDGER.locked();
    let Some(file) = ledger_file() else {
        return;
    };
    let mut entries = entries_now();

    change(&mut entries);
    keep(&file, &entries);
}

fn remember(session: u64) {
    amend(|entries| entries.push((niri_socket(), session)));
}

/*
 * whether `session` is a cast this Kanade made and has not confirmed gone: a window's cast has no
 * pid behind it (a D-Bus session), so the ledger is the only thing that says it is ours, and one of
 * ours is not shown as another program's in the privacy dots
 */
pub fn ours(session: u64) -> bool {
    let _held = LEDGER.locked();

    ours_among(&entries_now(), &niri_socket(), session)
}

fn ours_among(entries: &[(String, u64)], niri: &str, session: u64) -> bool {
    entries
        .iter()
        .any(|(socket, number)| socket == niri && *number == session)
}

fn forget(session: u64) {
    let niri = niri_socket();

    amend(|entries| entries.retain(|(socket, number)| !(*socket == niri && *number == session)));
}

// `<socket> <session>` a line; a socket path has no tab or newline
fn ledger(entries: &[(String, u64)]) -> String {
    entries
        .iter()
        .map(|(socket, session)| format!("{socket}\t{session}\n"))
        .collect()
}

fn entries(text: &str) -> Vec<(String, u64)> {
    text.lines()
        .filter_map(|line| {
            let (socket, session) = line.rsplit_once('\t')?;

            Some((socket.to_owned(), session.parse().ok()?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Json {
        json::parse(text).unwrap()
    }

    #[test]
    fn a_session_is_named_by_its_path() {
        assert_eq!(
            session_of("/org/gnome/Mutter/ScreenCast/Session/u109"),
            Some(109)
        );

        for not_one in [
            "",
            "/org/gnome/Mutter/ScreenCast/Session/x1",
            "/org/gnome/Mutter/ScreenCast/Stream/u1",
            "/org/gnome/Mutter/ScreenCast/Session/u",
        ] {
            assert_eq!(session_of(not_one), None, "{not_one:?}");
        }
    }

    #[test]
    fn a_window_is_read_from_niris_list() {
        let windows = parse(
            r#"{"Windows":[
                {"id":3,"app_id":"foot","layout":{"window_size":[800,600]}},
                {"id":149,"app_id":null,"layout":{"window_size":[1433,1018]}}
            ]}"#,
        );

        assert_eq!(
            listed(&windows, 149),
            Some(Found {
                id: 149,
                width: 1433,
                height: 1018,
                app: None
            })
        );
        assert_eq!(
            listed(&windows, 3).and_then(|found| found.app),
            Some("foot".into())
        );
        assert_eq!(listed(&windows, 4), None);
    }

    // a cast that is not up yet has no node to record from
    #[test]
    fn a_cast_is_found_by_its_session() {
        let casts = parse(
            r#"{"Casts":[
                {"session_id":9,"pw_node_id":null},
                {"session_id":109,"pw_node_id":78}
            ]}"#,
        );

        assert_eq!(node(&casts, 109), Some(78));
        assert_eq!(node(&casts, 9), None);
        assert_eq!(node(&casts, 1), None);
    }

    /*
     * safety: only a window cast with no pid that Kanade wrote down is stopped. A browser's or OBS's
     * (a pid), an output's, and a window cast of another program (not in the ledger) are left
     */
    #[test]
    fn only_a_cast_kanade_made_is_stopped() {
        let casts = parse(
            r#"{"Casts":[
                {"session_id":1,"target":{"Window":{"id":5}},"pid":null},
                {"session_id":2,"target":{"Window":{"id":5}},"pid":1234},
                {"session_id":3,"target":{"Output":{"name":"DP-1"}},"pid":null},
                {"session_id":4,"target":{"Window":{"id":6}},"pid":null},
                {"session_id":5,"target":{"Nothing":{}},"pid":null}
            ]}"#,
        );

        assert_eq!(leftovers(&[1, 2, 3, 5], &casts), [1]);
        assert_eq!(leftovers(&[], &casts), Vec::<u64>::new());
        assert_eq!(leftovers(&[1], &parse("{}")), Vec::<u64>::new());
    }

    /*
     * safety: a ledger of a Kanade that runs is its own to stop from, so another's start leaves it;
     * a dead one's (and the unowned file of an earlier version) is taken
     */
    #[test]
    fn only_a_dead_kanades_ledger_is_reclaimed() {
        let dir = std::env::temp_dir().join(format!("kanade-ledgers-{}", process::id()));

        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap();

        for name in [
            "window-casts",
            "window-casts.10",
            "window-casts.20",
            "window-casts.30",
            "window-casts.x",
            "window-casts.20.part",
            "other",
        ] {
            fs::write(dir.join(name), "").unwrap();
        }

        let taken: Vec<String> = stale(&dir, 30, |pid| pid == 20)
            .iter()
            .map(|file| file.file_name().unwrap().to_str().unwrap().to_owned())
            .collect();

        fs::remove_dir_all(&dir).unwrap();

        // 20 lives, 30 is this one
        assert_eq!(taken, ["window-casts", "window-casts.10"]);
        assert!(alive(process::id()));
        assert!(!alive(u32::MAX));
    }

    #[test]
    fn the_ledger_reads_back_what_it_wrote() {
        let written = vec![
            (String::from("/run/user/1/niri.wayland-1.77.sock"), 109),
            (String::from("/run/user/1/niri.wayland-1.88.sock"), 3),
        ];

        assert_eq!(entries(&ledger(&written)), written);
        // a line that is no entry is not one
        assert_eq!(
            entries("junk\n\t\nsock\tx\nsock\t4\n"),
            [(String::from("sock"), 4)]
        );
    }

    #[test]
    fn a_window_cast_in_the_ledger_is_ours() {
        let entries = vec![(String::from("sock1"), 42), (String::from("sock2"), 99)];

        assert!(ours_among(&entries, "sock1", 42));
        assert!(!ours_among(&entries, "sock1", 43));
        assert!(!ours_among(&entries, "sock2", 42));
    }

    #[test]
    fn a_reused_pid_inherits_and_clears_stale_ledger() {
        let dir = std::env::temp_dir().join(format!("kanade-test-ledger-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let pid = 99999;
        let file = dir.join(format!("window-casts.{pid}"));
        fs::write(&file, "sock\t101\n").unwrap();

        let inherited = inherit(&dir, pid);
        assert_eq!(inherited, vec![(String::from("sock"), 101)]);
        assert!(!file.exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
