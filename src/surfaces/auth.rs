//! polkit's prompt: fixed request, field, status and action lanes; bounded request disclosure.

use std::sync::OnceLock;
use std::time::Instant;

use kanade_runtime::KeyboardState;
use kanade_runtime::service::Service;
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Arc, Canvas, Cap, Center, Column, Cursor, End, Key, Rectangle, Row, Shape, Start, Text, Widget,
    children, request_frame,
};

use super::password;
use crate::config;
use crate::island::geometry;
use crate::island::motion::Mode;
use crate::island::presentation::Surface;
use crate::island::service::IslandService;
use crate::sources::polkit::{self, Focus, Prompt};
use crate::theme::space::INSET;
use crate::theme::{self, text};

const WIDTH: f32 = geometry::AUTH.width - 2.0 * INSET;
const BODY: f32 = 166.0;

fn lane(height: f32, child: impl Widget + 'static) -> Rectangle {
    Rectangle::new()
        .width(WIDTH)
        .height(height)
        .align_child(Start, Start)
        .clip()
        .child(child)
}

pub(crate) fn surface(_open: bool) -> Rectangle {
    let prompt = Prompt::read();
    let colors = theme::island();
    let focused = prompt.focus();
    let details = prompt.details();
    let header = Row::new(children![
        Rectangle::new()
            .width(WIDTH - 80.0)
            .height(20.0)
            .align_child(Start, Center)
            .child(
                Text::new("Authentication")
                    .size(text::BODY)
                    .weight(text::SEMIBOLD)
                    .color(colors.on_surface)
            ),
        Rectangle::new()
            .width(80.0)
            .height(20.0)
            .radius(4.0)
            .border(
                if focused == Focus::Details { 2.0 } else { 0.0 },
                colors.on_surface
            )
            .align_child(Center, Center)
            .cursor(Cursor::Pointer)
            .semantics(move || Semantics::new(Role::Button, "request details")
                .label(if details { "Hide details" } else { "Details" })
                .focused(focused == Focus::Details)
                .selected(details))
            .on_click(super::on_left(polkit::details))
            .child(
                Text::new(if details { "Back" } else { "Details" })
                    .size(text::LABEL_SMALL)
                    .color(colors.on_surface_variant)
            )
    ])
    .align(Center);

    let body = if details {
        disclosure(&prompt)
    } else {
        let failed = prompt.note().is_some();
        let caps = KeyboardState::read().caps_lock;
        let content = Column::new(children![
            lane(
                36.0,
                Text::new(prompt.message())
                    .size(text::LABEL)
                    .color(colors.on_surface_variant)
                    .wrap_anywhere()
                    .max_lines(2)
                    .elide()
            ),
            lane(8.0, Rectangle::new().width(0.0).height(0.0)),
            lane(
                18.0,
                Text::new(format!("Password for {}", prompt.user()))
                    .size(text::BODY_LARGE)
                    .weight(text::SEMIBOLD)
                    .color(colors.on_surface)
                    .elide()
            ),
            lane(8.0, Rectangle::new().width(0.0).height(0.0)),
            password::auth(
                prompt.secret(),
                WIDTH,
                focused == Focus::Password,
                prompt.checking(),
                failed,
                caps
            ),
            lane(6.0, Rectangle::new().width(0.0).height(0.0)),
            status(&prompt),
            lane(6.0, Rectangle::new().width(0.0).height(0.0)),
        ]);
        lane(BODY, content)
    };
    let actions = lane(
        40.0,
        Rectangle::new()
            .width(WIDTH)
            .height(40.0)
            .align_child(End, Center)
            .child(
                Row::new(children![
                    action(
                        "cancel",
                        "Cancel",
                        Focus::Cancel,
                        true,
                        focused,
                        polkit::cancel
                    ),
                    action(
                        "authenticate",
                        "Authenticate",
                        Focus::Authenticate,
                        prompt.can_submit() && !details,
                        focused,
                        polkit::submit
                    ),
                ])
                .gap(8.0),
            ),
    );

    Rectangle::new()
        .width(geometry::AUTH.width)
        .height(geometry::AUTH.height)
        .padding(INSET)
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::Region, "authentication").label("Authentication"))
        .child(Column::new(children![
            lane(20.0, header),
            lane(14.0, Rectangle::new().width(0.0).height(0.0)),
            body,
            actions
        ]))
}

fn action(
    node: &'static str,
    label: &'static str,
    target: Focus,
    enabled: bool,
    focus: Focus,
    press: fn(),
) -> Rectangle {
    let primary = target == Focus::Authenticate;
    let width = if primary { 128.0 } else { 100.0 };
    let colors = theme::island();
    let fill = if enabled {
        colors.on_surface
    } else {
        colors.surface_container_high
    };
    let ink = if primary && enabled {
        colors.surface
    } else if enabled {
        colors.on_surface
    } else {
        colors.on_surface_variant
    };
    let button = Rectangle::new()
        .width(width)
        .height(40.0)
        .radius(20.0)
        .border(
            if focus == target { 2.0 } else { 1.0 },
            if focus == target {
                colors.on_surface
            } else {
                colors.outline
            },
        )
        .align_child(Center, Center)
        .cursor(if enabled {
            Cursor::Pointer
        } else {
            Cursor::Default
        })
        .semantics(move || {
            Semantics::new(Role::Button, node)
                .label(label)
                .disabled(!enabled)
                .focused(focus == target)
        })
        .on_click(super::on_left(move || {
            if enabled {
                polkit::focus(target);
                press();
            }
        }))
        .child(
            Text::new(label)
                .size(text::LABEL)
                .weight(text::SEMIBOLD)
                .color(ink)
                .elide(),
        );
    if primary { button.fill(fill) } else { button }
}

