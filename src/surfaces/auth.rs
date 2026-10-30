//! polkit's authentication prompt on the Island: what polkit asks, whose password is typed, the
//! field, and Cancel. Nothing but its own answer or the session lock closes it (ADR 0146).

use kanade_runtime::service::Service;
use kanade_runtime::{Center, Column, Key, Rectangle, Text, Widget};

use super::pad;
use super::password;
use crate::island::presentation::Surface;
use crate::island::service::IslandService;
use crate::sources::polkit::{self, Prompt};
use crate::theme;
use crate::theme::space::INSET;

// how tall the note a failed try leaves is, one line
const LINE: f32 = 16.0;

// polkit's message, whose password is typed, and the note a failed try leaves, then Cancel and what confirms the password
pub(crate) fn surface(_open: bool) -> Rectangle {
    let prompt = Prompt::read();

    let width = pad::WIDTH - 2.0 * INSET;

    let message = Text::new(prompt.message())
        .size(theme::text::LABEL_SMALL)
        .color(theme::island().on_surface_variant)
        .elide();

    let who = Text::new(format!("Password for {}", prompt.user()))
        .size(theme::text::BODY)
        .color(theme::island().on_surface)
        .weight(theme::text::SEMIBOLD)
        .elide();

    let note = prompt.note().map(|note| {
        Box::new(
            Rectangle::new()
                .width(width)
                .height(LINE)
                .align_child(Center, Center)
                .child(
                    Text::new(note)
                        .size(theme::text::LABEL_SMALL)
                        .color(theme::island().on_surface_variant)
                        .elide(),
                ),
        ) as Box<dyn Widget>
    });

    let mut content: Vec<Box<dyn Widget>> = vec![
        Box::new(message),
        Box::new(who),
        Box::new(password::field(prompt.secret(), width, "Password")),
    ];
    content.extend(note);
    content.push(Box::new(pad::pills(vec![
        Box::new(pad::pill("cancel", "Cancel", false, polkit::cancel)),
        Box::new(pad::primary_pill(
            "authenticate",
            "Authenticate",
            polkit::submit,
        )),
    ])));

    pad::frame(
        "Authentication",
        Rectangle::new()
            .width(pad::WIDTH)
            .height(pad::BODY)
            .align_child(Center, Center)
            .child(Column::new(content).gap(4.0).align(Center)),
    )
}

// Enter, Backspace, a character, and Escape: the Surface answers every key itself, so none of
// them reaches the window behind
pub(crate) fn key(_monitor: &str, key: Key) -> bool {
    // only the Surface this prompt draws answers
    if IslandService::read().surface() != Some(Surface::Auth) {
        return false;
    }

    match key {
        Key::Character(letter) => polkit::type_char(letter),
        Key::Backspace => polkit::erase(),
        Key::Enter => polkit::submit(),
        Key::Escape => polkit::cancel(),
        _ => return false,
    };

    true
}
