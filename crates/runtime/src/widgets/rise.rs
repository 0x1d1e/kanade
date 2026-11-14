use crate::graphics::{Area, Renderer, Transform};
use crate::input::{Handlers, Target, nest};
use crate::{Size, Widget};

/*
 * a widget shown at an opacity and lifted by some pixels, its layout and hit area as they would be:
 * the same size as the widget, its targets moved with it. For content that arrives a part at a time
 */
pub struct Rise {
    child: Box<dyn Widget>,
    opacity: f32,
    lift: f32,
    inert: bool,
}

impl Rise {
    pub fn new(child: impl Widget + 'static) -> Self {
        Self {
            child: Box::new(child),
            opacity: 1.0,
            lift: 0.0,
            inert: false,
        }
    }

    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;

        self
    }

    // Pointer eligibility belongs to the displayed pose, including any enclosing content fade.
    pub fn inert(mut self, inert: bool) -> Self {
        self.inert = inert;
        self
    }

    // how far below its place it is drawn, down
    pub fn lift(mut self, lift: f32) -> Self {
        self.lift = lift;

        self
    }
}

impl Widget for Rise {
    fn width(&self) -> Size {
        self.child.width()
    }

    fn height(&self) -> Size {
        self.child.height()
    }

    fn draw(&self, renderer: &mut Renderer, area: Area) {
        // arrived: drawn as if it were not wrapped, with no group of its own
        if self.opacity == 1.0 && self.lift == 0.0 {
            self.child.draw(renderer, area);

            return;
        }

        renderer.transformed(Transform::from_translate(0.0, self.lift), |renderer| {
            let mut group = renderer.group();

            self.child.draw(&mut group, area);
            renderer.blend(group, self.opacity);
        });
    }

    fn collect_targets(&self, area: Area, targets: &mut Vec<Target>) {
        let first = targets.len();
        let mut barrier = Target::new(area, Handlers::default());
        if self.inert {
            barrier.make_inert();
        }
        targets.push(barrier);
        let children = targets.len();
        self.child.collect_targets(area, targets);
        nest(&mut targets[children..], 0);
        if self.inert {
            for target in &mut targets[children..] {
                target.make_inert();
            }
        }

        let Some(inverse) = Transform::from_translate(0.0, self.lift).invert() else {
            return;
        };

        // the lift is undone first, then whatever the child added inside it
        for target in &mut targets[first..] {
            target.inverse = inverse.post_concat(target.inverse);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;
    use crate::input::Pointer;
    use crate::{Button, Rectangle, Stack};

    #[test]
    fn a_section_below_the_effective_opacity_gate_consumes_clicks() {
        let calls = Rc::new(Cell::new([0, 0]));
        let frame = |outer: f32| {
            let (back, front) = (calls.clone(), calls.clone());
            let behind = Rectangle::new()
                .width(100.0)
                .height(100.0)
                .on_click(move |_| {
                    let mut value = back.get();
                    value[0] += 1;
                    back.set(value);
                });
            let section = Rectangle::new()
                .width(100.0)
                .height(100.0)
                .on_click(move |_| {
                    let mut value = front.get();
                    value[1] += 1;
                    front.set(value);
                });
            let section = Rise::new(section)
                .opacity(0.8)
                .lift(10.0)
                .inert(outer * 0.8 < 0.5);
            let mut targets = Vec::new();
            Stack::new(crate::children![behind, section])
                .collect_targets(Area::new(0.0, 0.0, 100.0, 100.0), &mut targets);
            targets
        };
        let mut pointer = Pointer::default();
        pointer.set_targets(frame(0.6));
        pointer.move_to(20.0, 20.0);
        pointer.press();
        pointer.release(Button::Left);
        assert_eq!(calls.get(), [0, 0]);
        pointer.set_targets(frame(0.7));
        pointer.press();
        pointer.set_targets(frame(0.6));
        pointer.set_targets(frame(0.7));
        pointer.release(Button::Left);
        assert_eq!(calls.get(), [0, 0]);
        pointer.press();
        pointer.release(Button::Left);
        assert_eq!(calls.get(), [0, 1]);
    }
}
