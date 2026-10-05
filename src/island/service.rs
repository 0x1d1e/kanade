use std::collections::HashMap;
use std::time::Duration;

use amane::{Animation, Blend, Keyboard, Service};

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

    // opened without a press (IPC, keybind), so OnDemand never got focus; held until collapse,
    // since niri drops the focus on any switch to OnDemand, even right after a press (#4)
    held: bool,

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

    fn held(&self, monitor: &str) -> bool {
        self.islands.get(monitor).is_some_and(|island| island.held)
    }

    /*
     * OnDemand focuses only on a press, so it must be on before the press that expands (#2).
     * Exclusive would keep the keyboard from overlays opened later (niri gives it to the first
     * mapped exclusive surface), so only an island opened without a press holds it (#4)
     */
    pub fn keyboard(&self, monitor: &str) -> Keyboard {
        if self.held(monitor) {
            Keyboard::Exclusive
        } else if self.expanded(monitor) || self.armed(monitor) {
            Keyboard::OnDemand
        } else {
            Keyboard::None
        }
    }

    // expands without a press, so the island holds the keyboard to still get Escape
    pub fn open(&mut self, monitor: &str) {
        self.set_expanded(monitor, true);
        self.island(monitor).held = true;
    }

    pub fn set_expanded(&mut self, monitor: &str, expanded: bool) {
        let island = self.island(monitor);

        island.expanded = expanded;
        island.held &= expanded;

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

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: &str = "eDP-1";

    #[test]
    fn untouched_island_takes_no_keyboard() {
        assert_eq!(IslandService::new().keyboard(MONITOR), Keyboard::None);
    }

    #[test]
    fn pointer_expanded_island_waits_for_the_press() {
        let mut island = IslandService::new();

        island.set_armed(MONITOR, true);
        assert_eq!(island.keyboard(MONITOR), Keyboard::OnDemand);

        island.set_expanded(MONITOR, true);
        assert_eq!(island.keyboard(MONITOR), Keyboard::OnDemand);
    }

    #[test]
    fn opened_island_holds_the_keyboard_through_pointer_input() {
        let mut island = IslandService::new();

        island.open(MONITOR);
        assert_eq!(island.keyboard(MONITOR), Keyboard::Exclusive);

        island.set_armed(MONITOR, true);
        island.set_expanded(MONITOR, true);
        assert_eq!(island.keyboard(MONITOR), Keyboard::Exclusive);
    }

    #[test]
    fn collapse_releases_a_held_keyboard() {
        let mut island = IslandService::new();

        island.open(MONITOR);
        island.set_expanded(MONITOR, false);
        assert_eq!(island.keyboard(MONITOR), Keyboard::None);

        // a later pointer expand does not bring the hold back
        island.set_expanded(MONITOR, true);
        assert_eq!(island.keyboard(MONITOR), Keyboard::OnDemand);
    }
}
