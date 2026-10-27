//! Open Sauce One, drawn when `appearance.font` is unset. The bytes are vendored so the face
//! draws when the family is not installed; a set family is not installed here, and still resolves
//! through fontconfig.

use kanade_runtime::{install_face, installed_face};

use crate::theme;

const REGULAR: &[u8] = include_bytes!("../assets/fonts/OpenSauceOne-Regular.ttf");
const MEDIUM: &[u8] = include_bytes!("../assets/fonts/OpenSauceOne-Medium.ttf");
const SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/OpenSauceOne-SemiBold.ttf");

// the weights the shell and the lock screen set. A weight not listed falls back
pub fn install() {
    install_face(
        theme::FONT,
        &[
            ("Regular", REGULAR),
            ("Medium", MEDIUM),
            ("SemiBold", SEMIBOLD),
        ],
    );
}

// the vendored face of `style`, none when it was not installed or that weight is not vendored
pub fn bytes(style: &str) -> Option<&'static [u8]> {
    installed_face(theme::FONT, style)
}
