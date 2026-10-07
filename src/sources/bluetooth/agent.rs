//! This shell's own BlueZ agent: what a pairing asked from the Bluetooth sub-surface needs from the
//! user, a code to compare, one to type on the device or the device's own PIN, shown there as the
//! `Prompt`. BlueZ asks
//! the agent of the program that asked to pair, so pairings other programs ask, or devices ask,
//! still go to theirs.

use std::hash::{BuildHasher, RandomState};
use std::mem;
use std::sync::{Mutex, Once, PoisonError};

use amane::{Argument, Bus, Method, Service, Value};

use super::BLUEZ;
use crate::supervise;

const PATH: &str = "/org/kanade/bluetooth/agent";
const AGENT: &str = "org.bluez.Agent1";

// shows codes and answers yes or no, typing none: a keyboard pairs by typing the code shown
const CAPABILITY: &str = "DisplayYesNo";

// what the pairing under way needs from the user
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Prompt {
    #[default]
    None,

    // the device at `device` shows `passkey` too; the user says whether they match
    Confirm {
        device: String,
        passkey: String,
    },

    // the user types `code` on the device at `device`
    Show {
        device: String,
        code: String,
    },

    // the device at `device` has a PIN of its own, mostly in its manual; `pin` is what is typed of it
    Pin {
        device: String,
        pin: String,
    },
}

impl Service for Prompt {
    fn new() -> Self {
        Prompt::default()
    }

    fn listen() {}
}

impl Prompt {
    pub fn device(&self) -> Option<&str> {
        match self {
            Prompt::None => None,
            Prompt::Confirm { device, .. }
            | Prompt::Show { device, .. }
            | Prompt::Pin { device, .. } => Some(device),
        }
    }
}

// a PIN is at most 16 bytes
const LONGEST_PIN: usize = 16;

// BlueZ's question the Confirm or Pin prompt waits to answer
static PENDING: Mutex<Option<Method>> = Mutex::new(None);

static ANSWERING: Once = Once::new();

/*
 * answers BlueZ from now on, and registers again before each pairing: BlueZ forgets the agent when
 * it restarts, and says AlreadyExists, harmlessly, when it has not
 */
pub fn register() {
    ANSWERING.call_once(|| {
        // before registering, so BlueZ's first question is not missed
        let mut methods = Bus::system().methods(PATH, AGENT);

        supervise::spawn("bluetooth agent", move || {
            for method in &mut methods {
                answer(method);
            }
        });
    });

    Bus::system().call(
        BLUEZ,
        "/org/bluez",
        "org.bluez.AgentManager1",
        "RegisterAgent",
        &[Argument::Path(PATH.into()), Argument::from(CAPABILITY)],
    );
}

fn answer(method: Method) {
    let arguments = method.arguments();
    let device = arguments.first().map_or("", Value::text).to_owned();
    let number = |at: usize| arguments.get(at).map_or(0.0, Value::number) as u32;

    match method.name() {
        "RequestConfirmation" => {
            let passkey = format!("{:06}", number(1));

            *PENDING.lock().unwrap_or_else(PoisonError::into_inner) = Some(method);
            show(Prompt::Confirm { device, passkey });
        }

        "DisplayPasskey" => {
            method.reply(&[]);
            show(Prompt::Show {
                device,
                code: format!("{:06}", number(1)),
            });
        }

        "DisplayPinCode" => {
            method.reply(&[]);

            let code = arguments.get(1).map_or("", Value::text).to_owned();
            show(Prompt::Show { device, code });
        }

        // a device of the old kind: a keyboard types a code, the rest have one fixed, never guessed
        "RequestPinCode" => {
            if keyboard(&device) {
                let code = format!("{:06}", random());

                method.reply(&[Argument::from(code.as_str())]);
                show(Prompt::Show { device, code });
            } else {
                *PENDING.lock().unwrap_or_else(PoisonError::into_inner) = Some(method);
                show(Prompt::Pin {
                    device,
                    pin: String::new(),
                });
            }
        }

        "RequestPasskey" => {
            let passkey = random();

            method.reply(&[Argument::from(passkey)]);
            show(Prompt::Show {
                device,
                code: format!("{passkey:06}"),
            });
        }

        // the user asked for this pairing, from the sub-surface
        "RequestAuthorization" | "Release" => method.reply(&[]),

        "Cancel" => {
            method.reply(&[]);
            clear();
        }

        // never asked of an agent that is not the default; unanswered, BlueZ refuses in time
        _ => {}
    }
}

// the user saw the same passkey on the device, or typed its PIN; an empty PIN answers nothing
pub fn confirm() {
    let pin = match &*Prompt::read() {
        Prompt::Confirm { .. } => None,
        Prompt::Pin { pin, .. } if !pin.is_empty() => Some(pin.clone()),
        _ => return,
    };

    let pending = PENDING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();

    if let Some(method) = pending {
        match &pin {
            Some(pin) => method.reply(&[Argument::from(pin.as_str())]),
            None => method.reply(&[]),
        }
    }

    show(Prompt::None);
}

/*
 * a letter typed on the Pin prompt, or the last one erased for none. Written in one go, so a
 * pairing ending meanwhile is not brought back
 */
pub fn type_pin(letter: Option<char>) {
    if let Prompt::Pin { pin, .. } = &mut *Prompt::write() {
        *pin = typed(mem::take(pin), letter);
    }
}

// `pin` with `letter` typed, or its last one erased; spaces and what does not fit are not typed
fn typed(mut pin: String, letter: Option<char>) -> String {
    match letter {
        Some(letter) if letter.is_whitespace() || letter.is_control() => {}
        Some(letter) if pin.len() + letter.len_utf8() <= LONGEST_PIN => pin.push(letter),
        Some(_) => {}
        None => {
            pin.pop();
        }
    }

    pin
}

// the pairing of the device at `device` ended, or was cancelled: nothing more to ask of the user
pub fn over(device: &str) {
    if Prompt::read().device() == Some(device) {
        clear();
    }
}

// unanswered, BlueZ's question fails once it gives up waiting
fn clear() {
    PENDING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    show(Prompt::None);
}

fn show(prompt: Prompt) {
    if *Prompt::read() != prompt {
        *Prompt::write() = prompt;
    }
}

fn keyboard(device: &str) -> bool {
    let icon = Bus::system().property(BLUEZ, device, "org.bluez.Device1", "Icon");

    icon.text().starts_with("input-keyboard")
}

// six digits nobody could guess, from the hasher's random keys
fn random() -> u32 {
    (RandomState::new().hash_one(()) % 1_000_000) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_is_typed_and_erased_up_to_16_bytes() {
        let pin = "123"
            .chars()
            .fold(String::new(), |pin, letter| typed(pin, Some(letter)));
        assert_eq!(pin, "123");

        assert_eq!(typed(pin.clone(), None), "12");
        assert_eq!(typed(pin.clone(), Some(' ')), "123");
        assert_eq!(typed(String::new(), None), "");

        let full = "0".repeat(15);
        assert_eq!(typed(full.clone(), Some('\u{e9}')), full);
        assert_eq!(typed(full.clone(), Some('9')).len(), 16);
    }
}
