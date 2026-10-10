//! Kanade's own niri EventStream (plan 3, 5.3), tracking the overview and casts too. It writes what
//! niri says as the `Niri` Service: a plain focused output, whether the overview is open and how
//! many times it opened, the focused workspace, whether anything casts and the window each output
//! shows. It knows none of what reads it: a Module `watch`es `Niri` in its own start. A screenshot
//! niri saves and each window it opens, closes or focuses are events, not state, so a Module takes
//! those by `on_screenshot` and `on_windows`. `ask` sends niri one request on a connection of its
//! own.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::io::{self, BufRead, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use kanade_runtime::service::Service;

use super::json::Json;
use crate::island::activity::Workspace;
use crate::supervise;

// what the core gets from niri; the default is also what a lost socket degrades to
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    // none while unknown, which counts every monitor as focused
    pub focused_output: Option<String>,

    pub overview: bool,

    // how many times the overview has opened, so one opened and closed between two looks still shows
    pub overviews: u32,

    // none while unknown, like focus
    pub workspace: Option<Focused>,

    // niri has at least one cast, paused ones included
    pub casting: bool,

    // the window each output shows, by output; empty while unknown
    pub shown: BTreeMap<String, Showing>,
}

// what niri last said, written only when it changes; a lost stream writes the default again
#[derive(Debug, Default)]
pub struct Niri {
    pub seen: Seen,
}

impl Service for Niri {
    fn new() -> Self {
        Self::default()
    }

    // written only by `follow`
    fn listen() {}
}

// the window an output shows, as niri says: its active workspace's active window
#[derive(Debug, Clone, PartialEq)]
pub struct Showing {
    pub app_id: Option<String>,

    // logical, as an output's size is
    pub tile: (f64, f64),
}

// niri's window id, which stays with the window for as long as it is open
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: WindowId,

    // the Wayland app id; none until the app sets one, and some never do
    pub app_id: Option<String>,

    pub focused: bool,

    // asks for attention
    pub urgent: bool,
}

// what niri said of its windows
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heard {
    // every window, replacing those known
    All(Vec<Window>),

    // one window, new or changed; one that is focused takes the focus from every other
    Opened(Window),

    Closed(WindowId),

    // none: no window has the focus
    Focused(Option<WindowId>),

    Urgent(WindowId, bool),

    // the stream ended, so no window is known
    Lost,
}

// the focused workspace, as niri last said
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Focused {
    // niri's id, which stays with the workspace when it moves or another one comes or goes
    pub id: u64,

    pub output: Option<String>,

    pub workspace: Workspace,
}

// a workspace as niri lists it
#[derive(Debug)]
struct Listed {
    output: Option<String>,

    // 1 based, on its output
    index: u32,

    name: Option<String>,

    // the one its output shows
    active: bool,

    // the window it shows
    window: Option<u64>,
}

// niri's events only say what changed, so the rest is remembered here
#[derive(Debug, Default)]
struct Known {
    // every workspace by id
    workspaces: HashMap<u64, Listed>,

    focused: Option<u64>,

    overview: bool,

    overviews: u32,

    // every cast by stream id
    casts: HashSet<u64>,

    // every window by id: which app's, and its tile
    windows: HashMap<u64, Showing>,
}

