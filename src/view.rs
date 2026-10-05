use amane::{Horizontal, Layer, LayerWindow, Monitor, Parent, Rectangle, Service, Vertical, Zone};

use crate::island::service::IslandService;
use crate::theme;

// one island window per monitor, an empty transparent canvas until Phase 1
pub fn island(_monitor: &Monitor) -> LayerWindow {
    // reading subscribes this window to island changes
    let _island = IslandService::read();

    LayerWindow::new()
        .width(theme::CANVAS_WIDTH)
        .height(theme::CANVAS_HEIGHT)
        .anchor_vertical(Vertical::Top)
        .anchor_horizontal(Horizontal::Middle)
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace("kanade")
        .click_through()
        .child(Rectangle::new().width(Parent).height(Parent))
}
