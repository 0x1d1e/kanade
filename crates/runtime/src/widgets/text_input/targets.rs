use std::rc::Rc;

use crate::graphics::Area;
use crate::input::focus::FocusHandler;
use crate::input::ime::{self, Batch, Caret, Context};
use crate::input::{Cursor, Handlers, Target, focus, paste};
use crate::{Button, Key, Modifiers};

use super::field::Effect;
use super::{TextHandler, TextInput, fields};

pub fn collect_targets(text_input: &TextInput, area: Area, targets: &mut Vec<Target>) {
    let id = text_input.id;
    let on_key = key_handler(text_input);

    let editor: Rc<dyn ime::Editor> = Rc::new(Editor {
        id,
        sensitive: text_input.password,
        on_change: text_input.on_change.clone(),
    });

    // the handler holds this redraw's on_change and on_submit, so a focused input takes the new one
    if focus::wanted(id) || focus::has(id) || text_input.focused {
        focus::set(id, on_key.clone(), editor.clone());
    }

    let on_click = {
        let editor = editor.clone();

        move |_: Button| {
            focus::set(id, on_key.clone(), editor.clone());
        }
    };

    let handlers = Handlers {
        click: Some(Rc::new(on_click)),
        cursor: Some(Cursor::Text),
        ..Handlers::default()
    };

    let mut target = Target::new(area, handlers);

    // only the input that has the keyboard has an input method to tell where it is
    if focus::has(id) {
        target.caret = Some(Caret {
            id,
            area: text_input.caret_area(&fields::get(id), area),
        });
    }

    targets.push(target);
}

// the input method reaches the input through this, kept by the focus like the key handler
struct Editor {
    id: &'static str,
    sensitive: bool,
    on_change: Option<TextHandler>,
}

impl ime::Editor for Editor {
    fn context(&self) -> Context {
        fields::get(self.id).context(self.sensitive)
    }

    fn apply(&self, batch: &Batch) -> bool {
        let mut field = fields::get(self.id);

        let effect = field.apply_done(batch);

        // stored before the handler runs, so it reads the new text
        fields::set(self.id, field.clone());

        if effect == Effect::Unchanged {
            return false;
        }

        changed(field.text, self.on_change.as_ref());

        true
    }
}

fn key_handler(text_input: &TextInput) -> FocusHandler {
    let id = text_input.id;

    let on_change = text_input.on_change.clone();
    let on_submit = text_input.on_submit.clone();

    let on_key = move |key: Key, modifiers: Modifiers| match key {
        Key::Enter => submit(id, on_submit.as_ref()),
        Key::Escape => focus::clear(),
        _ => edit(id, key, modifiers, on_change.as_ref()),
    };

    Rc::new(on_key)
}

fn submit(id: &'static str, on_submit: Option<&TextHandler>) {
    let Some(on_submit) = on_submit else {
        return;
    };

    on_submit(fields::get(id).text);
}

fn edit(id: &'static str, key: Key, modifiers: Modifiers, on_change: Option<&TextHandler>) {
    let mut field = fields::get(id);

    let effect = field.apply(key, modifiers);

    // stored before the handler runs, so it reads the new text
    fields::set(id, field.clone());

    match effect {
        Effect::Unchanged => {}
        Effect::Changed => changed(field.text, on_change),
        Effect::Paste => ask_for_paste(id, on_change.cloned()),
    }
}

fn changed(text: String, on_change: Option<&TextHandler>) {
    let Some(on_change) = on_change else {
        return;
    };

    on_change(text);
}

fn ask_for_paste(id: &'static str, on_change: Option<TextHandler>) {
    paste::request(Box::new(move |text| {
        // the keyboard moved on while the text was on its way
        if !focus::has(id) {
            return;
        }

        let mut field = fields::get(id);

        if field.paste(&text) == Effect::Unchanged {
            return;
        }

        fields::set(id, field.clone());

        changed(field.text, on_change.as_ref());
    }));
}
