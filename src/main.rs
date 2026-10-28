mod appearance;
mod atomic;
mod autohide;
mod backoff;
mod banners;
#[cfg(test)]
mod boundary;
mod bus;
mod cli;
mod clock;
mod cluster;
mod config;
mod desk;
mod dock;
mod doctor;
mod face;
mod gesture;
mod hooks;
mod icon;
mod idle;
mod ipc;
mod island;
mod later;
mod lock;
mod look;
mod merge;
mod modules;
mod niri_config;
mod pane;
mod raster;
mod reload;
mod scene;
mod sddm;
mod settings;
mod shadow;
mod sources;
mod supervise;
mod surfaces;
mod theme;
mod tiles;
mod ui;
mod update;
mod view;

use std::env;
use std::process::ExitCode;

use kanade_runtime::App;

// with no verb, the shell, so its user unit (`lock::UNIT`) starts it; else a verb for it
fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();

    if arguments == [sources::clipboard::HAND_OVER] {
        sources::clipboard::hand_over_selection();
        return ExitCode::SUCCESS;
    }

    // a browser starts its native messaging host with the manifest's path and the extension
    if arguments
        .iter()
        .any(|argument| argument == appearance::pywalfox::EXTENSION)
    {
        return appearance::pywalfox::host();
    }

    if !arguments.is_empty() {
        return cli::run(&arguments);
    }

    kanade_runtime::limit_allocator();

    // the first window opens at it; a reload changes it live
    kanade_runtime::set_ui_scale(config::get().appearance.ui_scale());

    supervise::follow_services();
    settings::recover_outputs();

    /*
     * `systemctl --user restart` or Ctrl+C mid-trial puts a monitor back before the shell ends, and a
     * pin made a moment before the signal reaches its file first: the signal ends the process where
     * it is, so the tail below never runs. The flush goes first, as it is bounded by its own limit
     * (`FLUSHED`) inside the terminate action's, and the revert may take most of that
     */
    let terminate = || {
        sources::clipboard::pins::flush();
        settings::revert_outputs();
    };

    if let Err(why) = clock::on_terminate(terminate) {
        eprintln!("kanade: SIGTERM will not put outputs back: {why}");
    }
    modules::start(theme::font(App::new()))
        .on_key_release(surfaces::confirm::release)
        .run();

    // a cast niri holds on Kanade's behalf ends with it, rather than with the next start's reclaim
    sources::recording::shutdown();

    // the save worker is detached, so a pin made a moment before the shell ends waits here for its
    // file, before the revert that may take most of the wait
    sources::clipboard::pins::flush();
    settings::revert_outputs();

    ExitCode::SUCCESS
}
