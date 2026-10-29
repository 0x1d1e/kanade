use std::collections::HashMap;
use std::path::PathBuf;

use crate::{Button, Cursor, Scroll};

use super::Target;
use super::target::{DragEndHandler, HoldHandler, HoverHandler};

/*
 * a target across frames: its key and, among the targets of the frame that share it, its order.
 * A position in the list would point at another target once widgets before it come or go
 */
type Id = (u64, u32);
type Dragged = (Id, Option<DragEndHandler>);

#[derive(Default)]
pub struct Pointer {
    // the hit areas of the last drawn frame, and the id of each
    targets: Vec<Target>,
    ids: Vec<Id>,

    x: f32,
    y: f32,

    // whether the pointer is over the window, and not left it since
    inside: bool,

    // the handler is the one the target had when last seen, so a target that turned inert or
    // went away still hears the pointer leave
    hovered: Vec<(Id, HoverHandler)>,
    pressed: Option<Id>,
    dragged: Option<Dragged>,

    // the target a press is held on, with the handler it was told by, which a redraw may not have
    held: Option<(Id, HoldHandler)>,
}

impl Pointer {
    // the hit areas of a new frame replace the last frame's
    pub fn set_targets(&mut self, targets: Vec<Target>) {
        let mut seen: HashMap<u64, u32> = HashMap::new();

        self.ids = targets
            .iter()
            .map(|target| {
                let order = seen.entry(target.key).or_default();
                let id = (target.key, *order);

                *order += 1;

                id
            })
            .collect();
        self.targets = targets;

        // a target that went away mid-hold lets the hold go
        if self
            .held
            .as_ref()
            .is_some_and(|(id, _)| self.index(*id).is_none())
        {
            self.stop_hold();
        }

        // a target that went away mid-drag ends the drag, its button still down
        if self
            .dragged
            .as_ref()
            .is_some_and(|(id, _)| self.index(*id).is_none())
        {
            self.stop_drag(false);
        }

        // the new frame's handler for each hovered target still there that has one
        for (id, handler) in &mut self.hovered {
            if let Some(new) = self
                .ids
                .iter()
                .position(|other| other == id)
                .and_then(|index| self.targets[index].handlers.hover.clone())
            {
                *handler = new;
            }
        }
    }

    // where the caret of the text input `id` was drawn, in the window: left, top, width, height
    pub fn caret(&self, id: &str) -> Option<(f32, f32, f32, f32)> {
        self.targets
            .iter()
            .find(|target| target.caret.is_some_and(|caret| caret.id == id))
            .and_then(Target::caret_in_window)
    }

    // the last frame's targets, in the order they were collected
    pub fn targets(&self) -> &[Target] {
        &self.targets
    }

    // the target a press began on and the release has not come for
    pub fn pressed_index(&self) -> Option<usize> {
        self.pressed.and_then(|id| self.index(id))
    }

    // where the pointer last was in the window, and whether it is over the window
    pub fn position(&self) -> (f32, f32, bool) {
        (self.x, self.y, self.inside)
    }

    // the innermost target at a spot that a press would reach: one that takes a click, or a drag
    // (a slider's bar takes only a drag, and a click on a parent behind it does not cover it)
    pub fn pressed_at(&self, x: f32, y: f32) -> Option<usize> {
        self.find_topmost_at(x, y, |target| {
            target.handlers.click.is_some() || target.handlers.drag.is_some()
        })
    }

    // where in the last frame an id is, if it still is
    fn index(&self, id: Id) -> Option<usize> {
        self.ids.iter().position(|other| *other == id)
    }

    // the look under a pointer that is over the window, for after a redraw changed what is there
    pub fn cursor_inside(&self) -> Option<Cursor> {
        self.inside.then(|| self.cursor())
    }

    // the innermost target under the pointer that has the handler asked for
    fn find_topmost(&self, has_handler: fn(&Target) -> bool) -> Option<usize> {
        self.find_topmost_at(self.x, self.y, has_handler)
    }

