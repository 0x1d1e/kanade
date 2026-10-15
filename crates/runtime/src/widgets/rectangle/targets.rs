use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;

use crate::graphics::Area;
use crate::input::{Cursor, Handlers, Target, anchor, clip, nest};
use crate::ui::{self, Semantics};
use crate::{Button, Point, Rectangle, Scroll};

use super::{child, transform};

impl Rectangle {
    /*
     * an inert rectangle still draws but its children react to nothing, and a click on it is taken
     * and ignored, so it never reaches what is beneath. Its targets stay in the list, since the
     * pointer holds a press, a drag and the hovered ones by key
     */
    pub fn inert(mut self, inert: bool) -> Self {
        self.inert = inert;

        self
    }

    /*
     * what the rectangle and what is in it stand for, as a list item's id: the pointer follows a
     * press and a hover by it, not by the item's place, which moves as items come and go
     */
    pub fn identity(mut self, identity: impl Hash) -> Self {
        let mut hasher = DefaultHasher::new();

        identity.hash(&mut hasher);
        self.identity = Some(hasher.finish());

        self
    }

    /*
     * says what the rectangle is for `kanade ui` (ADR 0095). Asked for only while `ui.describe`
     * keeps a tree, so with it off nothing is built, and the content a node shows is never read
     */
    pub fn semantics(mut self, semantics: impl FnOnce() -> Semantics) -> Self {
        if ui::enabled() {
            self.semantics = Some(semantics().scrubbed(ui::mode()));
        }

        self
    }

    pub fn on_click(mut self, handler: impl Fn(Button) + 'static) -> Self {
        self.handlers.click = Some(Rc::new(handler));

        self
    }

    // true when the pointer comes in, false when it goes out
    pub fn on_hover(mut self, handler: impl Fn(bool) + 'static) -> Self {
        self.handlers.hover = Some(Rc::new(handler));

        self
    }

    pub fn on_scroll(mut self, handler: impl Fn(Scroll) + 'static) -> Self {
        self.handlers.scroll = Some(Rc::new(handler));

        self
    }

    /*
     * runs on a left press and on every move until the release,
     * even after the pointer leaves the rectangle
     */
    pub fn on_drag(mut self, handler: impl Fn(Point) + 'static) -> Self {
        self.handlers.drag = Some(Rc::new(handler));

        self
    }

    /*
     * true when the left button goes down on the rectangle, false when it comes up, the pointer
     * leaves the rectangle or the window, or the rectangle goes away: for a press that counts only
     * while it lasts. It sets no click, so a click set beneath it still gets the short press
     */
    pub fn on_hold(mut self, handler: impl Fn(bool) + 'static) -> Self {
        self.handlers.hold = Some(Rc::new(handler));

        self
    }

    // how the pointer looks while it is over the rectangle
    pub fn cursor(mut self, cursor: Cursor) -> Self {
        self.handlers.cursor = Some(cursor);

        self
    }

    /*
     * runs with the files another program dragged here and let go over the rectangle: absolute
     * paths, which the program chose and are to be checked before they are opened
     */
    pub fn on_drop(mut self, handler: impl Fn(Vec<PathBuf>) + 'static) -> Self {
        self.handlers.drop = Some(Rc::new(handler));

        self
    }

    // runs on every move while the pointer is over the rectangle
    pub fn on_move(mut self, handler: impl Fn(Point) + 'static) -> Self {
        self.handlers.motion = Some(Rc::new(handler));

        self
    }
}

pub fn collect_targets(rectangle: &Rectangle, area: Area, targets: &mut Vec<Target>) {
    let first = targets.len();

    collect_in_place(rectangle, area, targets);

    if rectangle.semantics.is_some() {
        targets[first].end = targets.len();
    }

    if let Some(identity) = rectangle.identity {
        anchor(&mut targets[first..], identity);
    }

    let local = transform::local(rectangle, area);

    // a rectangle scaled to nothing takes no input
    let Some(inverse) = local.invert() else {
        for target in &mut targets[first..] {
            target.area = Area::new(0.0, 0.0, 0.0, 0.0);
        }

        return;
    };

    // this rectangle's transform is undone first, then whatever its children added inside it
    for target in &mut targets[first..] {
        target.inverse = inverse.post_concat(target.inverse);
    }
}

