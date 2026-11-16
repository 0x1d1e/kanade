use ttf_parser::Face;
use unicode_linebreak::{BreakOpportunity, linebreaks};
use unicode_segmentation::UnicodeSegmentation;

use crate::graphics::shape;

// how text that is too wide for its area gets handled
#[derive(Debug, Clone, Copy, Default)]
pub struct Rules {
    pub wrap: bool,
    pub break_words: bool,
    pub elide: bool,
    pub max_lines: Option<usize>,
}

pub fn measure(content: &str, font: &'static Face<'static>, size: f32) -> f32 {
    shape::width(content, font, size)
}

// the height of one line, from the top of the tallest letter to the bottom of the lowest
pub fn height(font: &Face, size: f32) -> f32 {
    shape::metrics(font, size).line_height
}

// the distance from the top of one line to the top of the next
pub fn spacing(font: &Face, size: f32) -> f32 {
    shape::metrics(font, size).spacing
}

// the height of several lines stacked, without a gap after the last one
pub fn stack_height(font: &Face, size: f32, count: usize) -> f32 {
    if count == 0 {
        return 0.0;
    }

    let above_last = spacing(font, size) * (count - 1) as f32;

    above_last + height(font, size)
}

pub fn arrange(
    content: &str,
    font: &'static Face<'static>,
    size: f32,
    width: f32,
    rules: Rules,
) -> Vec<String> {
    let mut lines = match rules.wrap {
        true => wrap(content, font, size, width),
        false => vec![String::from(content)],
    };

    if rules.break_words {
        lines = lines
            .into_iter()
            .flat_map(|line| {
                let advances = shape::grapheme_advances(&line, font, size);
                let mut index = 0;

                break_long(&line, width, move |_| {
                    let advance = advances.get(index).copied().unwrap_or(0.0);

                    index += 1;

                    advance
                })
            })
            .collect();
    }

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

/*
 * fills each line with whole words until the next UAX-14 opportunity would not fit; a word wider
 * than the area still gets a line of its own, and `break_words` cuts it afterwards
 */
fn wrap(content: &str, font: &'static Face<'static>, size: f32, width: f32) -> Vec<String> {
    let mut lines = Vec::new();

    for paragraph in content.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());

            continue;
        }

        wrap_paragraph(paragraph, font, size, width, &mut lines);
    }

    lines
}

fn wrap_paragraph(
    paragraph: &str,
    font: &'static Face<'static>,
    size: f32,
    width: f32,
    lines: &mut Vec<String>,
) {
    let starts: Vec<usize> = paragraph.grapheme_indices(true).map(|(at, _)| at).collect();

    // where each grapheme ends, so a summed run finds its width without shaping again
    let mut prefix = Vec::with_capacity(starts.len() + 1);
    prefix.push(0.0);

    for advance in shape::grapheme_advances(paragraph, font, size) {
        let last = prefix.last().copied().unwrap_or(0.0);

        prefix.push(last + advance);
    }

    pack(paragraph, &starts, &prefix, width, lines);
}

// chooses each line's end at a UAX-14 opportunity, given where the graphemes start and how wide
fn pack(paragraph: &str, starts: &[usize], prefix: &[f32], width: f32, lines: &mut Vec<String>) {
    let count = starts.len();

    // UAX-14 opportunities, snapped down to grapheme boundaries so nothing is split
    let mut opportunities: Vec<(usize, usize, BreakOpportunity)> = Vec::new();

    for (at, opportunity) in linebreaks(paragraph) {
        let index = if at >= paragraph.len() {
            starts.len()
        } else {
            match starts.binary_search(&at) {
                Ok(index) => index,
                Err(0) => 0,
                Err(index) => index - 1,
            }
        };

        if opportunities
            .last()
            .is_some_and(|last: &(usize, usize, BreakOpportunity)| last.1 == index)
        {
            continue;
        }

        let end = starts.get(index).copied().unwrap_or(paragraph.len());

        opportunities.push((end, index, opportunity));
    }

    let mut start = 0;
    let mut cursor = 0;

    while start < count {
        while cursor < opportunities.len() && opportunities[cursor].1 <= start {
            cursor += 1;
        }

        if cursor == opportunities.len() {
            break;
        }

        let mut best = None;
        let mut scan = cursor;

        while scan < opportunities.len() {
            let (_, index, opportunity) = opportunities[scan];
            let used = prefix[index] - prefix[start];

            // always let one grapheme through, even below the width
            if used > width && index > start + 1 {
                break;
            }

            best = Some(opportunities[scan]);
            scan += 1;

            // no later opportunity can be taken, the break is mandatory
            if opportunity == BreakOpportunity::Mandatory {
                break;
            }
        }

        let (end, index, _) = match best {
            Some(best) => {
                cursor = scan;

                best
            }
            // nothing fits before the first opportunity: the word takes a line of its own
            None => {
                cursor += 1;

                opportunities[cursor - 1]
            }
        };

        lines.push(String::from(&paragraph[starts[start]..end]));

        // the space an allowed break sits after hangs at the line's end, as it always did
        if let Some(line) = lines.last_mut() {
            let trimmed = line.trim_end_matches(' ').len();

            line.truncate(trimmed);
        }

        start = index;
    }
}

