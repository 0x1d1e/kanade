//! A lock screen drawn, as macOS's: the date over a large time near the top; who is signed in, a
//! black pill to type the password in and what became of the last one near the bottom.

use std::borrow::Cow;
use std::f32::consts::TAU;

use tiny_skia::{FillRule, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform};

use crate::picture;
use crate::text::{Font, Line};
use crate::{Backdrop, Color, Scene};

// the field, and the status under it, in logical pixels
const WIDTH: f32 = 240.0;
const HEIGHT: f32 = 36.0;
const STATUS: f32 = 20.0;
const AVATAR: f32 = 72.0;
const GAP: f32 = 20.0;

// the date and the time under it, as large as macOS's
const DATE: f32 = 24.0;
const TIME: f32 = 112.0;
const NAME: f32 = 15.0;
const BODY: f32 = 14.0;

// how far down the output the date starts, and up from its foot the status ends
const ABOVE: f32 = 0.09;
const BELOW: f32 = 0.08;

// the layout label's size, and the room it takes at the field's end (three capitals and a gap)
const LAYOUT: f32 = 12.0;
const LAYOUT_ROOM: f32 = 32.0;

// a dot of the password, and how far apart they are
const DOT: f32 = 3.5;
const PITCH: f32 = 11.0;

// the soft shadow under the pill, as the shell's panes cast, in logical pixels, and how dark
const SHADOW_BLUR: f32 = 12.0;
const SHADOW_DROP: f32 = 4.0;
const SHADOW: u8 = 56;

// how far the field shakes either way at first
pub(crate) const SHAKE: f32 = 12.0;

// the fonts of a scene, read
pub(crate) struct Faces {
    pub regular: Font,
    pub medium: Font,
    pub semibold: Font,
    pub display: Font,
    pub fallback: Option<Font>,
}

impl Faces {
    // `font`, else the fallback at its weight where only that has every character of `text`
    fn covering<'a>(&'a self, font: &'a Font, text: &str) -> Cow<'a, Font> {
        match &self.fallback {
            Some(fallback) if !font.covers(text) && fallback.covers(text) => {
                Cow::Owned(fallback.weighted(font.weight()))
            }
            _ => Cow::Borrowed(font),
        }
    }
}

// the field as typed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Field {
    // how many characters are typed, shown as dots
    pub typed: usize,
    pub caps_lock: bool,
}

// a rounded field or avatar, in logical pixels
#[derive(Debug, Clone, Copy, PartialEq)]
struct Body {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
}

// all a lock surface is drawn from
pub(crate) struct Look<'a> {
    pub scene: &'a Scene,

    // none when they do not read: the lock screen still takes a password, with no text
    pub faces: Option<&'a Faces>,
    pub prepared: Option<&'a Pixmap>,

    // their picture, cut round at `avatar` across
    pub face: Option<&'a Pixmap>,
    pub field: Field,

    // how far the field is shaken aside now, in logical pixels
    pub shaken: f32,

    // the output's logical size, and physical pixels per logical one
    pub size: (f32, f32),
    pub scale: f32,
}

// how many physical pixels across the face is drawn at `scale`
pub(crate) fn avatar(scale: f32) -> u32 {
    (AVATAR * scale).round() as u32
}

/*
 * the lock surface as `width` by `height` physical pixels; none only when it is too large to
 * allocate
 */
