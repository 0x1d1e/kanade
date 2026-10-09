//! Stable geometry and presentation state for native layer/xdg/lock surfaces.
//!
//! A Wayland backend converts Spec changes to protocol requests and frame
//! callbacks to State events. The Island is still responsible for its spring,
//! layout, Activity selection and Surface contents.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Background,
    Bottom,
    Top,
    Overlay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardMode {
    None,
    OnDemand,
    Exclusive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alignment {
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    /// Clip against a local surface, keeping input out of the empty canvas.
    pub fn clipped_to(self, width: u32, height: u32) -> Option<Self> {
        let x0 = (self.x as i64).clamp(0, width as i64);
        let y0 = (self.y as i64).clamp(0, height as i64);
        let x1 = (self.x as i64 + self.width as i64).clamp(0, width as i64);
        let y1 = (self.y as i64 + self.height as i64).clamp(0, height as i64);
        (x1 > x0 && y1 > y0).then_some(Self {
            x: x0 as i32,
            y: y0 as i32,
            width: (x1 - x0) as u32,
            height: (y1 - y0) as u32,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub width: u32,
    pub height: u32,
    pub layer: Layer,
    pub edge: Edge,
    pub align: Alignment,
    pub keyboard: KeyboardMode,
    pub exclusive_zone: i32,
    pub input: Vec<Rect>,
}

impl Spec {
    /// None of the rest of a fixed canvas captures pointer input.
    pub fn input_regions(&self) -> Vec<Rect> {
        self.input
            .iter()
            .filter_map(|rect| rect.clipped_to(self.width, self.height))
            .collect()
    }
}

/// A callback-driven frame gate. No timer or redraw when a window is idle.
/// Changing a hidden window retains no pending frame. Showing it requires
/// another compositor configure / initial presentation from the backend.
#[derive(Debug, Default)]
pub struct State {
    configured: bool,
    visible: bool,
    frame_ready: bool,
    dirty: bool,
}

impl State {
    pub fn configure(&mut self) {
        self.configured = true;
        self.frame_ready = true;
        self.dirty = true;
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.dirty = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.configured = false;
        self.frame_ready = false;
        self.dirty = false;
    }

    pub fn invalidate(&mut self) {
        if self.visible {
            self.dirty = true;
        }
    }

    pub fn frame_callback(&mut self) {
        if self.visible && self.configured {
            self.frame_ready = true;
        }
    }

    /// Returns true exactly once per available frame while visible and dirty.
    pub fn begin_frame(&mut self) -> bool {
        if !self.visible || !self.configured || !self.frame_ready || !self.dirty {
            return false;
        }
        self.frame_ready = false;
        self.dirty = false;
        true
    }

    /// The renderer requests the next compositor frame only while animating.
    pub fn submitted(&mut self, animation_active: bool) {
        self.dirty |= animation_active;
    }

    pub fn needs_frame(&self) -> bool {
        self.visible && self.configured && self.dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_follows_body_and_never_leaks_off_surface() {
        let spec = Spec {
            width: 560,
            height: 380,
            layer: Layer::Top,
            edge: Edge::Top,
            align: Alignment::Center,
            keyboard: KeyboardMode::OnDemand,
            exclusive_zone: -1,
            input: vec![
                Rect {
                    x: 150,
                    y: 0,
                    width: 260,
                    height: 40,
                },
                Rect {
                    x: 600,
                    y: 5,
                    width: 20,
                    height: 20,
                },
                Rect {
                    x: -10,
                    y: -10,
                    width: 30,
                    height: 25,
                },
            ],
        };
        assert_eq!(
            spec.input_regions(),
            vec![
                Rect {
                    x: 150,
                    y: 0,
                    width: 260,
                    height: 40,
                },
                Rect {
                    x: 0,
                    y: 0,
                    width: 20,
                    height: 15,
                },
            ]
        );
    }

    #[test]
    fn idle_windows_do_not_paint() {
        let mut state = State::default();
        assert!(!state.begin_frame());
        state.show();
        assert!(!state.begin_frame()); // No configure.
        state.configure();
        assert!(state.begin_frame());
        state.submitted(false);
        state.frame_callback();
        assert!(!state.begin_frame()); // Still image costs zero frames.
        state.invalidate();
        assert!(state.begin_frame());
        state.submitted(true);
        assert!(state.needs_frame());
        state.frame_callback();
        assert!(state.begin_frame());
    }

    #[test]
    fn hidden_window_never_paints_and_has_no_stale_frame() {
        let mut state = State::default();
        state.show();
        state.configure();
        state.hide();
        state.frame_callback(); // A callback from the old mapping is stale.
        assert!(!state.begin_frame());
        state.show();
        assert!(!state.begin_frame());
        state.configure();
        assert!(state.begin_frame());
    }
}
