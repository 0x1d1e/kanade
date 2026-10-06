//! Microphone and camera privacy (plan 5.1, 7, #34): one Persistent Critical Privacy Activity
//! while any app captures from a microphone or a camera, green, saying which and who. It withdraws
//! when the last capture stops.
//!
//! PipeWire is the only place that knows: the portal has no signal for a microphone, and apps
//! outside a sandbox skip it. `pw-dump --monitor` prints PipeWire's graph, then every object that
//! changes, so this blocks on its output and wakes only when the graph does. An app capturing is a
//! stream node with an active link from a source node; a link still negotiating or paused moves
//! nothing. A camera an app opens directly (`/dev/video*`) bypasses PipeWire and is not seen.

use std::collections::{BTreeSet, HashMap};
use std::io::{self, BufRead};
use std::time::Instant;

use amane::Service;

use super::json::Json;
use super::wake;
use crate::island::activity::{Activity, Detail, Id, Kind, Priority, Sensors};
use crate::island::service::IslandService;

const NODE: &str = "PipeWire:Interface:Node";
const LINK: &str = "PipeWire:Interface:Link";

// a node as far as capture goes
#[derive(Debug)]
struct Node {
    // like Audio/Source or Stream/Input/Audio
    class: String,

    // the app it belongs to, or the node's own name when it gives none
    app: Option<String>,

    // a level meter like pavucontrol's, which reads a source without capturing it
    monitor: bool,
}

impl Node {
    fn microphone(&self) -> bool {
        self.class == "Audio/Source" || self.class.starts_with("Audio/Source/")
    }

    fn camera(&self) -> bool {
        self.class == "Video/Source" || self.class.starts_with("Video/Source/")
    }

    fn captures(&self) -> bool {
        self.class.starts_with("Stream/Input/") && !self.monitor
    }
}

// PipeWire's graph, as far as nodes and the links between them go
#[derive(Debug, Default)]
struct Graph {
    nodes: HashMap<u64, Node>,

    // output node to input node, by link id, of the links media flows over; a link per channel, so
    // one capture has several
    links: HashMap<u64, (u64, u64)>,
}

impl Graph {
    // one object as pw-dump prints it; without info it was removed
    fn apply(&mut self, object: &Json) {
        let Some(id) = object.get("id").and_then(Json::as_u64) else {
            return;
        };

        // ids are unique across types, and the removal names none
        self.nodes.remove(&id);
        self.links.remove(&id);

        let Some(info) = object.get("info").filter(|info| **info != Json::Null) else {
            return;
        };

        match object.get("type").and_then(Json::as_str) {
            Some(NODE) => {
                let props = info.get("props");
                let text = |key| props?.get(key)?.as_str().map(String::from);

                if let Some(class) = text("media.class") {
                    let monitor = props.and_then(|props| props.get("stream.monitor"));

                    self.nodes.insert(
                        id,
                        Node {
                            class,
                            app: text("application.name").or_else(|| text("node.name")),
                            monitor: monitor.and_then(Json::as_bool) == Some(true),
                        },
                    );
                }
            }
            // a link is negotiating before it is active and paused while its nodes stop, both
            // moving nothing
            Some(LINK) if info.get("state").and_then(Json::as_str) == Some("active") => {
                let node = |key| info.get(key).and_then(Json::as_u64);

                if let (Some(output), Some(input)) = (node("output-node-id"), node("input-node-id"))
                {
                    self.links.insert(id, (output, input));
                }
            }
            _ => {}
        }
    }

    // none while nothing captures
    fn sensors(&self) -> Option<Sensors> {
        let mut microphone = false;
        let mut camera = false;
        let mut apps = BTreeSet::new();

        for (output, input) in self.links.values() {
            let (Some(source), Some(stream)) = (self.nodes.get(output), self.nodes.get(input))
            else {
                continue;
            };

            if !stream.captures() || !(source.microphone() || source.camera()) {
                continue;
            }

            microphone |= source.microphone();
            camera |= source.camera();
            apps.extend(stream.app.clone());
        }

        (microphone || camera).then(|| Sensors {
            microphone,
            camera,
            apps: apps.into_iter().collect(),
        })
    }
}

// Critical, so it shows over everything below it and becomes a Satellite beside another Critical
fn activity(sensors: Sensors) -> Activity {
    Activity::persistent(id(), Priority::Critical).with_detail(Detail::Privacy(sensors))
}

// one, whichever sensors and apps, so a camera joining the microphone replaces it
fn id() -> Id {
    Id::new(Kind::Privacy, "privacy")
}

