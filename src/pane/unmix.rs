//! Taking the shell's own drawing out of a copy of the screen (ADR 0100). The compositor blends the
//! window over the page, `copy = ours + (1 - alpha) * page`, with `ours` premultiplied, and the
//! shell knows what it presented, so `page = (copy - ours) / (1 - alpha)`. The shader does this
//! per pixel (`glass.wgsl`, `unmix`) with the constants below, which `glass.rs` hands it; the
//! reference here pins the rule the shader follows.

// the most opaque the shell's drawing may be for the page under it to be told: past it a level of
// error in the copy is magnified over 20 times
pub(super) const MAX_ALPHA: f32 = 0.95;

// how far past 0 or 1 a recovered channel may be, and still be the page rounded: the copy and the
// frame are 8 bit, and the copy may be of the frame before. More than this means they differ
pub(super) const SLACK: f32 = 0.03;

#[cfg(test)]
// half a level of 8 bit, which dividing by what remains of the page magnifies
const HALF_LEVEL: f32 = 0.5 / 255.0;

/*
 * the page under `ours` (rgba, premultiplied) where the screen shows `copy` (rgb), in 0 to 1; none
 * where it cannot be told: the drawing is nearly opaque, or the result is no color, which means the
 * copy was not taken of this frame
 */
#[cfg(test)]
pub(super) fn unmix(copy: [f32; 3], ours: [f32; 4]) -> Option<[f32; 3]> {
    let remaining = 1.0 - ours[3];

    if remaining < 1.0 - MAX_ALPHA {
        return None;
    }

    let margin = SLACK + HALF_LEVEL / remaining;
    let mut page = [0.0; 3];

    for channel in 0..3 {
        let value = (copy[channel] - ours[channel]) / remaining;

        if !(-margin..=1.0 + margin).contains(&value) {
            return None;
        }

        page[channel] = value.clamp(0.0, 1.0);
    }

    Some(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    // what the compositor shows: ours over the page, both in 8 bit
    fn blend(page: [f32; 3], ours: [f32; 4]) -> [f32; 3] {
        let level = |value: f32| (value * 255.0).round() / 255.0;

        [0, 1, 2].map(|c| level(ours[c] + (1.0 - ours[3]) * page[c]))
    }

    fn near(a: [f32; 3], b: [f32; 3], tolerance: f32) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() <= tolerance)
    }

    #[test]
    fn a_blend_unmixes_back_to_the_page() {
        let page = [0.8, 0.35, 0.05];

        // a scrim: black at a quarter, premultiplied
        let copy = blend(page, [0.0, 0.0, 0.0, 0.25]);
        let back = unmix(copy, [0.0, 0.0, 0.0, 0.25]).expect("a quarter can be told");

        assert!(near(back, page, 1.0 / 255.0 / 0.75));

        // light glass: a veil of white at 11%
        let veil = [0.11, 0.11, 0.11, 0.11];
        let back = unmix(blend(page, veil), veil).expect("a veil can be told");

        assert!(near(back, page, 1.0 / 255.0 / 0.89));
    }

    #[test]
    fn nothing_of_ours_returns_the_copy() {
        let page = [0.1, 0.5, 0.9];

        assert!(near(unmix(page, [0.0; 4]).unwrap(), page, 1e-6));
    }

    // an opaque pixel of ours, a letter or an icon, hides the page: it is not guessed
    #[test]
    fn what_is_nearly_opaque_cannot_be_told() {
        let ours = [0.9, 0.9, 0.9, 1.0];

        assert_eq!(unmix(blend([0.2; 3], ours), ours), None);

        let ours = [0.5, 0.5, 0.5, 0.96];

        assert_eq!(unmix(blend([0.2; 3], ours), ours), None);

        // just under the limit it still can
        let ours = [0.2, 0.2, 0.2, 0.94];

        assert!(unmix(blend([0.2; 3], ours), ours).is_some());
    }

    // a copy that cannot be a blend of this frame (the page moved, or the window did) is rejected
    #[test]
    fn a_result_that_is_no_color_is_rejected() {
        let ours = [0.4, 0.4, 0.4, 0.5];

        // darker than ours alone: a negative page
        assert_eq!(unmix([0.1, 0.5, 0.5], ours), None);

        // brighter than ours and a full page: over 1
        assert_eq!(unmix([0.5, 0.95, 0.95], ours), None);
    }

    // rounding within a level or two is the page, clamped
    #[test]
    fn rounding_is_clamped() {
        let ours = [0.0, 0.0, 0.0, 0.5];

        // 1 / 255 under black and over white, in a copy that halves them
        let back = unmix([-0.002, 0.502, 0.5], ours).expect("within the slack");

        assert_eq!(back, [0.0, 1.0, 1.0]);
    }
}
