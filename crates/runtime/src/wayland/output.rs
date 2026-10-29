use smithay_client_toolkit::output::{OutputHandler, OutputInfo, OutputState};
use wayland_client::{Connection, QueueHandle, protocol::wl_output::WlOutput};

use crate::Monitor;
use crate::monitor;
use crate::ui_scale;

use super::{WaylandState, surface::View};

impl OutputHandler for WaylandState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output
    }

    // also sent for the monitors that were already there when the shell started
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        let Some(info) = self.output.info(&output) else {
            return;
        };

        let monitor = describe(&info);
        self.publish(None);
        self.publish_toplevels();

        let views = self.per_monitor.clone();

        for view in views {
            let view = View::Monitor(view, monitor.clone());

            self.open(view, Some(output.clone()));
        }
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        let Some(info) = self.output.info(&output) else {
            return;
        };

        let monitor = describe(&info);
        self.publish(None);
        self.publish_toplevels();

        for window in &mut self.windows {
            if window.output.as_ref() != Some(&output) {
                continue;
            }

            if let View::Monitor(_, current) = &mut window.view {
                *current = monitor.clone();
            }

            window.request_frame();
        }
    }

    // a window on a monitor that is gone goes the way a closed one does
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        let gone = self
            .windows
            .iter()
            .filter(|window| window.output.as_ref() == Some(&output))
            .map(|window| window.role.wl_surface().clone())
            .collect::<Vec<_>>();

        // each window goes the way a closed one does, so the keyboard is let go and a card being
        // typed into keeps what was typed; close takes it out of the windows itself
        for surface in &gone {
            self.close(surface);
        }

        self.publish(Some(&output));
        self.publish_toplevels();
    }
}

impl WaylandState {
    // every monitor there is, but one `gone`, to the Monitors Service
    pub(super) fn publish(&self, gone: Option<&WlOutput>) {
        let all = self
            .output
            .outputs()
            .filter(|output| Some(output) != gone)
            .filter_map(|output| self.output.info(&output))
            .map(|info| describe(&info))
            .collect();

        monitor::publish(all);
    }
}

pub fn describe(info: &OutputInfo) -> Monitor {
    let name = info.name.clone().unwrap_or_default();

    let current = info.modes.iter().find(|mode| mode.current);

    // the logical size already has scaling and rotation applied, the mode does not
    let (width, height) = match (info.logical_size, current) {
        (Some(size), _) => size,
        (None, Some(mode)) => mode.dimensions,
        (None, None) => (0, 0),
    };

    // the views see the output in the virtual space, rounded down so nothing laid out in it can
    // be bigger than the output once the compositor's pixels are put back
    let ui = ui_scale::ui_scale();

    Monitor {
        name,

        width: floor(width, ui),
        height: floor(height, ui),
    }
}

// a length over the scale, down to a whole pixel; the sliver keeps 1920 / 0.8 from falling to 2399
fn floor(length: i32, ui: f32) -> u32 {
    (length as f32 / ui + 0.001).floor() as u32
}
