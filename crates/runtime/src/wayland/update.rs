use wayland_client::{
    Connection, Dispatch, QueueHandle,
    protocol::wl_region::{self, WlRegion},
};

use crate::{InputArea, LayerWindow, ui_scale};

use super::{
    WaylandState,
    layer::{self, Settings},
    surface::{Content, OpenWindow, Role},
};

impl OpenWindow {
    // settings changed by input or services reach the compositor before anything is drawn
    pub fn update_surface(&mut self, content: &Content) {
        // a normal window's title and size are only read when it opens
        let window = match content {
            Content::Layer(window) => window,

            Content::Normal(_) => return,
        };

        self.update_input_region(window);

        // a region in the compositor's logical pixels, so a changed UI scale resends it
        self.blur.set(
            &compositor_pixels(&window.blur),
            self.role.wl_surface(),
            &self.compositor,
            &self.qh,
        );

        // a normal window has no layer settings
        let Role::Layer {
            surface: layer_surface,
            settings: current,
        } = &mut self.role
        else {
            return;
        };

        let settings = Settings::new(window, ui_scale::ui_scale());

        if settings == *current {
            return;
        }

        let was_visible = current.visible;

        *current = settings;

        // the rest waits until the window shows again, which sends every setting
        if !settings.visible {
            if was_visible {
                self.hide();
            }

            return;
        }

        /*
         * a commit without a buffer also shows a hidden window again,
         * the compositor answers with a configure and drawing starts there
         */
        layer::apply(layer_surface, &settings);

        self.role.commit();
    }

    // only a changed region is sent, it takes effect with the next commit like the rest
    pub fn update_input_region(&mut self, window: &LayerWindow) {
        let region = window.input_region.as_deref().map(compositor_pixels);

        if region == self.input_region {
            return;
        }

        self.input_region = region;

        let surface = self.role.wl_surface();

        // no region set means the whole window takes the pointer again
        let Some(areas) = &self.input_region else {
            surface.set_input_region(None);

            return;
        };

        let region = self.compositor.create_region(&self.qh, ());

        for area in areas {
            region.add(area.x, area.y, area.width, area.height);
        }

        surface.set_input_region(Some(&region));

        // the surface keeps its own copy, so the region can go right away
        region.destroy();
    }

    fn hide(&mut self) {
        // taking the buffer away unmaps the window and gives back its reserved space
        self.role.wl_surface().attach(None, 0, 0);

        self.role.commit();

        // nothing is drawn until showing the window brings a new configure
        self.width = 0;
        self.height = 0;

        // a frame callback asked for before hiding may never come
        self.frame_requested = false;
    }
}

// areas of the virtual space in the compositor's logical pixels, grown to whole ones
fn compositor_pixels(areas: &[InputArea]) -> Vec<InputArea> {
    let ui = ui_scale::ui_scale();

    areas
        .iter()
        .map(|area| {
            let left = (area.x as f32 * ui).floor() as i32;
            let top = (area.y as f32 * ui).floor() as i32;
            let right = ((area.x + area.width) as f32 * ui).ceil() as i32;
            let bottom = ((area.y + area.height) as f32 * ui).ceil() as i32;

            InputArea {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            }
        })
        .collect()
}

// a region never sends events, but wayland-client still needs somewhere to send them
impl Dispatch<WlRegion, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlRegion,
        _: wl_region::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
