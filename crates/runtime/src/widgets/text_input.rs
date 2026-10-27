mod field;
mod fields;
mod targets;

pub use field::single_line;

use std::rc::Rc;

use crate::graphics::{Area, Renderer};
use crate::input::{Target, focus};
use crate::{Color, Size, Text};

use super::Widget;

use field::Field;

// shared, so the key handler kept after a redraw still reaches it
pub type TextHandler = Rc<dyn Fn(String)>;

const CURSOR_WIDTH: f32 = 1.5;

// the letters of the composed text an input method's preedit takes
struct Composing {
    from: usize,
    to: usize,

    // the letters of the preedit its cursor marks, none when the method hides it
    marked: Option<(usize, usize)>,
}

pub struct TextInput {
    // the view builds a fresh input on every redraw, so the text is kept under this name
    id: &'static str,

    placeholder: String,
    size: f32,
    color: Color,
    width: Size,

    // shows a dot in place of each letter
    password: bool,

    // takes the keyboard as soon as it is drawn, without a click
    focused: bool,

    on_change: Option<TextHandler>,
    on_submit: Option<TextHandler>,
}

impl TextInput {
    pub fn new(id: &'static str) -> Self {
        Self {
            id,
            placeholder: String::new(),
            size: 16.0,
            color: Color::BLACK,
            width: Size::Parent,
            password: false,
            focused: false,
            on_change: None,
            on_submit: None,
        }
    }

    pub fn width(mut self, width: impl Into<Size>) -> Self {
        self.width = width.into();

        self
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;

        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = color;

        self
    }

    // shown faded while the input is empty
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();