// runs on its own thread for good; without pw-dump there is no privacy indicator
pub fn follow() {
    let error = wake::run("pw-dump", &["--monitor", "--no-colors"], |output| {
        let mut shown = None;
        let lost = watch(output, &mut shown, &mut post);

        // nobody can say any more whether something captures
        if shown.is_some() {
            post(None);
        }

        lost
    });

    eprintln!("kanade: cannot run pw-dump ({error}), no microphone or camera indicator");
}

/*
 * follows pw-dump until its output ends, posting what changes the Sensors, so the graph changing
 * otherwise wakes nothing; returns why it ended. Each print is a JSON array whose closing bracket
 * alone on a line ends it, the objects inside being indented
 */
fn watch(
    lines: impl BufRead,
    shown: &mut Option<Sensors>,
    post: &mut impl FnMut(Option<&Sensors>),
) -> io::Error {
    let mut graph = Graph::default();
    let mut print = String::new();

    for line in lines.lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => return error,
        };

        print.push_str(&line);
        print.push('\n');

        if line != "]" {
            continue;
        }

        // a print this reader cannot follow is skipped, the next one still counts
        let Some(objects) = Json::parse(&print) else {
            eprintln!("kanade: skipped a pw-dump print that is not JSON");
            print.clear();
            continue;
        };

        print.clear();

        for object in objects.as_array().unwrap_or_default() {
            graph.apply(object);
        }

        let sensors = graph.sensors();

        if sensors != *shown {
            post(sensors.as_ref());
            *shown = sensors;
        }
    }

    io::ErrorKind::UnexpectedEof.into()
}