// the hit areas as if the rectangle were not rotated, scaled or moved
fn collect_in_place(rectangle: &Rectangle, area: Area, targets: &mut Vec<Target>) {
    let mut handlers = rectangle.handlers.clone();

    if rectangle.inert {
        handlers = Handlers {
            click: Some(Rc::new(|_| {})),
            ..Handlers::default()
        };
    }

    // added before the child, so the child wins where both react to the same thing
    let mut own_target = Target::new(area, handlers);

    own_target.semantics = rectangle.semantics.clone().map(|mut semantics| {
        semantics.disabled |= rectangle.inert;

        semantics
    });

    targets.push(own_target);
    let own = targets.len();

    let Some(child) = &rectangle.child else {
        return;
    };

    let child_area = child::area(rectangle, child.as_ref(), area);

    let first = targets.len();

    child.collect_targets(child_area, targets);

    nest(&mut targets[first..], 0);

    // the rounded corners are left out, they are too small to miss a click by
    if rectangle.clip {
        clip(&mut targets[first..], area);
    }

    if rectangle.inert {
        for target in &mut targets[own..] {
            target.handlers = Handlers::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use crate::input::Pointer;
    use crate::widgets::Widget;
    use crate::{Row, Stack};

    use super::*;

    // a button and a slider side by side, in a rectangle that may turn inert between two frames
    fn frame(inert: bool, pressed: &Rc<Cell<u32>>, dragged: &Rc<Cell<u32>>) -> Vec<Target> {
        let (pressed, dragged) = (pressed.clone(), dragged.clone());
        let button = Rectangle::new()
            .width(50.0)
            .height(50.0)
            .on_click(move |_| pressed.set(pressed.get() + 1));
        let slider = Rectangle::new()
            .width(50.0)
            .height(50.0)
            .on_drag(move |_| dragged.set(dragged.get() + 1));
        let row = crate::Row::new(crate::children![button, slider]);
        let outer = Rectangle::new()
            .width(100.0)
            .height(50.0)
            .inert(inert)
            .child(row);
        let mut targets = Vec::new();

        outer.collect_targets(Area::new(0.0, 0.0, 100.0, 50.0), &mut targets);

        targets
    }

    // turning inert between a press and its release, or in a drag, reaches no handler, and shifts no other
    #[test]
    fn a_rectangle_turning_inert_mid_press_or_drag_reaches_nothing() {
        let (pressed, dragged) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let mut pointer = Pointer::default();

        pointer.set_targets(frame(false, &pressed, &dragged));
        let count = frame(false, &pressed, &dragged).len();

        pointer.move_to(25.0, 25.0);
        pointer.press();
        pointer.move_to(75.0, 25.0);
        pointer.start_drag();

        assert_eq!(dragged.get(), 1);

        let inert = frame(true, &pressed, &dragged);

        assert_eq!(inert.len(), count);

        pointer.set_targets(inert);
        pointer.move_to(25.0, 25.0);
        pointer.release(Button::Left);
        pointer.move_to(75.0, 25.0);
        pointer.drag();

        assert_eq!((pressed.get(), dragged.get()), (0, 1));

        // a press taken while inert is no one's either
        pointer.press();
        pointer.release(Button::Left);

        assert_eq!(pressed.get(), 0);
    }

    // the Island's own target lies beneath its content: a click on inert content stops there, while hover, motion and the wheel still reach it
    #[test]
    fn a_click_on_inert_content_stops_but_hover_motion_and_wheel_pass() {
        let seen: Rc<Cell<[u32; 5]>> = Rc::default();
        let bump = |at: usize| {
            let seen = seen.clone();

            move || {
                let mut counts = seen.get();

                counts[at] += 1;
                seen.set(counts);
            }
        };
        let (click, hover, motion, wheel) = (bump(0), bump(1), bump(2), bump(3));
        let inner = bump(4);
        let beneath = Rectangle::new()
            .width(100.0)
            .height(50.0)
            .on_click(move |_| click())
            .on_hover(move |_| hover())
            .on_move(move |_| motion())
            .on_scroll(move |_| wheel());
        let content = Rectangle::new()
            .width(100.0)
            .height(50.0)
            .inert(true)
            .child(
                Rectangle::new()
                    .width(100.0)
                    .height(50.0)
                    .on_scroll(move |_| inner()),
            );
        let mut targets = Vec::new();

        Stack::new(crate::children![beneath, content])
            .collect_targets(Area::new(0.0, 0.0, 100.0, 50.0), &mut targets);

        let mut pointer = Pointer::default();

        pointer.set_targets(targets);
        pointer.move_to(10.0, 10.0);
        pointer.report_motion();
        pointer.scroll(Scroll { x: 0.0, y: 1.0 });
        pointer.press();
        pointer.release(Button::Left);

        // hover entered, one move, the wheel on the target beneath rather than the inert child, no click
        assert_eq!(seen.get(), [0, 1, 1, 1, 0]);
    }

    // a form of more or fewer targets swapped in beside it moves none of the targets after it
    // out from under a press, a drag or a hover begun before
    #[test]
    fn targets_after_a_swapped_form_keep_their_press_hover_and_drag() {
        let seen: Rc<Cell<[u32; 4]>> = Rc::default();
        let bump = |at: usize| {
            let seen = seen.clone();

            move || {
                let mut counts = seen.get();

                counts[at] += 1;
                seen.set(counts);
            }
        };
        let (click, hover, drag, other) = (bump(0), bump(1), bump(2), bump(3));

        // the form has `buttons` targets, then comes one with a click, a hover and a drag
        let frame = |buttons: usize| {
            let form: Vec<Box<dyn Widget>> = (0..buttons)
                .map(|_| {
                    let other = other.clone();

                    Box::new(
                        Rectangle::new()
                            .width(10.0)
                            .height(50.0)
                            .on_click(move |_| other()),
                    ) as Box<dyn Widget>
                })
                .collect();
            let (click, hover, drag) = (click.clone(), hover.clone(), drag.clone());
            let after = Rectangle::new()
                .width(50.0)
                .height(50.0)
                .on_click(move |_| click())
                .on_hover(move |_| hover())
                .on_drag(move |_| drag());
            let mut targets = Vec::new();

            Row::new(crate::children![Row::new(form).width(50.0), after])
                .collect_targets(Area::new(0.0, 0.0, 100.0, 50.0), &mut targets);

            targets
        };
        let mut pointer = Pointer::default();

        pointer.set_targets(frame(1));
        pointer.move_to(75.0, 25.0);
        pointer.press();
        pointer.start_drag();

        pointer.set_targets(frame(3));
        pointer.move_to(80.0, 25.0);
        pointer.drag();
        pointer.release(Button::Left);

        // one hover in, no stray hover out, one drag report on the press and one on the move, one click
        assert_eq!(seen.get(), [1, 1, 2, 0]);
    }

    // a hovered target that turned inert or went away still hears the pointer leave, once
    #[test]
    fn a_hovered_target_turning_inert_or_going_away_hears_the_pointer_leave() {
        let events: Rc<std::cell::RefCell<Vec<bool>>> = Rc::default();
        let frame = |inert: bool, present: bool| {
            let events = events.clone();
            let mut form: Vec<Box<dyn Widget>> = Vec::new();

            if present {
                form.push(Box::new(
                    Rectangle::new()
                        .width(50.0)
                        .height(50.0)
                        .inert(inert)
                        .on_hover(move |inside| events.borrow_mut().push(inside))
                        .on_click(|_| {}),
                ));
            }

            let mut targets = Vec::new();

            Row::new(form).collect_targets(Area::new(0.0, 0.0, 50.0, 50.0), &mut targets);

            targets
        };

        for (inert, present) in [(true, true), (false, false)] {
            events.borrow_mut().clear();

            let mut pointer = Pointer::default();

            pointer.set_targets(frame(false, true));
            pointer.move_to(25.0, 25.0);
            pointer.set_targets(frame(inert, present));
            pointer.move_to(26.0, 25.0);
            pointer.move_to(27.0, 25.0);

            // the inert one keeps no hover handler of its own: it hears the exit from the old one
            assert_eq!(
                *events.borrow(),
                [true, false],
                "inert {inert}, present {present}"
            );
        }
    }

    // a list item named by what it is keeps its press and hover as items come above it, and one
    // that takes its place is another target, not the same one
    #[test]
    fn items_named_by_identity_follow_the_item_and_not_its_place() {
        let log: Rc<std::cell::RefCell<Vec<String>>> = Rc::default();
        let frame = |ids: &[u32]| {
            let items: Vec<Box<dyn Widget>> = ids
                .iter()
                .map(|&id| {
                    let (click, hover) = (log.clone(), log.clone());

                    Box::new(
                        Rectangle::new()
                            .width(50.0)
                            .height(20.0)
                            .identity(id)
                            .on_click(move |_| click.borrow_mut().push(format!("click {id}")))
                            .on_hover(move |inside| {
                                hover.borrow_mut().push(format!("hover {id} {inside}"))
                            }),
                    ) as Box<dyn Widget>
                })
                .collect();
            let mut targets = Vec::new();

            crate::Column::new(items)
                .collect_targets(Area::new(0.0, 0.0, 50.0, 100.0), &mut targets);

            targets
        };
        let mut pointer = Pointer::default();

        // item 1 is on top; the pointer rests on it and presses
        pointer.set_targets(frame(&[1, 2]));
        pointer.move_to(25.0, 10.0);
        pointer.press();

        // item 3 comes above, and item 1 is pushed down, out from under the pointer
        pointer.set_targets(frame(&[3, 1, 2]));
        pointer.release(Button::Left);

        // the pointer is over item 3 now: the release is not item 1's click, nor item 3's
        assert_eq!(*log.borrow(), ["hover 1 true"]);

        pointer.move_to(25.0, 11.0);

        assert_eq!(
            *log.borrow(),
            ["hover 1 true", "hover 1 false", "hover 3 true"]
        );

        // a press that stays on its item still clicks it, wherever the item went
        pointer.press();
        pointer.set_targets(frame(&[1, 3, 2]));
        pointer.move_to(25.0, 10.0);
        pointer.release(Button::Left);

        assert_eq!(log.borrow().last().unwrap(), "hover 1 true");
        assert!(!log.borrow().iter().any(|line| line.starts_with("click")));

        pointer.press();
        pointer.release(Button::Left);

        assert_eq!(log.borrow().last().unwrap(), "click 1");
    }

    // a hold is told it began, and once, however it ends: the release, the pointer leaving the
    // target or the window, or the target going away
    #[test]
    fn a_hold_ends_once_by_release_leaving_or_going_away() {
        let events: Rc<std::cell::RefCell<Vec<bool>>> = Rc::default();
        let frame = |present: bool| {
            let events = events.clone();
            let mut targets = Vec::new();

            if present {
                Rectangle::new()
                    .width(50.0)
                    .height(50.0)
                    .on_hold(move |held| events.borrow_mut().push(held))
                    .collect_targets(Area::new(0.0, 0.0, 50.0, 50.0), &mut targets);
            }

            targets
        };
        let mut pointer = Pointer::default();

        pointer.set_targets(frame(true));
        pointer.move_to(25.0, 25.0);

        // a release
        assert!(pointer.start_hold());
        assert!(pointer.stop_hold());
        assert!(!pointer.stop_hold());
        assert_eq!(*events.borrow(), [true, false]);

        // out of the target
        events.borrow_mut().clear();
        pointer.start_hold();
        pointer.move_to(75.0, 25.0);
        pointer.move_to(25.0, 25.0);
        assert_eq!(*events.borrow(), [true, false]);

        // out of the window
        events.borrow_mut().clear();
        pointer.start_hold();
        pointer.leave();
        assert_eq!(*events.borrow(), [true, false]);

        // gone with the next frame
        events.borrow_mut().clear();
        pointer.move_to(25.0, 25.0);
        pointer.start_hold();
        pointer.set_targets(frame(false));
        pointer.stop_hold();
        assert_eq!(*events.borrow(), [true, false]);
    }
}
