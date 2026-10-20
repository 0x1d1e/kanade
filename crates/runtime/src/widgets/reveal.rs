use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::graphics::{Area, Renderer};
use crate::input::Target;
use crate::{Size, Widget};

thread_local! {
    // where the last widget to ask was drawn, for the scroll area around it to take
    static MARK: Cell<Option<(Area, u64)>> = const { Cell::new(None) };

    /*
     * the last token each scroll area acted on, by the area's own name: an area honors a token
     * only for itself, so one revealing a row says nothing about another whose widget asked with
     * a token of the same number
     */
    static HONORED: RefCell<HashMap<&'static str, u64>> = RefCell::new(HashMap::new());

    // the scroll area drawing right now, whose name tells a widget asking which one it is in
    static AREA: Cell<Option<&'static str>> = const { Cell::new(None) };
}

/*
 * a widget a scroll area around it moves into view once per token: the keyboard's ring arriving on
 * a row below the fold. A token the area already acted on does nothing, so scrolling by hand
 * afterwards is not undone
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
        let honored = AREA
            .with(|area| HONORED.with_borrow(|honored| honored.get(area.get().as_ref()?).copied()));

        if honored != Some(self.token) {
            MARK.set(Some((area, self.token)));
        }

        self.child.draw(renderer, area);
    }

    fn collect_targets(&self, area: Area, targets: &mut Vec<Target>) {
        self.child.collect_targets(area, targets);
    }
}

/*
 * the scroll area `id` starts drawing its child, so a widget inside it asks for that area; the one
 * already drawing, which an area inside this one is inside, is given back
 */
pub(super) fn enter(id: &'static str) -> Option<&'static str> {
    let outer = AREA.replace(Some(id));

    MARK.set(None);

    outer
}

/*
 * the area `id` finished drawing its child: the area of a widget that asked and was not acted on
 * yet, which is then the one token that area has acted on; `outer` draws again, if there is one
 */
pub(super) fn leave(id: &'static str, outer: Option<&'static str>) -> Option<Area> {
    let marked = MARK.take();

    AREA.set(outer);

    let (area, token) = marked?;

    HONORED.with_borrow_mut(|honored| {
        honored.insert(id, token);
    });

    Some(area)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::graphics::Renderer;

    // the child every reveal here wraps, drawing nothing
    struct Blank;

    impl Widget for Blank {
        fn width(&self) -> Size {
            Size::Fixed(1.0)
        }

        fn height(&self) -> Size {
            Size::Fixed(1.0)
        }

        fn draw(&self, _: &mut Renderer, _: Area) {}
    }

    // a scroll area drawing `child` under `id`, as its own draw does, and taking what it asked
    fn scroll(id: &'static str, child: Reveal) -> Option<Area> {
        let mut renderer = Renderer::new(1.0);

        let outer = enter(id);
        child.draw(&mut renderer, Area::new(0.0, 0.0, 10.0, 10.0));

        leave(id, outer)
    }

    /*
     * two scroll areas whose widgets ask with the same token number: the first revealing it must
     * not stop the second from revealing its own, which one token shared by both did
     */
    #[test]
    fn each_area_honors_its_own_token() {
        assert!(
            scroll("one", Reveal::new(Blank, 7)).is_some(),
            "the area never revealed its widget"
        );

        assert!(
            scroll("two", Reveal::new(Blank, 7)).is_some(),
            "a token another area already honored revealed nothing"
        );

        assert!(
            scroll("one", Reveal::new(Blank, 7)).is_none(),
            "the area revealed the same token twice, undoing a scroll by hand"
        );
    }
}
