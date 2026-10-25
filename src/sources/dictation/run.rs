//! Carries out what the machine decides (ADR 0111): `pw-record` for the audio, a thread for the
//! request, `wl-copy` or `wtype` for the text, the Activity for what the user sees. The one lock around the
//! machine and what it works on is held for each step, never for a wait: the key and the consent
//! are read, the request made and the text inserted on threads of their own, so the draw thread never
//! waits on the keyring, the network or a program.
//!
//! The audio is read from the recorder's stdout into memory and goes nowhere else; it is dropped
//! once sent, cancelled or failed. `pw-record` dies with Kanade (`wake::hold_stream`).

use std::collections::VecDeque;
use std::io::{self, Read};
use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::process::{Child, ChildStderr, ChildStdout, ExitStatus};
use std::sync::Mutex;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::{MutexExt, service::Service};

use super::api::Error;
use super::gate::{self, Key};
use super::insert::{self, Focus, Insert, Reason, TYPER};
use super::machine::{Effect, Event, Machine, State};
use super::{Request, transcribe, wav};
use crate::clock;
use crate::config;
use crate::island::activity::{
    Action, Activity, Detail, Dictation, Id, Interrupt, Kind, Lifetime, Priority, Scope,
};
use crate::island::service::IslandService;
use crate::later::Later;
use crate::lock;
use crate::sources::capture::SHOWN;
use crate::sources::wake::{self, NOT_FOUND};
use crate::sources::windows::Windows;
use crate::sources::{caffeine, clipboard};

pub const RECORDER: &str = "pw-record";

// the Activity's actions
pub const STOP: &str = "stop";
pub const CANCEL: &str = "cancel";

// how long a recorder told to stop may take before it is killed
const STOPPING: Duration = Duration::from_secs(3);

// how often a recorder that closed its output is looked at for having exited
const EXITING: Duration = Duration::from_millis(20);

// how long `wtype` may take to type or to send the paste chord before it is killed
const TYPING: Duration = Duration::from_secs(30);
const PASTING: Duration = Duration::from_secs(5);

static RUNTIME: Mutex<Runtime> = Mutex::new(Runtime {
    machine: Machine::new(),
    pcm: Vec::new(),
    key: None,
    child: None,
    focus: None,
    gate: None,
    stop_cancel: None,
});

struct Runtime {
    machine: Machine,

    // the recording so far, then taken by the request; empty while no recording is under way
    pcm: Vec<u8>,

    // read once at the start and held until the request, so it is not read again after the audio
    key: Option<Key>,

    // the recorder and the recording it belongs to; reaped by its reader once its output closes
    child: Option<(u64, Child)>,

    // the window with the focus when the recording started; the text goes only there
    focus: Option<Focus>,

    // the ticket of a gate the machine asked for, until `request` hands it to the gate thread
    gate: Option<u64>,

    // cancelled immediately if session lock requests shutdown
    stop_cancel: Option<Sender<()>>,
}

// the arguments of `pw-record`: raw 16 kHz mono s16 on stdout, named so the graph shows it as Kanade
fn arguments() -> [String; 10] {
    [
        "--raw",
        "--rate",
        &wav::RATE.to_string(),
        "--channels",
        &wav::CHANNELS.to_string(),
        "--format",
        "s16",
        "-P",
        "application.name=Kanade",
        // the file: stdout
        "-",
    ]
    .map(String::from)
}

/*
 * does what `request` asks and answers `later`. A toggle that would start has the key and the
 * consent read on the gate thread first, as the keyring may prompt
 */
