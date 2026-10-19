use smithay_client_toolkit::reexports::calloop::{LoopHandle, ping::make_ping};

use crate::changes;

use super::WaylandState;

pub fn insert(handle: &LoopHandle<'static, WaylandState>) {
    let (ping, source) = make_ping().expect("failed to create wake ping");

    handle
        .insert_source(source, |_, _, state| {
            crate::memory::trim_if_due();

            if crate::memory::take_redraw() {
                state.request_frames();
            }

            state.follow_ui_scale();

            state.open_requested();

            state.close_requested();

            state.follow_idle();

            state.request_changed_frames();
        })
        .expect("failed to insert wake ping");

    changes::set_waker(move || ping.ping());
}
