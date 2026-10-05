//! Kanade's own niri EventStream (plan 3, 5.3): Amane's niri backend is private and tracks neither
//! the overview nor casts. Hands the core a plain focused output and whether the overview is open.

use std::collections::HashMap;
use std::env;
use std::io::{self, BufRead, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::time::Instant;

use amane::Service;

use super::json::Json;
use crate::island::service::IslandService;

// what the core gets from niri; the default is also what a lost socket degrades to
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    // none while unknown, which counts every monitor as focused
    pub focused_output: Option<String>,

    pub overview: bool,
}

// niri's events only say what changed, so the rest is remembered here
#[derive(Debug, Default)]
struct Niri {
    // every workspace by id, and the output it is on, if any
    outputs: HashMap<u64, Option<String>>,

    focused: Option<u64>,

    overview: bool,
}

impl Niri {
    // anything niri sends that is not about focus or the overview changes nothing
    fn apply(&mut self, event: &Json) {
        if let Some(changed) = event.get("WorkspacesChanged") {
            let workspaces = changed.get("workspaces").and_then(Json::as_array);

            self.outputs.clear();
            self.focused = None;

            for workspace in workspaces.unwrap_or_default() {
                let Some(id) = workspace.get("id").and_then(Json::as_u64) else {
                    continue;
                };

                let output = workspace.get("output").and_then(Json::as_str);

                self.outputs.insert(id, output.map(String::from));

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
        }
    }

    fn seen(&self) -> Seen {
        Seen {
            focused_output: self
                .focused
                .and_then(|id| self.outputs.get(&id).cloned().flatten()),
            overview: self.overview,
        }
    }
}

// runs on its own thread for good; without niri, or once its socket is lost, every monitor is focused
pub fn follow() {
    run(connect().map(BufReader::new), post);
}

fn run(stream: io::Result<impl BufRead>, mut post: impl FnMut(&Seen)) {
    let mut posted = Seen::default();

    let lost = match stream {
        Ok(stream) => watch(stream, &mut posted, &mut post),
        Err(error) => error,
    };

    // plan 5.3: logged, no toast
    eprintln!("kanade: niri event stream lost ({lost}), every monitor counts as focused");

    if posted != Seen::default() {
        post(&Seen::default());
    }
}

fn connect() -> io::Result<UnixStream> {
    let path = env::var("NIRI_SOCKET").map_err(|_| io::Error::other("NIRI_SOCKET is not set"))?;

    let mut stream = UnixStream::connect(path)?;

    stream.write_all(b"\"EventStream\"\n")?;

    Ok(stream)
}

/*
 * follows niri until the stream ends, posting only what changes, so workspace switches on one
 * output and the stream of window events wake nothing; returns why it ended
 */
fn watch(lines: impl BufRead, posted: &mut Seen, post: &mut impl FnMut(&Seen)) -> io::Error {
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

        niri.apply(&event);

        let seen = niri.seen();

        if seen != *posted {
            post(&seen);
            *posted = seen;
        }
    }

    io::ErrorKind::UnexpectedEof.into()
}

fn post(seen: &Seen) {
    let mut island = IslandService::write();

    island.set_focused_output(seen.focused_output.clone());
    island.set_overview(seen.overview, Instant::now());
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
        let lost = watch(text.as_bytes(), &mut posted, &mut |seen| {
            posts.push(seen.clone())
        });

        assert_eq!(posts.last().cloned().unwrap_or_default(), posted);

        (posts, lost.kind())
    }

    fn focused(output: &str) -> Seen {
        Seen {
            focused_output: Some(output.into()),
            overview: false,
        }
    }

    #[test]
    fn focus_follows_the_focused_workspace_across_outputs() {
        let (posts, _) = posts(&[OK, WORKSPACES, &activated(3, true), &activated(1, true)]);

        assert_eq!(
            posts,
            [focused("eDP-1"), focused("HDMI-A-1"), focused("eDP-1")]
        );
    }

    #[test]
    fn switching_workspaces_on_one_output_posts_nothing() {
        let (posts, _) = posts(&[OK, WORKSPACES, &activated(2, true), &activated(1, true)]);

        assert_eq!(posts, [focused("eDP-1")]);
    }

    // activated without focus only changed what an output shows
    #[test]
    fn unfocused_activation_keeps_the_focus() {
        let (posts, _) = posts(&[OK, WORKSPACES, &activated(3, false)]);

        assert_eq!(posts, [focused("eDP-1")]);
    }

    #[test]
    fn workspace_moved_to_another_output_takes_the_focus_along() {
        let moved = WORKSPACES.replacen("\"eDP-1\"", "\"HDMI-A-1\"", 1);

        let (posts, _) = posts(&[OK, WORKSPACES, &moved]);

        assert_eq!(posts, [focused("eDP-1"), focused("HDMI-A-1")]);
    }

    // a fresh list replaces the old one whole: gone workspaces and a focus nobody has are unknown
    #[test]
    fn a_new_workspace_list_forgets_the_old_one() {
        let unfocused = WORKSPACES.replace("\"is_focused\":true", "\"is_focused\":false");
        let without_web =
            r#"{"WorkspacesChanged":{"workspaces":[{"id":1,"output":"eDP-1","is_focused":true}]}}"#;

        assert_eq!(
            posts(&[OK, WORKSPACES, &unfocused]).0,
            [focused("eDP-1"), Seen::default()]
        );
        assert_eq!(
            posts(&[OK, WORKSPACES, without_web, &activated(3, true)]).0,
            [focused("eDP-1"), Seen::default()]
        );
    }

    #[test]
    fn overview_opens_and_closes() {
        let (posts, _) = posts(&[OK, WORKSPACES, &overview(true), &overview(false)]);

        let open = Seen {
            overview: true,
            ..focused("eDP-1")
        };

        assert_eq!(posts, [focused("eDP-1"), open, focused("eDP-1")]);
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
        ]);

        // workspace 9 is unknown, so its output is too, which is the default
        assert_eq!(posts, []);
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
        let text = [OK, WORKSPACES, &overview(true)].join("\n");
        let mut posts = Vec::new();

        run(Ok(text.as_bytes()), |seen| posts.push(seen.clone()));

        assert_eq!(posts.last(), Some(&Seen::default()));
        assert_eq!(posts.len(), 3);

        // nothing was posted, so nothing has to be taken back
        let mut posts = Vec::new();

        run(Err::<&[u8], _>(io::ErrorKind::NotFound.into()), |seen| {
            posts.push(seen.clone())
        });
        run(Ok(OK.as_bytes()), |seen| posts.push(seen.clone()));

        assert_eq!(posts, []);
    }
}
