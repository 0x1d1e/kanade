//! Where a dictation stands (ADR 0111), as a pure machine: events in, the new state and effects out.
//! It reads no clock and starts no process; the audio heard is its only measure of time, so the 120 s
//! cap and the seconds shown are the same count. `run.rs` carries the effects out.
//!
//! Idle -> Gating -> Recording -> Transcribing -> Done | Failed; a Done or Failed is Idle again for
//! the next toggle. Gating is the wait for the key and the consent to be read, which may take a
//! keyring prompt: a second toggle in it is refused, and a cancel ends it, so the verdict that comes
//! after (it names its ticket) is ignored. Every event from a recording names its serial, and one for a recording no longer current
//! is ignored, so a late result never lands on a newer dictation.

use super::api::Error;
use super::gate::Gate;
use super::wav::{self, BYTES_PER_SECOND};

// a recording shorter than this (0.2 s of audio) is not sent: it holds no words
pub const LEAST: usize = BYTES_PER_SECOND / 5;

// why a recording ended because the session locked
pub const LOCKED: &str = "Cancelled: the session locked";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Idle,

    // asked for the gate's verdict, which `ticket` names; becomes the serial of the recording
    Gating { ticket: u64 },
    Recording { serial: u64, held: usize },

    // the recorder is told to stop, or stopped by itself at the cap; the rest of its audio is still read
    Transcribing { serial: u64, held: usize },
    // the text, and what was done with it: typed, pasted or copied, and why copied if it was not asked for
    Done { text: String, how: String },
    Failed(String),
}

impl State {
    // the whole seconds of audio held
    pub fn seconds(&self) -> u32 {
        match self {
            State::Recording { held, .. } | State::Transcribing { held, .. } => {
                (held / BYTES_PER_SECOND) as u32
            }
            _ => 0,
        }
    }

    // the serial of the recording under way, none otherwise
    pub fn serial(&self) -> Option<u64> {
        match self {
            State::Recording { serial, .. } | State::Transcribing { serial, .. } => Some(*serial),
            _ => None,
        }
    }

    // the ticket of the gate being read, none when none is
    pub fn gating(&self) -> Option<u64> {
        match self {
            State::Gating { ticket } => Some(*ticket),
            _ => None,
        }
    }

    // whether a dictation is under way that a cancel would end
    pub fn busy(&self) -> bool {
        self.gating().is_some() || self.serial().is_some()
    }

    // whether a toggle would start a recording
    pub fn starts(&self) -> bool {
        matches!(self, State::Idle | State::Done { .. } | State::Failed(_))
    }

    // as `kanade dictate status` says it; never the text
    pub fn said(&self) -> String {
        match self {
            State::Idle => String::from("idle"),
            State::Gating { .. } => String::from("starting"),
            State::Recording { .. } => format!("recording, {} s heard", self.seconds()),
            State::Transcribing { .. } => String::from("transcribing"),
            State::Done { how, .. } => format!("done; {}", how.to_lowercase()),
            State::Failed(why) => format!("failed: {why}"),
        }
    }
}

#[derive(Debug)]
pub enum Event {
    // starts a dictation by asking for the gate, or stops the recording
    Toggle,

    // the gate's verdict for the ticket `Effect::Gate` named: a start needs `Ready`
    Gated {
        ticket: u64,
        gate: Gate,
    },
    Cancel,

    // the session locked while recording `serial` was heard: the microphone is not left open behind
    // the lock screen, so a recording not yet stopped is given up. One stopped is on its way anyway
    Locked {
        serial: u64,
    },

    // `len` bytes of PCM, a whole number of frames, heard from the recording `serial`
    Audio {
        serial: u64,
        len: usize,
    },

    // the recorder of `serial` ended and its audio is all read; `said` is why when it was unasked for
    Ended {
        serial: u64,
        said: Option<String>,
    },
    Transcribed {
        serial: u64,
        result: Result<String, Error>,
    },

