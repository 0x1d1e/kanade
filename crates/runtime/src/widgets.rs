mod canvas;
mod column;
mod layout;
mod rectangle;
mod rise;
mod row;
mod scroll_area;
mod shape;
mod stack;
mod text;
mod text_input;

use crate::Size;
use crate::graphics::{Area, Renderer};
use crate::input::Target;

pub use canvas::Canvas;
pub use column::Column;
pub use layout::{Direction, Layout};
pub use rectangle::Rectangle;
pub use rise::Rise;
pub use row::Row;
pub use scroll_area::ScrollArea;
pub use shape::{Arc, Circle, Line, Path, Shape};
pub use stack::Stack;
pub use text::Text;
pub use text_input::TextInput;

pub trait Widget {
    fn width(&self) -> Size;

    fn height(&self) -> Size;

    fn draw(&self, renderer: &mut Renderer, area: Area);

    // widgets that react to the pointer add their area, containers pass theirs on to children
    fn collect_targets(&self, _: Area, _: &mut Vec<Target>) {}
}

// a boxed widget is a widget, so a wrapper can hold one chosen at run time
impl Widget for Box<dyn Widget> {
    fn width(&self) -> Size {
        self.as_ref().width()
    }

    fn height(&self) -> Size {
        self.as_ref().height()
    }

    fn draw(&self, renderer: &mut Renderer, area: Area) {
        self.as_ref().draw(renderer, area);
    }

    fn collect_targets(&self, area: Area, targets: &mut Vec<Target>) {
        self.as_ref().collect_targets(area, targets);
    }
}
