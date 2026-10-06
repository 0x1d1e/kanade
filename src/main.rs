#[cfg(test)]
mod boundary;
mod config;
mod icon;
mod ipc;
mod island;
mod sources;
mod surfaces;
mod theme;
mod view;

use std::thread;

use amane::{App, Apps, Service};

fn main() {
    let config = config::get();

    // before anything reads the island, which takes its timings once
    island::service::configure(config.island);
    theme::follow(config.palette.as_deref());

    thread::spawn(sources::niri::follow);
    thread::spawn(sources::battery::follow);
    thread::spawn(sources::media::follow);
    thread::spawn(sources::osd::follow);
    thread::spawn(sources::notifications::follow);
    thread::spawn(sources::system::follow);
    thread::spawn(sources::privacy::follow);
    sources::timer::spawn();

    // the first read starts Amane's app scan, which takes seconds, so the Launcher opens on a list
    thread::spawn(|| drop(Apps::read()));

    App::new()
        .window_per_monitor(view::island)
        .ipc("island", ipc::island)
        .run();
}
