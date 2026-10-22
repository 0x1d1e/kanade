use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState, pointer::ThemeSpec};
use wayland_client::{Connection, QueueHandle, protocol::wl_seat::WlSeat};

use super::WaylandState;

impl SeatHandler for WaylandState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {
        self.follow_idle();
    }

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Pointer => {
                // a seat can offer the pointer again without having said it left
                self.drop_pointer();

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

                self.follow_data_device(qh, &seat);
            }

            Capability::Keyboard => {
                // a seat can offer the keyboard again without having said it left
                self.drop_keyboard();

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

                self.follow_data_device(qh, &seat);
                self.follow_ime(qh, &seat);
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
            Capability::Pointer => self.drop_pointer(),

            Capability::Keyboard => self.drop_keyboard(),

            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, seat: WlSeat) {
        self.data_device = None;

        self.forget_ime_of(&seat);
        self.forget_idle();
        self.follow_idle();
    }
}

impl WaylandState {
    /*
     * the pointer is gone, so nothing is over it and the next pointer gets its cursor set again:
     * `show_cursor` keeps the shape it last set, and a re-plugged pointer starts with none
     */
    fn drop_pointer(&mut self) {
        if let Some(pointer) = self.pointer_device.take() {
            pointer.pointer().release();
        }

        self.cursor_shown = None;
    }

    /*
     * the same teardown the keyboard's own leave does, since a compositor that unplugs the keyboard
     * may never send one: a text input cannot stay focused with no keyboard, a press that counts
     * only while its key is held cannot stay held, and a window that stands only while focused
     * hears it here
     */
    fn drop_keyboard(&mut self) {
        let keyboard = self.keyboard_device.take();

        if keyboard.is_none() && self.keyboard_focus.is_none() {
            return;
        }

        let focused = self.keyboard_focus.take();

        let lost = focused.as_ref().and_then(|surface| {
            self.window(surface)
                .and_then(|window| window.on_lost.clone())
        });

        self.modifiers = crate::Modifiers::default();

        crate::input::focus::clear();
        crate::input::released::send();

        if let Some(keyboard) = keyboard {
            keyboard.release();
        }

        if let Some(lost) = lost {
            lost();
        }

        if let Some(surface) = focused {
            self.request_frame_on(&surface);
        }
    }

    // the clipboard follows the keyboard and drags the pointer, so the first of either brings it
    fn follow_data_device(&mut self, qh: &QueueHandle<Self>, seat: &WlSeat) {
        if self.data_device.is_some() {
            return;
        }

        if let Some(manager) = &self.data_device_manager {
            self.data_device = Some(manager.get_data_device(qh, seat));
        }
    }
}
