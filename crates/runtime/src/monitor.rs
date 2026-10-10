use crate::service::Service;

// a screen the compositor shows windows on, handed to views made once per monitor
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub name: String,

    pub width: u32,
    pub height: u32,
}

/// The monitors there are now, kept by the runtime as the compositor adds,
/// changes and removes them, so what depends on all of them can `watch` it.
#[derive(Debug, Default)]
pub struct Monitors {
    all: Vec<Monitor>,
}

impl Monitors {
    pub fn all(&self) -> &[Monitor] {
        &self.all
    }
}

impl Service for Monitors {
    fn new() -> Self {
        Self::default()
    }

    // written only by the runtime's output events
    fn listen() {}
}

// the monitors as they now are; the same again changes nothing
pub(crate) fn publish(all: Vec<Monitor>) {
    let mut monitors = Monitors::write();

    if monitors.all == all {
        monitors.quiet();
    } else {
        monitors.all = all;
    }
}
