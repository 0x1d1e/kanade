use std::rc::Rc;

use crate::Scroll;
use crate::graphics::Area;
use crate::input::{Handlers, PIXELS_PER_LINE, Target, clip, nest};
use crate::ui::{self, Role, Semantics};

use super::{ScrollArea, offsets};

pub fn collect_targets(scroll_area: &ScrollArea, area: Area, targets: &mut Vec<Target>) {
    let id = scroll_area.id;
    let farthest = scroll_area.farthest(area);

    let on_scroll = move |scroll: Scroll| {
        let current = offsets::get(id).clamp(0.0, farthest);
        let moved = scroll.y * PIXELS_PER_LINE;

        let offset = (current + moved).clamp(0.0, farthest);

        offsets::set(id, offset);
    };

    let handlers = Handlers {
        scroll: Some(Rc::new(on_scroll)),
        ..Handlers::default()
    };

    // added before the child, so a child with its own on_scroll still wins
    let mut own = Target::new(area, handlers);

    // named by its own id, which is fixed in the source, so `kanade ui scroll` can wheel it
    if ui::enabled() {
        own.semantics = Some(Semantics::new(Role::List, id).scrubbed(ui::mode()));
    }

    let at = targets.len();
    targets.push(own);

    let first = targets.len();

    scroll_area
        .child
        .collect_targets(scroll_area.child_area(area), targets);

    nest(&mut targets[first..], 0);

    // the parts scrolled out of view don't react to the pointer
    clip(&mut targets[first..], area);

    if targets[at].semantics.is_some() {
        targets[at].end = targets.len();
    }
}