pub(crate) fn paint(look: &Look, (width, height): (u32, u32)) -> Option<Pixmap> {
    let fits = |pixmap: &Pixmap| pixmap.width() == width && pixmap.height() == height;
    let prepared = look.prepared.filter(|prepared| fits(prepared));

    // the backdrop, or the flat color it is
    let solid;
    let backdrop = match prepared {
        Some(prepared) => prepared,
        None => {
            let mut pixmap = Pixmap::new(width.max(1), height.max(1))?;
            let color = match &look.scene.backdrop {
                Backdrop::Wallpaper { otherwise, .. } | Backdrop::Solid(otherwise) => *otherwise,
            };
            pixmap.fill(color.skia());
            solid = pixmap;
            &solid
        }
    };
    let mut pixmap = backdrop.clone();
    let pill = Pill {
        fill: look.scene.palette.pill,
        rim: look.scene.palette.rim,
        scale: look.scale,
    };

    let scale = look.scale;
    let (logical_width, logical_height) = look.size;
    let palette = look.scene.palette;
    let middle = logical_width / 2.0;

    // `text` in `font` of `faces` at `size`, centered on `middle`, its baseline where `baseline`
    // says from the line set; nothing without fonts
    let text = |pixmap: &mut Pixmap,
                font: fn(&Faces) -> &Font,
                text: &str,
                size: f32,
                tabular: bool,
                baseline: &dyn Fn(&Line) -> f32,
                color: Color| {
        let Some(faces) = look.faces else {
            return;
        };
        let font = faces.covering(font(faces), text);
        let line = font.shape(text, size * scale, tabular);
        let x = middle - line.width / scale / 2.0;
        font.draw(
            pixmap,
            &line,
            (x * scale, baseline(&line) * scale),
            color,
            1.0,
        );
    };

    // the date and the time, from `ABOVE` down
    let top = logical_height * ABOVE;
    let date = look
        .faces
        .map(|faces| faces.semibold.shape(&look.scene.date, DATE * scale, false));
    let (ascender, descender) = date.as_ref().map_or((DATE, DATE * 0.25), |date| {
        (date.ascender / scale, date.descender / scale)
    });
    let date_baseline = top + ascender;
    text(
        &mut pixmap,
        |faces| &faces.semibold,
        &look.scene.date,
        DATE,
        false,
        &|_| date_baseline,
        palette.text,
    );

    // the time's capitals start a little under the date, as macOS sets them
    text(
        &mut pixmap,
        |faces| &faces.display,
        &look.scene.time,
        TIME,
        true,
        &|time| date_baseline + descender + 8.0 + time.cap / scale,
        palette.text,
    );

    // the column at the bottom, from the status up
    let status_top = logical_height * (1.0 - BELOW) - STATUS;
    let field_top = field_top(look);
    let name_height = look.faces.map_or(NAME * 1.25, |faces| {
        let name = faces.covering(&faces.semibold, &look.scene.name).shape(
            &look.scene.name,
            NAME * scale,
            false,
        );
        (name.ascender + name.descender) / scale
    });
    let name_top = field_top - GAP - name_height;
    let avatar_top = name_top - GAP - AVATAR;

    // their picture in a circle, else their initial on the pill
    let avatar = Body {
        x: middle - AVATAR / 2.0,
        y: avatar_top,
        width: AVATAR,
        height: AVATAR,
        radius: AVATAR / 2.0,
    };
    match look.face {
        Some(face) => pixmap.draw_pixmap(
            (avatar.x * scale).round() as i32,
            (avatar.y * scale).round() as i32,
            face.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        ),
        None => {
            pill.draw(&mut pixmap, avatar);

            let initial: String = look
                .scene
                .name
                .chars()
                .next()
                .map(|first| first.to_uppercase().collect())
                .unwrap_or_default();
            text(
                &mut pixmap,
                |faces| &faces.semibold,
                &initial,
                AVATAR * 0.42,
                false,
                &|line| avatar_top + AVATAR / 2.0 + line.cap / scale / 2.0,
                palette.text,
            );
        }
    }

    text(
        &mut pixmap,
        |faces| &faces.semibold,
        &look.scene.name,
        NAME,
        false,
        &|name| name_top + name.ascender / scale,
        palette.text,
    );

    // the field, shaken aside after a password is refused
    let field = field(look);
    pill.draw(&mut pixmap, field);
    typed(&mut pixmap, look, field);

    // what became of the last password
    if !look.scene.status.is_empty() {
        let color = if look.scene.critical {
            palette.critical
        } else {
            palette.muted
        };
        text(
            &mut pixmap,
            |faces| &faces.medium,
            &look.scene.status,
            BODY,
            false,
            &|line| status_top + STATUS / 2.0 + line.cap / scale / 2.0,
            color,
        );
    }

    Some(pixmap)
}

// the field, shaken aside after a password is refused
fn field(look: &Look) -> Body {
    Body {
        x: look.size.0 / 2.0 - WIDTH / 2.0 + look.shaken,
        y: field_top(look),
        width: WIDTH,
        height: HEIGHT,
        radius: HEIGHT / 2.0,
    }
}

