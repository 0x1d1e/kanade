//! How a recording moves from one state to the next (ADR 0116), as a pure function of the state and
//! what happened: no program run, no clock, no file read. The caller does the effects it returns and
//! feeds back what came of them. Everything that happens carries the serial of the child it is about,
//! so the end of one that is no longer the recording's is dropped.

// where a recording stands
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,

    // the user is choosing an area on screen
    Picking {
        serial: u64,
    },

    // a recorder runs or is starting, to write segment `segment`
    Running {
        serial: u64,
        segment: u32,

        // asked to finish the segment so as to pause, whose end then pauses
        pausing: bool,

        // asked to end the whole recording, whose segment's end then saves it
        stopping: bool,
    },

    // `segments` are finished, none runs
    Paused {
        segments: u32,
    },

    // the finished `segments` are being joined into the file
    Joining,
}

// what a recorder left behind when it ended
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    // a file with its frames
    Written,

    // it ended well but wrote none: niri sends a frame only when the screen changes
    Empty,

    Lost(String),
}

// how the area a user was asked to choose came out
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    Chosen,
    Left,
    Failed(String),
}

// what happened
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    // `pick` for an area the user is yet to choose
    Start {
        pick: bool,
    },

    // `framed` says the running segment has a frame in its file, as a pause needs
    Pause {
        framed: bool,
    },
    Resume,
    Stop,

    // the session locked: whatever is recorded is saved and no recording goes on
    Lock,

    Picked {
        serial: u64,
        pick: Pick,
    },
    Spawned {
        serial: u64,
        started: Result<(), String>,
    },
    Ended {
        serial: u64,
        segment: Segment,
    },
    Joined(Result<(), String>),
}

// what the caller is to do
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    // runs the area picker for this serial
    Pick(u64),

    // ends the picker
    Cancel,

    // starts a recorder for `segment`, for this serial
    Spawn { serial: u64, segment: u32 },

    // interrupts the running recorder, so it finishes its file
    Interrupt,

    // joins the first `segments` segments into the file; the one is the file already
    Join { segments: u32 },

    // the pause is done
    Paused,

    // a resume that did not happen, why
    Declined(String),

    // the area was not chosen, the user having left or cancelled it, why
    Left(String),

    // the recording could not begin, why: the picker failed, which the user is to be told
    Failed(String),

    // the recording is over: saved, or why it was not
    Over(Result<(), String>),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Step {
    pub effects: Vec<Effect>,

    // why the event was refused, none if it was taken
    pub refused: Option<String>,
}

impl Step {
    fn to(effects: Vec<Effect>) -> Step {
        Step {
            effects,
            refused: None,
        }
    }

    fn nothing() -> Step {
        Step::to(Vec::new())
    }

    fn refuse(why: &str) -> Step {
        Step {
            effects: Vec::new(),
            refused: Some(why.to_owned()),
        }
    }
}

#[derive(Debug)]
pub struct Machine {
    phase: Phase,

    // the last serial given, so none is given twice
    serial: u64,
}

