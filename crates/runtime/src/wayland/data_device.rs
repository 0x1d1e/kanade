use smithay_client_toolkit::data_device_manager::{
    data_device::DataDeviceHandler,
    data_offer::{DataOfferHandler, DragOffer},
};
use wayland_client::{
    Connection, QueueHandle,
    protocol::{
        wl_data_device::WlDataDevice, wl_data_device_manager::DndAction, wl_surface::WlSurface,
    },
};

use crate::input::paste::Paste;

use super::WaylandState;

// the only text the runtime pastes: what the other program says it is, and so how to read it
const TEXT: &str = "text/plain;charset=utf-8";

// what one paste may hold; a text input takes a line, not a file
const PASTE_LIMIT: usize = 64 * 1024;

impl DataDeviceHandler for WaylandState {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataDevice,
        _: f64,
        _: f64,
        _: &WlSurface,
    ) {
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}

    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64) {}

    // the selection is looked at when a paste asks for it
    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}

    fn drop_performed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
}

// the runtime takes nothing from a drag but what the handler above looks at
impl DataOfferHandler for WaylandState {
    fn source_actions(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }

    fn selected_action(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }
}

impl WaylandState {
    // reads the selection as text, and runs `done` with it, nothing is pasted when it is not text
    pub(super) fn paste(&mut self, done: Paste, surface: &WlSurface) {
        let Some(offer) = self
            .data_device
            .as_ref()
            .and_then(|device| device.data().selection_offer())
        else {
            return;
        };

        if !offer.with_mime_types(|types| types.iter().any(|kind| kind == TEXT)) {
            return;
        }

        let Ok(pipe) = offer.receive(String::from(TEXT)) else {
            return;
        };

        // the request has to reach the other program before it can write anything
        let _ = self.connection.flush();

        let surface = surface.clone();

        self.receive(
            pipe,
            PASTE_LIMIT,
            Box::new(move |state, bytes| {
                // a source's bytes are not trusted to be text
                let Some(text) = bytes.and_then(|bytes| String::from_utf8(bytes).ok()) else {
                    return;
                };

                done(text);

                state.request_frame_on(&surface);
            }),
        );
    }
}