// Only the opt-in overflow mode splits a token, always at a grapheme boundary.
fn break_long(line: &str, width: f32, mut advance: impl FnMut(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut part = String::new();
    let mut used = 0.0;

    for grapheme in line.graphemes(true) {
        let next = advance(grapheme);

        if !part.is_empty() && used + next > width {
            lines.push(std::mem::take(&mut part));
            used = 0.0;
        }

        part.push_str(grapheme);
        used += next;
    }

    lines.push(part);
    lines
}

// drops graphemes from the end until the rest and "…" fit
fn elide(line: &str, font: &'static Face<'static>, size: f32, width: f32) -> String {
    let kept = line.trim_end();
    let dot = measure("…", font, size);

    let advances = shape::grapheme_advances(kept, font, size);
    let mut index = 0;

    elided(kept, dot, width, move |_| {
        let advance = advances.get(index).copied().unwrap_or(0.0);

        index += 1;

        advance
    })
}

/*
 * `kept` cut to its longest grapheme prefix that still fits with `dot` after it, or none at all,
 * so only the "…" shows
 */
fn elided(kept: &str, dot: f32, width: f32, mut advance: impl FnMut(&str) -> f32) -> String {
    let mut pen = 0.0;
    let mut fitting = 0;

    for grapheme in kept.graphemes(true) {
        pen += advance(grapheme);

        if pen + dot <= width {
            fitting += grapheme.len();
        } else {
            break;
        }
    }

    let mut shortened = String::with_capacity(fitting + '…'.len_utf8());

    shortened.push_str(&kept[..fitting]);
    shortened.push('…');

    shortened
}

// where the glyphs' ink starts and ends, counted from the pen's starting point
pub fn ink(content: &str, font: &'static Face<'static>, size: f32) -> (f32, f32) {
    shape::ink(content, font, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflowing_tokens_remain_complete_and_make_progress_in_narrow_areas() {
        let value = "org.example.界.action".repeat(100);
        let widths = |grapheme: &str| if grapheme == "界" { 2.0 } else { 1.0 };
        let lines = break_long(&value, 12.0, widths);
        assert_eq!(lines.concat(), value);
        assert!(
            lines
                .iter()
                .all(|line| line.graphemes(true).map(widths).sum::<f32>() <= 12.0)
        );
        let narrow = break_long(&value, 0.0, widths);
        assert_eq!(narrow.concat(), value);
        assert!(narrow.iter().all(|line| line.graphemes(true).count() == 1));
    }

    // a long CJK line has no spaces, so only UAX-14 opportunities can wrap it
    #[test]
    fn a_long_cjk_line_wraps_at_unicode_opportunities() {
        // every ideograph is one grapheme, an opportunity comes between any two of them
        let value = "東京都渋谷区神宮前".repeat(40);

        let lines = wrapped_with(&value, 1.0, 4.0);

        assert!(lines.len() > 1);
        assert_eq!(lines.concat(), value);
        assert!(lines.iter().all(|line| line.graphemes(true).count() <= 4));
    }

    // the shaped widths are what wrap counts: a spaced-less CJK line still fits its width
    #[test]
    fn a_shaped_cjk_line_fits_the_width_it_is_wrapped_in() {
        let face = crate::graphics::font::load_weighted(None, crate::Weight::Regular);
        let value = "東京都渋谷区神宮前".repeat(40);

        let lines = wrap(&value, face, 16.0, 60.0);

        assert!(lines.len() > 1);
        assert_eq!(lines.concat(), value);
        assert!(
            lines
                .iter()
                .all(|line| measure(line, face, 16.0) <= 60.0 + 0.01)
        );
    }

    // an emoji sequence or a combining mark is one grapheme, so a narrow wrap cannot split it
    #[test]
    fn wrapping_never_splits_a_grapheme_cluster() {
        let value = "a👩‍👩‍👧‍👦e\u{301}b";

        let lines = wrapped_with(value, 1.0, 1.5);

        assert_eq!(lines.concat(), value);
        assert!(lines.iter().any(|line| line.contains("e\u{301}")));
    }

    // the elision as it was: every shorter prefix, the "…" after it, measured whole again
    fn elided_one_grapheme_at_a_time(
        line: &str,
        width: f32,
        advance: impl Fn(&str) -> f32,
    ) -> String {
        let mut kept = String::from(line);

        loop {
            let shortened = format!("{kept}…");
            let total: f32 = shortened.graphemes(true).map(&advance).sum();

            if kept.is_empty() || total <= width {
                return shortened;
            }

            kept.truncate(
                kept.graphemes(true)
                    .next_back()
                    .map_or(0, |last| kept.len() - last.len()),
            );
        }
    }

    // a grapheme is the unit dropped: a mark or an emoji is never left half behind
    #[test]
    fn eliding_drops_whole_graphemes() {
        let advance = |grapheme: &str| match grapheme {
            "…" => 7.0,
            "i" | "l" | "." => 3.0,
            "m" | "w" => 13.0,
            " " => 4.0,
            "e\u{301}" => 9.0,
            _ => 8.0,
        };

        for width in [0.0f32, 7.0, 40.0, 123.5, 1000.0] {
            let line = "the quick e\u{301} brown fox m";
            let kept = line.trim_end();

            assert_eq!(
                elided(kept, advance("…"), width, advance),
                elided_one_grapheme_at_a_time(kept, width, advance),
                "eliding at width {width} cut it elsewhere than it used to"
            );
        }
    }

    /*
     * a stand-in for the shaped widths, so the UAX-14 choice can be tested without a font:
     * every grapheme counts as one pixel
     */
    fn wrapped_with(value: &str, each: f32, width: f32) -> Vec<String> {
        let starts: Vec<usize> = value.grapheme_indices(true).map(|(at, _)| at).collect();

        let mut prefix = Vec::with_capacity(starts.len() + 1);
        prefix.push(0.0);

        for _ in 0..starts.len() {
            let last = prefix.last().copied().unwrap_or(0.0);

            prefix.push(last + each);
        }

        let mut lines = Vec::new();

        pack(value, &starts, &prefix, width, &mut lines);

        lines
    }
}
