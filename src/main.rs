#[cfg(test)]
mod boundary;
mod cli;
mod clock;
mod cluster;
mod config;
mod icon;
mod ipc;
mod island;
mod modules;
mod raster;
mod reload;
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

    if !arguments.is_empty() {
        return cli::run(&arguments);
    }

    modules::start(App::new()).run();
    ExitCode::SUCCESS
}