pub fn request(request: Request, later: Later) {
    let mut runtime = RUNTIME.locked();

    match request {
        // the gate is read on the gate thread, as the keyring may be slow
        Request::Status => {
            let said = runtime.machine.state().said();

            if let Err(why) = ask(Job::Status {
                said,
                later: later.clone(),
            }) {
                later.refused(why);
            }
        }
        Request::Cancel => {
            // a finished result dismissed is not a dictation cancelled
            let busy = runtime.machine.state().busy();

            runtime.feed(Event::Cancel, &[]);
            later.done(match busy {
                true => "cancelled",
                false => "nothing to cancel",
            });
        }
        // nothing is recorded from a locked session
        Request::Toggle if runtime.machine.state().starts() && lock::holds() => {
            later.refused("the session is locked");
        }
        Request::Toggle if runtime.machine.state().starts() => {
            let refused = runtime.feed(Event::Toggle, &[]);

            match runtime.gate.take() {
                // the recorder starts from the gate thread's answer, which is fed by it
                Some(ticket) => {
                    if let Err(why) = ask(Job::Gate {
                        ticket,
                        later: later.clone(),
                    }) {
                        runtime.feed(Event::Cancel, &[]);
                        later.refused(why);
                    }
                }
                None => later.result(answer(&runtime, refused)),
            }
        }
        Request::Toggle => {
            let refused = runtime.feed(Event::Toggle, &[]);

            later.result(answer(&runtime, refused));
        }
        // handled by the CLI itself, which never asks the shell
        Request::Key | Request::Consent | Request::Forget => {
            later.refused("this runs in your terminal, not in the shell");
        }
    }
}

// what the gate thread is asked
enum Job {
    // a gate to read for the start `ticket` names, and who is to be told how it went
    Gate { ticket: u64, later: Later },

    // how it stands, `said`, with the gate beside it
    Status { said: String, later: Later },
}

/*
 * the one gate thread, started by the first toggle and living as long as Kanade. This is the thread
 * that starts `pw-record`, and that matters: a child started through `setpriv --pdeathsig` dies when
 * the THREAD that started it ends, not the process (ADR 0027; `wake::run`, `caffeine::request`). A
 * thread made for one toggle and gone after it would kill the recorder as it exits. The recorder
 * is therefore started only from here and from the draw thread (`Effect::Start` follows a
 * `Gated` event, and `Gated` is fed only by `begin`, which runs on this thread).
 */
static GATE: Mutex<Option<Sender<Job>>> = Mutex::new(None);

fn ask(job: Job) -> Result<(), String> {
    let mut gate = GATE.locked();

    if let Some(sender) = gate.as_ref() {
        match sender.send(job) {
            Ok(()) => return Ok(()),
            // the thread is gone, which a panic it did not survive could do; start another
            Err(mpsc::SendError(back)) => {
                *gate = None;
                return start_gate(&mut gate, back);
            }
        }
    }

    start_gate(&mut gate, job)
}

fn start_gate(gate: &mut Option<Sender<Job>>, job: Job) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel::<Job>();

    thread::Builder::new()
        .name(String::from("dictation gate"))
        .spawn(move || {
            for job in receiver {
                match job {
                    Job::Gate { ticket, later } => survive(
                        || begin(ticket, &later),
                        || {
                            abort(ticket);
                            later.refused("internal error");
                        },
                    ),
                    Job::Status { said, later } => {
                        survive(|| status(&said, &later), || later.refused("internal error"))
                    }
                }
            }
        })
        .map_err(|error| format!("cannot start a thread: {error}"))?;

    sender
        .send(job)
        .map_err(|_| String::from("the gate thread ended"))?;
    *gate = Some(sender);

    Ok(())
}

// gives up on the start `ticket` names, if it is still the one waited on
fn abort(ticket: u64) {
    let mut runtime = RUNTIME.locked();

    if runtime.machine.state().gating() == Some(ticket) {
        runtime.feed(Event::Cancel, &[]);
    }
}

// how it stands and whether it could start, from the existence of the key and the consent only
fn status(said: &str, later: &Later) {
    let gate = match gate::load_key() {
        Ok(key) => gate::decide(key.is_some(), gate::read_consent().as_deref()).said(),
        Err(_) => "the key cannot be read",
    };

    later.done(format!("{said}; gate: {gate}"));
}

// a toggle that starts, with the gate read first; off the draw thread
fn begin(ticket: u64, later: &Later) {
    let key = gate::load_key();
    let marker = gate::read_consent();

    let mut runtime = RUNTIME.locked();

    // cancelled while the keyring was asked: the key read is dropped, and no one is waiting on a start
    if runtime.machine.state().gating() != Some(ticket) {
        later.refused("cancelled");
        return;
    }

    let decided = match key {
        Ok(key) => {
            let decided = gate::decide(key.is_some(), marker.as_deref());

            if decided == gate::Gate::Ready {
                runtime.key = key;
            }

            decided
        }
        Err(why) => {
            runtime.feed(Event::Cancel, &[]);
            later.refused(format!("cannot read the API key: {why}"));
            return;
        }
    };

    // the session may have locked while the keyring was asked
    if decided == gate::Gate::Ready && lock::holds() {
        runtime.feed(Event::Cancel, &[]);
        later.refused("the session is locked");
        return;
    }

    let refused = runtime.feed(
        Event::Gated {
            ticket,
            gate: decided,
        },
        &[],
    );

    later.result(answer(&runtime, refused));
}

