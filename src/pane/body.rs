//! The Island's body and Dock plate, united into one outline when they meet (ADR 0031).

// a body's size and shape in its canvas, in logical pixels from the canvas's top left corner
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,

    // a second rounded rect it flows into, as one pane
    pub join: Option<Join>,
}

// a second rounded rect in the body's canvas, which the body is smoothly united with
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Join {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,

    // how far the two rects' edges pull toward each other where they meet, in logical pixels
    pub blend: f32,
}

impl Body {
    // the smallest rect holding the body and what it joins, as x, y, width and height
    pub fn bounds(self) -> (f32, f32, f32, f32) {
        let Some(join) = self.join else {
            return (self.x, self.y, self.width, self.height);
        };

        let (x, y) = (self.x.min(join.x), self.y.min(join.y));

        (
            x,
            y,
            (self.x + self.width).max(join.x + join.width) - x,
            (self.y + self.height).max(join.y + join.height) - y,
        )
    }
}
