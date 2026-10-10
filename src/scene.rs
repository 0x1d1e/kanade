//! Where the parts of one output's Island window stand: the body, what takes the pointer, the
//! glass behind it and, merged with the Dock (ADR 0031), the Dock's plate under one pane. A pure
//! function of the Island's pose, the shell and the Dock's strip: `view::island` draws from it,
//! hit-tests with it, hands its input region to the runtime and its body to liquid glass at the
//! instants the capture covers, so each reads one answer. What reads Services, as the pose at an
//! instant, stays with its caller.

use crate::autohide;
use crate::glass;
use crate::island::geometry::{self, Canvas, Hang, Rect, Shape};
use crate::merge::{self, Plate, Shell};

/*
 * where the Island's canvas stands on its output: its size, and which edge and side it hangs
 * from. Everything in the canvas is placed by it
 */
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stand {
    pub canvas: Canvas,
    pub hang: Hang,
}

// the Island's body at an instant: its shape, bounded by the canvas, and how far out it is, 0 to 1
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub shape: Shape,
    pub out: f32,
}

impl Stand {
    // a pose, whose shape is what the canvas holds of it
    pub fn pose(self, shape: Shape, out: f32) -> Pose {
        Pose {
            shape: geometry::bounded(shape, self.canvas),
            out,
        }
    }

    // the body in the canvas, slid `out` of the way out
    pub fn body(self, pose: Pose) -> Rect {
        autohide::slide(
            geometry::body(pose.shape, self.hang, self.canvas),
            self.hang,
            pose.out,
        )
    }

    /*
     * what takes the pointer in the canvas: the body's area, and the small form `under` it, which
     * a morph took from a still pointer; with the Island hidden, the strip beside its edge
     */
    pub fn area(self, pose: Pose, under: Option<Shape>) -> Rect {
        geometry::holding(
            autohide::area(
                geometry::body(pose.shape, self.hang, self.canvas),
                self.hang,
                self.canvas,
                pose.out,
            ),
            under,
            self.hang,
            self.canvas,
        )
    }

    // the body as liquid glass knows it, in the canvas
    pub fn liquid(self, pose: Pose) -> glass::Body {
        let rect = self.body(pose);

        glass::Body {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            radius: pose.shape.radius,
            join: None,
        }
    }
}

// what the merged shell needs of the Dock's layout, in the Dock's own coordinates
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dock {
    // the strip now, where the glass goes, and its corners' radius
    pub strip: Rect,
    pub radius: f32,

    // how far the pointer holds it out of the Island it is folded into, 0 to 1; 1 otherwise
    pub held: f32,

    // how far the icons grown under the pointer pushed each side of a Dock around the Island out
    pub swelled: (f32, f32),

    // where the pointer reaches it; none while it takes none
    pub area: Option<Rect>,
}

/*
 * the Island's window: its canvas, and when merged with the Dock the shell the canvas and the Dock
 * lie in, with the Dock once it is laid out around the Island's body
 */
pub struct Scene {
    stand: Stand,
    shell: Option<Shell>,
    dock: Option<Dock>,
}

impl Scene {
    pub fn new(stand: Stand, shell: Option<Shell>) -> Self {
        Self {
            stand,
            shell,
            dock: None,
        }
    }

    // once the Dock is laid out around the body
    pub fn with_dock(self, dock: Dock) -> Self {
        Self {
            dock: Some(dock),
            ..self
        }
    }

    // where the Island's canvas lies in the window
    pub fn origin(&self) -> (f32, f32) {
        self.shell.as_ref().map_or((0.0, 0.0), |shell| shell.island)
    }

    // the window's size: the canvas, or the shell that holds it and the Dock
    pub fn window(&self) -> (f32, f32) {
        self.shell.as_ref().map_or(
            (self.stand.canvas.width, self.stand.canvas.height),
            |shell| shell.size,
        )
    }

    // a rectangle of the canvas, in the window
    pub fn in_window(&self, rect: Rect) -> Rect {
        moved_by(rect, self.origin())
    }

    // the Dock's plate in the window, once it is laid out
    pub fn plate(&self) -> Option<Plate> {
        let (shell, dock) = self.shell.as_ref().zip(self.dock.as_ref())?;

        Some(Plate {
            rect: moved_by(dock.strip, shell.dock),
            radius: dock.radius,
        })
    }

    /*
     * the body as liquid glass knows it, in the window: merged, joined to the Dock's plate, which
     * among the icons is the plate around this very body, unfolded as far as the pointer holds
     * the icons out
     */
    pub fn glass(&self, pose: Pose) -> glass::Body {
        let body = self.stand.liquid(pose);
        let origin = self.origin();

        match (&self.shell, &self.dock, self.plate()) {
            (Some(shell), Some(dock), _) if shell.among() => {
                let at = Rect {
                    x: body.x + origin.0,
                    y: body.y + origin.1,
                    width: body.width,
                    height: body.height,
                };
                let unfold = shell.unfold(dock.held, body.height);

                united(
                    body,
                    origin,
                    shell.plate(at, body.radius, unfold, dock.swelled),
                )
            }
            (.., Some(plate)) => united(body, origin, plate),
            _ => body,
        }
    }

    /*
     * where the pointer reaches the window: the Island's area, and the Dock's if merged, in the
     * window's coordinates
     */
    pub fn reach(&self, pose: Pose, under: Option<Shape>) -> Vec<Rect> {
        let island = self.in_window(self.stand.area(pose, under));
        let dock = self
            .shell
            .as_ref()
            .zip(self.dock.and_then(|dock| dock.area))
            .map(|(shell, area)| moved_by(area, shell.dock));

        std::iter::once(island).chain(dock).collect()
    }
}

// a rect moved by an origin
fn moved_by(rect: Rect, origin: (f32, f32)) -> Rect {
    Rect {
        x: rect.x + origin.0,
        y: rect.y + origin.1,
        ..rect
    }
}

// the body, from the Island's canvas at `origin` in the window, united with the Dock's `plate` there
fn united(body: glass::Body, origin: (f32, f32), plate: Plate) -> glass::Body {
    glass::Body {
        x: body.x + origin.0,
        y: body.y + origin.1,
        join: Some(glass::Join {
            x: plate.rect.x,
            y: plate.rect.y,
            width: plate.rect.width,
            height: plate.rect.height,
            radius: plate.radius,
            blend: merge::BLEND,
        }),
        ..body
    }
}

// a united body from the point `origin` of its window, as a pane that begins there sees it
pub fn within(body: glass::Body, origin: (f32, f32)) -> glass::Body {
    glass::Body {
        x: body.x - origin.0,
        y: body.y - origin.1,
        join: body.join.map(|join| glass::Join {
            x: join.x - origin.0,
            y: join.y - origin.1,
            ..join
        }),
        ..body
    }
}
