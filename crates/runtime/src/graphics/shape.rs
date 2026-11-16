/*
 * Text as the font shapes it: rustybuzz runs GSUB/GPOS over each run, unicode-bidi says which
 * way the runs go, and the glyphs come out with their own offsets. Runs and whole lines are
 * cached, so a frame that repeats its content does not shape it again.
 */

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock};

use rustybuzz::ttf_parser::Tag;
use rustybuzz::{Direction, Feature, Language, UnicodeBuffer};
use ttf_parser::{Face, GlyphId};
use unicode_bidi::{BidiInfo, Level};
use unicode_segmentation::UnicodeSegmentation;

use crate::graphics::font;

// like the glyph atlas, the caches start over when they reach this many entries
const CACHE_LIMIT: usize = 512;

// one glyph where it draws: `x` from the line's pen, `y` down from the baseline
pub(crate) struct Glyph {
    pub face: &'static Face<'static>,
    pub id: u16,
    pub x: f32,
    pub y: f32,
    pub advance: f32,

    // the byte in the shaped text this glyph belongs to
    pub cluster: usize,
}

// one line, shaped and laid out in visual order
pub(crate) struct Line {
    pub glyphs: Vec<Glyph>,
    pub width: f32,
    pub baseline: f32,
}

// the vertical measurements of a face at a size
pub(crate) struct Metrics {
    pub line_height: f32,
    pub spacing: f32,
    pub baseline: f32,
}

impl Line {
    fn empty(metrics: &Metrics) -> Line {
        Line {
            glyphs: Vec::new(),
            width: 0.0,
            baseline: metrics.baseline,
        }
    }
}

pub(crate) fn metrics(font: &Face, size: f32) -> Metrics {
    let units = size / f32::from(font.units_per_em());

    let ascent = f32::from(font.ascender()) * units;
    let descent = f32::from(font.descender()) * units;

    // descent is negative, so this adds the part below the baseline
    let line_height = ascent - descent;

    Metrics {
        line_height,
        spacing: line_height + f32::from(font.line_gap()) * units,
        baseline: ascent,
    }
}

pub(crate) fn line(
    text: &str,
    font: &'static Face<'static>,
    size: f32,
    tabular: bool,
) -> Arc<Line> {
    let key = Key::new(text, font, size, tabular);

    let cached = LINES.with_borrow(|lines| {
        lines
            .get(&key)
            .filter(|cached| cached.text == text)
            .map(|cached| Arc::clone(&cached.line))
    });

    if let Some(line) = cached {
        return line;
    }

    let built = Arc::new(build(text, font, size, tabular));

    LINES.with_borrow_mut(|lines| {
        if lines.len() >= CACHE_LIMIT {
            lines.clear();
        }

        lines.insert(
            key,
            CachedLine {
                text: String::from(text),
                line: Arc::clone(&built),
            },
        );
    });

    built
}

pub(crate) fn width(text: &str, font: &'static Face<'static>, size: f32) -> f32 {
    line(text, font, size, false).width
}

// where the glyphs' ink starts and ends, counted from the pen's starting point
pub(crate) fn ink(text: &str, font: &'static Face<'static>, size: f32) -> (f32, f32) {
    let shaped = line(text, font, size, false);

    let mut left = f32::MAX;
    let mut right = f32::MIN;

    for glyph in &shaped.glyphs {
        let Some(bounds) = glyph.face.glyph_bounding_box(GlyphId(glyph.id)) else {
            continue;
        };

        let units = size / f32::from(glyph.face.units_per_em());

        left = left.min(glyph.x + f32::from(bounds.x_min) * units);
        right = right.max(glyph.x + f32::from(bounds.x_max) * units);
    }

    // nothing but spaces is as wide as its advance
    if left > right {
        return (0.0, shaped.width);
    }

    (left, right)
}

/*
 * each grapheme's share of the shaped line, in logical order: a ligature or a mark that shapes
 * into its base counts with the cluster HarfBuzz gives it, so the sums reach the line's width
 */
