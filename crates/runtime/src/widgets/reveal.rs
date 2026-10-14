use std::cell::Cell;

use crate::graphics::{Area, Renderer};
use crate::input::Target;
use crate::{Size, Widget};

thread_local! {
    // where the last widget to ask was drawn, for the scroll area around it to take
    static MARK: Cell<Option<(Area, u64)>> = const { Cell::new(None) };

    // the last token a scroll area acted on
    static HONORED: Cell<u64> = const { Cell::new(0) };
}

/*
 * a widget a scroll area around it moves into view once per token: the keyboard's ring arriving on
 * a row below the fold. A token already seen does nothing, so scrolling by hand afterwards is
 * not undone
 */
pub struct Reveal {
    child: Box<dyn Widget>,
    token: u64,
}

impl Reveal {
    pub fn new(child: impl Widget + 'static, token: u64) -> Self {
        Self {
            child: Box::new(child),
            token,
        }
    }
}

impl Widget for Reveal {
    fn width(&self) -> Size {
        self.child.width()
    }

    fn height(&self) -> Size {
        self.child.height()
    }

    fn draw(&self, renderer: &mut Renderer, area: Area) {
        if HONORED.get() != self.token {
            MARK.set(Some((area, self.token)));
        }

        self.child.draw(renderer, area);
    }

    fn collect_targets(&self, area: Area, targets: &mut Vec<Target>) {
        self.child.collect_targets(area, targets);
    }
}

pub(super) fn clear() {
    MARK.set(None);
}

// the area of a widget that asked and was not acted on yet, which is then marked as acted on
pub(super) fn take() -> Option<Area> {
    let (area, token) = MARK.take()?;

    HONORED.set(token);

    Some(area)
}
