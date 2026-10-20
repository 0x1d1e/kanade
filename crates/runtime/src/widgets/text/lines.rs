use ttf_parser::Face;

use crate::graphics::font;

// how text that is too wide for its area gets handled
#[derive(Debug, Clone, Copy, Default)]
pub struct Rules {
    pub wrap: bool,
    pub elide: bool,
    pub max_lines: Option<usize>,
}

pub fn measure(content: &str, font: &Face, size: f32) -> f32 {
    let mut total = 0.0;

    for letter in content.chars() {
        total += font::measure_letter(font, letter, size).advance;
    }

    total
}

// the height of one line, from the top of the tallest letter to the bottom of the lowest
pub fn height(font: &Face, size: f32) -> f32 {
    let units = size / f32::from(font.units_per_em());

    let ascent = f32::from(font.ascender()) * units;
    let descent = f32::from(font.descender()) * units;

    // descent is negative, so this adds the part below the baseline
    ascent - descent
}

// the distance from the top of one line to the top of the next
pub fn spacing(font: &Face, size: f32) -> f32 {
    let units = size / f32::from(font.units_per_em());

    let gap = f32::from(font.line_gap()) * units;

    height(font, size) + gap
}

// the height of several lines stacked, without a gap after the last one
pub fn stack_height(font: &Face, size: f32, count: usize) -> f32 {
    if count == 0 {
        return 0.0;
    }

    let above_last = spacing(font, size) * (count - 1) as f32;

    above_last + height(font, size)
}

pub fn arrange(content: &str, font: &Face, size: f32, width: f32, rules: Rules) -> Vec<String> {
    let mut lines = match rules.wrap {
        true => wrap(content, font, size, width),
        false => vec![String::from(content)],
    };

    let limit = rules.max_lines.unwrap_or(usize::MAX);

    let cut = lines.len() > limit;

    lines.truncate(limit);

    if !rules.elide {
        return lines;
    }

    let Some(last) = lines.last_mut() else {
        return lines;
    };

    // a line that was cut off also gets the "…", even if it fits
    if cut || measure(last, font, size) > width {
        *last = elide(last, font, size, width);
    }

    lines
}

// fills each line with whole words until the next one would not fit
fn wrap(content: &str, font: &Face, size: f32, width: f32) -> Vec<String> {
    let mut lines = Vec::new();

    for paragraph in content.split('\n') {
        let mut line = String::new();

        for word in paragraph.split(' ') {
            // a word wider than the area still gets a line of its own
            if line.is_empty() {
                line = String::from(word);

                continue;
            }

            let longer = format!("{line} {word}");

            if measure(&longer, font, size) <= width {
                line = longer;

                continue;
            }

            lines.push(line);

            line = String::from(word);
        }

        lines.push(line);
    }

    lines
}

// drops letters from the end until the rest and "…" fit
fn elide(line: &str, font: &Face, size: f32, width: f32) -> String {
    let dot = font::measure_letter(font, '…', size).advance;

    elided(line.trim_end(), dot, width, |letter| {
        font::measure_letter(font, letter, size).advance
    })
}

/*
 * `kept` cut to its longest prefix that still fits with `dot` after it, or none at all, so only
 * the "…" shows; the advances accumulate once over the line instead of measuring every shorter
 * copy of it again, which cost a long line a measure per letter dropped
 */
fn elided(kept: &str, dot: f32, width: f32, advance: impl Fn(char) -> f32) -> String {
    let mut pen = 0.0;
    let mut fitting = 0;

    for (at, letter) in kept.char_indices() {
        pen += advance(letter);

        if pen + dot <= width {
            fitting = at + letter.len_utf8();
        }
    }

    let mut shortened = String::with_capacity(fitting + '…'.len_utf8());

    shortened.push_str(&kept[..fitting]);
    shortened.push('…');

    shortened
}

// where the letters' ink starts and ends, counted from the pen's starting point
pub fn ink(content: &str, font: &Face, size: f32) -> (f32, f32) {
    let mut pen = 0.0;

    let mut left = f32::MAX;
    let mut right = f32::MIN;

    for letter in content.chars() {
        let measured = font::measure_letter(font, letter, size);

        // a space has no ink, only an advance
        if let Some(bounds) = measured.face.glyph_bounding_box(measured.id) {
            left = left.min(pen + f32::from(bounds.x_min) * measured.units);
            right = right.max(pen + f32::from(bounds.x_max) * measured.units);
        }

        pen += measured.advance;
    }

    // nothing but spaces is as wide as its advance
    if left > right {
        return (0.0, pen);
    }

    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    // the elision as it was: every shorter copy of the line, the "…" after it, measured whole again
    fn elided_one_letter_at_a_time(
        line: &str,
        width: f32,
        advance: impl Fn(char) -> f32,
    ) -> String {
        let mut kept = String::from(line);

        loop {
            let shortened = format!("{kept}…");
            let total: f32 = shortened.chars().map(&advance).sum();

            if kept.is_empty() || total <= width {
                return shortened;
            }

            kept.pop();
        }
    }

    /*
     * the sums that replaced a measure per letter dropped have to cut the line where it was cut
     * before, letter for letter: a long line elided a letter short or a letter long is wrong text
     */
    #[test]
    fn eliding_cuts_where_it_always_did() {
        // letters of uneven widths, so a sum that drifts shows up as a different cut
        let advance = |letter: char| match letter {
            '…' => 7.0,
            'i' | 'l' | '.' => 3.0,
            'm' | 'w' => 13.0,
            ' ' => 4.0,
            _ => 8.0,
        };

        // ten thousand letters, the length a long line of text reaches and where the old measure
        // per letter dropped was slowest
        let line: String = "the quick brown fox jumps over the lazy dog "
            .chars()
            .cycle()
            .take(10_000)
            .collect();

        for width in [0.0f32, 7.0, 40.0, 123.5, 1000.0, 7993.25, 40_000.0] {
            let kept = line.trim_end();

            assert_eq!(
                elided(kept, advance('…'), width, advance),
                elided_one_letter_at_a_time(kept, width, advance),
                "eliding at width {width} cut it elsewhere than it used to"
            );
        }
    }
}
