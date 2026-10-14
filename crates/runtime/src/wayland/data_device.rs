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

use crate::input::dropped;
use crate::input::paste::Paste;

use super::WaylandState;

// the only text the runtime pastes: what the other program says it is, and so how to read it
const TEXT: &str = "text/plain;charset=utf-8";

// what one paste may hold; a text input takes a line, not a file
const PASTE_LIMIT: usize = 64 * 1024;

// file managers offer dragged files as a list of file:// links
const URI_LIST: &str = "text/uri-list";

// what a list of links may hold, well over the most files a drop is allowed to name
const DROP_LIMIT: usize = 64 * 1024;

impl DataDeviceHandler for WaylandState {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataDevice,
        x: f64,
        y: f64,
        surface: &WlSurface,
    ) {
        self.answer_drag(surface, x, y);
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}

    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, x: f64, y: f64) {
        let Some(offer) = self.drag_offer() else {
            return;
        };

        self.answer_drag(&offer.surface, x, y);
    }

    // the selection is looked at when a paste asks for it
    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}

    fn drop_performed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {
        self.take_drop();
    }
}

// the actions are asked for and read back off the offer, not followed here
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
    fn drag_offer(&self) -> Option<DragOffer> {
        self.data_device.as_ref()?.data().drag_offer()
    }

    // says whether a drag can be dropped at this spot: only files, only over a widget taking them
    fn answer_drag(&mut self, surface: &WlSurface, x: f64, y: f64) {
        let Some(offer) = self.drag_offer() else {
            return;
        };

        let files = offer.with_mime_types(|types| types.iter().any(|kind| kind == URI_LIST));

        let wanted = files
            && self
                .window(surface)
                .is_some_and(|window| window.pointer.accepts_drop(x as f32, y as f32));

        // a drag that is turned down shows the pointer that it can't be dropped here
        if wanted {
            offer.accept_mime_type(offer.serial, Some(String::from(URI_LIST)));
            offer.set_actions(DndAction::Copy, DndAction::Copy);
        } else {
            offer.accept_mime_type(offer.serial, None);
            offer.set_actions(DndAction::empty(), DndAction::empty());
        }
    }

    // reads the links on the event loop, and hands the files to the widget under the drop
    fn take_drop(&mut self) {
        let Some(offer) = self.drag_offer() else {
            return;
        };

        // finishing a drop nobody accepted is a protocol error, so only a copy was asked for
        let accepted = offer.selected_action == DndAction::Copy;

        let Some(pipe) = accepted
            .then(|| offer.receive(String::from(URI_LIST)).ok())
            .flatten()
        else {
            offer.destroy();

            return;
        };

        // the request has to reach the other program before it can write anything
        let _ = self.connection.flush();

        self.receive(
            pipe,
            DROP_LIMIT,
            Box::new(move |state, bytes| {
                // a source's bytes are not trusted to be text, nor the text to be links
                let files = bytes
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .map(|list| dropped::parse(&list))
                    .unwrap_or_default();

                if files.is_empty() {
                    offer.destroy();

                    return;
                }

                offer.finish();
                offer.destroy();

                let surface = offer.surface.clone();

                let handled = state.window(&surface).is_some_and(|window| {
                    window
                        .pointer
                        .drop_files(offer.x as f32, offer.y as f32, files)
                });

                if handled {
                    state.request_frame_on(&surface);
                }
            }),
        );
    }

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
