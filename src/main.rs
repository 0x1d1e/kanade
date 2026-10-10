mod autohide;
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
mod glass;
mod icon;
mod ipc;
mod island;
mod lock;
mod look;
mod merge;
mod modules;
mod raster;
mod reload;
mod scene;
mod settings;
mod shadow;
mod sources;
mod supervise;
mod surfaces;
mod theme;
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

    if !arguments.is_empty() {
        return cli::run(&arguments);
    }

    modules::start(theme::font(App::new())).run();
    ExitCode::SUCCESS
}
