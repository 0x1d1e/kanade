mod lines;

use ttf_parser::Face;

use crate::graphics::{Area, Renderer, font};
use crate::{Color, Size, Weight};

use super::Widget;

use lines::Rules;

pub struct Text {
    content: String,
    size: f32,
    color: Color,
    font: Option<String>,
    weight: Weight,
    rules: Rules,

    // sized by the letters' own shapes instead of their advance, so an icon centers exactly
    tight: bool,

    // every digit in a cell as wide as the widest, so a changing number does not shift its neighbours
    tabular: bool,
}

impl Text {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            size: 16.0,
            color: Color::BLACK,
            font: None,
            weight: Weight::Regular,
            rules: Rules::default(),
            tight: false,
            tabular: false,
        }
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;

        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = color;

        self
    }

    pub fn font(mut self, family: &str) -> Self {
        self.font = Some(String::from(family));

        self
    }

    pub fn weight(mut self, weight: impl Into<Weight>) -> Self {
        self.weight = weight.into();

        self
    }

    pub fn wrap(mut self) -> Self {
        self.rules.wrap = true;

        self
    }

    pub fn elide(mut self) -> Self {
        self.rules.elide = true;

        self
    }

    pub fn max_lines(mut self, count: usize) -> Self {
        self.rules.max_lines = Some(count);

        self
    }

    pub fn tight(mut self) -> Self {
        self.tight = true;

        self
    }

    /*
     * Digits take equal room, a clock or a level that counts does not jitter. Open Sauce One has
     * `tnum` but the runtime shapes letter by letter, so the cells are laid here; for short,
     * one-line numerals, not wrapped or elided text
     */
    pub fn tabular(mut self) -> Self {
        debug_assert!(
            !(self.rules.wrap || self.rules.elide || self.tight),
            "tabular text is one line of numerals, not wrapped, elided or tight"
        );
        self.tabular = true;

        self
    }

    // the room one digit takes when digits are tabular
    fn cell(&self) -> f32 {
        let face = self.face();

        ('0'..='9')
            .map(|digit| lines::measure(digit.encode_utf8(&mut [0; 4]), face, self.size))
            .fold(0.0, f32::max)
    }

    // the content as runs: each digit alone with its cell, anything between as one run
    fn runs(&self) -> Vec<(String, f32)> {
        let face = self.face();
        let cell = self.cell();
        let mut runs: Vec<(String, f32)> = Vec::new();
        let mut other = String::new();

        let flush = |other: &mut String, runs: &mut Vec<(String, f32)>| {
            if !other.is_empty() {
                let width = lines::measure(other, face, self.size);

                runs.push((std::mem::take(other), width));
            }
        };

        for letter in self.content.chars() {
            if letter.is_ascii_digit() {
                flush(&mut other, &mut runs);
                runs.push((String::from(letter), cell));
            } else {
                other.push(letter);
            }
        }
        flush(&mut other, &mut runs);

        runs
    }

    // how tall it is once laid out in this width, for wrapped text whose height depends on it
    pub fn height_in(&self, width: f32) -> f32 {
        let font = self.face();

        let arranged = lines::arrange(&self.content, font, self.size, width, self.rules);

        lines::stack_height(font, self.size, arranged.len())
    }

    fn face(&self) -> &'static Face<'static> {
        font::load_weighted(self.font.as_deref(), self.weight)
    }
}

impl Widget for Text {
    fn width(&self) -> Size {
        // wrapped or elided text takes the width it is given and fits itself into it
        if self.rules.wrap || self.rules.elide {
            return Size::Parent;
        }

        if self.tight {
            let (left, right) = lines::ink(&self.content, self.face(), self.size);

            return Size::Fixed(right - left);
        }

        if self.tabular {
            return Size::Fixed(self.runs().iter().map(|(_, width)| width).sum());
        }

        Size::Fixed(lines::measure(&self.content, self.face(), self.size))
    }

    fn height(&self) -> Size {
        /*
         * the width is only known when drawing,
         * so wrapped text without a line limit cannot know its height yet
         */
        let count = match (self.rules.wrap, self.rules.max_lines) {
            (false, _) => 1,
            (true, Some(count)) => count,
            (true, None) => return Size::Parent,
        };

        Size::Fixed(lines::stack_height(self.face(), self.size, count))
    }

    fn draw(&self, renderer: &mut Renderer, area: Area) {
        let font = self.face();

        // the ink starts at the area's left edge, not the pen
        let area = if self.tight {
            let (left, _) = lines::ink(&self.content, font, self.size);

            Area::new(area.x - left, area.y, area.width, area.height)
        } else {
            area
        };

        if self.tabular {
            let line_height = lines::height(font, self.size);
            let mut left = area.x;

            for (run, width) in self.runs() {
                // a digit narrower than its cell sits in the middle of it
                let own = lines::measure(&run, font, self.size);
                let inset = if run.chars().all(|c| c.is_ascii_digit()) {
                    (width - own) / 2.0
                } else {
                    0.0
                };

                renderer.text(
                    &run,
                    font,
                    self.size,
                    self.color,
                    Area::new(left + inset, area.y, own.max(1.0), line_height),
                );

                left += width;
            }

            return;
        }

        let arranged = lines::arrange(&self.content, font, self.size, area.width, self.rules);

        let line_height = lines::height(font, self.size);
        let spacing = lines::spacing(font, self.size);

        let mut top = area.y;

        for line in arranged {
            let line_area = Area::new(area.x, top, area.width, line_height);

            renderer.text(&line, font, self.size, self.color, line_area);

            top += spacing;
        }
    }
}
