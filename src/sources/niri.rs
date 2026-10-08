//! Kanade's own niri EventStream (plan 3, 5.3): Amane's niri backend is private and tracks neither
//! the overview nor casts. Hands the core a plain focused output and whether the overview is open,
//! `workspace` the focused workspace, `privacy` whether anything captures the screen, and `capture`
//! each screenshot niri saves, `windows` every window opened, closed or focused. `ask` sends niri
//! one request on a connection of its own.

use std::collections::{HashMap, HashSet};
use std::env;
use std::io::{self, BufRead, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use amane::Service;

use super::capture;
use super::json::Json;
use super::privacy::Privacy;
use super::windows::{self, Heard, Window, WindowId};
use super::workspace;
use crate::banners::Banners;
use crate::island::activity::{Activity, Id, Workspace};
use crate::island::service::IslandService;
use crate::osd::Osd;
use crate::supervise;

// what the core gets from niri; the default is also what a lost socket degrades to
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    // none while unknown, which counts every monitor as focused
    pub focused_output: Option<String>,

    pub overview: bool,

    // none while unknown, like focus
    pub workspace: Option<Focused>,

    // niri has at least one cast, paused ones included
    pub casting: bool,
}

// what the island does about a change niri made
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Post(Activity),
    Withdraw(Id),
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
}

// niri's events only say what changed, so the rest is remembered here
#[derive(Debug, Default)]
struct Niri {
    // every workspace by id
    workspaces: HashMap<u64, Listed>,

    focused: Option<u64>,

    overview: bool,

    // every cast by stream id
    casts: HashSet<u64>,
}

