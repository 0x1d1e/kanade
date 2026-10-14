use smithay_client_toolkit::output::{OutputHandler, OutputInfo, OutputState};
use wayland_client::{Connection, QueueHandle, protocol::wl_output::WlOutput};

use crate::Monitor;
use crate::monitor;

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

    // dropping a window destroys its layer surface
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        self.windows
            .retain(|window| window.output.as_ref() != Some(&output));

        self.backdrops_gone(&output);
        self.publish(Some(&output));
        self.publish_toplevels();
    }
}

impl WaylandState {
    // every monitor there is, but one `gone`, to the Monitors Service
    fn publish(&self, gone: Option<&WlOutput>) {
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

    Monitor {
        name,

        width: width as u32,
        height: height as u32,
    }
}
