use std::collections::HashMap;
use std::time::Duration;

use amane::{Animation, Blend, Service};

use super::geometry::{EXPANDED, REST, Shape};

// plan 5.2 starting values, expand and collapse take the same time
const MORPH: Duration = Duration::from_millis(180);

// the one Amane-facing piece of island/: owns the Arbiter and per-monitor Presentation
pub struct IslandService {
    // by monitor name, an island nobody touched yet is at rest
    islands: HashMap<String, Island>,
}

#[derive(Default)]
struct Island {
    expanded: bool,

    // the pointer is on the body, so a press there can take keyboard focus
    armed: bool,

    // none until the first morph, a new Animation counts as moving for its whole duration
    shape: Option<Animation<Shape>>,
}

impl Service for IslandService {
    fn new() -> Self {
        Self {
            islands: HashMap::new(),
        }
    }

    // changes only through input and sources, nothing to poll
    fn listen() {}
}

impl IslandService {
    pub fn expanded(&self, monitor: &str) -> bool {
        self.islands
            .get(monitor)
            .is_some_and(|island| island.expanded)
    }

    // read in the view, keeps the window drawing until the morph arrives
    pub fn shape(&self, monitor: &str) -> Shape {
        self.islands
            .get(monitor)
            .and_then(|island| island.shape.as_ref())
            .map_or(REST, Animation::value)
    }

    pub fn armed(&self, monitor: &str) -> bool {
        self.islands.get(monitor).is_some_and(|island| island.armed)
    }

    pub fn set_expanded(&mut self, monitor: &str, expanded: bool) {
        let island = self.island(monitor);

        island.expanded = expanded;

        island
            .shape
            .get_or_insert_with(|| Animation::new(REST).duration(MORPH))
            .to(if expanded { EXPANDED } else { REST });
    }

    pub fn set_armed(&mut self, monitor: &str, armed: bool) {
        self.island(monitor).armed = armed;
    }

    fn island(&mut self, monitor: &str) -> &mut Island {
        self.islands.entry(monitor.to_owned()).or_default()
    }
}

impl Blend for Shape {
    fn blend(from: Self, to: Self, amount: f32) -> Self {
        Self {
            width: f32::blend(from.width, to.width, amount),
            height: f32::blend(from.height, to.height, amount),
            radius: f32::blend(from.radius, to.radius, amount),
        }
    }
}