impl Known {
    // anything niri sends that is not about workspaces, windows, focus, the overview or casts changes
    // nothing
    fn apply(&mut self, event: &Json) {
        if let Some(changed) = event.get("WorkspacesChanged") {
            let workspaces = changed.get("workspaces").and_then(Json::as_array);

            self.workspaces.clear();
            self.focused = None;

            for workspace in workspaces.unwrap_or_default() {
                let Some(id) = workspace.get("id").and_then(Json::as_u64) else {
                    continue;
                };

                let text = |key| workspace.get(key).and_then(Json::as_str).map(String::from);
                let index = workspace.get("idx").and_then(Json::as_u64);

                self.workspaces.insert(
                    id,
                    Listed {
                        output: text("output"),
                        index: index.and_then(|index| index.try_into().ok()).unwrap_or(0),
                        name: text("name"),
                        active: workspace.get("is_active").and_then(Json::as_bool) == Some(true),
                        window: workspace.get("active_window_id").and_then(Json::as_u64),
                    },
                );

                if workspace.get("is_focused").and_then(Json::as_bool) == Some(true) {
                    self.focused = Some(id);
                }
            }
        } else if let Some(activated) = event.get("WorkspaceActivated") {
            let id = activated.get("id").and_then(Json::as_u64);

            // not focused: it only became the one shown on its own output
            if activated.get("focused").and_then(Json::as_bool) == Some(true) {
                self.focused = id;
            }

            if let Some(output) = id
                .and_then(|id| self.workspaces.get(&id))
                .map(|listed| listed.output.clone())
            {
                for (other, listed) in &mut self.workspaces {
                    if listed.output == output {
                        listed.active = Some(*other) == id;
                    }
                }
            }
        } else if let Some(changed) = event.get("WorkspaceActiveWindowChanged") {
            if let Some(listed) = changed
                .get("workspace_id")
                .and_then(Json::as_u64)
                .and_then(|id| self.workspaces.get_mut(&id))
            {
                listed.window = changed.get("active_window_id").and_then(Json::as_u64);
            }
        } else if let Some(changed) = event.get("WindowsChanged") {
            let windows = changed.get("windows").and_then(Json::as_array);

            self.windows = windows
                .unwrap_or_default()
                .iter()
                .filter_map(shown)
                .collect();
        } else if let Some(opened) = event.get("WindowOpenedOrChanged") {
            self.windows.extend(opened.get("window").and_then(shown));
        } else if let Some(closed) = event.get("WindowClosed") {
            if let Some(id) = closed.get("id").and_then(Json::as_u64) {
                self.windows.remove(&id);
            }
        } else if let Some(changed) = event.get("WindowLayoutsChanged") {
            for change in changed
                .get("changes")
                .and_then(Json::as_array)
                .unwrap_or_default()
            {
                if let Some([id, layout]) = change.as_array()
                    && let Some(window) = id.as_u64().and_then(|id| self.windows.get_mut(&id))
                    && let Some(tile) = self::tile(layout)
                {
                    window.tile = tile;
                }
            }
        } else if let Some(overview) = event.get("OverviewOpenedOrClosed")
            && let Some(open) = overview.get("is_open").and_then(Json::as_bool)
        {
            if open && !self.overview {
                self.overviews = self.overviews.wrapping_add(1);
            }

            self.overview = open;
        } else if let Some(changed) = event.get("CastsChanged") {
            let casts = changed.get("casts").and_then(Json::as_array);

            self.casts = casts
                .unwrap_or_default()
                .iter()
                .filter_map(stream)
                .collect();
        } else if let Some(started) = event.get("CastStartedOrChanged") {
            self.casts.extend(started.get("cast").and_then(stream));
        } else if let Some(stopped) = event.get("CastStopped")
            && let Some(id) = stopped.get("stream_id").and_then(Json::as_u64)
        {
            self.casts.remove(&id);
        }
    }

    fn seen(&self) -> Seen {
        let focused = self
            .focused
            .and_then(|id| Some((id, self.workspaces.get(&id)?)));

        let workspace = focused.map(|(id, listed)| Focused {
            id,
            output: listed.output.clone(),
            workspace: Workspace {
                index: listed.index,
                count: self.on(&listed.output),
                name: listed.name.clone(),
            },
        });

        let shown = self
            .workspaces
            .values()
            .filter(|listed| listed.active)
            .filter_map(|listed| {
                let window = self.windows.get(&listed.window?)?;

                Some((listed.output.clone()?, window.clone()))
            })
            .collect();

        Seen {
            focused_output: focused.and_then(|(_, listed)| listed.output.clone()),
            overview: self.overview,
            overviews: self.overviews,
            workspace,
            casting: !self.casts.is_empty(),
            shown,
        }
    }