fn status(prompt: &Prompt) -> Rectangle {
    let colors = theme::island();
    if prompt.checking() {
        static START: OnceLock<Instant> = OnceLock::new();
        let angle = if config::get().island.motion == Mode::Reduced {
            0.0
        } else {
            request_frame();
            START.get_or_init(Instant::now).elapsed().as_secs_f32() * (360.0 / 1.3) % 360.0
        };
        let spinner = Canvas::new().width(14.0).height(14.0).shapes(vec![
            Box::new(Arc::new().radius(6.0).stroke(1.5, colors.outline)),
            Box::new(
                Arc::new()
                    .radius(6.0)
                    .start(angle)
                    .sweep(90.0)
                    .stroke(1.5, colors.on_surface)
                    .cap(Cap::Round),
            ),
        ]);
        return lane(
            32.0,
            Row::new(children![
                spinner,
                Text::new("Verifying password…")
                    .size(text::LABEL_SMALL)
                    .color(colors.on_surface_variant)
            ])
            .gap(7.0)
            .align(Center),
        );
    }
    let Some(note) = prompt.note() else {
        return lane(32.0, Rectangle::new().width(0.0).height(0.0));
    };
    let critical = theme::signals().critical;
    lane(
        32.0,
        Column::new(children![
            lane(
                16.0,
                Text::new(note)
                    .size(text::LABEL_SMALL)
                    .color(critical)
                    .elide()
            ),
            lane(
                16.0,
                Text::new(format!(
                    "{} {} left for this request",
                    prompt.tries(),
                    if prompt.tries() == 1 { "try" } else { "tries" }
                ))
                .size(text::LABEL_SMALL)
                .color(critical)
                .elide()
            ),
        ]),
    )
}

// The disclosure occupies only the request/field/status area; actions stay in their own lane.
// Text is content, never a semantic label, and neither the cookie nor the details map is read.
fn detail_content(prompt: &Prompt) -> Column {
    let colors = theme::island();
    let mut content: Vec<Box<dyn Widget>> = Vec::new();
    for (label, value) in [
        ("Full request", prompt.message()),
        ("Action ID", prompt.action()),
    ] {
        content.push(Box::new(lane(
            18.0,
            Text::new(label)
                .size(text::LABEL_SMALL)
                .weight(text::SEMIBOLD)
                .color(colors.on_surface),
        )));
        let value = if value.is_empty() {
            "Unavailable"
        } else {
            value
        };
        let text = Text::new(value)
            .size(text::LABEL)
            .color(colors.on_surface_variant)
            .wrap_anywhere();
        let height = text.height_in(WIDTH - 12.0);
        content.push(Box::new(
            Rectangle::new()
                .width(WIDTH - 12.0)
                .height(height)
                .child(text),
        ));
        content.push(Box::new(lane(8.0, Rectangle::new().width(0.0).height(0.0))));
    }
    let note =
        Text::new("An action ID describes a policy action; it does not verify an app’s identity.")
            .size(text::LABEL_SMALL)
            .color(colors.on_surface_variant)
            .wrap_anywhere();
    let height = note.height_in(WIDTH - 12.0);
    content.push(Box::new(
        Rectangle::new()
            .width(WIDTH - 12.0)
            .height(height)
            .child(note),
    ));
    Column::new(content).width(WIDTH)
}

fn maximum_scroll(prompt: &Prompt) -> f32 {
    let content = detail_content(prompt);
    match Widget::height(&content) {
        kanade_runtime::Size::Fixed(height) => (height - BODY + 8.0).max(0.0),
        _ => 0.0,
    }
}

fn disclosure(prompt: &Prompt) -> Rectangle {
    let maximum = maximum_scroll(prompt);
    let focus = prompt.focus() == Focus::DetailsBody;
    let content = detail_content(prompt);
    let height = Widget::height(&content);
    let metadata = format!("{}\n{}", prompt.message(), prompt.action());
    lane(
        BODY,
        Rectangle::new()
            .width(WIDTH)
            .height(BODY - 6.0)
            .clip()
            .border(if focus { 2.0 } else { 0.0 }, theme::island().on_surface)
            .semantics(move || {
                Semantics::new(Role::Region, "request metadata")
                    .content(metadata.clone())
                    .focused(focus)
            })
            .on_click(super::on_left(|| polkit::focus(Focus::DetailsBody)))
            .on_scroll(move |delta| polkit::scroll(delta.y * 24.0, maximum))
            .child(
                Rectangle::new()
                    .width(WIDTH)
                    .height(height)
                    .translate(0.0, -prompt.scroll().min(maximum))
                    .child(content),
            ),
    )
}

pub(crate) fn key(monitor: &str, key: Key) -> bool {
    let island = IslandService::read();
    if island.surface() != Some(Surface::Auth) || island.expanded_on() != Some(monitor) {
        return false;
    }
    drop(island);
    let maximum = {
        let prompt = Prompt::read();
        if prompt.details() {
            maximum_scroll(&prompt)
        } else {
            0.0
        }
    };
    polkit::key(key, maximum);
    true
}
