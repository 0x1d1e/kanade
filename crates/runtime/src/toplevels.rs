use crate::service::Service;

// a fullscreen window: on which monitor, and which app's, which is all niri's IPC has to match it by
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FullscreenWindow {
    pub output: String,
    pub app_id: Option<String>,
}

/// The windows the compositor says are fullscreen, from its
/// `wlr-foreign-toplevel-management` list, kept by the runtime. niri's IPC says
/// how big a window is, not whether it is fullscreen, and a window maximized to
/// the edges is as big.
#[derive(Debug, Default)]
pub struct Toplevels {
    fullscreen: Vec<FullscreenWindow>,

    // why the list is not followed, as when the compositor has none
    unavailable: Option<String>,
}

impl Toplevels {
    /// Every fullscreen window, on a monitor the compositor has named; empty
    /// while unknown.
    pub fn fullscreen(&self) -> &[FullscreenWindow] {
        &self.fullscreen
    }

    pub fn unavailable(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }
}

impl Service for Toplevels {
    fn new() -> Self {
        Self::default()
    }

    // written only by the runtime's toplevel events
    fn listen() {}
}

// the fullscreen windows as they now are, sorted; the same again changes nothing
pub(crate) fn publish(mut fullscreen: Vec<FullscreenWindow>) {
    fullscreen.sort();

    let mut toplevels = Toplevels::write();

    if toplevels.fullscreen == fullscreen {
        toplevels.quiet();
    } else {
        toplevels.fullscreen = fullscreen;
    }
}

pub(crate) fn unavailable(why: &str) {
    Toplevels::write().unavailable = Some(why.to_owned());
}