impl Niri {
    // anything niri sends that is not about workspaces, focus, the overview or casts changes nothing
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
                    },
                );

                if workspace.get("is_focused").and_then(Json::as_bool) == Some(true) {
                    self.focused = Some(id);
                }
            }
        } else if let Some(activated) = event.get("WorkspaceActivated") {
            // not focused: it only became the one shown on its own output
            if activated.get("focused").and_then(Json::as_bool) == Some(true) {
                self.focused = activated.get("id").and_then(Json::as_u64);
            }
        } else if let Some(overview) = event.get("OverviewOpenedOrClosed")
            && let Some(open) = overview.get("is_open").and_then(Json::as_bool)
        {
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

        Seen {
            focused_output: focused.and_then(|(_, listed)| listed.output.clone()),
            overview: self.overview,
            workspace,
            casting: !self.casts.is_empty(),
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

// a cast's stream id, which niri stops it by
fn stream(cast: &Json) -> Option<u64> {
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

// which Modules beside the core hear from niri; one that is off hears nothing
#[derive(Debug, Clone, Copy)]
pub struct Posts {
    pub workspace: bool,
    pub privacy: bool,
    pub banners: bool,
    pub osd: bool,
    pub capture: bool,
    pub windows: bool,
}

// runs on its own thread for good; without niri, or once its socket is lost, every monitor is focused
pub fn follow(posts: Posts) {
    // kept across a restart, so the new stream's first events post only what changed meanwhile
    let mut posted = Seen::default();

    supervise::run("niri", || {
        run(
            connect().map(BufReader::new),
            &mut posted,
            |before, seen| {
                post(posts, before, seen);
            },
            |path| {
                if posts.capture {
                    capture::captured(path);
                }
            },
            |heard| {
                if posts.windows {
                    windows::hear(heard);
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
    let mut niri = Niri::default();

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

/*
 * the core hears of focus and the overview only when they change, of the focused workspace only
 * as a switch, so a list that only renumbers wakes nothing; the privacy cluster hears of casts
 * only as the first starts or the last stops, the Banners and the OSD of focus only as it moves
 */
fn post(posts: Posts, before: &Seen, seen: &Seen) {
    if posts.privacy && before.casting != seen.casting {
        Privacy::write().casting = seen.casting;
    }

    if posts.banners && before.focused_output != seen.focused_output {
        Banners::write().focus(seen.focused_output.clone(), Instant::now());
    }

    if posts.osd && before.focused_output != seen.focused_output {
        Osd::write().focus(seen.focused_output.clone());
    }

    let focus = (&before.focused_output, before.overview) != (&seen.focused_output, seen.overview);
    let change = posts
        .workspace
        .then(|| workspace::change(before, seen))
        .flatten();

    if !focus && change.is_none() {
        return;
    }

    let now = Instant::now();
    let mut island = IslandService::write();

    if focus {
        island.set_niri(seen.focused_output.clone(), seen.overview, now);
    }

    match change {
        Some(Change::Post(activity)) => island.post(activity, now),
        Some(Change::Withdraw(id)) => island.withdraw(&id, now),
        None => {}
    }
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

    // what set_niri hears of these posts: only focus and the overview, and only as they change
    fn focus(posts: &[Seen]) -> Vec<Seen> {
        let mut focus: Vec<Seen> = posts
            .iter()
            .map(|seen| Seen {
                workspace: None,
                ..seen.clone()
            })
            .collect();

        focus.dedup();
        focus
    }

    fn focused(output: &str) -> Seen {
        Seen {
            focused_output: Some(output.into()),
            overview: false,
            workspace: None,
            casting: false,
        }
    }

    // the focused workspace of each post
    fn workspaces(posts: &[Seen]) -> Vec<Option<(u64, Workspace)>> {
        posts
            .iter()
            .map(|seen| {
                let focused = seen.workspace.clone()?;

                Some((focused.id, focused.workspace))
            })
            .collect()
    }

    fn workspace(index: u32, count: u32, name: Option<&str>) -> Workspace {
        Workspace {
            index,
            count,
            name: name.map(String::from),
        }
    }

    #[test]
    fn focus_follows_the_focused_workspace_across_outputs() {
        let (posts, _) = posts(&[OK, WORKSPACES, &activated(3, true), &activated(1, true)]);

        assert_eq!(
            focus(&posts),
            [focused("eDP-1"), focused("HDMI-A-1"), focused("eDP-1")]
        );
        assert_eq!(
            workspaces(&posts)[1],
            Some((3, workspace(1, 1, Some("web"))))
        );
    }

    #[test]
    fn switching_workspaces_on_one_output_keeps_the_focus() {
        let (posts, _) = posts(&[OK, WORKSPACES, &activated(2, true), &activated(1, true)]);

        assert_eq!(focus(&posts), [focused("eDP-1")]);
        assert_eq!(
            workspaces(&posts),
            [
                Some((1, workspace(1, 2, None))),
                Some((2, workspace(2, 2, None))),
                Some((1, workspace(1, 2, None))),
            ]
        );
    }

    // niri adds an empty workspace below the one a window first opens on
    #[test]
    fn a_workspace_coming_renumbers_the_focused_one() {
        let added = WORKSPACES.replacen(
            "[",
            r#"[{"id":4,"idx":3,"name":null,"output":"eDP-1","is_focused":false},"#,
            1,
        );

        let (posts, _) = posts(&[OK, WORKSPACES, &added]);

        assert_eq!(focus(&posts), [focused("eDP-1")]);
        assert_eq!(
            workspaces(&posts),
            [
                Some((1, workspace(1, 2, None))),
                Some((1, workspace(1, 3, None))),
            ]
        );
    }

    // activated without focus only changed what an output shows
    #[test]
    fn unfocused_activation_keeps_the_focus() {
        let (posts, _) = posts(&[OK, WORKSPACES, &activated(3, false)]);

        assert_eq!(focus(&posts), [focused("eDP-1")]);
        assert_eq!(posts.len(), 1);
    }

    #[test]
    fn workspace_moved_to_another_output_takes_the_focus_along() {
        let moved = WORKSPACES.replacen("\"eDP-1\"", "\"HDMI-A-1\"", 1);

        let (posts, _) = posts(&[OK, WORKSPACES, &moved]);

        assert_eq!(focus(&posts), [focused("eDP-1"), focused("HDMI-A-1")]);
    }

    // a fresh list replaces the old one whole: gone workspaces and a focus nobody has are unknown
    #[test]
    fn a_new_workspace_list_forgets_the_old_one() {
        let unfocused = WORKSPACES.replace("\"is_focused\":true", "\"is_focused\":false");
        let without_web =
            r#"{"WorkspacesChanged":{"workspaces":[{"id":1,"output":"eDP-1","is_focused":true}]}}"#;

        assert_eq!(
            posts(&[OK, WORKSPACES, &unfocused]).0.last(),
            Some(&Seen::default())
        );
        assert_eq!(
            posts(&[OK, WORKSPACES, without_web, &activated(3, true)])
                .0
                .last(),
            Some(&Seen::default())
        );
    }

    #[test]
    fn overview_opens_and_closes() {
        let (posts, _) = posts(&[OK, WORKSPACES, &overview(true), &overview(false)]);

        let open = Seen {
            overview: true,
            ..focused("eDP-1")
        };

        assert_eq!(focus(&posts), [focused("eDP-1"), open, focused("eDP-1")]);
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

    // one only put on the clipboard has no file to show
    #[test]
    fn each_screenshot_saved_is_heard_once() {
        let text = [
            OK,
            r#"{"ScreenshotCaptured":{"path":"/home/k/Pictures/Screenshots/a.png"}}"#,
            r#"{"ScreenshotCaptured":{"path":null}}"#,
            &overview(true),
            r#"{"ScreenshotCaptured":{"path":"/tmp/b.png"}}"#,
        ]
        .join("\n");
        let mut shots = Vec::new();

        watch(
            text.as_bytes(),
            &mut Seen::default(),
            &mut |_, _| {},
            &mut |path| shots.push(path),
            &mut drop,
        );

        assert_eq!(shots, ["/home/k/Pictures/Screenshots/a.png", "/tmp/b.png"]);
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

    #[test]
    fn losing_the_stream_forgets_the_windows() {
        let mut heard = Vec::new();

        run(
            Err::<&[u8], _>(io::ErrorKind::NotFound.into()),
            &mut Seen::default(),
            |_, _| {},
            drop,
            |told| heard.push(told),
        );

        assert_eq!(heard, [Heard::Lost]);
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

    #[test]
    fn the_stream_ending_says_so() {
        assert_eq!(posts(&[OK, WORKSPACES]).1, io::ErrorKind::UnexpectedEof);
        assert_eq!(posts(&[]).1, io::ErrorKind::UnexpectedEof);
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

    // niri at the other end of a socket, answering `reply` after `delay`, or closing unanswered
    fn exchanged(reply: Option<&str>, delay: Duration) -> Exchange {
        let (ours, theirs) = UnixStream::pair().expect("a socket pair");
        let reply = reply.map(|reply| format!("{reply}\n"));

        let niri = std::thread::spawn(move || {
            let mut request = String::new();
            BufReader::new(&theirs)
                .read_line(&mut request)
                .expect("the request");
            std::thread::sleep(delay);

            if let Some(reply) = reply {
                // the asker may have stopped listening
                let _ = (&theirs).write_all(reply.as_bytes());
            }
            request
        });

        let exchange = exchange(ours, r#"{"Action":{}}"#, Duration::from_millis(50))
            .expect("the request was sent");

        assert_eq!(niri.join().expect("niri"), "{\"Action\":{}}\n");
        exchange
    }

    #[test]
    fn an_answer_is_niris_reply_or_refusal() {
        assert!(matches!(
            exchanged(Some(r#"{"Ok":"Handled"}"#), Duration::ZERO),
            Exchange::Answered(answer) if answer.as_str() == Some("Handled")
        ));
        assert!(matches!(
            exchanged(Some(r#"{"Err":"no window"}"#), Duration::ZERO),
            Exchange::Refused(error) if error.to_string() == "no window"
        ));
    }

    // niri may carry out a request it answers late, never or garbled, so that is no refusal
    #[test]
    fn a_late_missing_or_garbled_answer_is_unclear() {
        assert!(matches!(
            exchanged(Some(r#"{"Ok":"Handled"}"#), Duration::from_millis(300)),
            Exchange::Unclear(error) if error.to_string() == "did not answer within 50ms"
        ));
        assert!(matches!(
            exchanged(None, Duration::ZERO),
            Exchange::Unclear(error) if error.to_string() == "closed without answering"
        ));
        assert!(matches!(
            exchanged(Some(r#"{"Err":7}"#), Duration::ZERO),
            Exchange::Unclear(error) if error.to_string() == r#"answered unexpectedly: {"Err":7}"#
        ));
        assert!(matches!(
            exchanged(Some("garbled"), Duration::ZERO),
            Exchange::Unclear(error) if error.to_string() == "answered unexpectedly: garbled"
        ));
    }
}
