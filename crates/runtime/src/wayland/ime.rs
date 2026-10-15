use smithay_client_toolkit::reexports::protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3::{self, ZwpTextInputManagerV3},
    zwp_text_input_v3::{self, ChangeCause, ContentHint, ContentPurpose, ZwpTextInputV3},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    globals::GlobalList,
    protocol::{wl_seat::WlSeat, wl_surface::WlSurface},
};

use crate::TextInput;
use crate::input::focus;
use crate::input::ime::{Batch, Context};
use crate::ui_scale;

use super::WaylandState;

/*
 * the compositor's input method for the text input that has the keyboard (text-input-v3). It is
 * enabled while a Kanade text input is focused on a surface the compositor entered, told what
 * the input holds and where its caret is, and its preedit and commits are applied on `done`.
 * Keys the method takes never reach the keyboard handler, so nothing is typed twice
 */
pub struct Ime {
    manager: ZwpTextInputManagerV3,

    // on the first seat with a keyboard
    input: Option<ZwpTextInputV3>,

    // the seat `input` was made for
    seat: Option<WlSeat>,

    // the surface the compositor gave text input to, which is the one with the keyboard
    entered: Option<WlSurface>,

    // what the compositor was last told of the enabled input, none while disabled
    sent: Option<Sent>,

    // `commit` requests so far, which `done` echoes
    commits: u32,

    // a `done` that does not echo them answers an older state: nothing is sent until one does
    stale: bool,

    // what the method has asked since the last `done`
    pending: Batch,

    // the text changed by the method, for the next update to say why
    from_method: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct Sent {
    id: &'static str,
    context: Context,

    // left, top, width and height in the compositor's logical pixels for the surface
    caret: (i32, i32, i32, i32),
}

pub fn bind(globals: &GlobalList, qh: &QueueHandle<WaylandState>) -> Option<Ime> {
    // without the global nothing about text input changes
    let manager = globals.bind(qh, 1..=1, ()).ok()?;

    Some(Ime {
        manager,
        input: None,
        seat: None,
        entered: None,
        sent: None,
        commits: 0,
        stale: false,
        pending: Batch::default(),
        from_method: false,
    })
}

impl Ime {
    fn forget(&mut self) {
        self.sent = None;
        self.stale = false;
        self.pending = Batch::default();
        self.from_method = false;
    }
}

impl WaylandState {
    // the first seat that has a keyboard gets the text input
    pub(super) fn follow_ime(&mut self, qh: &QueueHandle<Self>, seat: &WlSeat) {
        let Some(ime) = &mut self.ime else {
            return;
        };

        if ime.input.is_none() {
            ime.input = Some(ime.manager.get_text_input(seat, qh, ()));
            ime.seat = Some(seat.clone());
        }
    }

    // a seat that goes only takes the text input made for it
    pub(super) fn forget_ime_of(&mut self, seat: &WlSeat) {
        if self
            .ime
            .as_ref()
            .is_some_and(|ime| ime.seat.as_ref() == Some(seat))
        {
            self.forget_ime();
        }
    }

    pub(super) fn forget_ime(&mut self) {
        let Some(ime) = &mut self.ime else {
            return;
        };

        if let Some(input) = ime.input.take() {
            input.destroy();
        }

        ime.seat = None;

        ime.entered = None;
        ime.forget();

        TextInput::end_composition();
    }