// what a toggle answers: why it was refused or the recorder failed, else how the dictation stands
fn answer(runtime: &Runtime, refused: Option<String>) -> Result<String, String> {
    match (refused, runtime.machine.state()) {
        (Some(why), _) => Err(why),
        (None, State::Failed(why)) => Err(why.clone()),
        (None, state) => Ok(state.said()),
    }
}

// presses the Activity's action for the recording `serial` names; one for another is stale
pub fn act(key: &str, serial: &str) {
    let mut runtime = RUNTIME.locked();

    match (runtime.machine.state().serial(), serial.parse::<u64>()) {
        (Some(now), Ok(named)) if now == named => {}
        _ => return,
    }

    match key {
        STOP => drop(runtime.feed(Event::Toggle, &[])),
        CANCEL => drop(runtime.feed(Event::Cancel, &[])),
        _ => {}
    }
}

/*
 * the session is locking: any recording or stopping dictation is stopped immediately, its child
 * process killed, any stopping timer cancelled, and audio reading aborted.
 */
pub fn locked() {
    let mut runtime = RUNTIME.locked();

    runtime.stop_cancel = None;

    if let Some((serial, child)) = runtime.child.as_mut() {
        let serial = *serial;
        drop(child.kill());

        if matches!(runtime.machine.state(), State::Recording { .. }) {
            runtime.feed(Event::Locked { serial }, &[]);
        }
    }
}

impl Runtime {
    /*
     * gives the machine `event`, with `chunk` the audio it is about, and carries out what it says;
     * returns why a toggle was refused
     */
    fn feed(&mut self, event: Event, chunk: &[u8]) -> Option<String> {
        let mut events = VecDeque::from([event]);
        let mut chunk = chunk;
        let mut refused = None;

        while let Some(event) = events.pop_front() {
            for effect in self.machine.step(event) {
                match effect {
                    Effect::Keep(len) => self.pcm.extend_from_slice(&chunk[..len]),
                    Effect::Gate(ticket) => self.gate = Some(ticket),
                    Effect::Start(serial) => {
                        if let Err(said) = self.start(serial) {
                            events.push_back(Event::Ended {
                                serial,
                                said: Some(said),
                            });
                        }
                    }
                    Effect::Stop => self.stop(),
                    Effect::Transcribe => {
                        if let Err((serial, why)) = self.transcribe() {
                            events.push_back(Event::Transcribed {
                                serial,
                                result: Err(Error::Offline(why)),
                            });
                        }
                    }
                    Effect::Insert(text) => {
                        if let Err((serial, text, why)) = self.insert(text) {
                            events.push_back(Event::Inserted {
                                serial,
                                text,
                                result: Err(why),
                            });
                        }
                    }
                    Effect::Show => self.show(),
                    Effect::Refuse(why, brief) => {
                        // a refusal to start says so on the island; one while it works must not hide it
                        if self.machine.state().starts() {
                            IslandService::write().post(failed(brief), Instant::now());
                        }

                        refused = Some(why);
                    }
                }
            }

            chunk = &[];
        }

        // nothing under way holds audio or the key
        if self.machine.state().serial().is_none() {
            self.pcm = Vec::new();
            self.key = None;
        }

        refused
    }

