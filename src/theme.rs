use amane::Color;

// near-black body, quiet at rest (plan 7); no drop shadow yet, a morphing one drops frames (#45)
pub const BODY: Color = Color::rgb(12, 12, 14);

// high-contrast foreground for text on the body
pub const FG: Color = Color::rgb(242, 242, 247);

// a Satellite or a badge, a step above the body so it reads beside it
pub const DOT: Color = Color::rgb(44, 44, 50);

// secondary text, like an artist under a title
pub const MUTED: Color = Color::rgb(152, 152, 160);

// where cover art goes while there is none to draw
pub const ART: Color = Color::rgb(44, 44, 50);

// a low battery, a warning that waits (plan 7)
pub const AMBER: Color = Color::rgb(255, 176, 32);

// reserved for critical, like a battery about to die (plan 7)
pub const RED: Color = Color::rgb(255, 69, 58);
