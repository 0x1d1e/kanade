#[cfg(test)]
mod boundary;
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

use amane::App;

fn main() {
    modules::start(App::new()).run();
}