    /*
     * brings the compositor's input method up to date with the focused text input: enabled
     * when one has the keyboard, told again when what it holds or where its caret is changed,
     * disabled when none has. Run after a window is drawn, since that is where the caret is known
     */
    pub(super) fn sync_ime(&mut self) {
        let Some(surface) = self.ime.as_ref().and_then(|ime| ime.entered.clone()) else {
            return;
        };

        let wanted = self.wanted_ime(&surface);

        let Some(ime) = &mut self.ime else {
            return;
        };

        let Some(input) = ime.input.clone() else {
            return;
        };

        let Some(wanted) = wanted else {
            // a disable gets no `done` to clear an old one's mark
            ime.stale = false;

            if ime.sent.take().is_some() {
                input.disable();
                input.commit();

                ime.commits = ime.commits.wrapping_add(1);

                TextInput::end_composition();
            }

            return;
        };

        if ime.stale || ime.sent.as_ref() == Some(&wanted) {
            return;
        }

        // another input starts from nothing, enabling it does too
        let switched = ime.sent.as_ref().is_none_or(|sent| sent.id != wanted.id);

        if switched {
            if ime.sent.is_some() {
                TextInput::end_composition();
            }

            input.enable();
        }

        let Context {
            sensitive,
            text,
            cursor,
            anchor,
        } = &wanted.context;

        // a password is told what it is and nothing of what is typed
        let (hint, purpose) = match sensitive {
            true => (
                ContentHint::SensitiveData | ContentHint::HiddenText,
                ContentPurpose::Password,
            ),
            false => (ContentHint::None, ContentPurpose::Normal),
        };

        if !sensitive {
            input.set_surrounding_text(text.clone(), *cursor as i32, *anchor as i32);
        }

        let cause = match std::mem::take(&mut ime.from_method) {
            true => ChangeCause::InputMethod,
            false => ChangeCause::Other,
        };

        let (x, y, width, height) = wanted.caret;

        input.set_text_change_cause(cause);
        input.set_content_type(hint, purpose);
        input.set_cursor_rectangle(x, y, width, height);
        input.commit();

        ime.commits = ime.commits.wrapping_add(1);
        ime.sent = Some(wanted);
    }

    // what the input method should be told now, none when no text input has the keyboard here
    fn wanted_ime(&mut self, surface: &WlSurface) -> Option<Sent> {
        if self.keyboard_focus.as_ref() != Some(surface) {
            return None;
        }

        let (id, editor) = focus::editor()?;

        let (x, y, width, height) = self.window(surface)?.pointer.caret(id)?;

        // the box is in the virtual space, the compositor wants its logical pixels
        let ui = ui_scale::ui_scale();

        Some(Sent {
            id,
            context: editor.context(),
            caret: (
                (x * ui).floor() as i32,
                (y * ui).floor() as i32,
                (width * ui).ceil() as i32,
                (height * ui).ceil() as i32,
            ),
        })
    }

    // applies what the method asked as one change, then draws it
    fn ime_done(&mut self, serial: u32) {
        let Some(ime) = &mut self.ime else {
            return;
        };

        let batch = std::mem::take(&mut ime.pending);

        ime.stale = serial != ime.commits;

        let (Some(surface), Some(sent)) = (ime.entered.clone(), &ime.sent) else {
            return;
        };

        // only the input the method was told about; focus may have moved since
        let Some((_, editor)) = focus::editor().filter(|(id, _)| *id == sent.id) else {
            return;
        };

        if editor.apply(&batch) {
            ime.from_method = true;
        }

        // the preedit and the caret are drawn, and the method hears of the result then
        self.request_frame_on(&surface);
    }
}

impl Dispatch<ZwpTextInputManagerV3, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &ZwpTextInputManagerV3,
        _: zwp_text_input_manager_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpTextInputV3, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(ime) = &mut state.ime else {
            return;
        };

        match event {
            zwp_text_input_v3::Event::Enter { surface } => {
                ime.entered = Some(surface);
                ime.forget();

                state.sync_ime();
            }

            zwp_text_input_v3::Event::Leave { surface } => {
                ime.entered = None;
                ime.forget();

                // the compositor disabled it, and what was being composed goes with it
                TextInput::end_composition();

                state.request_frame_on(&surface);
            }

            zwp_text_input_v3::Event::PreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => {
                ime.pending.preedit = text.map(|text| (text, cursor_begin, cursor_end));
            }

            zwp_text_input_v3::Event::CommitString { text } => {
                ime.pending.commit = text;
            }

            zwp_text_input_v3::Event::DeleteSurroundingText {
                before_length,
                after_length,
            } => {
                ime.pending.delete_before = before_length;
                ime.pending.delete_after = after_length;
            }

            zwp_text_input_v3::Event::Done { serial } => state.ime_done(serial),

            _ => {}
        }
    }
}