    // how many workspaces `output` has
    fn on(&self, output: &Option<String>) -> u32 {
        let count = self
            .workspaces
            .values()
            .filter(|listed| listed.output == *output)
            .count();

        count.try_into().unwrap_or(u32::MAX)
    }
}

// a window as `fullscreen` matches it, by id; none without an id or a tile
fn shown(window: &Json) -> Option<(u64, Showing)> {
    let id = window.get("id").and_then(Json::as_u64)?;
    let app_id = window
        .get("app_id")
        .and_then(Json::as_str)
        .map(String::from);
    let tile = tile(window.get("layout")?)?;

    Some((id, Showing { app_id, tile }))
}

fn tile(layout: &Json) -> Option<(f64, f64)> {
    match layout.get("tile_size")?.as_array()? {
        [width, height] => Some((width.as_f64()?, height.as_f64()?)),
        _ => None,
    }
}

// a cast's stream; none for Kanade's own, liquid glass capturing what is behind the Island (ADR 0037)
fn stream(cast: &Json) -> Option<u64> {
    let own = cast.get("pid").and_then(Json::as_u64) == Some(u64::from(std::process::id()));

    if own {
        return None;
    }

    cast.get("stream_id").and_then(Json::as_u64)
}

// where niri saved a screenshot; none for another event, or one it only put on the clipboard
fn captured(event: &Json) -> Option<String> {
    event
        .get("ScreenshotCaptured")?
        .get("path")?
        .as_str()
        .map(String::from)
}

// what a window event says; none for another event, and for those `windows` ignores, like layouts
fn windowed(event: &Json) -> Option<Heard> {
    if let Some(changed) = event.get("WindowsChanged") {
        let windows = changed.get("windows").and_then(Json::as_array);

        Some(Heard::All(
            windows
                .unwrap_or_default()
                .iter()
                .filter_map(window)
                .collect(),
        ))
    } else if let Some(opened) = event.get("WindowOpenedOrChanged") {
        opened.get("window").and_then(window).map(Heard::Opened)
    } else if let Some(closed) = event.get("WindowClosed") {
        closed
            .get("id")
            .and_then(Json::as_u64)
            .map(|id| Heard::Closed(WindowId(id)))
    } else if let Some(focus) = event.get("WindowFocusChanged") {
        // a null id: no window has the focus
        Some(Heard::Focused(
            focus.get("id").and_then(Json::as_u64).map(WindowId),
        ))
    } else if let Some(urgency) = event.get("WindowUrgencyChanged") {
        let id = urgency.get("id").and_then(Json::as_u64)?;
        let urgent = urgency.get("urgent").and_then(Json::as_bool)?;

        Some(Heard::Urgent(WindowId(id), urgent))
    } else {
        None
    }
}

// one of niri's windows as `windows` keeps it; none without an id
fn window(window: &Json) -> Option<Window> {
    let flag = |key| window.get(key).and_then(Json::as_bool).unwrap_or(false);

    Some(Window {
        id: WindowId(window.get("id").and_then(Json::as_u64)?),
        app_id: window
            .get("app_id")
            .and_then(Json::as_str)
            .map(String::from),
        focused: flag("is_focused"),
        urgent: flag("is_urgent"),
    })
}

// what a Module asked to hear, in the order asked; kept as `fn`s so a Module takes it in its start
static WINDOWS: Mutex<Vec<fn(Heard)>> = Mutex::new(Vec::new());
static SCREENSHOTS: Mutex<Vec<fn(String)>> = Mutex::new(Vec::new());

// set once `follow` runs, after which a Module can no longer ask to hear
static FOLLOWING: AtomicBool = AtomicBool::new(false);

// `then` hears every window event, on the niri thread, so it must not block. Taken before `follow`
// runs, or the windows niri lists first are missed
pub fn on_windows(then: fn(Heard)) {
    assert!(
        !FOLLOWING.load(Ordering::Relaxed),
        "niri is already followed"
    );

    WINDOWS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(then);
}

