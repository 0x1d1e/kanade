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

struct Island {
    expanded: bool,
    shape: Animation<Shape>,
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
            .map_or(REST, |island| island.shape.value())
    }

    pub fn set_expanded(&mut self, monitor: &str, expanded: bool) {
        let island = self
            .islands
            .entry(monitor.to_owned())
            .or_insert_with(|| Island {
                expanded: false,
                shape: Animation::new(REST).duration(MORPH),
            });

        island.expanded = expanded;

        island.shape.to(if expanded { EXPANDED } else { REST });
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
