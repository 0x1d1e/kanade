#[cfg(test)]
mod boundary;
mod island;
mod sources;
mod surfaces;
mod theme;
mod view;

use amane::App;

fn main() {
    App::new()
        .window_per_monitor(view::island)
        .ipc("island", view::ipc)
        .run();
}