// `then` hears the path of each screenshot niri saves, on the niri thread, so it must not block
pub fn on_screenshot(then: fn(String)) {
    assert!(
        !FOLLOWING.load(Ordering::Relaxed),
        "niri is already followed"
    );

    SCREENSHOTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(then);
}

// runs on its own thread for good; without niri, or once its socket is lost, every monitor is focused
pub fn follow() {
    FOLLOWING.store(true, Ordering::Relaxed);

    // kept across a restart, so the new stream's first events post only what changed meanwhile
    let mut posted = Seen::default();

    supervise::run("niri", || {
        run(
            connect().map(BufReader::new),
            &mut posted,
            |_, seen| Niri::write().seen = seen.clone(),
            |path| {
                let heard = SCREENSHOTS
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();

                for then in heard {
                    then(path.clone());
                }
            },
            |heard| {
                let hearing = WINDOWS
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();

                for then in hearing {
                    then(heard.clone());
                }
            },
        );
    });
}

fn run(
    stream: io::Result<impl BufRead>,
    posted: &mut Seen,
    mut post: impl FnMut(&Seen, &Seen),
    mut shot: impl FnMut(String),
    mut hear: impl FnMut(Heard),
) {
    let lost = match stream {
        Ok(stream) => watch(stream, posted, &mut post, &mut shot, &mut hear),
        Err(error) => error,
    };

    // plan 5.3: logged, no toast
    eprintln!("kanade: niri event stream lost ({lost}), every monitor counts as focused");

    if *posted != Seen::default() {
        post(posted, &Seen::default());
        *posted = Seen::default();
    }

    // the windows niri had may be gone by the time it is back; it lists them again then
    hear(Heard::Lost);
}

fn connect() -> io::Result<UnixStream> {
    let mut stream = UnixStream::connect(socket()?)?;

    stream.write_all(b"\"EventStream\"\n")?;

    Ok(stream)
}

fn socket() -> io::Result<String> {
    env::var("NIRI_SOCKET").map_err(|_| io::Error::other("NIRI_SOCKET is not set"))
}

// how long niri may take to answer `ask` or `act`
const PATIENCE: Duration = Duration::from_secs(2);

// how a request niri was sent came out
#[derive(Debug)]
enum Exchange {
    Answered(Json),
    Refused(io::Error),

    // sent but not answered, or answered with what is neither: niri may have carried it out
    Unclear(io::Error),
}

// what an action niri was sent came to
#[derive(Debug)]
pub enum Acted {
    Done,

    // niri took it but gave no clear answer, so it may or may not be carried out; why
    Unknown(String),
}

/*
 * what niri answers `request`, a query like `"Version"`, on a connection of its own; what it
 * refuses, or does not answer, is an error
 */
pub fn ask(request: &str) -> io::Result<Json> {
    match exchange(UnixStream::connect(socket()?)?, request, PATIENCE)? {
        Exchange::Answered(answer) => Ok(answer),
        Exchange::Refused(error) | Exchange::Unclear(error) => Err(error),
    }
}

/*
 * asks niri to carry out `action`, a request with an effect like `{"Action":...}`; an error is a
 * definite no, while a missing or unclear answer is told apart, so the action is not repeated as if
 * it failed
 */
pub fn act(action: &str) -> io::Result<Acted> {
    match exchange(UnixStream::connect(socket()?)?, action, PATIENCE)? {
        Exchange::Answered(_) => Ok(Acted::Done),
        Exchange::Refused(error) => Err(error),
        Exchange::Unclear(error) => Ok(Acted::Unknown(error.to_string())),
    }
}

/*
 * one `patience` for sending and answering both; an error before niri has the whole request means
 * it was not sent
 */
