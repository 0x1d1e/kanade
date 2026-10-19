//! Keyboard operation of the Settings window. The view registers each control it builds as a stop,
//! in the order it builds them, which is the Tab order: the side first, then the header, the page,
//! the footer. The ring is the one stop the keyboard is on; it survives a redraw by its id, and
//! shows as a 3:1 border on the control (`ThemeRoles::ring`). A key reaches here only when no text
//! input took it, and Up and Down pass through a focused input.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use kanade_runtime::{Key, TextInput, request_frame};

// a control the keyboard can reach, and what it does there
pub struct Stop {
    id: String,

    // Enter and Space
    press: Option<Rc<dyn Fn()>>,

    // a text input taking the keyboard as the ring arrives
    input: Option<&'static str>,

    // the arrow keys and, in a text input, Up and Down: one step back (-1) or on (1)
    step: Option<Rc<dyn Fn(i32)>>,
}

impl Stop {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            press: None,
            input: None,
            step: None,
        }
    }

    pub fn press(mut self, press: impl Fn() + 'static) -> Self {
        self.press = Some(Rc::new(press));

        self
    }

    pub fn input(mut self, id: &'static str) -> Self {
        self.input = Some(id);

        self
    }

    pub fn step(mut self, step: impl Fn(i32) + 'static) -> Self {
        self.step = Some(Rc::new(step));

        self
    }
}

thread_local! {
    // the stops of the last build
    static STOPS: RefCell<Vec<Stop>> = const { RefCell::new(Vec::new()) };

    static AT: RefCell<Option<String>> = const { RefCell::new(None) };

    // counts arrivals, so a scroll area brings the ring into view once for each
    static ARRIVALS: Cell<u64> = const { Cell::new(1) };
}

// a new build of the view starts over
pub fn begin() {
    STOPS.with_borrow_mut(Vec::clear);
}

// adds a stop; the token of the ring's arrival if the ring is on it
pub fn register(stop: Stop) -> Option<u64> {
    let here = AT.with_borrow(|at| at.as_deref() == Some(stop.id.as_str()));

    STOPS.with_borrow_mut(|stops| stops.push(stop));

    here.then(|| ARRIVALS.get())
}

// the window opens with the ring off
pub fn reset() {
    AT.set(None);
}

// Whether the keyboard ring is on a niri setting, including while search results show it.
pub fn on_niri_setting() -> bool {
    AT.with_borrow(|at| {
        at.as_deref().is_some_and(|id| {
            [
                "field settings niri ",
                "switch niri ",
                "choice niri ",
                "reset niri ",
            ]
            .iter()
            .any(|prefix| id.starts_with(prefix))
        })
    })
}

pub fn key(key: Key) {
    match key {
        Key::Tab => travel(1),
        Key::BackTab => travel(-1),
        Key::Enter | Key::Space => run(|stop| stop.press.clone().map(|press| move || press())),
        Key::Left | Key::Up => run(|stop| stop.step.clone().map(|step| move || step(-1))),
        Key::Right | Key::Down => run(|stop| stop.step.clone().map(|step| move || step(1))),
        Key::Escape => leave(),
        _ => {}
    }
}

// the ring to the next or previous stop, round the ends
fn travel(direction: i32) {
    let arrived = STOPS.with_borrow(|stops| {
        let count = stops.len() as i32;
        if count == 0 {
            return None;
        }

        let here = AT.with_borrow(|at| {
            let at = at.as_deref()?;

            stops.iter().position(|stop| stop.id == at)
        });
        let next = match here {
            Some(here) => (here as i32 + direction).rem_euclid(count),
            None if direction > 0 => 0,
            None => count - 1,
        };
        let stop = &stops[next as usize];

        Some((stop.id.clone(), stop.input))
    });
    let Some((id, input)) = arrived else {
        return;
    };

    match input {
        Some(input) => TextInput::focus(input),
        None => TextInput::blur(),
    }

    AT.set(Some(id));
    ARRIVALS.set(ARRIVALS.get() + 1);
    request_frame();
}

// the ring off, and the keyboard back from a text input
fn leave() {
    if AT.take().is_some() {
        TextInput::blur();
        request_frame();
    }
}

// what the ring's stop does for a key, run after the stops are released
fn run<F: Fn() + 'static>(action: impl Fn(&Stop) -> Option<F>) {
    let action = STOPS.with_borrow(|stops| {
        let at = AT.with_borrow(Clone::clone)?;

        stops.iter().find(|stop| stop.id == at).and_then(action)
    });

    if let Some(action) = action {
        action();
        request_frame();
    }
}