pub(crate) fn grapheme_advances(text: &str, font: &'static Face<'static>, size: f32) -> Vec<f32> {
    let shaped = line(text, font, size, false);

    let starts: Vec<usize> = text.grapheme_indices(true).map(|(at, _)| at).collect();

    let mut advances = vec![0.0; starts.len()];

    for glyph in &shaped.glyphs {
        let index = match starts.binary_search(&glyph.cluster) {
            Ok(index) => index,
            Err(0) => 0,
            Err(index) => index - 1,
        };

        if let Some(advance) = advances.get_mut(index) {
            *advance += glyph.advance;
        }
    }

    advances
}

fn build(text: &str, font: &'static Face<'static>, size: f32, tabular: bool) -> Line {
    let metrics = metrics(font, size);

    if text.is_empty() {
        return Line::empty(&metrics);
    }

    let bidi = BidiInfo::new(text, None);

    let mut glyphs = Vec::new();
    let mut pen = 0.0;

    for paragraph in &bidi.paragraphs {
        let range = paragraph.range.clone();

        if range.is_empty() {
            continue;
        }

        // L1 resets trailing spaces to the paragraph's level, as a line end does
        let levels = bidi.reordered_levels(paragraph, range.clone());

        // runs of graphemes at one bidi level, in logical order
        let mut runs: Vec<(usize, usize, Level)> = Vec::new();

        for (at, cluster) in text[range.clone()].grapheme_indices(true) {
            let start = range.start + at;
            let end = start + cluster.len();

            // a paragraph break has no ink and separates the runs around it
            if is_separator(cluster) {
                continue;
            }

            let level = levels[start];

            match runs.last_mut() {
                Some(run) if run.1 == start && run.2 == level => run.1 = end,
                _ => runs.push((start, end, level)),
            }
        }

        // the levels say which run is leftmost; each run is then shaped in its own direction
        let run_levels: Vec<Level> = runs.iter().map(|run| run.2).collect();

        for index in BidiInfo::reorder_visual(&run_levels) {
            let (start, end, level) = runs[index];

            lay_run(
                &text[start..end],
                start,
                &mut glyphs,
                &mut pen,
                font,
                size,
                level.is_rtl(),
                tabular,
            );
        }
    }

    Line {
        glyphs,
        width: pen,
        baseline: metrics.baseline,
    }
}

// one run of one bidi level, split by fallback face, shaped and placed from the pen
#[allow(clippy::too_many_arguments)]
fn lay_run(
    run: &str,
    offset: usize,
    glyphs: &mut Vec<Glyph>,
    pen: &mut f32,
    font: &'static Face<'static>,
    size: f32,
    rtl: bool,
    tabular: bool,
) {
    // a run keeps each cluster on one face, so a fallback never takes half a cluster
    let mut groups: Vec<(usize, usize, &'static Face<'static>)> = Vec::new();

    for (at, cluster) in run.grapheme_indices(true) {
        let face = font::face_for(font, cluster);
        let end = at + cluster.len();

        match groups.last_mut() {
            Some(group) if std::ptr::eq(group.2, face) => group.1 = end,
            _ => groups.push((at, end, face)),
        }
    }

    /*
     * each group is shaped right to left in visual order, so an RTL run's groups come out
     * rightmost first only once their own order is turned around
     */
    if rtl {
        groups.reverse();
    }

    for (start, end, face) in groups {
        let shaped = shaped_run(&run[start..end], face, size, rtl, tabular);

        for glyph in &shaped.glyphs {
            glyphs.push(Glyph {
                face: glyph.face,
                id: glyph.id,
                x: *pen + glyph.x,
                y: glyph.y,
                advance: glyph.advance,
                cluster: offset + start + glyph.cluster,
            });
        }

        *pen += shaped.advance;
    }
}