fn exchange(stream: UnixStream, request: &str, patience: Duration) -> io::Result<Exchange> {
    let deadline = Instant::now() + patience;

    stream.set_write_timeout(Some(patience))?;
    (&stream).write_all(format!("{request}\n").as_bytes())?;

    let mut reply = String::new();

    // a timeout of zero is refused, so a request that took all the time waits a moment more
    let left = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(1));
    let read = stream
        .set_read_timeout(Some(left))
        .and_then(|()| BufReader::new(stream).read_line(&mut reply));

    let unanswered = match read {
        Ok(0) => Some(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "closed without answering",
        )),
        Ok(_) => None,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            Some(io::Error::new(
                error.kind(),
                format!("did not answer within {patience:?}"),
            ))
        }
        Err(error) => Some(error),
    };

    if let Some(error) = unanswered {
        return Ok(Exchange::Unclear(error));
    }

    let parsed = Json::parse(&reply);

    if let Some(answer) = parsed.as_ref().and_then(|parsed| parsed.get("Ok")) {
        return Ok(Exchange::Answered(answer.clone()));
    }

    if let Some(refused) = parsed
        .as_ref()
        .and_then(|parsed| parsed.get("Err"))
        .and_then(Json::as_str)
    {
        return Ok(Exchange::Refused(io::Error::other(refused.to_owned())));
    }

    Ok(Exchange::Unclear(io::Error::other(format!(
        "answered unexpectedly: {}",
        reply.trim_end()
    ))))
}

/*
 * follows niri until the stream ends, posting what changes as it was and is now, so an event that
 * changes nothing the core follows wakes nothing; window events go to `hear`, which sorts out its
 * own. Returns why it ended
 */
