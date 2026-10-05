// view code for the full interactive Surfaces; read-only, never write() a Service

use amane::Button;

pub mod controls;
pub mod launcher;
pub mod media;
pub mod notifications;
mod slider;

/*
 * a Surface target pressed with the left button. Only the topmost target gets a click, so a right
 * click on it would otherwise never reach the island beneath: it pins the island there too (#31)
 */
fn on_left(press: impl Fn() + 'static) -> impl Fn(Button) + 'static {
    move |button| match button {
        Button::Left => press(),
        Button::Right => crate::view::pin(),
        _ => {}
    }
}