    fn start(&mut self, serial: u64) -> Result<(), String> {
        self.stop_cancel = None;

        if let Some((_, mut old)) = self.child.take() {
            drop(old.kill());
            drop(old.wait());
        }

        self.pcm = Vec::new();
        self.focus = focus();

        let arguments = arguments();
        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let mut child = wake::hold_stream(RECORDER, &arguments).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                format!("{RECORDER} not found")
            } else {
                format!("cannot run {RECORDER}: {error}")
            }
        })?;
        let (out, err) = (child.stdout.take(), child.stderr.take());

        let listening = thread::Builder::new()
            .name(String::from("dictation"))
            .spawn(move || listen(serial, out, err));

        if let Err(error) = listening {
            drop(child.kill());
            drop(child.wait());

            return Err(format!("cannot listen to {RECORDER}: {error}"));
        }

        self.child = Some((serial, child));

        Ok(())
    }

    // asks the recorder to finish, which closes its output; one that does not is killed
    fn stop(&mut self) {
        let Some((serial, child)) = &self.child else {
            return;
        };
        let serial = *serial;

        drop(clock::interrupt(child));

        let (sender, receiver) = mpsc::channel();
        self.stop_cancel = Some(sender);

        drop(
            thread::Builder::new()
                .name(String::from("dictation stop"))
                .spawn(move || {
                    if matches!(
                        receiver.recv_timeout(STOPPING),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    ) && let Some((now, child)) = RUNTIME.locked().child.as_mut()
                        && *now == serial
                    {
                        drop(child.kill());
                    }
                }),
        );
    }

    // sends what was heard; the serial and why when the thread cannot start
    fn transcribe(&mut self) -> Result<(), (u64, String)> {
        let serial = self.machine.state().serial().unwrap_or_default();
        let pcm = mem::take(&mut self.pcm);
        let key = self.key.take();

        thread::Builder::new()
            .name(String::from("dictation request"))
            .spawn(move || {
                // a panic must end the recording as failed, not leave it transcribing
                let result = survive(
                    || match key {
                        // the consent may have been taken back while it recorded: checked again
                        // now, the last moment before the audio leaves, which drops it if so
                        Some(_) if !gate::may_send(gate::read_consent().as_deref()) => {
                            Err(Error::Withdrawn)
                        }
                        Some(key) => transcribe(&key, &pcm),
                        None => Err(Error::Key),
                    },
                    || Err(Error::Offline(String::from("internal error"))),
                );

                RUNTIME
                    .locked()
                    .feed(Event::Transcribed { serial, result }, &[]);
            })
            .map(drop)
            .map_err(|error| (serial, format!("cannot start a thread: {error}")))
    }

    /*
     * puts `text` where the settings and the window in front say, on a thread of its own, as the
     * programs wait; the serial, the text and why when the thread cannot start
     */
    fn insert(&mut self, text: String) -> Result<(), (u64, String, String)> {
        let serial = self.machine.state().serial().unwrap_or_default();
        let start = self.focus.take();
        let held = text.clone();

        thread::Builder::new()
            .name(String::from("dictation insert"))
            .spawn(move || {
                let result = survive(
                    || insertion(serial, &text, start.as_ref()),
                    || Err(String::from("internal error")),
                );

                RUNTIME.locked().feed(
                    Event::Inserted {
                        serial,
                        text,
                        result,
                    },
                    &[],
                );
            })
            .map(drop)
            .map_err(|error| (serial, held, format!("cannot start a thread: {error}")))
    }

    // the Activity for how it stands, taken away once idle
    fn show(&self) {
        let now = Instant::now();
        let state = self.machine.state();

        match activity(state) {
            Some(activity) => IslandService::write().post(activity, now),
            None => IslandService::write().withdraw(&id(), now),
        }
    }
}

// the window with the focus, as the Windows service has it from niri; none when no window has it
fn focus() -> Option<Focus> {
    Windows::read()
        .running()
        .iter()
        .flat_map(|app| &app.windows)
        .find(|window| window.focused)
        .map(|window| Focus {
            window: window.id.0,
            app_id: window.app_id.clone(),
        })
}

// whether the dictation `serial` names is still the one waiting for its text: not cancelled since
fn current(serial: u64) -> bool {
    RUNTIME.locked().machine.state().serial() == Some(serial)
}

/*
 * decides where `text` goes and puts it there; what was done, in words, or why nothing could be.
 * Nothing is done for a dictation cancelled by now. The decision is made again right before the
 * keys are sent (`perform`), but not atomically with them: the focus can still move in between
 */
