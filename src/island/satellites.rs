//! The Satellites as they come and go: each grows out from under the body's end to its place
//! beside it, fading in, and tucks back under it fading out, so none pops. One that changes place
//! slides there. Driven by the Frame each sync, with time passed in.

use std::time::{Duration, Instant};

use super::activity::{Activity, Frame, Id};
use super::motion::{Mode, Spring};

// what a Satellite draws: an Activity's, or the count of the ones past the bound
#[derive(Debug, Clone, PartialEq)]
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

    // Marks disappear before the liquid shape finishes tucking under the body.
    ink: Spring<1>,
    ink_faded: f32,

    // gone from the Frame, tucking away until it rests
    leaving: bool,
}

impl Dot {
    fn opacity(&self, now: Instant) -> f32 {
        let goal = if self.leaving { 0.0 } else { 1.0 };

        self.faded + (goal - self.faded) * self.presence.progress(now)
    }

    fn content_opacity(&self, now: Instant) -> f32 {
        let goal = if self.leaving { 0.0 } else { 1.0 };
        self.ink_faded + (goal - self.ink_faded) * self.ink.progress(now)
    }

    fn go(&mut self, leaving: bool, response: Duration, mode: Mode, now: Instant) {
        self.faded = self.opacity(now);
        self.ink_faded = self.content_opacity(now);
        if self.presence.mode() != mode {
            self.presence = Spring::new([self.presence.at(now)[0]], mode);
            self.slot = Spring::new([self.slot.at(now)[0]], mode);
            self.slot.to([self.place as f32], response, now);
        }
        self.leaving = leaving;
        self.presence
            .to([if leaving { 0.0 } else { 1.0 }], response, now);
        self.ink = Spring::new([0.0], self.presence.mode());
        let response = if self.presence.mode() == Mode::Reduced {
            response
        } else {
            response.mul_f32(0.6)
        };
        self.ink.to([1.0], response, now);
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
    pub content_opacity: f32,
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
        reduced_fade: Duration,
        mode: Mode,
        now: Instant,
    ) {
        let (enter, leave) = if mode == Mode::Reduced {
            (reduced_fade, reduced_fade)
        } else {
            (enter, leave)
        };

        self.dots
            .retain(|dot| !(dot.leaving && dot.presence.settled(now)));

        for dot in &mut self.dots {
            if dot.presence.mode() != mode {
                dot.go(
                    dot.leaving,
                    if dot.leaving { leave } else { enter },
                    mode,
                    now,
                );
            }
            let staying = marks.iter().any(|mark| mark.key() == dot.mark.key());

            if !staying && !dot.leaving {
                dot.go(true, leave, mode, now);
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
                let mut ink = Spring::new([0.0], mode);
                ink.to(
                    [1.0],
                    if mode == Mode::Reduced {
                        enter
                    } else {
                        enter.mul_f32(0.6)
                    },
                    now,
                );

                self.dots.push(Dot {
                    mark,
                    place,
                    slot: Spring::new([place as f32], mode),
                    presence,
                    faded: 0.0,
                    ink,
                    ink_faded: 0.0,
                    leaving: false,
                });
                continue;
            };

            dot.mark = mark;

            if dot.leaving {
                dot.go(false, enter, mode, now);
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
                content_opacity: dot.content_opacity(now).clamp(0.0, 1.0),
            })
            .filter(|shown| shown.opacity > 0.0)
            .collect()
    }

    pub fn settled(&self, now: Instant) -> bool {
        self.dots
            .iter()
            .all(|dot| dot.presence.settled(now) && dot.slot.settled(now) && dot.ink.settled(now))
    }

    // Shape alpha can still change after Reduced geometry has snapped. Coverage must follow it.
    pub fn visibility_pending(&self, now: Instant) -> bool {
        self.dots.iter().any(|dot| {
            let goal = if dot.leaving { 0.0 } else { 1.0 };
            dot.faded != goal && !dot.presence.settled(now)
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(satellites: &Satellites, now: Instant) -> [f32; 4] {
        let dot = &satellites.shown(now)[0];
        [dot.slot, dot.presence, dot.opacity, dot.content_opacity]
    }

    #[test]
    fn a_returning_satellite_keeps_its_pose_and_presence_velocity() {
        let start = Instant::now();
        let response = Duration::from_millis(180);
        let reduced = Duration::from_millis(40);
        let mut satellites = Satellites::default();
        satellites.follow(
            vec![Mark::Overflow(2)],
            response,
            response,
            reduced,
            Mode::Spring,
            start,
        );
        let leave = start + Duration::from_secs(1);
        let before = pose(&satellites, leave);
        satellites.follow(vec![], response, response, reduced, Mode::Spring, leave);
        assert_eq!(pose(&satellites, leave), before);
        let back = leave + Duration::from_millis(50);
        let before = pose(&satellites, back);
        assert!(before[3] < before[2], "the mark leaves before the shape");
        let velocity = satellites.dots[0].presence.velocity(back);
        satellites.follow(
            vec![Mark::Overflow(3)],
            response,
            response,
            reduced,
            Mode::Spring,
            back,
        );
        assert_eq!(pose(&satellites, back), before);
        assert_eq!(satellites.dots[0].presence.velocity(back), velocity);
        assert_eq!(satellites.dots.len(), 1);
        let end = back + Duration::from_secs(2);
        assert!(satellites.settled(end));
        assert_eq!(pose(&satellites, end), [0.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn reduced_satellites_snap_geometry_and_finish_the_brief_fade() {
        let start = Instant::now();
        let response = Duration::from_millis(180);
        let reduced = Duration::from_millis(40);
        let mut satellites = Satellites::default();
        satellites.follow(
            vec![Mark::Overflow(2)],
            response,
            response,
            reduced,
            Mode::Spring,
            start,
        );
        let leave = start + Duration::from_secs(1);
        satellites.follow(vec![], response, response, reduced, Mode::Reduced, leave);
        let shown = pose(&satellites, leave);
        assert_eq!(shown[1], 0.0);
        assert_eq!(shown[2..], [1.0, 1.0]);
        assert!(!satellites.settled(leave));
        assert!(satellites.settled(leave + reduced));
        assert!(satellites.shown(leave + reduced).is_empty());
        satellites.follow(
            vec![],
            response,
            response,
            reduced,
            Mode::Reduced,
            leave + reduced,
        );
        assert!(satellites.dots.is_empty());
    }
}
