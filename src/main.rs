mod banners;
#[cfg(test)]
mod boundary;
mod cli;
mod clock;
mod cluster;
mod config;
mod dock;
mod doctor;
mod icon;
mod ipc;
mod island;
mod modules;
mod osd;
mod raster;
mod reload;
mod settings;
mod shadow;
mod sources;
mod supervise;
mod surfaces;
mod theme;
mod view;

use std::env;
use std::process::ExitCode;

use amane::App;

// with no verb, the shell, so niri's `spawn-at-startup "kanade"` starts it; else a verb for it
fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();

    if arguments == [sources::clipboard::HAND_OVER] {
        sources::clipboard::hand_over_selection();
        return ExitCode::SUCCESS;
    }

    if !arguments.is_empty() {
        return cli::run(&arguments);
    }

    modules::start(App::new()).run();
    ExitCode::SUCCESS
}