fn watch(
    lines: impl BufRead,
    posted: &mut Seen,
    post: &mut impl FnMut(&Seen, &Seen),
    shot: &mut impl FnMut(String),
    hear: &mut impl FnMut(Heard),
) -> io::Error {
    let mut lines = lines.lines();
    let mut niri = Known::default();

    // niri answers the request first, events follow on the same connection
    match lines.next() {
        Some(Ok(reply)) if Json::parse(&reply).is_some_and(|reply| reply.get("Ok").is_some()) => {}
        Some(Ok(reply)) => return io::Error::other(format!("niri refused: {reply}")),
        Some(Err(error)) => return error,
        None => return io::ErrorKind::UnexpectedEof.into(),
    }

    for line in lines {
        let line = match line {
            Ok(line) => line,
            Err(error) => return error,
        };

        // a line this reader cannot follow is skipped, the next event still counts
        let Some(event) = Json::parse(&line) else {
            eprintln!("kanade: skipped a niri event that is not JSON: {line}");
            continue;
        };

        if let Some(path) = captured(&event) {
            shot(path);
        }

        if let Some(heard) = windowed(&event) {
            hear(heard);
        }

        niri.apply(&event);

        let seen = niri.seen();

        if seen != *posted {
            post(posted, &seen);
            *posted = seen;
        }
    }

    io::ErrorKind::UnexpectedEof.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"{"Ok":"Handled"}"#;

    // two outputs, workspace 1 focused on eDP-1, workspace 3 shown on HDMI-A-1
    const WORKSPACES: &str = r#"{"WorkspacesChanged":{"workspaces":[{"id":1,"idx":1,"name":null,"output":"eDP-1","is_urgent":false,"is_active":true,"is_focused":true,"active_window_id":2},{"id":2,"idx":2,"name":null,"output":"eDP-1","is_urgent":false,"is_active":false,"is_focused":false,"active_window_id":null},{"id":3,"idx":1,"name":"web","output":"HDMI-A-1","is_urgent":false,"is_active":true,"is_focused":false,"active_window_id":null}]}}"#;

    fn activated(id: u64, focused: bool) -> String {
        format!(r#"{{"WorkspaceActivated":{{"id":{id},"focused":{focused}}}}}"#)
    }

    fn overview(open: bool) -> String {
        format!(r#"{{"OverviewOpenedOrClosed":{{"is_open":{open}}}}}"#)
    }

    // every post for this stream, and why it ended
    fn posts(lines: &[&str]) -> (Vec<Seen>, io::ErrorKind) {
        let mut posts = Vec::new();
        let mut posted = Seen::default();

        let text = lines.join("\n");
        let lost = watch(
            text.as_bytes(),
            &mut posted,
            &mut |before, seen| {
                assert_eq!(before, posts.last().unwrap_or(&Seen::default()));
                posts.push(seen.clone())
            },
            &mut |path| panic!("no screenshot was taken, yet {path}"),
            &mut drop,
        );

        assert_eq!(posts.last().cloned().unwrap_or_default(), posted);

        (posts, lost.kind())
    }

    // a watcher that looks once sees only the last post
    #[test]
    fn an_overview_opened_and_closed_is_counted() {
        let (posts, _) = posts(&[
            OK,
            WORKSPACES,
            &overview(true),
            &activated(2, true),
            &overview(false),
        ]);
        let last = posts.last().expect("the stream changed things");

        assert!(!last.overview);
        assert_eq!(last.overviews, 1);
        assert_eq!(last.workspace.as_ref().map(|focused| focused.id), Some(2));
    }

    #[test]
    fn other_events_and_junk_post_nothing() {
        let (posts, _) = posts(&[
            OK,
            r#"{"WindowsChanged":{"windows":[]}}"#,
            r#"{"KeyboardLayoutsChanged":{"keyboard_layouts":{"names":["English (US)"],"current_idx":0}}}"#,
            "not json",
            &activated(9, true),
            r#"{"CastsChanged":{"casts":[]}}"#,
            r#"{"ScreenshotCaptured":{"path":null}}"#,
        ]);

        // workspace 9 is unknown, so its output is too, which is the default
        assert_eq!(posts, []);
    }

    // what `windows` hears of this stream
    fn heard(lines: &[&str]) -> Vec<Heard> {
        let text = [&[OK], lines].concat().join("\n");
        let mut heard = Vec::new();

        watch(
            text.as_bytes(),
            &mut Seen::default(),
            &mut |_, _| {},
            &mut drop,
            &mut |told| heard.push(told),
        );

        heard
    }

    fn window(id: u64, app_id: Option<&str>, focused: bool, urgent: bool) -> Window {
        Window {
            id: WindowId(id),
            app_id: app_id.map(String::from),
            focused,
            urgent,
        }
    }

    #[test]
    fn window_events_are_heard_as_windows() {
        let heard = heard(&[
            r#"{"WindowsChanged":{"windows":[{"id":1,"title":"~","app_id":"kitty","pid":7,"workspace_id":1,"is_focused":true,"is_floating":false,"is_urgent":false,"layout":{},"focus_timestamp":null},{"id":2,"title":null,"app_id":null,"pid":null,"workspace_id":null,"is_focused":false,"is_floating":true,"is_urgent":true}]}}"#,
            r#"{"WindowOpenedOrChanged":{"window":{"id":3,"title":"a","app_id":"firefox","is_focused":true,"is_urgent":false}}}"#,
            r#"{"WindowFocusChanged":{"id":1}}"#,
            r#"{"WindowFocusChanged":{"id":null}}"#,
            r#"{"WindowUrgencyChanged":{"id":3,"urgent":true}}"#,
            r#"{"WindowClosed":{"id":3}}"#,
            r#"{"WindowLayoutsChanged":{"changes":[]}}"#,
            r#"{"WindowFocusTimestampChanged":{"id":1,"focus_timestamp":null}}"#,
            r#"{"WindowOpenedOrChanged":{"window":{"title":"no id"}}}"#,
            &overview(true),
        ]);

        assert_eq!(
            heard,
            [
                Heard::All(vec![
                    window(1, Some("kitty"), true, false),
                    window(2, None, false, true),
                ]),
                Heard::Opened(window(3, Some("firefox"), true, false)),
                Heard::Focused(Some(WindowId(1))),
                Heard::Focused(None),
                Heard::Urgent(WindowId(3), true),
                Heard::Closed(WindowId(3)),
            ]
        );
    }

    fn cast(stream: u64) -> String {
        format!(
            r#"{{"CastStartedOrChanged":{{"cast":{{"stream_id":{stream},"session_id":1,"kind":"PipeWire","target":{{"Output":{{"name":"eDP-1"}}}},"is_dynamic_target":false,"is_active":true,"pid":null,"pw_node_id":null}}}}}}"#
        )
    }

    fn stopped(stream: u64) -> String {
        format!(r#"{{"CastStopped":{{"stream_id":{stream}}}}}"#)
    }

    // whether each post says something casts
    fn casting(posts: &[Seen]) -> Vec<bool> {
        posts.iter().map(|seen| seen.casting).collect()
    }

    // a share and a recording at once are one capture, until both stop
    #[test]
    fn casting_lasts_from_the_first_cast_to_the_last() {
        let (posts, _) = posts(&[OK, &cast(1), &cast(2), &stopped(1), &cast(2), &stopped(2)]);

        assert_eq!(casting(&posts), [true, false]);
    }

    // liquid glass's capture is Kanade's own, and records nothing
    #[test]
    fn kanade_s_own_capture_is_no_cast() {
        let own = format!(
            r#"{{"CastStartedOrChanged":{{"cast":{{"stream_id":7,"session_id":7,"kind":"WlrScreencopy","target":{{"Output":{{"name":"eDP-1"}}}},"is_dynamic_target":false,"is_active":true,"pid":{},"pw_node_id":null}}}}}}"#,
            std::process::id()
        );
        let list = format!(
            r#"{{"CastsChanged":{{"casts":[{{"stream_id":8,"pid":{}}}]}}}}"#,
            std::process::id()
        );

        let (own_only, _) = posts(&[OK, &own, &list]);
        assert!(!casting(&own_only).contains(&true));

        // and hides no other
        let (beside, _) = posts(&[OK, &own, &cast(1)]);
        assert_eq!(casting(&beside), [true]);
    }

    #[test]
    fn a_cast_list_replaces_the_casts_known() {
        let list = |streams: &str| {
            format!(
                r#"{{"CastsChanged":{{"casts":[{}]}}}}"#,
                streams
                    .split(',')
                    .filter(|stream| !stream.is_empty())
                    .map(|stream| format!(r#"{{"stream_id":{stream}}}"#))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };

        let replaced = posts(&[OK, &list("3"), &stopped(9), &list("4,5"), &list("")]).0;

        assert_eq!(casting(&replaced), [true, false]);

        // a stop for a cast the list no longer has changes nothing
        let forgotten = posts(&[OK, &cast(1), &list("2"), &stopped(1)]).0;

        assert_eq!(casting(&forgotten), [true]);
    }

    #[test]
    fn a_refused_request_ends_the_stream() {
        let (posts, lost) = posts(&[r#"{"Err":"nope"}"#, WORKSPACES]);

        assert_eq!(posts, []);
        assert_eq!(lost, io::ErrorKind::Other);
    }

    // the core is told once, then niri is gone for good
    #[test]
    fn losing_the_stream_degrades_to_every_monitor_focused() {
        let text = [OK, WORKSPACES, &overview(true), &cast(1)].join("\n");
        let mut posts = Vec::new();

        run(
            Ok(text.as_bytes()),
            &mut Seen::default(),
            |_, seen| posts.push(seen.clone()),
            drop,
            drop,
        );

        // nobody can say a cast still runs, so its capture goes too
        assert_eq!(posts.last(), Some(&Seen::default()));
        assert_eq!(posts.len(), 4);

        // nothing was posted, so nothing has to be taken back
        let mut posts = Vec::new();

        run(
            Err::<&[u8], _>(io::ErrorKind::NotFound.into()),
            &mut Seen::default(),
            |_, seen| posts.push(seen.clone()),
            drop,
            drop,
        );
        run(
            Ok(OK.as_bytes()),
            &mut Seen::default(),
            |_, seen| posts.push(seen.clone()),
            drop,
            drop,
        );

        assert_eq!(posts, []);
    }
}
