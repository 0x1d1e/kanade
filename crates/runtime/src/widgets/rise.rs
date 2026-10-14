use crate::graphics::{Area, Renderer, Transform};
use crate::input::Target;
use crate::{Size, Widget};

/*
 * a widget shown at an opacity and lifted by some pixels, its layout and hit area as they would be:
 * the same size as the widget, its targets moved with it. For content that arrives a part at a time
 */
pub struct Rise {
    child: Box<dyn Widget>,
    opacity: f32,
    lift: f32,
}

impl Rise {
    pub fn new(child: impl Widget + 'static) -> Self {
        Self {
            child: Box::new(child),
            opacity: 1.0,
            lift: 0.0,
        }
    }

    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;

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

        self.child.collect_targets(area, targets);

        let Some(inverse) = Transform::from_translate(0.0, self.lift).invert() else {
            return;
        };

        // the lift is undone first, then whatever the child added inside it
        for target in &mut targets[first..] {
            target.inverse = inverse.post_concat(target.inverse);
        }
    }
}