    fn find_topmost_at(&self, x: f32, y: f32, has_handler: fn(&Target) -> bool) -> Option<usize> {
        let mut topmost = None;

        // children are collected after their parents, so the last match is the innermost
        for (index, target) in self.targets.iter().enumerate() {
            if target.contains(x, y) && has_handler(target) {
                topmost = Some(index);
            }
        }

        topmost
    }

    // hover

    pub fn move_to(&mut self, x: f32, y: f32) -> bool {
        self.x = x;
        self.y = y;
        self.inside = true;

        // a press held on a target ends where the pointer leaves it
        if let Some((id, _)) = &self.held
            && !self
                .index(*id)
                .is_some_and(|index| self.targets[index].contains(x, y))
        {
            self.stop_hold();
        }

        let mut inside = Vec::new();

        for (index, target) in self.targets.iter().enumerate() {
            if let Some(hover) = &target.handlers.hover
                && target.contains(x, y)
            {
                inside.push((self.ids[index], hover.clone()));
            }
        }

        self.update_hovered(inside)
    }

    pub fn leave(&mut self) -> bool {
        // a button let go outside the window is not a click
        self.pressed = None;
        self.inside = false;

        // the release will not reach this window, so the drag and the hold end here, unreleased
        self.stop_drag(false);
        self.stop_hold();

        self.update_hovered(Vec::new())
    }

    // tells the targets the pointer entered or left, and returns whether any were told
    fn update_hovered(&mut self, inside: Vec<(Id, HoverHandler)>) -> bool {
        let mut changed = false;

        for (id, hover) in &self.hovered {
            if !inside.iter().any(|(other, _)| other == id) {
                hover(false);

                changed = true;
            }
        }

        for (id, hover) in &inside {
            if !self.hovered.iter().any(|(other, _)| other == id) {
                hover(true);

                changed = true;
            }
        }

        self.hovered = inside;

        changed
    }

    // motion

    // tells the innermost target under the pointer where the pointer is on it
    pub fn report_motion(&self) -> bool {
        let Some(index) = self.find_topmost(|target| target.handlers.motion.is_some()) else {
            return false;
        };

        let target = &self.targets[index];

        let Some(motion) = &target.handlers.motion else {
            return false;
        };

        let point = target.point(self.x, self.y);

        motion(point);

        true
    }

    // the look asked for by the innermost target under the pointer
    pub fn cursor(&self) -> Cursor {
        let Some(index) = self.find_topmost(|target| target.handlers.cursor.is_some()) else {
            return Cursor::Default;
        };

        self.targets[index]
            .handlers
            .cursor
            .unwrap_or(Cursor::Default)
    }

    // click

    pub fn press(&mut self) {
        self.pressed = self
            .find_topmost(|target| target.handlers.click.is_some())
            .map(|index| self.ids[index]);
    }

    // a click only counts when the button is let go over the widget it was pressed on
    pub fn release(&mut self, button: Button) -> bool {
        let Some(pressed) = self.pressed.take() else {
            return false;
        };

        let released = self.find_topmost(|target| target.handlers.click.is_some());

        // a redraw since the press may have removed this target
        let Some(index) = self.index(pressed) else {
            return false;
        };

        if released != Some(index) {
            return false;
        }

        let Some(click) = &self.targets[index].handlers.click else {
            return false;
        };

        click(button);

        true
    }

    // drag

    pub fn start_drag(&mut self) -> bool {
        self.dragged = self
            .find_topmost(|target| target.handlers.drag.is_some())
            .map(|index| {
                let id = self.ids[index];
                let drag_end = self.targets[index].handlers.drag_end.clone();

                (id, drag_end)
            });

        self.drag()
    }