fn post(sensors: Option<&Sensors>) {
    let now = Instant::now();
    let mut island = IslandService::write();

    match sensors {
        Some(sensors) => island.post(activity(sensors.clone()), now),
        None => island.withdraw(&id(), now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MICROPHONE: u64 = 56;
    const CAMERA: u64 = 58;
    const SPEAKER: u64 = 55;

    fn node(id: u64, class: &str, app: Option<&str>, monitor: bool) -> String {
        let app = app.map_or_else(String::new, |app| {
            format!(r#""application.name": "{app}","#)
        });

        format!(
            r#"  {{
    "id": {id},
    "type": "{NODE}",
    "info": {{
      "state": "running",
      "props": {{
        {app}
        "node.name": "node{id}",
        "media.class": "{class}",
        "stream.monitor": {monitor}
      }}
    }}
  }}"#
        )
    }

    fn link(id: u64, output: u64, input: u64) -> String {
        link_in(id, output, input, "active")
    }

    fn link_in(id: u64, output: u64, input: u64, state: &str) -> String {
        format!(
            r#"  {{
    "id": {id},
    "type": "{LINK}",
    "info": {{
      "output-node-id": {output},
      "input-node-id": {input},
      "state": "{state}"
    }}
  }}"#
        )
    }

    fn removed(id: u64) -> String {
        format!("  {{\n    \"id\": {id},\n    \"info\": null\n  }}")
    }

    fn print(objects: &[String]) -> String {
        format!("[\n{}\n]\n", objects.join(",\n"))
    }

    // the devices of this machine, nothing capturing
    fn devices() -> String {
        print(&[
            node(SPEAKER, "Audio/Sink", None, false),
            node(MICROPHONE, "Audio/Source", None, false),
            node(CAMERA, "Video/Source", None, false),
        ])
    }

    fn capture(stream: u64, class: &str, app: &str, source: u64) -> String {
        print(&[
            node(stream, &format!("Stream/Input/{class}"), Some(app), false),
            link(stream + 100, source, stream),
            link(stream + 200, source, stream),
        ])
    }

    // what each print posted, and what shows at the end
    fn posts(prints: &[String]) -> Vec<Option<Sensors>> {
        let mut posts = Vec::new();
        let mut shown = None;

        let text = prints.concat();
        let lost = watch(text.as_bytes(), &mut shown, &mut |sensors| {
            posts.push(sensors.cloned())
        });

        assert_eq!(lost.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(posts.last().cloned().flatten(), shown);

        posts
    }

    fn sensors(microphone: bool, camera: bool, apps: &[&str]) -> Option<Sensors> {
        Some(Sensors {
            microphone,
            camera,
            apps: apps.iter().map(|app| app.to_string()).collect(),
        })
    }

    #[test]
    fn devices_alone_post_nothing() {
        assert_eq!(posts(&[devices()]), vec![]);
    }

    #[test]
    fn an_app_capturing_the_microphone_posts_until_it_stops() {
        let posts = posts(&[
            devices(),
            capture(74, "Audio", "Firefox", MICROPHONE),
            print(&[removed(174), removed(274)]),
            print(&[removed(74)]),
        ]);

        assert_eq!(posts, vec![sensors(true, false, &["Firefox"]), None]);
    }

    #[test]
    fn a_camera_joining_reposts_with_both_and_every_app() {
        let posts = posts(&[
            devices(),
            capture(74, "Audio", "Firefox", MICROPHONE),
            capture(80, "Video", "Chromium", CAMERA),
        ]);

        assert_eq!(
            posts,
            vec![
                sensors(true, false, &["Firefox"]),
                sensors(true, true, &["Chromium", "Firefox"]),
            ]
        );
    }

    // its node comes before its links, so nothing captures until they do
    #[test]
    fn a_stream_without_links_captures_nothing() {
        let posts = posts(&[
            devices(),
            print(&[node(74, "Stream/Input/Audio", Some("pw-record"), false)]),
        ]);

        assert_eq!(posts, vec![]);
    }

    #[test]
    fn level_meters_and_desktop_audio_capture_no_sensor() {
        let posts = posts(&[
            devices(),
            print(&[
                node(74, "Stream/Input/Audio", Some("pavucontrol"), true),
                link(174, MICROPHONE, 74),
                node(75, "Stream/Input/Audio", Some("OBS"), false),
                link(175, SPEAKER, 75),
            ]),
        ]);

        assert_eq!(posts, vec![]);
    }

    #[test]
    fn a_virtual_microphone_counts_and_a_stream_without_app_gives_its_node_name() {
        let posts = posts(&[
            print(&[node(60, "Audio/Source/Virtual", None, false)]),
            print(&[
                node(74, "Stream/Input/Audio", None, false),
                link(174, 60, 74),
            ]),
        ]);

        assert_eq!(posts, vec![sensors(true, false, &["node74"])]);
    }

    #[test]
    fn the_same_app_twice_is_named_once() {
        let posts = posts(&[
            devices(),
            print(&[
                node(74, "Stream/Input/Audio", Some("Firefox"), false),
                link(174, MICROPHONE, 74),
                node(75, "Stream/Input/Audio", Some("Firefox"), false),
                link(175, MICROPHONE, 75),
            ]),
        ]);

        assert_eq!(posts, vec![sensors(true, false, &["Firefox"])]);
    }

    // the stream's state and links change while it captures, the island hears nothing of it
    #[test]
    fn changes_that_keep_the_sensors_post_nothing_more() {
        let posts = posts(&[
            devices(),
            capture(74, "Audio", "Firefox", MICROPHONE),
            capture(74, "Audio", "Firefox", MICROPHONE),
            print(&[removed(174)]),
        ]);

        assert_eq!(posts, vec![sensors(true, false, &["Firefox"])]);
    }

    #[test]
    fn a_print_that_is_not_json_is_skipped() {
        let posts = posts(&[
            devices(),
            "[\n  {\n]\n".into(),
            capture(74, "Audio", "Firefox", MICROPHONE),
        ]);

        assert_eq!(posts, vec![sensors(true, false, &["Firefox"])]);
    }

    // links as PipeWire makes them for a capture, then as the app pauses and resumes it
    fn capture_in(state: &str) -> String {
        print(&[
            link_in(174, MICROPHONE, 74, state),
            link_in(274, MICROPHONE, 74, state),
        ])
    }

    fn stream() -> String {
        print(&[node(74, "Stream/Input/Audio", Some("Firefox"), false)])
    }

    #[test]
    fn only_active_links_capture() {
        for state in [
            "init",
            "negotiating",
            "allocating",
            "paused",
            "error",
            "unlinked",
        ] {
            let posts = posts(&[devices(), stream(), capture_in(state)]);

            assert_eq!(posts, vec![], "{state}");
        }
    }

    #[test]
    fn a_capture_shows_while_active() {
        let posts = posts(&[
            devices(),
            stream(),
            capture_in("negotiating"),
            capture_in("active"),
            capture_in("paused"),
            capture_in("active"),
        ]);

        let firefox = sensors(true, false, &["Firefox"]);
        assert_eq!(posts, vec![firefox.clone(), None, firefox]);
    }

    #[test]
    fn a_capture_shows_while_one_of_its_links_is_active() {
        let posts = posts(&[
            devices(),
            stream(),
            capture_in("active"),
            print(&[link_in(174, MICROPHONE, 74, "paused")]),
        ]);

        assert_eq!(posts, vec![sensors(true, false, &["Firefox"])]);
    }

    #[test]
    fn the_activity_is_one_critical_persistent_privacy() {
        let activity = activity(sensors(true, false, &[]).unwrap());

        assert_eq!(activity.id(), &id());
        assert_eq!(activity.priority(), Priority::Critical);
        assert_eq!(
            activity.lifetime(),
            crate::island::activity::Lifetime::Persistent
        );
    }
}
