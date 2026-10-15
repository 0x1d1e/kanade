//! The shell's side of `kanade <verb> [args]` (`crate::cli`), behind the one IPC handler.
//! Runs on the draw thread; `island` only posts.

use std::time::Instant;

use crate::sources::notifications::Notifications;
use kanade_runtime::IpcReply;
use kanade_runtime::service::Service;

use crate::cli::{self, Call, Reply};
use crate::gesture;
use crate::idle;
use crate::island::command::{Command, Unparsed};
use crate::island::presentation::Surface;
use crate::island::service::{Effect, IslandService};
use crate::later::Later;
use crate::lock;
use crate::modules;
use crate::reload::{self, Outcome};
use crate::settings;
use crate::sources::capture::{self, Asked};
use crate::sources::clipboard;
use crate::sources::niri::Acted;
use crate::sources::notifications::{self, timed};
use crate::sources::osd;
use crate::sources::recording;
use crate::sources::timer;
use crate::sources::{
    apps, bluetooth, brightness, caffeine, charge_limit, focus, fullscreen, google, live, media,
    network, night_light, power, reminders, session, subscriptions, wallpaper, weather,
};
use crate::supervise;
use crate::surfaces::launcher;

pub fn answer(arguments: &[String], reply: IpcReply) {
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();

    let call = match cli::parse(&words) {
        // a client from another Kanade build may send what this one does not know
        Err(Unparsed::Usage) => Err(Reply::Refused(String::from(
            "not a command of this shell; run `kanade help` for what it takes",
        ))),
        Err(Unparsed::Invalid(invalid)) => Err(Reply::Refused(invalid.to_string())),
        Ok((module, _)) if !modules::on(module.name) => {
            Err(Reply::Refused(cli::module_off(module.name)))
        }
        Ok((_, call)) => Ok(call),
    };

    match call {
        // answered once the user chooses or dismisses it, so the client waits on the user
        Ok(Call::Dmenu(asked)) => launcher::choose::choose(asked, reply),
        // answered once the user has picked or left niri's picker, so the client waits on the user
        Ok(Call::PickColor) => capture::pick(reply),
        // answered once the three strokes are drawn or the overlay closes, so the client waits on the user
        Ok(Call::Gesture(gesture::Request::Record(name))) => gesture::record(name, reply),
        // answered when the source is done, or unknown once `LATER` passes (`crate::later`)
        Ok(Call::Record(request)) => recording::request(request, Later::new(reply)),
        // on the draw thread, which systemd-inhibit dies with
        Ok(Call::Caffeine(request)) => caffeine::request(request, Later::new(reply)),
        Ok(Call::Battery(request)) => charge_limit::request(request, Later::new(reply)),
        Ok(Call::Media(transport)) => media::request(transport, Later::new(reply)),
        Ok(Call::Wifi(switch)) => network::request(switch, Later::new(reply)),
        Ok(Call::Bluetooth(switch)) => bluetooth::request(switch, Later::new(reply)),
        Ok(Call::PowerProfile(request)) => power::request(request, Later::new(reply)),
        Ok(Call::Wallpaper(request)) => wallpaper::request(request, Later::new(reply)),
        Ok(Call::Lock) => lock::request(Later::new(reply)),
        Ok(call) => reply.send(&run(call).encode()),
        Err(refused) => reply.send(&refused.encode()),
    }
}

fn run(call: Call) -> Reply {
    match call {
        Call::Island(command) => island(command),

        // handed to the timer's thread, which posts its Activity itself
        Call::Timer(request) => {
            timer::request(request);
            Reply::Done(String::new())
        }
        Call::Live(request) => live::request(request).map_or_else(Reply::Refused, Reply::Done),
        // opens the overlay or answers from the state file; the user's stroke is no answer to this
        Call::Gesture(request) => gesture::run(request),
        // `answer` keeps these waiting
        Call::Dmenu(_)
        | Call::Record(_)
        | Call::Caffeine(_)
        | Call::Battery(_)
        | Call::Media(_)
        | Call::Wifi(_)
        | Call::Bluetooth(_)
        | Call::PowerProfile(_)
        | Call::Wallpaper(_)
        | Call::PickColor
        | Call::Lock => Reply::Refused(String::from("answered later, not by `run`")),
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
        // niri answers at once, but the client may wait on it as on a screenshot
        Call::Cast(to) => match capture::cast(&to) {
            Ok(Acted::Done) => Reply::Done(String::new()),
            Ok(Acted::Unknown(why)) => Reply::Unknown(why),
            Err(error) => Reply::Refused(error),
        },
        // through the settings file's writes, which the Settings window shares
        Call::NightLight(request) => {
            night_light::request(request).map_or_else(Reply::Refused, Reply::Done)
        }
        // through the same: it is the Controls tile's press as well
        // a timed Do Not Disturb ends on a thread of its own, so this answers at once
        Call::Dnd(request) => {
            timed::request(request, Instant::now()).map_or_else(Reply::Refused, Reply::Done)
        }
        Call::Focus(request) => focus::request(request).map_or_else(Reply::Refused, Reply::Done),
        // the sign-in and the sync are on threads of their own, so this answers at once
        Call::Google(request) => google::request(request).map_or_else(Reply::Refused, Reply::Done),
        // handed to the reminders' thread, so this answers at once
        Call::Reminders(request) => {
            reminders::request(request).map_or_else(Reply::Refused, Reply::Done)
        }
        // the fetch is on the weather's thread, so this answers at once
        Call::Weather(request) => {
            weather::request(request).map_or_else(Reply::Refused, Reply::Done)
        }
        // likewise on the subscriptions' thread
        Call::Subscriptions(request) => {
            subscriptions::request(request).map_or_else(Reply::Refused, Reply::Done)
        }
        // the level is set at once; the monitor's own write is on the ddc thread
        Call::Brightness(request) => {
            brightness::request(request).map_or_else(Reply::Refused, Reply::Done)
        }
        Call::LockStatus => Reply::Done(lock::status()),
        Call::IdleStatus => Reply::Done(idle::status()),
        // logind is asked on a thread of its own
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
    lines.extend(modules::reports());
    lines.extend(fullscreen::status());
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
                launcher::windows::refresh();
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
