//! Content crossfade (plan 4: a Presentation change is geometry plus content crossfade in one
//! motion). Driven by the morph's progress, not a timer of its own: the old content fades out over
//! the first half of the morph and the new fades in over the second, so at most one shows at once.
//! A morph that changes its mind takes over whatever shows, at its current opacity, so nothing pops.

use super::presentation::Presentation;

#[derive(Debug, Clone, Copy)]
pub struct Crossfade {
    // what showed when the current leg began, and how strongly
    from: Option<(Presentation, f32)>,

    to: Presentation,

    // how strongly `to` already showed when the leg began, nonzero only on a way back
    start: f32,
}

impl Crossfade {
    // settled on `presentation`
    pub fn new(presentation: Presentation) -> Self {
        Self {
            from: None,
            to: presentation,
            start: 1.0,
        }
    }

    pub fn target(&self) -> Presentation {
        self.to
    }

    // a new leg toward `next`, begun at `progress` of the current one
    pub fn to(&mut self, next: Presentation, progress: f32) {
        if next == self.to {
            return;
        }

        let showing = self.shown(progress).into_iter().flatten().next();

        (self.from, self.start) = match showing {
            // back to what still shows, it only has to grow back from here
            Some((presentation, opacity)) if presentation == next => (None, opacity),
            showing => (showing, 0.0),
        };

        self.to = next;
    }

    // what shows at `progress` of the current leg, and how strongly; never both at once
    pub fn shown(&self, progress: f32) -> [Option<(Presentation, f32)>; 2] {
        let out = (1.0 - 2.0 * progress).clamp(0.0, 1.0);
        let into = (2.0 * progress - 1.0).clamp(0.0, 1.0);

        let from = self
            .from
            .map(|(presentation, opacity)| (presentation, opacity * out));
        let to = (self.to, self.start + (1.0 - self.start) * into);

        [from, Some(to)].map(|shown| shown.filter(|&(_, opacity)| opacity > 0.0))
    }
}

// an untouched island rests, with nothing to show
impl Default for Crossfade {
    fn default() -> Self {
        Self::new(Presentation::Rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::presentation::Surface;

    use Presentation::{Compact, Expanded, Peek, Rest};

    const MEDIA: Presentation = Expanded(Surface::Media);

    fn shown(fade: &Crossfade, progress: f32) -> Vec<(Presentation, f32)> {
        fade.shown(progress).into_iter().flatten().collect()
    }

    // the opacity `presentation` shows with, 0 when it does not show
    fn opacity(fade: &Crossfade, presentation: Presentation, progress: f32) -> f32 {
        shown(fade, progress)
            .into_iter()
            .find(|&(shown, _)| shown == presentation)
            .map_or(0.0, |(_, opacity)| opacity)
    }

    fn steps() -> impl Iterator<Item = f32> {
        (0..=100).map(|step| step as f32 / 100.0)
    }

    #[test]
    fn settled_shows_only_its_presentation() {
        let fade = Crossfade::new(Compact);

        assert_eq!(shown(&fade, 1.0), [(Compact, 1.0)]);
        assert_eq!(shown(&fade, 0.0), [(Compact, 1.0)]);
    }

    #[test]
    fn old_fades_out_then_new_fades_in() {
        let mut fade = Crossfade::new(Compact);

        fade.to(MEDIA, 1.0);

        assert_eq!(shown(&fade, 0.0), [(Compact, 1.0)]);
        assert_eq!(shown(&fade, 0.25), [(Compact, 0.5)]);
        assert_eq!(shown(&fade, 0.5), []);
        assert_eq!(shown(&fade, 0.75), [(MEDIA, 0.5)]);
        assert_eq!(shown(&fade, 1.0), [(MEDIA, 1.0)]);
    }

    #[test]
    fn never_two_at_once() {
        let mut fade = Crossfade::new(Rest);

        for (next, at) in [(Compact, 0.3), (Peek, 0.8), (MEDIA, 0.6), (Compact, 0.1)] {
            fade.to(next, at);

            for progress in steps() {
                assert!(shown(&fade, progress).len() <= 1, "{fade:?} at {progress}");
            }
        }
    }

    // a changed mind continues from what shows, whatever the new target
    #[test]
    fn retargeting_never_pops() {
        let targets = [Rest, Compact, Peek, MEDIA];

        for first in targets {
            // a leg already headed for Peek does not start over
            for second in targets.into_iter().filter(|&second| second != Peek) {
                for at in steps() {
                    let mut fade = Crossfade::new(Rest);

                    fade.to(first, 1.0);
                    fade.to(second, 0.0);

                    let before = shown(&fade, at);

                    fade.to(Peek, at);

                    let after = shown(&fade, 0.0);

                    assert_eq!(before, after, "{first:?} -> {second:?} -> Peek at {at}");
                }
            }
        }
    }

    #[test]
    fn way_back_grows_what_still_shows() {
        let mut fade = Crossfade::new(Compact);

        fade.to(MEDIA, 1.0);
        fade.to(Compact, 0.2);

        // Compact showed at 0.6 when the mind changed, it only grows from there
        let mut last = opacity(&fade, Compact, 0.0);
        assert!((last - 0.6).abs() < 1e-6, "{last}");

        for progress in steps() {
            let now = opacity(&fade, Compact, progress);

            assert!(now >= last, "dipped at {progress}: {now}");
            assert_eq!(opacity(&fade, MEDIA, progress), 0.0);

            last = now;
        }

        assert_eq!(last, 1.0);
    }

    #[test]
    fn same_target_changes_nothing() {
        let mut fade = Crossfade::new(Compact);

        fade.to(MEDIA, 1.0);
        fade.to(MEDIA, 0.7);

        assert_eq!(shown(&fade, 0.25), [(Compact, 0.5)]);
    }
}
