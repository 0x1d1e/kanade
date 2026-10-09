//! The shell's side of `kanade <verb> [args]` (`crate::cli`), behind the one IPC handler.
//! Runs on the draw thread; `island` only posts.

use std::time::Instant;

use amane::{Notifications, Service};

use crate::cli::{self, Call, Reply};
use crate::dock;
use crate::island::command::{Command, Unparsed};
use crate::island::presentation::Surface;
use crate::island::service::{Effect, IslandService};
use crate::lock;
use crate::modules;
use crate::reload::{self, Outcome};
use crate::settings;
use crate::sources::capture::{self, Asked};
use crate::sources::clipboard::{self, Clipboard};
use crate::sources::notifications::{self, Daemon};
use crate::sources::osd;
use crate::sources::recording;
use crate::sources::timer;
use crate::sources::tray::Tray;
use crate::sources::windows::Windows;
use crate::sources::{apps, caffeine, google, session, sleep, wallpaper, weather};
use crate::supervise;

pub fn answer(arguments: &[String]) -> String {
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();

    let reply = match cli::parse(&words) {
        // a client from another Kanade build may send what this one does not know
        Err(Unparsed::Usage) => Reply::Refused(cli::usage()),
        Err(Unparsed::Invalid(invalid)) => Reply::Refused(invalid.to_string()),
        Ok((module, _)) if !modules::on(module.name) => {
            Reply::Refused(format!("module {} is off", module.name))
        }
        Ok((_, call)) => run(call),
    };

    reply.encode()
}

fn run(call: Call) -> Reply {
    match call {
        Call::Island(command) => island(command),

        // handed to the timer's thread, which posts its Activity itself
        Call::Timer(request) => {
            timer::request(request);
            Reply::Done(String::new())
        }
        Call::ClearNotifications => {
            Notifications::clear();
            Reply::Done(String::new())
        }
        Call::ClearClipboard => {
            clipboard::clear();
            Reply::Done(String::new())
        }
        // niri answers at once; an area is saved later, once the user picks it
        Call::Screenshot(mode) => match capture::screenshot(mode) {
            Ok(Asked::Taken(path)) => Reply::Done(path),
            Ok(Asked::Unknown(why)) => Reply::Unknown(why),
            Err(error) => Reply::Refused(error),
        },
        // on the draw thread, which the recorder ends with (`recording::start`); `kanade` waits on it
        Call::Record(request) => record(request),
        // on the draw thread, which systemd-inhibit dies with (`caffeine::request`)
        Call::Caffeine(request) => {
            caffeine::request(request).map_or_else(Reply::Refused, Reply::Done)
        }
        // handed to the wallpaper's thread, as awww takes a while; `kanade` waits on it
        Call::Wallpaper(request) => {
            wallpaper::request(request).map_or_else(Reply::Refused, Reply::Done)
        }
        // the sign-in and the sync are on threads of their own, so this answers at once
        Call::Google(request) => google::request(request).map_or_else(Reply::Refused, Reply::Done),
        // the fetch is on the weather's thread, so this answers at once
        Call::Weather(request) => {
            weather::request(request).map_or_else(Reply::Refused, Reply::Done)
        }
        // on the draw thread, which the password field and the lock screen live on; returns before
        // niri locks, which the client waits for (`lock status`)
        Call::Lock => match lock::start() {
            Ok(lock::Started::Requested(text)) => Reply::Done(text),
            Ok(lock::Started::Unknown(why)) => Reply::Unknown(why),
            Err(error) => Reply::Refused(error),
        },
        Call::LockStatus => Reply::Done(lock::status()),
        // on the draw thread, which the lock screen lives on; logind is asked on a thread of its own
        Call::Session(leave) => session::request(leave).map_or_else(Reply::Refused, Reply::Done),
        Call::Osd(asked) => osd::show(asked, modules::osd_reads())
            .map_or_else(Reply::Refused, |()| Reply::Done(String::new())),
        // on the draw thread, which the window's text inputs live on
        Call::Settings(page) => {
            settings::open(page);
            Reply::Done(String::new())
        }
        Call::CloseSettings => {
            settings::close();
            Reply::Done(String::new())
        }
        Call::Reload => config(reload::reload(), "reloaded"),
        Call::Validate => config(reload::validate(), "valid"),

        Call::Status => Reply::Done(status().join("\n")),
        Call::Modules => Reply::Done(modules::list().join("\n")),
        // through the settings file's writes, which the Settings window shares
        Call::Turn(name, on) => modules::turn(name, on).map_or_else(Reply::Refused, Reply::Done),
    }
}

// answered at once, as niri gives the recorder no frame while the draw thread waits
fn record(request: recording::Request) -> Reply {
    let reply = match request {
        recording::Request::Start => recording::start(),
        recording::Request::Stop => recording::stop(),
        recording::Request::Status => Ok(recording::status().to_string()),
    };

    reply.map_or_else(Reply::Refused, Reply::Done)
}

// the versions, then the config, the Modules and what went wrong with the sources, a line each
fn status() -> Vec<String> {
    let mut lines = vec![format!(
        "kanade {}, protocol {}, pid {}",
        env!("CARGO_PKG_VERSION"),
        cli::PROTOCOL,
        std::process::id()
    )];

    lines.extend(reload::status());
    lines.extend(modules::status());

    // a Module that is off reads no Service
    if modules::on("notifications") {
        lines.push(Daemon::read().status());
    }
    if modules::on("tray") {
        lines.push(Tray::read().status());
    }
    if modules::on("clipboard") {
        lines.push(Clipboard::read().status());
    }
    if modules::on("windows") {
        lines.push(Windows::read().status());
    }
    if modules::on("dock") {
        lines.push(dock::status());
    }
    if modules::on("lock") {
        lines.push(sleep::status());
    }
    if modules::on("wallpaper") {
        lines.push(wallpaper::status());
    }
    if modules::on("google-calendar") {
        lines.push(google::status());
    }
    if modules::on("weather") {
        lines.push(weather::status());
    }

    lines.extend(supervise::status());
    lines
}

fn island(command: Command) -> Reply {
    // the Weather Surface opens only from a verb; one asked to open wants a forecast that is not
    // old, as after a suspend (`weather::opened`). A toggle that closes it asks too, harmlessly,
    // as only an old forecast is fetched. Another way to open it must ask as well
    if let Command::Open(Surface::Weather) | Command::Toggle(Surface::Weather) = &command {
        weather::opened();
    }

    // its own statement, so the read is released before the write; a write wakes every window
    // even when nothing changed, so only a real change writes
    let effect = IslandService::read().resolve(command);

    match effect {
        Ok(Some(Effect::Dnd(dnd))) => notifications::set_dnd(dnd, Instant::now()),
        Ok(Some(effect)) => {
            // an app installed since the last opening shows in this one (`apps::refresh`); another
            // way to open the Launcher must refresh as well
            if let Effect::Open(_, Surface::Launcher) = &effect {
                apps::refresh();
            }

            IslandService::write().apply(effect, Instant::now());
        }
        Ok(None) => {}
        Err(error) => return Reply::Refused(error.to_string()),
    }

    Reply::Done(String::new())
}

fn config(outcome: Outcome, valid: &str) -> Reply {
    match outcome {
        Outcome::Valid(pending) => Reply::Done(
            std::iter::once(String::from(valid))
                .chain(
                    pending
                        .iter()
                        .map(|key| format!("{key} is pending restart")),
                )
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        Outcome::Invalid(problems) => Reply::Refused(format!(
            "invalid, the config in effect stays:\n{}",
            problems.join("\n")
        )),
    }
}