fn insertion(serial: u64, text: &str, start: Option<&Focus>) -> Result<String, String> {
    let config = config::get();
    let tool = wake::found(TYPER);
    let decide = || {
        insert::decide(
            config.dictate.insert,
            start,
            focus().as_ref(),
            &config.dictate.never_in,
            tool,
            lock::holds(),
        )
    };
    let chosen = decide();

    if !current(serial) {
        return Err(String::from("cancelled"));
    }

    // none once the dictation was cancelled, which wins over everything but a key already sent
    let again = || current(serial).then(decide);

    perform(&Programs, text, chosen, again).map(|done| done.said().to_owned())
}

// what puts the text anywhere; the programs, or a stand-in under test
trait Tools {
    fn copy(&self, text: &str, sensitive: bool) -> io::Result<()>;
    fn type_line(&self, line: &str) -> Result<(), wake::Failed>;
    fn paste_chord(&self) -> Result<(), wake::Failed>;
}

struct Programs;

impl Tools for Programs {
    fn copy(&self, text: &str, sensitive: bool) -> io::Result<()> {
        clipboard::put_private(text, sensitive)
    }

    fn type_line(&self, line: &str) -> Result<(), wake::Failed> {
        wake::feed_within(TYPER, &["-"], line.as_bytes(), TYPING)
    }

    fn paste_chord(&self) -> Result<(), wake::Failed> {
        wake::act_within(TYPER, &["-M", "ctrl", "-k", "v", "-m", "ctrl"], PASTING)
    }
}

/*
 * does `chosen`; the tool failing is not the end of the text, which is copied then. `again` decides
 * afresh, and is asked just before the keys are sent: a window that took the focus, or a lock, while
 * the clipboard was set gets the copy, and none (the dictation was cancelled) gets nothing more. What
 * is typed is the text as one line without control characters (`insert::typed`), so a terminal is
 * never given a line break or an escape; a paste copies the text as it was said, since the target's
 * bracketed paste is what makes its line breaks safe and a copy may be read in an editor. Returns
 * what was done. Neither the text nor what the tools print goes in an error
 */
fn perform(
    tools: &impl Tools,
    text: &str,
    chosen: Insert,
    again: impl Fn() -> Option<Insert>,
) -> Result<Insert, String> {
    let copy = |text: &str, reason: Reason| match tools.copy(text, reason.sensitive()) {
        Ok(()) => Ok(Insert::Copy(reason)),
        Err(error) => Err(format!("cannot copy: {}", error.kind())),
    };
    let cancelled = || Err(String::from("cancelled"));

    match chosen {
        Insert::Copy(reason) => copy(text, reason),
        Insert::Type => match again() {
            None => cancelled(),
            Some(Insert::Copy(reason)) => copy(text, reason),
            Some(_) => match tools.type_line(&insert::typed(text)) {
                Ok(()) => Ok(Insert::Type),
                Err(_) => copy(text, Reason::ToolFailed),
            },
        },
        Insert::Paste => {
            // the clipboard holds what Ctrl+V pastes, marked sensitive; the chord is what may fail
            // or be held back, and the copy already made is then the answer
            let done = copy(text, Reason::ToolFailed)?;

            match again() {
                None => cancelled(),
                Some(Insert::Copy(reason)) => Ok(Insert::Copy(reason)),
                Some(_) => match tools.paste_chord() {
                    Ok(()) => Ok(Insert::Paste),
                    Err(_) => Ok(done),
                },
            }
        }
    }
}

// runs `work`; a panic in it gives `otherwise` instead, so a thread's failure ends the dictation
fn survive<T>(work: impl FnOnce() -> T, otherwise: impl FnOnce() -> T) -> T {
    panic::catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|_| otherwise())
}

/*
 * reads the recorder `serial` names until its output closes, handing the machine each whole frame
 * heard, then tells it the recorder ended
 */
fn listen(serial: u64, out: Option<ChildStdout>, err: Option<ChildStderr>) {
    survive(
        || hear(serial, out, err),
        || {
            let mut runtime = RUNTIME.locked();

            if let Some((_, mut child)) = runtime.child.take_if(|(now, _)| *now == serial) {
                drop(child.kill());
                drop(child.wait());
            }

            runtime.feed(
                Event::Ended {
                    serial,
                    said: Some(String::from("internal error")),
                },
                &[],
            );
        },
    );
}

