//! The Satellites as they come and go (#37): each grows out from under the body's end to its place
//! beside it, fading in, and tucks back under it fading out, so none pops. One that changes place
//! slides there. Driven by the Frame each sync, with time passed in.

use std::time::{Duration, Instant};

use super::activity::{Activity, Frame, Id};
use super::motion::{Mode, Spring};

// what a Satellite draws: an Activity's, or the count of the ones past the bound
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    Activity(Activity),
    Overflow(usize),
}

impl Mark {
    // the same Satellite across Frames, whatever it draws now
    fn key(&self) -> Option<&Id> {
        match self {
            Mark::Activity(activity) => Some(activity.id()),
            Mark::Overflow(_) => None,
        }
    }

    // a Frame's, nearest the body first
    pub fn of(frame: &Frame) -> Vec<Mark> {
        frame
            .satellites
            .iter()
            .cloned()
            .map(Mark::Activity)
            .chain((frame.overflow > 0).then_some(Mark::Overflow(frame.overflow)))
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Satellites {
    dots: Vec<Dot>,
}

#[derive(Debug, Clone)]
struct Dot {
    mark: Mark,

    // its place beside the body, 0 nearest
    place: usize,
    slot: Spring<1>,

    // 0 tucked under the body, 1 in its place
    presence: Spring<1>,

    // its opacity when the presence's leg began, so a change of mind fades on from there
    faded: f32,

    // gone from the Frame, tucking away until it rests
    leaving: bool,
}

impl Dot {
    fn opacity(&self, now: Instant) -> f32 {
        let goal = if self.leaving { 0.0 } else { 1.0 };

        self.faded + (goal - self.faded) * self.presence.progress(now)
    }

    fn go(&mut self, leaving: bool, response: Duration, now: Instant) {
        self.faded = self.opacity(now);
        self.leaving = leaving;
        self.presence
            .to([if leaving { 0.0 } else { 1.0 }], response, now);
    }
}

// one Satellite as it shows at a moment
#[derive(Debug, Clone, PartialEq)]
pub struct Shown<'a> {
    pub mark: &'a Mark,

    // where it stands, 0 nearest the body, between places while it slides
    pub slot: f32,

    // how far out from under the body, 0 to 1
    pub presence: f32,

    pub opacity: f32,
}

impl Satellites {
    /*
     * the dots morph toward `marks`: a new one comes out in `enter`, one gone tucks away in
     * `leave`, one back before it was gone comes out again from where it is, and one that changed
     * place slides there in `enter`. One that came out where another was left goes over it
     */
    pub fn follow(
        &mut self,
        marks: Vec<Mark>,
        enter: Duration,
        leave: Duration,
        mode: Mode,
        now: Instant,
    ) {
        self.dots
            .retain(|dot| !(dot.leaving && dot.presence.settled(now)));

        for dot in &mut self.dots {
            let staying = marks.iter().any(|mark| mark.key() == dot.mark.key());

            if !staying && !dot.leaving {
                dot.go(true, leave, now);
            }
        }

        for (place, mark) in marks.into_iter().enumerate() {
            let Some(dot) = self
                .dots
                .iter_mut()
                .find(|dot| dot.mark.key() == mark.key())
            else {
                let mut presence = Spring::new([0.0], mode);
                presence.to([1.0], enter, now);

                self.dots.push(Dot {
                    mark,
                    place,
                    slot: Spring::new([place as f32], mode),
                    presence,
                    faded: 0.0,
                    leaving: false,
                });
                continue;
            };

            dot.mark = mark;

            if dot.leaving {
                dot.go(false, enter, now);
            }

            if dot.place != place {
                dot.place = place;
                dot.slot.to([place as f32], enter, now);
            }
        }
    }

    // what shows at `now`, the ones leaving first so the ones staying draw over them
    pub fn shown(&self, now: Instant) -> Vec<Shown<'_>> {
        let (leaving, staying): (Vec<&Dot>, Vec<&Dot>) =
            self.dots.iter().partition(|dot| dot.leaving);

        leaving
            .into_iter()
            .chain(staying)
            .map(|dot| Shown {
                mark: &dot.mark,
                slot: dot.slot.at(now)[0],
                presence: dot.presence.at(now)[0].clamp(0.0, 1.0),
                opacity: dot.opacity(now).clamp(0.0, 1.0),
            })
            .filter(|shown| shown.opacity > 0.0)
            .collect()
    }

