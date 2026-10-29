use std::path::PathBuf;
use std::rc::Rc;

use crate::graphics::{Area, Transform};
use crate::ui::Semantics;
use crate::{Button, Cursor, Key, Point, Scroll};

use super::ime::Caret;

/*
 * shared, so the hit areas kept after a redraw
 * still work once the widget tree they came from is dropped
 */
pub type ClickHandler = Rc<dyn Fn(Button)>;
pub type HoverHandler = Rc<dyn Fn(bool)>;
pub type ScrollHandler = Rc<dyn Fn(Scroll)>;
pub type KeyHandler = Rc<dyn Fn(Key)>;
// the keyboard focus left the window, for what only stands while it is there
pub type LostHandler = Rc<dyn Fn()>;
pub type PointHandler = Rc<dyn Fn(Point)>;
pub type DragEndHandler = Rc<dyn Fn(bool)>;
pub type DropHandler = Rc<dyn Fn(Vec<PathBuf>)>;
pub type HoldHandler = Rc<dyn Fn(bool)>;

#[derive(Clone, Default)]
pub struct Handlers {
    pub click: Option<ClickHandler>,
    pub hover: Option<HoverHandler>,
    pub scroll: Option<ScrollHandler>,
    pub drag: Option<PointHandler>,
    pub drag_end: Option<DragEndHandler>,
    pub motion: Option<PointHandler>,
    pub drop: Option<DropHandler>,
    pub hold: Option<HoldHandler>,

    // not a handler, but it belongs to the same spot the pointer is over
    pub cursor: Option<Cursor>,
}

// where a widget was drawn, and what it does when the pointer uses that spot
pub struct Target {
    pub area: Area,
    pub handlers: Handlers,

    // turns a window position into the area's own space, undoing rotated, scaled or moved parents
    pub inverse: Transform,

    /*
     * what names it across frames: the places of its widget and its parents among their siblings,
     * so it stays the same target when the widgets before it come or go inside a container of
     * their own, as one form gives way to another, and not when a sibling is added or removed
     * before its container: a container that varies keeps a fixed place (see `nest`)
     */
    pub key: u64,

    // a text input's caret, for its input method; the area above is where it sits
    pub caret: Option<Caret>,

    // what the target is, for `kanade ui` (ADR 0095); none for the targets no view said anything of
    pub semantics: Option<Semantics>,

    // one past the last target collected inside this one, set where there are semantics, so a node
    // finds its parent: the targets from the next one to here are in it
    pub end: usize,

    // named by what it is (see `anchor`), so the places above it no longer change its key
    anchored: bool,
}

impl Target {
    pub fn new(area: Area, handlers: Handlers) -> Self {
        Self {
            area,
            handlers,
            inverse: Transform::IDENTITY,
            key: 0,
            caret: None,
            semantics: None,
            end: 0,
            anchored: false,
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (local_x, local_y) = self.inverse.map(x, y);

        self.area.contains(local_x, local_y)
    }

    // the caret's box in the window, where parents have moved, scaled or turned it
    pub fn caret_in_window(&self) -> Option<(f32, f32, f32, f32)> {
        self.in_window(self.caret?.area)
    }

    // the target's own area in the window
    pub fn bounds_in_window(&self) -> Option<(f32, f32, f32, f32)> {
        self.in_window(self.area)
    }

    // a box of the target's space as the smallest upright box around it in the window
    fn in_window(&self, area: Area) -> Option<(f32, f32, f32, f32)> {
        let into_window = self.inverse.invert()?;

        let Area {
            x,
            y,
            width,
            height,
        } = area;

        let corners = [
            into_window.map(x, y),
            into_window.map(x + width, y),
            into_window.map(x, y + height),
            into_window.map(x + width, y + height),
        ];

        let left = corners
            .iter()
            .map(|corner| corner.0)
            .fold(f32::MAX, f32::min);
        let top = corners
            .iter()
            .map(|corner| corner.1)
            .fold(f32::MAX, f32::min);
        let right = corners
            .iter()
            .map(|corner| corner.0)
            .fold(f32::MIN, f32::max);
        let bottom = corners
            .iter()
            .map(|corner| corner.1)
            .fold(f32::MIN, f32::max);

        Some((left, top, right - left, bottom - top))
    }

    // the position measured from the area's top-left corner
    pub fn point(&self, x: f32, y: f32) -> Point {
        let (local_x, local_y) = self.inverse.map(x, y);

        Point {
            x: local_x - self.area.x,
            y: local_y - self.area.y,
        }
    }
}

// the targets a parent's child at `place` collected are named by that place too
pub fn nest(targets: &mut [Target], place: usize) {
    for target in targets.iter_mut().filter(|target| !target.anchored) {
        target.key = (target.key ^ (place as u64 + 1)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

/*
 * the targets a widget collected are named by `identity`, what it stands for, and not by the
 * places of its parents: of a list that gains and loses items, an item keeps its key as the ones
 * before it change. Two items with one identity in a frame are told apart by their order
 */
pub fn anchor(targets: &mut [Target], identity: u64) {
    for target in targets.iter_mut().filter(|target| !target.anchored) {
        target.key = (target.key ^ identity).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
        target.anchored = true;
    }
}

/*
 * a target that is clipped only reacts where it still shows,
 * the ones fully clipped out stay in the list so positions don't shift
 */
pub fn clip(targets: &mut [Target], area: Area) {
    for target in targets {
        target.area = target.area.intersect(area);
    }
}