fn hear(serial: u64, out: Option<ChildStdout>, err: Option<ChildStderr>) {
    hear_stream(serial, out, err, lock::holds);
}

fn hear_stream<R: Read>(
    serial: u64,
    out: Option<R>,
    err: Option<ChildStderr>,
    is_locked: impl Fn() -> bool,
) {
    let said = thread::Builder::new()
        .name(String::from("dictation said"))
        .spawn(move || caffeine::last_lines(err))
        .ok();

    if let Some(mut out) = out {
        let (mut buffer, mut held) = ([0u8; 8192], Vec::new());

        loop {
            // the microphone is not left open behind the lock screen: immediately drop any held
            // audio buffers, do not feed Event::Audio, and terminate the reader loop
            if is_locked() {
                held.clear();
                let mut runtime = RUNTIME.locked();
                runtime.feed(Event::Locked { serial }, &[]);
                break;
            }

            match out.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => held.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }

            if is_locked() {
                held.clear();
                let mut runtime = RUNTIME.locked();
                runtime.feed(Event::Locked { serial }, &[]);
                break;
            }

            // a frame cut by the read waits for the rest of it
            let whole = held.len() / wav::FRAME * wav::FRAME;

            if whole > 0 {
                RUNTIME
                    .locked()
                    .feed(Event::Audio { serial, len: whole }, &held[..whole]);
                held.drain(..whole);
            }
        }
    }

    let said = said
        .and_then(|reading| reading.join().ok())
        .unwrap_or_default();
    let status = reap(serial);

    RUNTIME.locked().feed(
        Event::Ended {
            serial,
            said: ended(status, said),
        },
        &[],
    );
}

// the exit status of the recorder `serial` names, once it exited; killed if it does not
fn reap(serial: u64) -> Option<ExitStatus> {
    for _ in 0..100 {
        {
            let mut runtime = RUNTIME.locked();

            match runtime.child.as_mut() {
                Some((now, child)) if *now == serial => {
                    if let Ok(Some(status)) = child.try_wait() {
                        runtime.child = None;
                        runtime.stop_cancel = None;
                        return Some(status);
                    }
                }
                _ => return None,
            }
        }

        thread::sleep(EXITING);
    }

    let mut runtime = RUNTIME.locked();
    runtime.stop_cancel = None;

    match runtime.child.take_if(|(now, _)| *now == serial) {
        Some((_, mut child)) => {
            drop(child.kill());
            child.wait().ok()
        }
        None => None,
    }
}

// why the recorder ended, none when it did as asked
fn ended(status: Option<ExitStatus>, said: VecDeque<String>) -> Option<String> {
    let status = status?;

    if status.success() {
        return None;
    }

    if status.code() == Some(NOT_FOUND) {
        return Some(format!("{RECORDER} not found"));
    }

    let said = Vec::from(said).join("; ");

    Some(match said.is_empty() {
        true => format!("{RECORDER} ended ({status})"),
        false => format!("{RECORDER} ended ({status}): {said}"),
    })
}

// one dictation at a time, each state replacing the one before
fn id() -> Id {
    Id::new(Kind::Dictation, "dictation")
}

// Ongoing while it records or transcribes, with Stop and Cancel; the outcome is shown a while
fn activity(state: &State) -> Option<Activity> {
    let working = |detail, actions: Vec<Action>| {
        Activity::new(
            id(),
            Priority::Ongoing,
            Lifetime::Persistent,
            Scope::Global,
            Interrupt::None,
        )
        .expect("a Persistent that does not auto-expand is valid")
        .with_actions(actions)
        .with_detail(Detail::Dictation(detail))
    };
    let action = |key: &str, label: &str| Action {
        key: key.to_owned(),
        label: label.to_owned(),
    };

    match state {
        State::Idle | State::Gating { .. } => None,
        State::Recording { serial, .. } => Some(working(
            Dictation::Recording {
                serial: serial.to_string(),
                seconds: state.seconds(),
            },
            vec![action(STOP, "Stop"), action(CANCEL, "Cancel")],
        )),
        State::Transcribing { serial, .. } => Some(working(
            Dictation::Transcribing {
                serial: serial.to_string(),
            },
            // the upload is under way and cannot be recalled: this drops the result, not the request
            vec![action(CANCEL, "Discard")],
        )),
        State::Done { text, how } => Some(outcome(Dictation::Done {
            text: text.clone(),
            how: how.clone(),
        })),
        State::Failed(why) => Some(failed(why)),
    }
}

