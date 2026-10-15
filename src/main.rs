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
mod dock;
mod doctor;
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
mod pane;
mod raster;
mod reload;
mod scene;
mod settings;
mod shadow;
mod sources;
mod supervise;
mod surfaces;
mod theme;
mod tiles;
mod ui;
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

    // the first window opens at it; a reload changes it live
    kanade_runtime::set_ui_scale(config::get().appearance.ui_scale());

    supervise::follow_services();
    modules::start(theme::font(App::new()))
        .on_key_release(surfaces::confirm::release)
        .run();
    ExitCode::SUCCESS
}
