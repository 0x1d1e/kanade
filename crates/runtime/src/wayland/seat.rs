use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState, pointer::ThemeSpec};
use wayland_client::{Connection, QueueHandle, protocol::wl_seat::WlSeat};

use super::WaylandState;

impl SeatHandler for WaylandState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Pointer => {
                // the fallback theme draws its icons on a surface of their own
                let cursor_surface = self.compositor.create_surface(qh);

                let pointer = self
                    .seat
                    .get_pointer_with_theme::<_, ()>(
                        qh,
                        &seat,
                        self.shm.wl_shm(),
                        cursor_surface,
                        ThemeSpec::default(),
                    )
                    .expect("failed to get pointer");

                self.pointer_device = Some(pointer);
            }

            Capability::Keyboard => {
                // the compositor says only how soon and how often keys repeat; this loop does it
                let keyboard = self
                    .seat
                    .get_keyboard_with_repeat(
                        qh,
                        &seat,
                        None,
                        self.loop_handle.clone(),
                        Box::new(|state, _, event| state.repeat(&event)),
                    )
                    .expect("failed to get keyboard");

                self.keyboard_device = Some(keyboard);
            }

            _ => {}
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Pointer => {
                if let Some(pointer) = self.pointer_device.take() {
                    pointer.pointer().release();
                }
            }

            Capability::Keyboard => {
                if let Some(keyboard) = self.keyboard_device.take() {
                    keyboard.release();
                }
            }

            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}
