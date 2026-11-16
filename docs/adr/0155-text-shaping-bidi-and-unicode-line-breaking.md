# 155. Text is shaped, bidirectional and wrapped by the Unicode rules

Status: accepted. The runtime keeps its widget API; [ADR 0090](0090-text-input-v3-in-the-runtime.md) is unchanged, as it described the input-method protocol, not how a letter became a glyph.

## Context

The runtime mapped each `char` to a glyph in its own font, or a fontconfig fallback, and advanced the pen by that glyph's advance (`graphics/font.rs`, `graphics/renderer/text.rs`). Wrapping split on `' '` (`widgets/text/lines.rs`). A font's GSUB and GPOS never ran, so ligatures, kerning and contextual forms were lost; a long CJK string had no space to break at; and Arabic or Indic text drew every letter in its isolated form. Mixed-direction text was laid out left to right because nothing decided a paragraph or run direction.

The lock screen already shaped with rustybuzz. Pleamar uses cosmic-text for the same pieces: HarfRust for shaping, unicode-bidi, unicode-linebreak, and a cached layout. The runtime needs the same guarantees through its own primitives, not another text stack.

## Decision

- Each run of one bidi level is itemized by fallback face (a cluster never crosses faces) and shaped with rustybuzz 0.20, with the run's direction, the script guessed from its letters, and the session's language. The shaped glyphs carry their own x/y offsets, advance and source cluster; the line's width is their advances. Color emoji still draw through `emoji::picture(face, id)`.
- unicode-bidi 0.3 gives paragraph direction and reorders the level runs of a line into visual order. Runs are shaped in logical order; each line is laid out in visual order, and an RTL run's fallback-face groups are reversed with it.
- Wrapping uses unicode-linebreak 0.1 choices (UAX #14) at grapheme boundaries (unicode-segmentation 1). A mandatory break is always taken, a word wider than the width still gets its own line, and only `break_words` cuts a token, at grapheme boundaries. `max_lines` and `elide` keep their meaning; elision drops whole graphemes.
- Shaped runs and whole lines are cached in bounded thread-local maps (512 entries, cleared at the limit like the glyph atlas), keyed by text, face pointer, size bits and style. A frame that repeats its content does not shape it again.
- Measuring, line height, spacing and ink bounds come from the shaped line rather than summed per-letter advances. The text input's caret and selection use the shaped width of their prefix.
- The whole-pixel start and rounded per-glyph distance stay in the picture path, as do outline drawing for rotated or stretched text.

## Consequences

- Latin labels keep their layout; kerning and ligatures now come from the font. CJK wraps, Arabic and Indic use their shaped forms, and mixed Latin/Arabic lines take their bidi order.
- The cache is per draw thread and starts over at its bound, so a screen with thousands of distinct lines re-shapes after a clear instead of growing without limit.
- The lock screen is untouched, and the font loader, atlas, widget API and renderer seam are unchanged.
- Checked by unit tests of shaping, bidi order, UAX #14 wrapping and cache reuse; visual checks run on a live session.