        self
    }

    pub fn password(mut self) -> Self {
        self.password = true;

        self
    }

    pub fn focused(mut self) -> Self {
        self.focused = true;

        self
    }

    // replaces what the input holds, with the cursor at the end
    pub fn set_text(id: &'static str, text: &str) {
        let field = Field {
            text: String::from(text),
            cursor: text.chars().count(),
            ..Field::default()
        };

        fields::set(id, field);
    }

    // what the input holds now; on the draw thread, which the inputs live on
    pub fn text(id: &'static str) -> String {
        fields::get(id).text
    }

    // the input of this id takes the keyboard when it is next drawn
    pub fn focus(id: &'static str) {
        focus::want(id);
    }

    // whether this input currently receives keys
    pub fn is_focused(id: &'static str) -> bool {
        focus::has(id)
    }

    // gives up the keyboard only when this input has it
    pub fn blur_if_focused(id: &'static str) -> bool {
        focus::clear_if(id)
    }

    // takes the keyboard from whichever input has it
    pub fn blur() {
        focus::clear();
    }

    // no input is being composed in any more, which is what the input method leaving means
    pub(crate) fn end_composition() {
        fields::end_composition();
    }

    // runs after every change to the text, with the whole new text
    pub fn on_change(mut self, handler: impl Fn(String) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));

        self
    }

    // runs when Enter is pressed
    pub fn on_submit(mut self, handler: impl Fn(String) + 'static) -> Self {
        self.on_submit = Some(Rc::new(handler));

        self
    }

    // the text as it is drawn, which hides a password
    fn shown(&self, text: &str) -> String {
        if !self.password {
            return String::from(text);
        }

        "•".repeat(text.chars().count())
    }

    fn label(&self, content: impl Into<String>, color: Color) -> Text {
        Text::new(content).size(self.size).color(color)
    }

    // the text with what an input method is composing at the cursor, which is not in the field yet
    fn composed(field: &Field) -> String {
        let Some(preedit) = &field.preedit else {
            return field.text.clone();
        };

        let mut text = field.text.clone();

        text.insert_str(field.byte_index(field.cursor), &preedit.text);

        text
    }

    // how far from the input's left edge the first `index` letters of the composed text reach
    fn offset(&self, text: &str, index: usize, area: Area) -> f32 {
        let before: String = text.chars().take(index).collect();

        let before = self.label(self.shown(&before), self.color);

        before.width().resolve(area.width)
    }

    // the letters of the composed text the preedit takes, and where its cursor sits among them
    fn composing(field: &Field) -> Option<Composing> {
        let preedit = field.preedit.as_ref()?;

        let letters = |end: usize| preedit.text[..end].chars().count();

        let marked = preedit
            .cursor
            .map(|(from, to)| (letters(from), letters(to)));

        Some(Composing {
            from: field.cursor,
            to: field.cursor + letters(preedit.text.len()),
            marked,
        })
    }

    // the letter index the caret stands at, none while the input method hides it
    fn caret_at(field: &Field) -> Option<usize> {
        match Self::composing(field) {
            None => Some(field.cursor),
            Some(Composing { from, marked, .. }) => marked.map(|(_, to)| from + to),
        }
    }

    fn draw_cursor(&self, renderer: &mut Renderer, text: &str, at: usize, area: Area) {
        let offset = self.offset(text, at, area);

        let cursor = Area::new(area.x + offset, area.y, CURSOR_WIDTH, area.height);

        renderer.rectangle(cursor, self.color, 0.0);
    }

    // the text color at a third of its strength, behind the letters
    fn draw_selection(
        &self,
        renderer: &mut Renderer,
        text: &str,
        range: (usize, usize),
        area: Area,
    ) {
        let from = self.offset(text, range.0, area);
        let to = self.offset(text, range.1, area);

        let tint = Color::rgba(self.color.r, self.color.g, self.color.b, self.color.a / 3);

        renderer.rectangle(
            Area::new(area.x + from, area.y, to - from, area.height),
            tint,
            0.0,
        );
    }

    // an underline below the letters an input method is composing, the marked part tinted
    fn draw_preedit(&self, renderer: &mut Renderer, text: &str, field: &Field, area: Area) {
        let Some(Composing { from, to, marked }) = Self::composing(field) else {
            return;
        };

        if let Some((begin, end)) = marked.filter(|(begin, end)| begin != end) {
            self.draw_selection(renderer, text, (from + begin, from + end), area);
        }

        let left = self.offset(text, from, area);
        let right = self.offset(text, to, area);

        let underline = Area::new(area.x + left, area.y + area.height - 1.0, right - left, 1.0);

        renderer.rectangle(underline, self.color, 0.0);
    }

    // where the caret is, for an input method's popup to sit by; wide enough to point at
    fn caret_area(&self, field: &Field, area: Area) -> Area {
        let text = Self::composed(field);

        let at = Self::caret_at(field)
            .or_else(|| Self::composing(field).map(|composing| composing.to))
            .unwrap_or(field.cursor);

        // a password's caret would tell its length to the input method, so it stays at the start
        let offset = match self.password {
            true => 0.0,
            false => self.offset(&text, at, area).min(area.width),
        };

        Area::new(area.x + offset, area.y, 1.0, area.height)
    }
}

impl Widget for TextInput {
    fn width(&self) -> Size {
        self.width
    }

    fn height(&self) -> Size {
        self.label("", self.color).height()
    }

    fn draw(&self, renderer: &mut Renderer, area: Area) {
        let field = fields::get(self.id);
        let text = Self::composed(&field);

        // the placeholder is the text color at half strength
        let faded = Color::rgba(self.color.r, self.color.g, self.color.b, self.color.a / 2);

        let label = if text.is_empty() {
            self.label(self.placeholder.as_str(), faded)
        } else {
            self.label(self.shown(&text), self.color)
        };

        // text longer than the input is cut off at its edge
        let mut inside = renderer.group();

        let focused = focus::has(self.id);

        if focused && let Some(range) = field.selection() {
            self.draw_selection(&mut inside, &text, range, area);
        }

        label.draw(&mut inside, area);

        if focused {
            self.draw_preedit(&mut inside, &text, &field, area);

            if let Some(at) = Self::caret_at(&field) {
                self.draw_cursor(&mut inside, &text, at, area);
            }
        }

        renderer.clip(inside, area, 0.0);
    }

    fn collect_targets(&self, area: Area, targets: &mut Vec<Target>) {
        targets::collect_targets(self, area, targets);
    }
}
