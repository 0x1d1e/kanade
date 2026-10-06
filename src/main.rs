#[cfg(test)]
mod boundary;
mod clock;
mod config;
mod icon;
mod ipc;
mod island;
mod modules;
mod raster;
mod shadow;
mod sources;
mod surfaces;
mod theme;
mod view;

use amane::App;

fn main() {
    modules::start(App::new()).run();
}