    pub fn settled(&self, now: Instant) -> bool {
        self.dots
            .iter()
            .all(|dot| dot.presence.settled(now) && dot.slot.settled(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::activity::{Kind, Priority};

    const ENTER: Duration = Duration::from_millis(180);
    const LEAVE: Duration = Duration::from_millis(180);

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn mark(key: &str) -> Mark {
        Mark::Activity(Activity::persistent(
            Id::new(Kind::Timer, key),
            Priority::Ongoing,
        ))
    }

    fn follow(satellites: &mut Satellites, marks: &[Mark], mode: Mode, now: Instant) {
        satellites.follow(marks.to_vec(), ENTER, LEAVE, mode, now);
    }

    // mark, slot, presence and opacity of each, rounded so a test reads them
    fn at(satellites: &Satellites, now: Instant) -> Vec<(Mark, f32, f32, f32)> {
        let round = |value: f32| (value * 100.0).round() / 100.0;

        satellites
            .shown(now)
            .into_iter()
            .map(|shown| {
                (
                    shown.mark.clone(),
                    round(shown.slot),
                    round(shown.presence),
                    round(shown.opacity),
                )
            })
            .collect()
    }

    #[test]
    fn a_new_satellite_comes_out_from_under_the_body() {
        let now = Instant::now();
        let mut satellites = Satellites::default();

        follow(&mut satellites, &[mark("a")], Mode::Spring, now);
        assert_eq!(at(&satellites, now), vec![]);
        assert!(!satellites.settled(now));

        let [(_, slot, presence, opacity)] = at(&satellites, now + ms(60))[..] else {
            panic!("one shows");
        };
        assert_eq!(slot, 0.0);
        assert!(0.0 < presence && presence < 1.0, "{presence}");
        assert!(0.0 < opacity && opacity < 1.0, "{opacity}");

        let late = now + ms(1_000);
        assert_eq!(at(&satellites, late), vec![(mark("a"), 0.0, 1.0, 1.0)]);
        assert!(satellites.settled(late));
    }

    #[test]
    fn a_satellite_gone_tucks_away_then_is_dropped() {
        let now = Instant::now();
        let mut satellites = Satellites::default();
        follow(&mut satellites, &[mark("a")], Mode::Spring, now);

        let gone = now + ms(1_000);
        follow(&mut satellites, &[], Mode::Spring, gone);

        let [(_, _, presence, opacity)] = at(&satellites, gone + ms(60))[..] else {
            panic!("still shows while it leaves");
        };
        assert!(0.0 < presence && presence < 1.0, "{presence}");
        assert!(0.0 < opacity && opacity < 1.0, "{opacity}");

        let late = gone + ms(1_000);
        assert_eq!(at(&satellites, late), vec![]);
        assert!(satellites.settled(late));

        follow(&mut satellites, &[], Mode::Spring, late);
        assert!(satellites.dots.is_empty());
    }

    #[test]
    fn one_back_before_it_was_gone_fades_on_from_where_it_is() {
        let now = Instant::now();
        let mut satellites = Satellites::default();
        follow(&mut satellites, &[mark("a")], Mode::Spring, now);

        let gone = now + ms(1_000);
        follow(&mut satellites, &[], Mode::Spring, gone);

        let back = gone + ms(60);
        let before = at(&satellites, back);
        follow(&mut satellites, &[mark("a")], Mode::Spring, back);

        assert_eq!(at(&satellites, back), before);
        assert_eq!(satellites.dots.len(), 1);
        assert_eq!(
            at(&satellites, back + ms(1_000)),
            vec![(mark("a"), 0.0, 1.0, 1.0)]
        );
    }

    #[test]
    fn one_that_changes_place_slides_there() {
        let now = Instant::now();
        let mut satellites = Satellites::default();
        follow(&mut satellites, &[mark("a")], Mode::Spring, now);

        let later = now + ms(1_000);
        follow(
            &mut satellites,
            &[mark("b"), mark("a")],
            Mode::Spring,
            later,
        );

        let a = |at: Instant| {
            satellites
                .shown(at)
                .into_iter()
                .find(|shown| *shown.mark == mark("a"))
                .map(|shown| shown.slot)
        };
        assert_eq!(a(later), Some(0.0));
        assert!(a(later + ms(60)).is_some_and(|slot| 0.0 < slot && slot < 1.0));
        assert_eq!(a(later + ms(1_000)), Some(1.0));
    }

    #[test]
    fn the_overflow_count_changes_where_it_stands() {
        let now = Instant::now();
        let mut satellites = Satellites::default();
        follow(&mut satellites, &[Mark::Overflow(1)], Mode::Spring, now);

        let later = now + ms(1_000);
        follow(&mut satellites, &[Mark::Overflow(2)], Mode::Spring, later);

        assert_eq!(
            at(&satellites, later),
            vec![(Mark::Overflow(2), 0.0, 1.0, 1.0)]
        );
        assert!(satellites.settled(later));
    }

    #[test]
    fn the_ones_leaving_draw_beneath() {
        let now = Instant::now();
        let mut satellites = Satellites::default();
        follow(&mut satellites, &[mark("a")], Mode::Spring, now);

        let later = now + ms(1_000);
        follow(&mut satellites, &[mark("b")], Mode::Spring, later);

        let marks: Vec<Mark> = at(&satellites, later + ms(60))
            .into_iter()
            .map(|(mark, ..)| mark)
            .collect();
        assert_eq!(marks, vec![mark("a"), mark("b")]);
    }

    #[test]
    fn reduced_motion_snaps_in_place_and_only_fades() {
        let now = Instant::now();
        let mut satellites = Satellites::default();
        follow(&mut satellites, &[mark("a")], Mode::Reduced, now);

        let [(_, slot, presence, opacity)] = at(&satellites, now + ms(40))[..] else {
            panic!("one shows");
        };
        assert_eq!((slot, presence), (0.0, 1.0));
        assert_eq!(opacity, 0.5);
        assert!(satellites.settled(now + ms(80)));

        let later = now + ms(1_000);
        follow(
            &mut satellites,
            &[mark("b"), mark("a")],
            Mode::Reduced,
            later,
        );
        assert_eq!(
            at(&satellites, later)
                .into_iter()
                .find(|(shown, ..)| *shown == mark("a")),
            Some((mark("a"), 1.0, 1.0, 1.0))
        );
    }
}