fn failed(why: &str) -> Activity {
    outcome(Dictation::Failed {
        why: why.to_owned(),
    })
}

// on the output the user is looking at, as a finished recording is
fn outcome(detail: Dictation) -> Activity {
    Activity::new(
        id(),
        Priority::Actionable,
        Lifetime::Transient(SHOWN),
        Scope::FocusedOutput,
        Interrupt::None,
    )
    .expect("a Transient that does not auto-expand is valid")
    .with_detail(Detail::Dictation(detail))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    // records what would have been done
    #[derive(Default)]
    struct Fake {
        done: RefCell<Vec<String>>,
    }

    impl Tools for Fake {
        fn copy(&self, text: &str, sensitive: bool) -> io::Result<()> {
            self.done
                .borrow_mut()
                .push(format!("copy {sensitive} {text:?}"));
            Ok(())
        }

        fn type_line(&self, line: &str) -> Result<(), wake::Failed> {
            self.done.borrow_mut().push(format!("type {line:?}"));
            Ok(())
        }

        fn paste_chord(&self) -> Result<(), wake::Failed> {
            self.done.borrow_mut().push(String::from("chord"));
            Ok(())
        }
    }

    const WILD: &str = "ls\nrm -rf x\u{1b}[2J\u{3}";

    // the clipboard gets the text as it was said, line breaks and all, as a copy anywhere else does
    #[test]
    fn a_paste_copies_the_text_as_it_was_said() {
        let fake = Fake::default();

        let done = perform(&fake, WILD, Insert::Paste, || Some(Insert::Paste));

        assert_eq!(done, Ok(Insert::Paste));
        assert_eq!(
            *fake.done.borrow(),
            [format!("copy true {WILD:?}"), String::from("chord")]
        );
    }

    #[test]
    fn a_cancel_before_the_keys_wins_and_nothing_is_sent() {
        for chosen in [Insert::Type, Insert::Paste] {
            let fake = Fake::default();

            assert!(perform(&fake, "hello", chosen, || None).is_err());
            assert!(
                fake.done
                    .borrow()
                    .iter()
                    .all(|call| call != "chord" && !call.starts_with("type"))
            );
        }
    }

    #[test]
    fn focus_that_moved_before_the_keys_gets_a_sensitive_copy_instead() {
        let moved = || Some(Insert::Copy(Reason::FocusMoved));

        let fake = Fake::default();
        assert_eq!(
            perform(&fake, "hello", Insert::Type, moved),
            Ok(Insert::Copy(Reason::FocusMoved))
        );
        assert_eq!(*fake.done.borrow(), ["copy true \"hello\""]);

        let fake = Fake::default();
        assert_eq!(
            perform(&fake, "hello", Insert::Paste, moved),
            Ok(Insert::Copy(Reason::FocusMoved))
        );
        assert!(!fake.done.borrow().iter().any(|call| call == "chord"));
    }

    #[test]
    fn hear_stream_terminates_and_drops_audio_when_locked() {
        use std::io::Cursor;

        let serial = {
            let mut runtime = RUNTIME.locked();
            runtime.machine = Machine::new();
            runtime.pcm = Vec::new();
            runtime.machine.step(Event::Toggle);
            runtime.machine.step(Event::Gated {
                ticket: 1,
                gate: gate::Gate::Ready,
            });
            runtime.machine.state().serial().unwrap()
        };

        // Mock audio stream containing 2 frames of audio
        let audio_data = vec![0x42u8; wav::FRAME * 2];
        let cursor = Cursor::new(audio_data);

        // Lock holds immediately
        hear_stream(serial, Some(cursor), None, || true);

        let runtime = RUNTIME.locked();
        // Machine should be in Failed state due to lock
        assert!(matches!(runtime.machine.state(), State::Failed(_)));
        // No trailing audio should have been ingested into runtime.pcm
        assert!(runtime.pcm.is_empty());
    }
}