fn shaped_run(
    text: &str,
    face: &'static Face<'static>,
    size: f32,
    rtl: bool,
    tabular: bool,
) -> Arc<ShapedRun> {
    let key = Key::new_run(text, face, size, rtl, tabular);

    let cached = RUNS.with_borrow(|runs| {
        runs.get(&key)
            .filter(|cached| cached.text == text)
            .map(|cached| Arc::clone(&cached.run))
    });

    if let Some(run) = cached {
        return run;
    }

    let built = Arc::new(build_run(text, face, size, rtl, tabular));

    RUNS.with_borrow_mut(|runs| {
        if runs.len() >= CACHE_LIMIT {
            runs.clear();
        }

        runs.insert(
            key,
            CachedRun {
                text: String::from(text),
                run: Arc::clone(&built),
            },
        );
    });

    built
}

fn build_run(
    text: &str,
    face: &'static Face<'static>,
    size: f32,
    rtl: bool,
    tabular: bool,
) -> ShapedRun {
    let units = size / f32::from(face.units_per_em());

    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);

    buffer.set_direction(match rtl {
        true => Direction::RightToLeft,
        false => Direction::LeftToRight,
    });

    if let Some(language) = language() {
        buffer.set_language(language);
    }

    // the direction is set; the script comes from the run's own letters
    buffer.guess_segment_properties();

    let features = match tabular {
        true => vec![Feature::new(Tag::from_bytes(b"tnum"), 1, ..)],
        false => Vec::new(),
    };

    let shaped = rustybuzz::shape(&rustybuzz::Face::from_face(face.clone()), &features, buffer);

    let mut x = 0.0;
    let mut glyphs = Vec::with_capacity(shaped.len());

    for (info, position) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
        let advance = position.x_advance as f32 * units;

        glyphs.push(Glyph {
            face,
            id: info.glyph_id as u16,
            x: x + position.x_offset as f32 * units,

            // fonts point y upward, the screen points it downward
            y: -(position.y_offset as f32) * units,
            advance,
            cluster: info.cluster as usize,
        });

        x += advance;
    }

    ShapedRun { glyphs, advance: x }
}

// the language the text is set in, for the fonts whose forms depend on it
fn language() -> Option<Language> {
    static LANGUAGE: LazyLock<Option<Language>> = LazyLock::new(|| {
        let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))?;

        let tag = locale.split(['.', '@']).next()?.replace('_', "-");

        if tag.eq_ignore_ascii_case("c") || tag.eq_ignore_ascii_case("posix") {
            return None;
        }

        tag.parse().ok()
    });

    LANGUAGE.clone()
}

