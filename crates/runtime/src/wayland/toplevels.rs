use wayland_client::backend::ObjectId;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::{
    self as handle, ZwlrForeignToplevelHandleV1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::{
    self as manager, ZwlrForeignToplevelManagerV1,
};

use crate::toplevels::{self, FullscreenWindow};

use super::WaylandState;

// a window as the compositor last said, once its `done` closed the batch
#[derive(Debug, Default, Clone)]
pub struct Toplevel {
    fullscreen: bool,
    app_id: Option<String>,
    outputs: Vec<ObjectId>,

    // said since the last `done`
    pending: Pending,
}

#[derive(Debug, Default, Clone)]
struct Pending {
    fullscreen: Option<bool>,
    app_id: Option<String>,
    outputs: Option<Vec<ObjectId>>,
}

impl Toplevel {
    // as fullscreen on each output it is on that has a name
    fn on<'a>(
        &'a self,
        names: &'a [(ObjectId, String)],
    ) -> impl Iterator<Item = FullscreenWindow> + 'a {
        self.outputs
            .iter()
            .filter(|_| self.fullscreen)
            .filter_map(|id| names.iter().find(|(named, _)| named == id))
            .map(|(_, output)| FullscreenWindow {
                output: output.clone(),
                app_id: self.app_id.clone(),
            })
    }

    // the outputs as said since the last `done`
    fn entering(&mut self) -> &mut Vec<ObjectId> {
        self.pending
            .outputs
            .get_or_insert_with(|| self.outputs.clone())
    }

    fn done(&mut self) {
        let pending = std::mem::take(&mut self.pending);

        if let Some(fullscreen) = pending.fullscreen {
            self.fullscreen = fullscreen;
        }

        if let Some(app_id) = pending.app_id {
            self.app_id = Some(app_id);
        }

        if let Some(outputs) = pending.outputs {
            self.outputs = outputs;
        }
    }
}

// the protocol's state array: native-endian u32s
fn fullscreen(array: &[u8]) -> bool {
    array
        .as_chunks::<4>()
        .0
        .iter()
        .any(|&value| u32::from_ne_bytes(value) == handle::State::Fullscreen as u32)
}

impl WaylandState {
    // the fullscreen windows, to the Toplevels Service; also when an output gets its name
    pub(super) fn publish_toplevels(&self) {
        if self.toplevel_manager.is_none() {
            return;
        }

        let names: Vec<(ObjectId, String)> = self
            .output
            .outputs()
            .filter_map(|output| {
                let name = self.output.info(&output)?.name?;

                Some((output.id(), name))
            })
            .collect();

        toplevels::publish(
            self.toplevels
                .values()
                .flat_map(|toplevel| toplevel.on(&names))
                .collect(),
        );
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            manager::Event::Toplevel { toplevel } => {
                state.toplevels.insert(toplevel.id(), Toplevel::default());
            }

            // the compositor stops listing; nothing is known fullscreen then
            manager::Event::Finished => {
                state.toplevels.clear();
                state.publish_toplevels();
            }

            _ => {}
        }
    }

    event_created_child!(WaylandState, ZwlrForeignToplevelManagerV1, [
        manager::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        toplevel: &ZwlrForeignToplevelHandleV1,
        event: handle::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = toplevel.id();

        if let handle::Event::Closed = event {
            state.toplevels.remove(&id);
            toplevel.destroy();
            state.publish_toplevels();

            return;
        }

        let Some(known) = state.toplevels.get_mut(&id) else {
            return;
        };

        match event {
            handle::Event::State { state } => known.pending.fullscreen = Some(fullscreen(&state)),
            handle::Event::AppId { app_id } => known.pending.app_id = Some(app_id),
            handle::Event::OutputEnter { output } => known.entering().push(output.id()),
            handle::Event::OutputLeave { output } => {
                known.entering().retain(|entered| *entered != output.id());
            }
            handle::Event::Done => {
                known.done();
                state.publish_toplevels();
            }
            _ => {}
        }
    }
}

// the toplevel list, when the compositor has one
pub fn bind(
    globals: &wayland_client::globals::GlobalList,
    qh: &QueueHandle<WaylandState>,
) -> Option<ZwlrForeignToplevelManagerV1> {
    match globals.bind::<ZwlrForeignToplevelManagerV1, _, _>(qh, 1..=3, ()) {
        Ok(manager) => Some(manager),
        Err(_) => {
            toplevels::unavailable("the compositor has no wlr-foreign-toplevel-management");

            None
        }
    }
}
