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

    /*
     * a Frame's, nearest the body first. With `segment` the top Satellite is the body's trailing
     * segment, not a dot beside it
     */
    pub fn of(frame: &Frame, segment: bool) -> Vec<Mark> {
        frame
            .satellites
            .iter()
            .skip(usize::from(segment))
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

    // whether a dot still moves; under Reduced motion they are already where they go
    pub fn moving(&self, now: Instant) -> bool {
        self.dots.iter().any(|dot| {
            [&dot.presence, &dot.slot]
                .into_iter()
                .any(|spring| spring.mode() == Mode::Spring && !spring.settled(now))
        })
    }
}