fn field_top(look: &Look) -> f32 {
    look.size.1 * (1.0 - BELOW) - STATUS - GAP - HEIGHT
}

// how many dots the field shows: a character each, as many as fit, the newest kept as it scrolls
fn dots(look: &Look, field: Body) -> usize {
    let inset = HEIGHT / 2.0;
    let caps = 16.0;
    let room = field.width
        - inset * 2.0
        - if look.field.caps_lock {
            caps + 6.0
        } else {
            0.0
        }
        - if look.scene.layout.is_empty() {
            0.0
        } else {
            LAYOUT_ROOM
        };
    let fit = ((room - DOT * 2.0) / PITCH).floor().max(0.0) as usize + 1;

    look.field.typed.min(fit)
}

/*
 * the caret's rectangle in physical pixels, drawn over a `paint` by `draw_caret` and apart from it,
 * so a blink changes only this
 */
pub(crate) fn caret(look: &Look) -> Option<Rect> {
    let field = field(look);
    let dots = dots(look, field);
    let inset = HEIGHT / 2.0;
    let x = if dots == 0 {
        field.x + inset
    } else {
        field.x + inset + (dots - 1) as f32 * PITCH + DOT * 2.0 + 3.0
    };
    let scale = look.scale;

    Rect::from_xywh(
        x * scale,
        (field.y + field.height / 2.0 - 9.0) * scale,
        1.5 * scale,
        18.0 * scale,
    )
}