impl Machine {
    pub const fn new() -> Machine {
        Machine {
            phase: Phase::Idle,
            serial: 0,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    // whether anything is recorded, or about to be, or being saved
    pub fn busy(&self) -> bool {
        self.phase != Phase::Idle
    }

    // the serial of the recorder that runs, or of the picker
    pub fn serial(&self) -> Option<u64> {
        match self.phase {
            Phase::Picking { serial } | Phase::Running { serial, .. } => Some(serial),
            _ => None,
        }
    }

    fn next(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    fn run(&mut self, segment: u32) -> (Phase, Effect) {
        let serial = self.next();

        (
            Phase::Running {
                serial,
                segment,
                pausing: false,
                stopping: false,
            },
            Effect::Spawn { serial, segment },
        )
    }

    pub fn step(&mut self, event: Event) -> Step {
        let (phase, step) = self.transition(event);

        self.phase = phase;
        step
    }

    fn transition(&mut self, event: Event) -> (Phase, Step) {
        let phase = self.phase;

        match (phase, event) {
            (Phase::Idle, Event::Start { pick: true }) => {
                let serial = self.next();

                (
                    Phase::Picking { serial },
                    Step::to(vec![Effect::Pick(serial)]),
                )
            }
            (Phase::Idle, Event::Start { pick: false }) => {
                let (phase, spawn) = self.run(1);

                (phase, Step::to(vec![spawn]))
            }
            (Phase::Picking { .. }, Event::Start { .. }) => (
                phase,
                Step::refuse("already choosing an area to record; leave it first"),
            ),
            (_, Event::Start { .. }) => (phase, Step::refuse("already recording")),

            // the chosen area, or not
            (
                Phase::Picking { serial },
                Event::Picked {
                    serial: picked,
                    pick,
                },
            ) if serial == picked => match pick {
                Pick::Chosen => {
                    let (phase, spawn) = self.run(1);

                    (phase, Step::to(vec![spawn]))
                }
                Pick::Left => (
                    Phase::Idle,
                    Step::to(vec![Effect::Left(String::from("no area chosen"))]),
                ),
                Pick::Failed(why) => (Phase::Idle, Step::to(vec![Effect::Failed(why)])),
            },
            (Phase::Picking { .. }, Event::Stop | Event::Lock) => (
                Phase::Idle,
                Step::to(vec![
                    Effect::Cancel,
                    Effect::Left(String::from("recording cancelled")),
                ]),
            ),

            (
                Phase::Running {
                    serial,
                    segment,
                    pausing,
                    stopping,
                },
                Event::Pause { framed },
            ) => {
                if stopping {
                    (phase, Step::refuse("the recording is ending"))
                } else if pausing {
                    (phase, Step::refuse("already pausing"))
                } else if !framed {
                    (
                        phase,
                        Step::refuse("the recording has no frame yet; the screen has not changed"),
                    )
                } else {
                    (
                        Phase::Running {
                            serial,
                            segment,
                            pausing: true,
                            stopping,
                        },
                        Step::to(vec![Effect::Interrupt]),
                    )
                }
            }
            (Phase::Paused { .. }, Event::Pause { .. }) => {
                (phase, Step::refuse("the recording is already paused"))
            }
            (Phase::Joining, Event::Pause { .. }) => {
                (phase, Step::refuse("the recording is being saved"))
            }
            (_, Event::Pause { .. }) => (phase, Step::refuse("not recording")),

            (Phase::Paused { segments }, Event::Resume) => {
                let (phase, spawn) = self.run(segments + 1);

                (phase, Step::to(vec![spawn]))
            }
            (Phase::Running { .. }, Event::Resume) => {
                (phase, Step::refuse("the recording is not paused"))
            }
            (Phase::Joining, Event::Resume) => {
                (phase, Step::refuse("the recording is being saved"))
            }
            (_, Event::Resume) => (phase, Step::refuse("not recording")),

            // asked once, so a second stop does not cut the recorder's finishing short
            (
                Phase::Running {
                    serial,
                    segment,
                    pausing,
                    stopping,
                },
                Event::Stop | Event::Lock,
            ) => {
                let interrupt = !pausing && !stopping;

                (
                    Phase::Running {
                        serial,
                        segment,
                        pausing,
                        stopping: true,
                    },
                    Step::to(if interrupt {
                        vec![Effect::Interrupt]
                    } else {
                        Vec::new()
                    }),
                )
            }
            (Phase::Paused { segments }, Event::Stop | Event::Lock) => {
                (Phase::Joining, Step::to(vec![Effect::Join { segments }]))
            }
            (Phase::Joining, Event::Stop) => (phase, Step::nothing()),
            (Phase::Idle, Event::Stop) => (phase, Step::refuse("not recording")),
            (_, Event::Lock) => (phase, Step::nothing()),

            // the recorder is there now: whatever was asked of it before it was is asked again
            (
                Phase::Running {
                    serial,
                    segment,
                    pausing,
                    stopping,
                },
                Event::Spawned {
                    serial: spawned,
                    started,
                },
            ) if serial == spawned => match started {
                Ok(()) => (
                    phase,
                    Step::to(if pausing || stopping {
                        vec![Effect::Interrupt]
                    } else {
                        Vec::new()
                    }),
                ),
                // the segments before it are whole: a resume that fails leaves them
                Err(_) if segment > 1 && stopping => (
                    Phase::Joining,
                    Step::to(vec![Effect::Join {
                        segments: segment - 1,
                    }]),
                ),
                Err(why) if segment > 1 => (
                    Phase::Paused {
                        segments: segment - 1,
                    },
                    Step::to(vec![Effect::Declined(why)]),
                ),
                Err(why) => (Phase::Idle, Step::to(vec![Effect::Over(Err(why))])),
            },

            (
                Phase::Running {
                    serial,
                    segment,
                    pausing,
                    stopping,
                },
                Event::Ended {
                    serial: ended,
                    segment: left,
                },
            ) if serial == ended => match left {
                Segment::Written if pausing && !stopping => (
                    Phase::Paused { segments: segment },
                    Step::to(vec![Effect::Paused]),
                ),
                Segment::Written => (
                    Phase::Joining,
                    Step::to(vec![Effect::Join { segments: segment }]),
                ),
                // a resume stopped before the screen gave it a frame leaves the segments before it
                Segment::Empty if segment > 1 && !pausing => (
                    Phase::Joining,
                    Step::to(vec![Effect::Join {
                        segments: segment - 1,
                    }]),
                ),
                Segment::Empty => (
                    Phase::Idle,
                    Step::to(vec![Effect::Over(Err(String::from(
                        "stopped before the screen gave a frame",
                    )))]),
                ),
                /*
                 * a segment lost on its own is damage to that segment alone: the ones before it are
                 * whole, so they are joined and kept, as one that ended before a frame is. A pause
                 * the loss came in is over with them: one clip, saved, is better than a Failed one
                 */
                Segment::Lost(_) if segment > 1 => (
                    Phase::Joining,
                    Step::to(vec![Effect::Join {
                        segments: segment - 1,
                    }]),
                ),
                Segment::Lost(why) => (Phase::Idle, Step::to(vec![Effect::Over(Err(why))])),
            },

            (Phase::Joining, Event::Joined(joined)) => {
                (Phase::Idle, Step::to(vec![Effect::Over(joined)]))
            }

            // about a child or a picker that is no longer the recording's
            (
                _,
                Event::Picked { .. }
                | Event::Spawned { .. }
                | Event::Ended { .. }
                | Event::Joined(_),
            ) => (phase, Step::nothing()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(machine: &Machine) -> (u64, u32) {
        match machine.phase() {
            Phase::Running {
                serial, segment, ..
            } => (serial, segment),
            other => panic!("not running: {other:?}"),
        }
    }

    // a screen recording that has a frame
    fn recording() -> Machine {
        let mut machine = Machine::new();

        machine.step(Event::Start { pick: false });

        let (serial, _) = running(&machine);

        machine.step(Event::Spawned {
            serial,
            started: Ok(()),
        });
        machine
    }

    fn ended(machine: &mut Machine, segment: Segment) -> Step {
        let (serial, _) = running(machine);

        machine.step(Event::Ended { serial, segment })
    }

    #[test]
    fn a_recording_saves_its_one_segment() {
        let mut machine = recording();

        assert_eq!(machine.step(Event::Stop).effects, [Effect::Interrupt]);
        // a second stop does not interrupt again
        assert_eq!(machine.step(Event::Stop).effects, []);
        assert_eq!(
            ended(&mut machine, Segment::Written).effects,
            [Effect::Join { segments: 1 }]
        );
        assert_eq!(
            machine.step(Event::Joined(Ok(()))).effects,
            [Effect::Over(Ok(()))]
        );
        assert_eq!(machine.phase(), Phase::Idle);
    }

    #[test]
    fn a_pause_finishes_a_segment_and_a_resume_starts_the_next() {
        let mut machine = recording();

        assert_eq!(
            machine.step(Event::Pause { framed: true }).effects,
            [Effect::Interrupt]
        );
        assert_eq!(
            machine
                .step(Event::Pause { framed: true })
                .refused
                .as_deref(),
            Some("already pausing")
        );
        assert_eq!(
            ended(&mut machine, Segment::Written).effects,
            [Effect::Paused]
        );
        assert_eq!(machine.phase(), Phase::Paused { segments: 1 });

        let resumed = machine.step(Event::Resume);

        assert_eq!(resumed.refused, None);
        assert!(matches!(
            resumed.effects[..],
            [Effect::Spawn { segment: 2, .. }]
        ));

        // stopped while recording the second: both are joined
        machine.step(Event::Stop);
        assert_eq!(
            ended(&mut machine, Segment::Written).effects,
            [Effect::Join { segments: 2 }]
        );
    }

    #[test]
    fn stop_from_paused_joins_what_there_is() {
        let mut machine = recording();

        machine.step(Event::Pause { framed: true });
        ended(&mut machine, Segment::Written);

        assert_eq!(
            machine.step(Event::Stop).effects,
            [Effect::Join { segments: 1 }]
        );
        assert_eq!(machine.phase(), Phase::Joining);
        // saving is no time for more
        assert!(machine.step(Event::Resume).refused.is_some());
        assert!(machine.step(Event::Start { pick: false }).refused.is_some());
        assert_eq!(machine.step(Event::Stop).effects, []);
    }

    #[test]
    fn a_stop_while_pausing_saves_once_the_segment_is_finished() {
        let mut machine = recording();

        machine.step(Event::Pause { framed: true });

        // the recorder was asked already, and is not asked again
        assert_eq!(machine.step(Event::Stop).effects, []);
        assert_eq!(
            ended(&mut machine, Segment::Written).effects,
            [Effect::Join { segments: 1 }]
        );
    }

    #[test]
    fn no_pause_comes_before_the_first_frame() {
        let mut machine = recording();

        assert!(
            machine
                .step(Event::Pause { framed: false })
                .refused
                .is_some()
        );
        assert!(matches!(
            machine.phase(),
            Phase::Running { pausing: false, .. }
        ));
    }

    #[test]
    fn a_resume_stopped_before_its_frame_keeps_the_segments_before() {
        let mut machine = recording();

        machine.step(Event::Pause { framed: true });
        ended(&mut machine, Segment::Written);
        machine.step(Event::Resume);
        machine.step(Event::Stop);

        assert_eq!(
            ended(&mut machine, Segment::Empty).effects,
            [Effect::Join { segments: 1 }]
        );
    }

    #[test]
    fn a_first_segment_with_no_frame_saves_nothing() {
        let mut machine = recording();

        machine.step(Event::Stop);

        assert!(matches!(
            ended(&mut machine, Segment::Empty).effects[..],
            [Effect::Over(Err(_))]
        ));
        assert_eq!(machine.phase(), Phase::Idle);
    }

    #[test]
    fn a_recorder_that_fails_ends_the_recording() {
        let mut machine = recording();

        assert_eq!(
            ended(&mut machine, Segment::Lost(String::from("gone"))).effects,
            [Effect::Over(Err(String::from("gone")))]
        );
        assert_eq!(machine.phase(), Phase::Idle);
    }

    /*
     * a confirmed defect: a stall or a failure on a resumed segment ended the recording as failed,
     * throwing away segments that were whole. Pausing, resuming and stalling must leave one clip
     */
    #[test]
    fn a_later_segment_lost_keeps_the_segments_before_it() {
        let mut machine = recording();

        machine.step(Event::Pause { framed: true });
        ended(&mut machine, Segment::Written);
        machine.step(Event::Resume);
        machine.step(Event::Stop);

        assert_eq!(
            ended(&mut machine, Segment::Lost(String::from("gone"))).effects,
            [Effect::Join { segments: 1 }]
        );
        assert_eq!(machine.phase(), Phase::Joining);
        // the last segment's damage does not make the join a failure
        assert_eq!(
            machine.step(Event::Joined(Ok(()))).effects,
            [Effect::Over(Ok(()))]
        );
        assert_eq!(machine.phase(), Phase::Idle);

        // and while it records, not only after a stop
        let mut stalling = self::recording();

        stalling.step(Event::Pause { framed: true });
        ended(&mut stalling, Segment::Written);
        stalling.step(Event::Resume);

        assert_eq!(
            ended(&mut stalling, Segment::Lost(String::from("gone"))).effects,
            [Effect::Join { segments: 1 }]
        );
    }

    // a pause that failed before its frame is no damage to the segments before it either
    #[test]
    fn a_later_segment_lost_while_pausing_keeps_the_segments_before_it() {
        let mut machine = recording();

        machine.step(Event::Pause { framed: true });
        ended(&mut machine, Segment::Written);
        machine.step(Event::Resume);
        machine.step(Event::Pause { framed: true });

        assert_eq!(
            ended(&mut machine, Segment::Lost(String::from("gone"))).effects,
            [Effect::Join { segments: 1 }]
        );
        assert_eq!(machine.phase(), Phase::Joining);
    }

    #[test]
    fn a_resume_that_cannot_start_leaves_the_recording_paused() {
        let mut machine = recording();

        machine.step(Event::Pause { framed: true });
        ended(&mut machine, Segment::Written);
        machine.step(Event::Resume);

        let (serial, _) = running(&machine);

        assert_eq!(
            machine
                .step(Event::Spawned {
                    serial,
                    started: Err(String::from("no recorder")),
                })
                .effects,
            [Effect::Declined(String::from("no recorder"))]
        );
        assert_eq!(machine.phase(), Phase::Paused { segments: 1 });
    }

    // a picker that fails is told to the user, unlike one they leave
    #[test]
    fn a_picker_that_fails_is_a_failure_not_a_cancel() {
        let mut machine = Machine::new();

        machine.step(Event::Start { pick: true });

        let serial = machine.serial().unwrap();

        assert_eq!(
            machine
                .step(Event::Picked {
                    serial,
                    pick: Pick::Failed(String::from("slurp is gone"))
                })
                .effects,
            [Effect::Failed(String::from("slurp is gone"))]
        );
        assert!(!machine.busy());
    }

    // a stop that comes before the recorder exists is made good once it does
    #[test]
    fn a_stop_before_the_recorder_started_interrupts_it_when_it_has() {
        let mut machine = Machine::new();

        machine.step(Event::Start { pick: false });

        let (serial, _) = running(&machine);

        assert_eq!(machine.step(Event::Stop).effects, [Effect::Interrupt]);
        assert_eq!(
            machine
                .step(Event::Spawned {
                    serial,
                    started: Ok(()),
                })
                .effects,
            [Effect::Interrupt]
        );
    }

    #[test]
    fn the_end_of_an_older_recorder_is_dropped() {
        let mut machine = recording();
        let (serial, _) = running(&machine);

        machine.step(Event::Pause { framed: true });
        ended(&mut machine, Segment::Written);
        machine.step(Event::Resume);

        let stale = machine.step(Event::Ended {
            serial,
            segment: Segment::Lost(String::from("old")),
        });

        assert_eq!(stale, Step::nothing());
        assert!(matches!(machine.phase(), Phase::Running { segment: 2, .. }));
    }

    #[test]
    fn an_area_is_picked_before_it_is_recorded_and_can_be_left() {
        let mut machine = Machine::new();
        let started = machine.step(Event::Start { pick: true });
        let Some(&Effect::Pick(serial)) = started.effects.first() else {
            panic!("no pick: {started:?}");
        };

        assert!(machine.step(Event::Start { pick: true }).refused.is_some());
        assert!(
            machine
                .step(Event::Pause { framed: true })
                .refused
                .is_some()
        );

        // a pick that is not this one's is dropped
        assert_eq!(
            machine.step(Event::Picked {
                serial: serial + 1,
                pick: Pick::Chosen
            }),
            Step::nothing()
        );

        let mut left = Machine::new();

        left.step(Event::Start { pick: true });
        assert_eq!(
            left.step(Event::Stop).effects,
            [
                Effect::Cancel,
                Effect::Left(String::from("recording cancelled"))
            ]
        );
        assert_eq!(left.phase(), Phase::Idle);

        assert!(matches!(
            machine
                .step(Event::Picked {
                    serial,
                    pick: Pick::Chosen
                })
                .effects[..],
            [Effect::Spawn { segment: 1, .. }]
        ));
        assert!(machine.busy());
    }

    #[test]
    fn a_pick_the_user_leaves_records_nothing() {
        let mut machine = Machine::new();

        machine.step(Event::Start { pick: true });

        let serial = machine.serial().unwrap();

        assert!(matches!(
            machine
                .step(Event::Picked {
                    serial,
                    pick: Pick::Left
                })
                .effects[..],
            [Effect::Left(_)]
        ));
        assert!(!machine.busy());
    }

    // the session locked: any recording is saved, whatever it was doing
    #[test]
    fn a_lock_ends_any_recording_and_leaves_an_idle_one_alone() {
        let mut idle = Machine::new();

        assert_eq!(idle.step(Event::Lock), Step::nothing());

        let mut recording = recording();

        assert_eq!(recording.step(Event::Lock).effects, [Effect::Interrupt]);
        assert_eq!(
            ended(&mut recording, Segment::Written).effects,
            [Effect::Join { segments: 1 }]
        );

        let mut paused = self::recording();

        paused.step(Event::Pause { framed: true });
        ended(&mut paused, Segment::Written);

        assert_eq!(
            paused.step(Event::Lock).effects,
            [Effect::Join { segments: 1 }]
        );

        let mut picking = Machine::new();

        picking.step(Event::Start { pick: true });
        assert_eq!(
            picking.step(Event::Lock).effects.first(),
            Some(&Effect::Cancel)
        );
        assert!(!picking.busy());
    }
}
