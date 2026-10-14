//! A pane's body: its outline in its canvas, what it is united with, and where a copy of what is
//! around it was taken for.

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

    /*
     * the body as rows of a shader's values, which come back with the copy captured around it
     * (`Seen`): its rect, the second rect, and both radii, the blend and 1 if there is a second
     */
    pub(super) fn rows(self) -> [[f32; 4]; 3] {
        let join = self.join.unwrap_or(Join {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            radius: 0.0,
            blend: 0.0,
        });

        [
            [self.x, self.y, self.width, self.height],
            [join.x, join.y, join.width, join.height],
            [
                self.radius,
                join.radius,
                join.blend,
                if self.join.is_some() { 1.0 } else { 0.0 },
            ],
        ]
    }
}

/*
 * where to capture for a body heading to `heading`: out to the further of each edge, so the
 * capture never samples the Island as drawn now, which is inside it
 */
pub fn ahead(body: Body, heading: Body) -> Body {
    let (x, y) = (body.x.min(heading.x), body.y.min(heading.y));

    // each part to the further of its edges, so the outline still has the parts it grows into
    let join = match (body.join, heading.join) {
        (Some(body), Some(heading)) => {
            let (x, y) = (body.x.min(heading.x), body.y.min(heading.y));

            Some(Join {
                x,
                y,
                width: (body.x + body.width).max(heading.x + heading.width) - x,
                height: (body.y + body.height).max(heading.y + heading.height) - y,
                radius: body.radius.max(heading.radius),
                blend: body.blend.max(heading.blend),
            })
        }
        (join, other) => join.or(other),
    };

    Body {
        x,
        y,
        width: (body.x + body.width).max(heading.x + heading.width) - x,
        height: (body.y + body.height).max(heading.y + heading.height) - y,
        radius: body.radius.max(heading.radius),
        join,
    }
}