fn is_separator(cluster: &str) -> bool {
    cluster
        .chars()
        .all(|letter| matches!(letter, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
}

// a shaped run, its glyphs' clusters counted from the run's start
struct ShapedRun {
    glyphs: Vec<Glyph>,
    advance: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    hash: u64,
    face: usize,
    size: u32,
    style: u8,
}

impl Key {
    fn new(text: &str, face: &Face, size: f32, tabular: bool) -> Key {
        Key {
            hash: hash(text),
            face: std::ptr::from_ref(face) as usize,
            size: size.to_bits(),
            style: u8::from(tabular),
        }
    }

    fn new_run(text: &str, face: &Face, size: f32, rtl: bool, tabular: bool) -> Key {
        Key {
            hash: hash(text),
            face: std::ptr::from_ref(face) as usize,
            size: size.to_bits(),
            style: u8::from(rtl) | u8::from(tabular) << 1,
        }
    }
}

fn hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();

    text.hash(&mut hasher);

    hasher.finish()
}

struct CachedLine {
    // the hash can collide, the text cannot
    text: String,
    line: Arc<Line>,
}

struct CachedRun {
    text: String,
    run: Arc<ShapedRun>,
}

thread_local! {
    static LINES: RefCell<HashMap<Key, CachedLine>> = RefCell::new(HashMap::new());
    static RUNS: RefCell<HashMap<Key, CachedRun>> = RefCell::new(HashMap::new());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Weight;

    fn face() -> &'static Face<'static> {
        font::load_weighted(None, Weight::Regular)
    }

    // a simple run is as wide as its shaped advances, which is what the line breaks count
    #[test]
    fn a_lines_width_is_the_sum_of_its_shaped_advances() {
        let shaped = line("Hamburgefonts", face(), 16.0, false);

        let sum: f32 = shaped.glyphs.iter().map(|glyph| glyph.advance).sum();

        assert!(!shaped.glyphs.is_empty());
        assert!((shaped.width - sum).abs() < 0.01);
        assert!(shaped.width > 0.0);
    }

    // a frame that draws the same text again gets the shaped line back, not a second shaping
    #[test]
    fn a_repeated_line_comes_from_the_cache() {
        let first = line("cached title", face(), 16.0, false);
        let second = line("cached title", face(), 16.0, false);

        assert!(Arc::ptr_eq(&first, &second));
    }

    // an Arabic word is shaped, not drawn as isolated placeholder boxes
    #[test]
    fn arabic_gets_shaped_glyphs_with_an_advance() {
        let shaped = line("مرحبا", face(), 16.0, false);

        assert!(!shaped.glyphs.is_empty());
        assert!(shaped.width > 0.0);
        assert!(
            shaped
                .glyphs
                .iter()
                .all(|glyph| glyph.id != 0 || glyph.advance == 0.0)
        );

        // joined forms change the glyphs the word would have letter by letter
        let isolated: Vec<u16> = "مرحبا"
            .chars()
            .filter_map(|letter| face().glyph_index(letter).map(|id| id.0))
            .collect();

        let shaped_ids: Vec<u16> = shaped.glyphs.iter().map(|glyph| glyph.id).collect();

        assert_ne!(shaped_ids, isolated);
    }

    // an RTL run is laid out in visual order: its clusters run backwards
    #[test]
    fn a_right_to_left_run_is_reordered() {
        let shaped = line("مرحبا", face(), 16.0, false);

        let clusters: Vec<usize> = shaped.glyphs.iter().map(|glyph| glyph.cluster).collect();

        assert!(clusters.windows(2).all(|pair| pair[0] >= pair[1]));
    }

    // a mixed line puts the Latin first and keeps every cluster whole
    #[test]
    fn a_mixed_line_is_ordered_without_splitting_clusters() {
        let text = "abc مرحبا def";
        let shaped = line(text, face(), 16.0, false);

        let boundaries: Vec<usize> = text
            .grapheme_indices(true)
            .map(|(at, _)| at)
            .chain(std::iter::once(text.len()))
            .collect();

        assert!(
            shaped
                .glyphs
                .iter()
                .all(|glyph| boundaries.contains(&glyph.cluster))
        );

        let latin: Vec<usize> = shaped
            .glyphs
            .iter()
            .map(|glyph| glyph.cluster)
            .filter(|cluster| *cluster < 3)
            .collect();

        assert_eq!(latin, vec![0, 1, 2]);

        // the Arabic comes after the Latin and is laid out right to left within itself
        let arabic: Vec<usize> = shaped
            .glyphs
            .iter()
            .map(|glyph| glyph.cluster)
            .filter(|cluster| (4..14).contains(cluster))
            .collect();

        assert!(!arabic.is_empty());
        assert!(arabic.windows(2).all(|pair| pair[0] >= pair[1]));

        let first_arabic = shaped
            .glyphs
            .iter()
            .position(|glyph| (4..14).contains(&glyph.cluster))
            .expect("Arabic glyphs");

        assert!(first_arabic >= 4);

        // and the Latin after it is laid out left to right again
        assert!(
            shaped
                .glyphs
                .iter()
                .skip(first_arabic)
                .any(|glyph| glyph.cluster >= 15)
        );
    }

    // an emoji or a combining mark is one cluster, measured once and never cut
    #[test]
    fn graphemes_keep_their_own_advance() {
        let text = "e\u{301}👍🏽";
        let advances = grapheme_advances(text, face(), 16.0);

        assert_eq!(advances.len(), 2);
        assert!(advances.iter().all(|advance| *advance > 0.0));
        assert!(
            (advances.iter().sum::<f32>() - line(text, face(), 16.0, false).width).abs() < 0.01
        );
    }
}