    // the text of recording `serial` was put where it goes, as `how` says, or could not be
    Inserted {
        serial: u64,
        text: String,
        result: Result<String, String>,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    // read the key and the consent, and answer with `Event::Gated` for this ticket
    Gate(u64),
    Start(u64),
    Stop,

    // keep this many bytes, from the start, of the audio just heard
    Keep(usize),

    // the audio is complete: send it
    Transcribe,

    // put the text where the settings and the focused window say; the machine waits for `Inserted`
    Insert(String),

    // what is shown changed: draw the state as it is now
    Show,

    // a toggle that starts nothing: why in full, then in a few words for the island
    Refuse(String, &'static str),
}

pub struct Machine {
    state: State,

    // recordings started since Kanade did, never reused, so a serial names one
    issued: u64,
}

impl Machine {
    pub const fn new() -> Machine {
        Machine {
            state: State::Idle,
            issued: 0,
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn step(&mut self, event: Event) -> Vec<Effect> {
        match (event, &mut self.state) {
            (Event::Toggle, state) if state.starts() => {
                self.issued += 1;
                self.state = State::Gating {
                    ticket: self.issued,
                };

                vec![Effect::Gate(self.issued)]
            }
            (Event::Gated { ticket, gate }, State::Gating { ticket: now }) if ticket == *now => {
                match refusal(gate) {
                    Some((why, brief)) => {
                        self.state = State::Idle;
                        vec![Effect::Refuse(why, brief)]
                    }
                    None => {
                        self.state = State::Recording {
                            serial: ticket,
                            held: 0,
                        };

                        vec![Effect::Start(ticket), Effect::Show]
                    }
                }
            }
            (Event::Toggle, State::Gating { .. }) => vec![Effect::Refuse(
                String::from("still checking the key and your consent; try again in a moment"),
                "Starting",
            )],
            (Event::Toggle, State::Recording { serial, held }) => {
                self.state = State::Transcribing {
                    serial: *serial,
                    held: *held,
                };

                vec![Effect::Stop, Effect::Show]
            }
            (Event::Toggle, _) => vec![Effect::Refuse(
                String::from("still transcribing; `kanade dictate cancel` gives up on it"),
                "Still transcribing",
            )],

            (Event::Cancel, State::Recording { .. }) => self.idle(vec![Effect::Stop]),
            (
                Event::Cancel,
                State::Gating { .. }
                | State::Transcribing { .. }
                | State::Done { .. }
                | State::Failed(_),
            ) => self.idle(Vec::new()),
            (Event::Cancel, State::Idle) => Vec::new(),

            (Event::Locked { serial }, State::Recording { serial: now, .. }) if serial == *now => {
                self.state = State::Failed(String::from(LOCKED));
                vec![Effect::Stop, Effect::Show]
            }

            (Event::Audio { serial, len }, State::Recording { serial: now, held })
                if serial == *now =>
            {
                let before = *held / BYTES_PER_SECOND;
                let (take, full) = wav::accept(*held, len);

                *held += take;

                let mut effects = vec![Effect::Keep(take)];

                if full {
                    self.state = State::Transcribing {
                        serial,
                        held: *held,
                    };
                    effects.extend([Effect::Stop, Effect::Show]);
                } else if *held / BYTES_PER_SECOND != before {
                    effects.push(Effect::Show);
                }

                effects
            }
            (Event::Audio { serial, len }, State::Transcribing { serial: now, held })
                if serial == *now =>
            {
                let (take, _) = wav::accept(*held, len);

                *held += take;

                vec![Effect::Keep(take)]
            }

            (Event::Ended { serial, said }, State::Recording { serial: now, .. })
                if serial == *now =>
            {
                let why = said.unwrap_or_else(|| String::from("the recorder stopped"));

                self.state = State::Failed(why);
                vec![Effect::Show]
            }
            (Event::Ended { serial, .. }, State::Transcribing { serial: now, held })
                if serial == *now =>
            {
                if *held < LEAST {
                    self.state = State::Failed(String::from("Too short to hear"));
                    vec![Effect::Show]
                } else {
                    vec![Effect::Transcribe]
                }
            }

            (Event::Transcribed { serial, result }, State::Transcribing { serial: now, .. })
                if serial == *now =>
            {
                match result {
                    Ok(text) => vec![Effect::Insert(text)],
                    Err(error) => {
                        self.state = State::Failed(short(&error));
                        vec![Effect::Show]
                    }
                }
            }
            (
                Event::Inserted {
                    serial,
                    text,
                    result,
                },
                State::Transcribing { serial: now, .. },
            ) if serial == *now => {
                self.state = match result {
                    Ok(how) => State::Done { text, how },
                    Err(why) => State::Failed(format!("Heard you, but could not insert it: {why}")),
                };
                vec![Effect::Show]
            }

            // a late word of a recording given up on or ended
            _ => Vec::new(),
        }
    }

    fn idle(&mut self, mut effects: Vec<Effect>) -> Vec<Effect> {
        self.state = State::Idle;
        effects.push(Effect::Show);
        effects
    }
}

// why a toggle that would start is refused, in full and in a few words
pub fn refusal(gate: Gate) -> Option<(String, &'static str)> {
    match gate {
        Gate::Ready => None,
        Gate::NoKey => Some((
            String::from("no Gemini API key; run `kanade dictate key` first"),
            "No API key: kanade dictate key",
        )),
        Gate::NoConsent => Some((
            String::from(
                "audio goes to Google and, on the free tier, may be used to improve its products; \
                 run `kanade dictate consent` to read that and accept it",
            ),
            "Consent needed: kanade dictate consent",
        )),
    }
}

// what the island says of an error: short, and never the server's words or the key
pub fn short(error: &Error) -> String {
    match error {
        Error::Key => String::from("Google refused the API key"),
        Error::Limit => String::from("Rate limit reached; try again later"),
        // the status is a number of the protocol, not the server's words; those go to the log
        Error::Server(status) => format!("Gemini is unavailable ({status}); try again"),
        Error::Refused(status, _) => format!("Gemini refused the request ({status})"),
        Error::Unreadable => String::from("The answer could not be read"),
        Error::Silence => String::from("No speech heard"),
        Error::Offline(_) => String::from("Cannot reach Gemini"),
        Error::Withdrawn => String::from("Consent withdrawn; nothing was sent"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording() -> Machine {
        let mut machine = Machine::new();

        assert_eq!(machine.step(Event::Toggle), [Effect::Gate(1)]);
        assert_eq!(
            machine.step(Event::Gated {
                ticket: 1,
                gate: Gate::Ready
            }),
            [Effect::Start(1), Effect::Show]
        );
        machine
    }

    fn hear(machine: &mut Machine, len: usize) -> Vec<Effect> {
        machine.step(Event::Audio { serial: 1, len })
    }

    #[test]
    fn nothing_starts_without_key_and_consent() {
        let mut machine = Machine::new();

        for gate in [Gate::NoKey, Gate::NoConsent] {
            machine.step(Event::Toggle);

            let effects = machine.step(Event::Gated { ticket: 1, gate });

            assert!(matches!(effects[..], [Effect::Refuse(..)]));
            assert_eq!(*machine.state(), State::Idle);

            // the ticket is spent: a verdict again, or for another, starts nothing
            assert!(
                machine
                    .step(Event::Gated {
                        ticket: 1,
                        gate: Gate::Ready
                    })
                    .is_empty()
            );
            machine = Machine::new();
        }
    }

    #[test]
    fn a_second_toggle_while_the_gate_is_read_is_refused() {
        let mut machine = Machine::new();

        assert_eq!(machine.step(Event::Toggle), [Effect::Gate(1)]);
        assert!(matches!(
            machine.step(Event::Toggle)[..],
            [Effect::Refuse(..)]
        ));
        assert_eq!(*machine.state(), State::Gating { ticket: 1 });
    }

    #[test]
    fn a_verdict_after_a_cancel_starts_nothing() {
        let mut machine = Machine::new();

        machine.step(Event::Toggle);
        assert_eq!(machine.step(Event::Cancel), [Effect::Show]);

        // not for the ticket asked before, nor for a new one asked since
        let late = Event::Gated {
            ticket: 1,
            gate: Gate::Ready,
        };

        assert!(machine.step(late).is_empty());
        assert_eq!(machine.step(Event::Toggle), [Effect::Gate(2)]);
        assert!(
            machine
                .step(Event::Gated {
                    ticket: 1,
                    gate: Gate::Ready
                })
                .is_empty()
        );
        assert_eq!(*machine.state(), State::Gating { ticket: 2 });
    }

    #[test]
    fn a_lock_cancels_a_recording_and_the_audio_after_it_goes_nowhere() {
        let mut machine = recording();

        hear(&mut machine, LEAST);
        assert_eq!(
            machine.step(Event::Locked { serial: 1 }),
            [Effect::Stop, Effect::Show]
        );
        assert_eq!(*machine.state(), State::Failed(String::from(LOCKED)));

        // a late chunk, a second lock event and the recorder's end change nothing
        assert!(hear(&mut machine, 64).is_empty());
        assert!(machine.step(Event::Locked { serial: 1 }).is_empty());
        assert!(
            machine
                .step(Event::Ended {
                    serial: 1,
                    said: None
                })
                .is_empty()
        );
        assert_eq!(*machine.state(), State::Failed(String::from(LOCKED)));
    }

    #[test]
    fn a_lock_leaves_a_stopped_recording_and_another_serial_alone() {
        let mut machine = recording();

        hear(&mut machine, LEAST);
        assert!(machine.step(Event::Locked { serial: 9 }).is_empty());

        machine.step(Event::Toggle);
        assert!(machine.step(Event::Locked { serial: 1 }).is_empty());
        assert!(matches!(machine.state(), State::Transcribing { .. }));
    }

    #[test]
    fn a_lock_event_stops_recording_immediately_without_trailing_audio_ingestion() {
        let mut machine = recording();

        hear(&mut machine, LEAST);
        assert_eq!(
            machine.step(Event::Locked { serial: 1 }),
            [Effect::Stop, Effect::Show]
        );
        assert_eq!(*machine.state(), State::Failed(String::from(LOCKED)));

        // Any trailing audio sent after lock produces no effects and keeps nothing
        assert!(hear(&mut machine, 1024).is_empty());
        assert_eq!(*machine.state(), State::Failed(String::from(LOCKED)));
    }

    #[test]
    fn a_dictation_goes_from_recording_to_the_text() {
        let mut machine = recording();

        hear(&mut machine, 2 * BYTES_PER_SECOND);
        assert_eq!(machine.state().seconds(), 2);

        // stopping needs no gate; the audio still on the pipe is kept
        assert_eq!(machine.step(Event::Toggle), [Effect::Stop, Effect::Show]);
        assert_eq!(hear(&mut machine, 64), [Effect::Keep(64)]);
        assert_eq!(
            machine.step(Event::Ended {
                serial: 1,
                said: None
            }),
            [Effect::Transcribe]
        );
        assert_eq!(
            machine.step(Event::Transcribed {
                serial: 1,
                result: Ok(String::from("hello"))
            }),
            [Effect::Insert(String::from("hello"))]
        );
        assert_eq!(
            machine.step(Event::Inserted {
                serial: 1,
                text: String::from("hello"),
                result: Ok(String::from("Copied"))
            }),
            [Effect::Show]
        );
        assert_eq!(
            *machine.state(),
            State::Done {
                text: String::from("hello"),
                how: String::from("Copied")
            }
        );

        // Done is idle for the next toggle, with a new serial
        assert_eq!(machine.step(Event::Toggle), [Effect::Gate(2)]);
    }

    #[test]
    fn the_cap_stops_the_recording_and_sends_what_fits() {
        let mut machine = recording();

        assert_eq!(
            hear(&mut machine, wav::MAX_BYTES + 100),
            [Effect::Keep(wav::MAX_BYTES), Effect::Stop, Effect::Show]
        );
        assert!(
            matches!(machine.state(), State::Transcribing { held, .. } if *held == wav::MAX_BYTES)
        );
        assert_eq!(hear(&mut machine, 100), [Effect::Keep(0)]);
    }

    #[test]
    fn a_recording_too_short_is_not_sent() {
        let mut machine = recording();

        hear(&mut machine, LEAST - 2);
        machine.step(Event::Toggle);

        machine.step(Event::Ended {
            serial: 1,
            said: None,
        });
        assert!(matches!(machine.state(), State::Failed(_)));
    }

    #[test]
    fn a_recorder_that_dies_unasked_fails_the_dictation() {
        let mut machine = recording();

        machine.step(Event::Ended {
            serial: 1,
            said: Some(String::from("pw-record not found")),
        });
        assert_eq!(
            *machine.state(),
            State::Failed(String::from("pw-record not found"))
        );
    }

    #[test]
    fn a_cancelled_dictation_ignores_what_comes_after() {
        let mut machine = recording();

        hear(&mut machine, LEAST);
        machine.step(Event::Toggle);
        assert_eq!(machine.step(Event::Cancel), [Effect::Show]);

        let late = machine.step(Event::Transcribed {
            serial: 1,
            result: Ok(String::from("late")),
        });

        assert!(late.is_empty());
        assert_eq!(*machine.state(), State::Idle);
        assert!(
            machine
                .step(Event::Ended {
                    serial: 1,
                    said: None
                })
                .is_empty()
        );
    }

    // to Transcribing with the text in hand, waiting for `Inserted`
    fn inserting() -> Machine {
        let mut machine = recording();

        hear(&mut machine, LEAST);
        machine.step(Event::Toggle);
        machine.step(Event::Ended {
            serial: 1,
            said: None,
        });
        machine.step(Event::Transcribed {
            serial: 1,
            result: Ok(String::from("hello")),
        });
        machine
    }

    #[test]
    fn text_that_cannot_be_inserted_fails_the_dictation_without_the_text() {
        let mut machine = inserting();

        machine.step(Event::Inserted {
            serial: 1,
            text: String::from("secret words"),
            result: Err(String::from("cannot copy: NotFound")),
        });

        let State::Failed(why) = machine.state() else {
            panic!("expected a failure");
        };

        assert!(why.contains("cannot copy"));
        assert!(!why.contains("secret words"));
    }

    #[test]
    fn an_insertion_of_a_dictation_given_up_on_is_ignored() {
        let mut machine = inserting();

        machine.step(Event::Cancel);

        let late = machine.step(Event::Inserted {
            serial: 1,
            text: String::from("late"),
            result: Ok(String::from("Typed")),
        });

        assert!(late.is_empty());
        assert_eq!(*machine.state(), State::Idle);
    }

    #[test]
    fn errors_become_short_text_that_never_echoes_the_server() {
        let refused = Error::Refused(400, String::from("bad value for [key]"));

        assert_eq!(short(&refused), "Gemini refused the request (400)");
        assert_eq!(
            short(&Error::Server(503)),
            "Gemini is unavailable (503); try again"
        );
        assert_eq!(short(&Error::Silence), "No speech heard");

        let mut machine = recording();

        hear(&mut machine, LEAST);
        machine.step(Event::Toggle);
        machine.step(Event::Ended {
            serial: 1,
            said: None,
        });
        machine.step(Event::Transcribed {
            serial: 1,
            result: Err(Error::Limit),
        });
        assert_eq!(
            *machine.state(),
            State::Failed(String::from("Rate limit reached; try again later"))
        );
    }
}