pub(crate) fn draw_caret(pixmap: &mut Pixmap, look: &Look, rect: Rect) {
    let mut paint = Paint::default();
    paint.set_color(look.scene.palette.text.skia());
    paint.anti_alias = true;
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

// what is typed in the field: a dot a character, else the placeholder, and caps lock
fn typed(pixmap: &mut Pixmap, look: &Look, field: Body) {
    let scale = look.scale;
    let palette = look.scene.palette;
    let inset = HEIGHT / 2.0;
    let middle = field.y + field.height / 2.0;
    let caps = 16.0;

    let dots = dots(look, field);

    let mut path = PathBuilder::new();
    for dot in 0..dots {
        let x = field.x + inset + DOT + dot as f32 * PITCH;
        path.push_circle(x * scale, middle * scale, DOT * scale);
    }
    if let Some(path) = path.finish() {
        fill(pixmap, &path, palette.text);
    }

    if look.field.typed == 0
        && let Some(faces) = look.faces
    {
        let line = faces.regular.shape("Enter password", BODY * scale, false);
        faces.regular.draw(
            pixmap,
            &line,
            (
                (field.x + inset + 4.0) * scale,
                (middle + line.cap / scale / 2.0) * scale,
            ),
            palette.muted,
            1.0,
        );
    }

    // caps lock on, as macOS's arrow at the field's end
    let mut right = field.x + field.width - inset;
    if look.field.caps_lock {
        if let Some(path) = caps_lock(right - caps, middle - caps / 2.0, caps, scale) {
            fill(pixmap, &path, palette.muted);
        }
        right -= caps + 6.0;
    }

    // the keyboard layout in use, right-aligned before it; nothing without text
    if !look.scene.layout.is_empty()
        && let Some(faces) = look.faces
    {
        let font = faces.covering(&faces.semibold, &look.scene.layout);
        let line = font.shape(&look.scene.layout, LAYOUT * scale, false);
        let x = right - line.width / scale;
        font.draw(
            pixmap,
            &line,
            (x * scale, (middle + line.cap / scale / 2.0) * scale),
            palette.muted,
            1.0,
        );
    }
}

// an arrow up over a bar, in a square `size` across from `x`, `y`
fn caps_lock(x: f32, y: f32, size: f32, scale: f32) -> Option<Path> {
    let at = |u: f32, v: f32| ((x + u * size) * scale, (y + v * size) * scale);
    let mut path = PathBuilder::new();

    let points = [
        (0.5, 0.0),
        (1.0, 0.5),
        (0.72, 0.5),
        (0.72, 0.72),
        (0.28, 0.72),
        (0.28, 0.5),
        (0.0, 0.5),
    ];
    let (u, v) = at(points[0].0, points[0].1);
    path.move_to(u, v);
    for (u, v) in points[1..].iter().map(|&(u, v)| at(u, v)) {
        path.line_to(u, v);
    }
    path.close();

    let (left, top) = at(0.28, 0.84);
    let (right, bottom) = at(0.72, 0.98);
    path.push_rect(Rect::from_ltrb(left, top, right, bottom)?);

    path.finish()
}

fn fill(pixmap: &mut Pixmap, path: &Path, color: Color) {
    let mut paint = Paint::default();
    paint.set_color(color.skia());
    paint.anti_alias = true;
    pixmap.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
}

/*
 * the shadow `body` casts, in logical pixels, blurred on a layer of its own; the pill over it
 * hides its middle
 */
fn shadow(pixmap: &mut Pixmap, body: Body, scale: f32) {
    let pad = SHADOW_BLUR * 2.0;
    let Some(mut layer) = Pixmap::new(
        ((body.width + pad * 2.0) * scale).ceil() as u32,
        ((body.height + pad * 2.0 + SHADOW_DROP) * scale).ceil() as u32,
    ) else {
        return;
    };

    let cast = Body {
        x: pad,
        y: pad + SHADOW_DROP,
        ..body
    };
    let Some(shape) = rounded(cast, scale) else {
        return;
    };
    fill(&mut layer, &shape, Color::rgba(0, 0, 0, SHADOW));

    // three box passes, as `picture::blur` makes them: about a gaussian of a third of `SHADOW_BLUR`
    picture::blur(
        &mut layer,
        (SHADOW_BLUR * scale / 3.0).round().max(1.0) as u32,
    );

    pixmap.draw_pixmap(
        ((body.x - pad) * scale).round() as i32,
        ((body.y - pad) * scale).round() as i32,
        layer.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

// how far the field is shaken aside `since` seconds after a password was refused
pub(crate) fn shaken(since: f32) -> Option<f32> {
    const LASTS: f32 = 0.5;
    const TIMES: f32 = 3.5;

    (since < LASTS).then(|| {
        let left = 1.0 - since / LASTS;
        SHAKE * (since / LASTS * TIMES * TAU).sin() * left * left
    })
}

// a pill of the shell's black, with the soft shadow its panes cast
struct Pill {
    fill: Color,
    rim: Color,
    scale: f32,
}

impl Pill {
    fn draw(&self, pixmap: &mut Pixmap, body: Body) {
        let Some(shape) = rounded(body, self.scale) else {
            return;
        };

        shadow(pixmap, body, self.scale);
        fill(pixmap, &shape, self.fill);

        let mut paint = Paint::default();
        paint.set_color(self.rim.skia());
        paint.anti_alias = true;
        let stroke = Stroke {
            width: self.scale,
            ..Stroke::default()
        };
        pixmap.stroke_path(&shape, &paint, &stroke, Transform::identity(), None);
    }
}

// a body's rounded outline, in physical pixels
fn rounded(body: Body, scale: f32) -> Option<Path> {
    let (x, y, width, height) = (
        body.x * scale,
        body.y * scale,
        body.width * scale,
        body.height * scale,
    );
    let radius = (body.radius * scale).min(width / 2.0).min(height / 2.0);

    // a quarter circle's control points sit this far along each side, as a circle's are
    let k = radius * 0.552_284_8;
    let mut path = PathBuilder::new();

    path.move_to(x + radius, y);
    path.line_to(x + width - radius, y);
    path.cubic_to(
        x + width - radius + k,
        y,
        x + width,
        y + radius - k,
        x + width,
        y + radius,
    );
    path.line_to(x + width, y + height - radius);
    path.cubic_to(
        x + width,
        y + height - radius + k,
        x + width - radius + k,
        y + height,
        x + width - radius,
        y + height,
    );
    path.line_to(x + radius, y + height);
    path.cubic_to(
        x + radius - k,
        y + height,
        x,
        y + height - radius + k,
        x,
        y + height - radius,
    );
    path.line_to(x, y + radius);
    path.cubic_to(x, y + radius - k, x + radius - k, y, x + radius, y);
    path.close();

    path.finish()
}