    // the drag is over: `released` when the button came up over the window, false when the pointer
    // left it or the target went away, so the button is still down
    pub fn stop_drag(&mut self, released: bool) -> bool {
        let Some((id, captured_end)) = self.dragged.take() else {
            return false;
        };

        let handler = self
            .index(id)
            .and_then(|index| self.targets[index].handlers.drag_end.clone())
            .or(captured_end);

        if let Some(end) = handler {
            end(released);
        }

        true
    }

    // keeps reporting to the target pressed on, even after the pointer leaves it
    pub fn drag(&self) -> bool {
        let Some((id, _)) = self.dragged else {
            return false;
        };

        // a redraw since the press may have removed this target
        let Some(target) = self.index(id).map(|index| &self.targets[index]) else {
            return false;
        };

        let Some(drag) = &target.handlers.drag else {
            return false;
        };

        let point = target.point(self.x, self.y);

        drag(point);

        true
    }

    // hold

    // tells the innermost target under the pointer with a hold handler that a press is on it
    pub fn start_hold(&mut self) -> bool {
        self.stop_hold();

        let Some(index) = self.find_topmost(|target| target.handlers.hold.is_some()) else {
            return false;
        };

        let Some(hold) = self.targets[index].handlers.hold.clone() else {
            return false;
        };

        hold(true);
        self.held = Some((self.ids[index], hold));

        true
    }

    // tells the target held that the press is over; false when none was held
    pub fn stop_hold(&mut self) -> bool {
        let Some((_, hold)) = self.held.take() else {
            return false;
        };

        hold(false);

        true
    }

    // scroll

    pub fn scroll(&self, scroll: Scroll) -> bool {
        let Some(topmost) = self.find_topmost(|target| target.handlers.scroll.is_some()) else {
            return false;
        };

        let Some(handler) = &self.targets[topmost].handlers.scroll else {
            return false;
        };

        handler(scroll);

        true
    }

    // drop

    // whether files let go at this spot would be taken; the offer's position is in window space
    pub fn accepts_drop(&self, x: f32, y: f32) -> bool {
        self.find_topmost_at(x, y, |target| target.handlers.drop.is_some())
            .is_some()
    }

    // returns whether a handler ran
    pub fn drop_files(&self, x: f32, y: f32, files: Vec<PathBuf>) -> bool {
        let Some(index) = self.find_topmost_at(x, y, |target| target.handlers.drop.is_some())
        else {
            return false;
        };

        let Some(drop) = &self.targets[index].handlers.drop else {
            return false;
        };

        drop(files);

        true
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::graphics::Area;
    use crate::input::target::Handlers;

    // a bar that takes only a drag, in a parent that takes a click: `kanade ui press` reaches the
    // bar, since a real press drags it, and not what lies over it
    #[test]
    fn a_drag_only_target_is_reached_by_a_press() {
        let click = Handlers {
            click: Some(Rc::new(|_| {})),
            ..Handlers::default()
        };
        let drag = Handlers {
            drag: Some(Rc::new(|_| {})),
            ..Handlers::default()
        };

        let mut pointer = Pointer::default();

        pointer.set_targets(vec![
            Target::new(Area::new(0.0, 0.0, 100.0, 100.0), click.clone()),
            Target::new(Area::new(10.0, 10.0, 50.0, 10.0), drag),
            Target::new(Area::new(10.0, 50.0, 50.0, 10.0), Handlers::default()),
            Target::new(Area::new(30.0, 10.0, 10.0, 10.0), click),
        ]);

        // the bar's left part; the parent's click is behind it
        assert_eq!(pointer.pressed_at(15.0, 15.0), Some(1));

        // a click-taking target over the bar covers it
        assert_eq!(pointer.pressed_at(35.0, 15.0), Some(3));

        // no handler of a press: the parent
        assert_eq!(pointer.pressed_at(15.0, 55.0), Some(0));
        assert_eq!(pointer.pressed_at(200.0, 200.0), None);
    }
}
