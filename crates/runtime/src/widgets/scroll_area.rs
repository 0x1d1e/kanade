mod offsets;
mod targets;

use super::reveal;

use crate::Size;
use crate::graphics::{Area, Renderer};
use crate::input::Target;

use super::Widget;

pub struct ScrollArea {
    // the view builds a fresh area on every redraw, so the offset is kept under this name
    id: &'static str,

    child: Box<dyn Widget>,

    width: Size,
    height: Size,
}

impl ScrollArea {
    pub fn new(id: &'static str, child: impl Widget + 'static) -> Self {
        Self {
            id,
            child: Box::new(child),
            width: Size::Parent,
            height: Size::Parent,
        }
    }

    pub fn width(mut self, width: impl Into<Size>) -> Self {
        self.width = width.into();

        self
    }

    pub fn height(mut self, height: impl Into<Size>) -> Self {
        self.height = height.into();

        self
    }

    // how far the child can move up before its bottom edge meets the area's
    fn farthest(&self, area: Area) -> f32 {
        let content = self.child.height().resolve(area.height);

        f32::max(content - area.height, 0.0)
    }

    // moves so `wanted`, drawn this frame, shows with some room around it, and draws once more
    fn bring_into_view(&self, wanted: Area, area: Area) {
        const ROOM: f32 = 16.0;

        let above = area.y + ROOM - wanted.y;
        let below = wanted.y + wanted.height + ROOM - (area.y + area.height);

        let by = if above > 0.0 {
            -above
        } else if below > 0.0 {
            below
        } else {
            return;
        };

        let current = offsets::get(self.id).clamp(0.0, self.farthest(area));
        offsets::set(self.id, (current + by).clamp(0.0, self.farthest(area)));

        crate::request_frame();
    }

    // the child at its full size, moved up by the offset
    fn child_area(&self, area: Area) -> Area {
        let offset = offsets::get(self.id).clamp(0.0, self.farthest(area));

        let width = self.child.width().resolve(area.width);
        let height = self.child.height().resolve(area.height);

        Area::new(area.x, area.y - offset, width, height)
    }
}

impl Widget for ScrollArea {
    fn width(&self) -> Size {
        self.width
    }

    fn height(&self) -> Size {
        self.height
    }

    fn draw(&self, renderer: &mut Renderer, area: Area) {
        let mut inside = renderer.group();

        reveal::clear();
        self.child.draw(&mut inside, self.child_area(area));

        renderer.clip(inside, area, 0.0);

        if let Some(wanted) = reveal::take() {
            self.bring_into_view(wanted, area);
        }
    }

    fn collect_targets(&self, area: Area, targets: &mut Vec<Target>) {
        targets::collect_targets(self, area, targets);
    }
}
