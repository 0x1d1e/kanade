#[cfg(test)]
mod boundary;
mod ipc;
mod island;
mod sources;
mod surfaces;
mod theme;
mod view;

use std::thread;

use amane::App;

fn main() {
    thread::spawn(sources::niri::follow);

    App::new()
        .window_per_monitor(view::island)
        .ipc("island", ipc::island)
        .run();
}
