//! A lock screen drawn, as macOS's: the date over a large time near the top; who is signed in, a
//! pill of liquid glass to type the password in and what became of the last one near the bottom.
//! The glass is the shell's, worked out on the CPU: the backdrop frosted, its rim bent as
//! `src/glass.wgsl` bends the Island's, see `refract`, a tint and the highlight of `src/glass.wgsl`.

use std::borrow::Cow;
use std::f32::consts::TAU;

use tiny_skia::{
    FillRule, FilterQuality, Paint, Path, PathBuilder, Pattern, Pixmap, PixmapPaint, Rect,
    SpreadMode, Transform,
};

use crate::picture::{self, Picture, Prepared};
use crate::refract::{self, Body, Capture, MARGIN};
use crate::text::{Font, Line};
use crate::{Backdrop, Color, Palette, Scene};

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

// a dot of the password, and how far apart they are
const DOT: f32 = 3.5;
const PITCH: f32 = 11.0;

// the soft shadow under the glass, as the shell's panes cast, in logical pixels, and how dark
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

    // the caret is shown, as it blinks
    pub caret: bool,
}

// all a lock surface is drawn from
pub(crate) struct Look<'a> {
    pub scene: &'a Scene,

    // none when they do not read: the lock screen still takes a password, with no text
    pub faces: Option<&'a Faces>,
    pub prepared: Option<&'a Prepared>,

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
    let prepared = look.prepared.filter(|prepared| fits(&prepared.shown));

    // the backdrop, and as seen through glass: its frost, or the flat color it is
    let solid;
    let (backdrop, frost) = match prepared {
        Some(prepared) => (&prepared.shown, Some(&prepared.frost)),
        None => {
            let mut pixmap = Pixmap::new(width.max(1), height.max(1))?;
            let color = match &look.scene.backdrop {
                Backdrop::Wallpaper { otherwise, .. } | Backdrop::Solid(otherwise) => *otherwise,
            };
            pixmap.fill(color.skia());
            solid = pixmap;
            (&solid, None)
        }
    };
    let mut pixmap = backdrop.clone();
    let glass = Glass {
        backdrop,
        frost: frost.unwrap_or(backdrop),
        size: (width, height),
        palette: look.scene.palette,
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
    let status_bottom = logical_height * (1.0 - BELOW);
    let status_top = status_bottom - STATUS;
    let field_top = status_top - GAP - HEIGHT;
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

    // their picture in a circle, else their initial on glass
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
            glass.draw(&mut pixmap, avatar);

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
    let field = Body {
        x: middle - WIDTH / 2.0 + look.shaken,
        y: field_top,
        width: WIDTH,
        height: HEIGHT,
        radius: HEIGHT / 2.0,
    };
    glass.draw(&mut pixmap, field);
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

// what is typed in the field: a dot a character, else the placeholder; the caret, and caps lock
fn typed(pixmap: &mut Pixmap, look: &Look, field: Body) {
    let scale = look.scale;
    let palette = look.scene.palette;
    let inset = HEIGHT / 2.0;
    let middle = field.y + field.height / 2.0;
    let caps = 16.0;

    // as many dots as fit, the newest kept, as the field scrolls
    let room = field.width
        - inset * 2.0
        - if look.field.caps_lock {
            caps + 6.0
        } else {
            0.0
        };
    let fit = ((room - DOT * 2.0) / PITCH).floor().max(0.0) as usize + 1;
    let dots = look.field.typed.min(fit);

    let mut path = PathBuilder::new();
    for dot in 0..dots {
        let x = field.x + inset + DOT + dot as f32 * PITCH;
        path.push_circle(x * scale, middle * scale, DOT * scale);
    }
    if let Some(path) = path.finish() {
        fill(pixmap, &path, palette.text);
    }

    let caret_x = if dots == 0 {
        field.x + inset
    } else {
        field.x + inset + (dots - 1) as f32 * PITCH + DOT * 2.0 + 3.0
    };

    if look.field.typed == 0
        && let Some(faces) = look.faces
    {
        let line = faces.regular.shape("Enter Password", BODY * scale, false);
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

    if look.field.caret
        && let Some(rect) = Rect::from_xywh(
            caret_x * scale,
            (middle - 9.0) * scale,
            1.5 * scale,
            18.0 * scale,
        )
    {
        let mut paint = Paint::default();
        paint.set_color(palette.text.skia());
        paint.anti_alias = true;
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }

    // caps lock on, as macOS's arrow at the field's end
    if look.field.caps_lock {
        let right = field.x + field.width - inset;
        if let Some(path) = caps_lock(right - caps, middle - caps / 2.0, caps, scale) {
            fill(pixmap, &path, palette.muted);
        }
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
 * the shadow `body` casts, in logical pixels, blurred on a layer of its own; the glass over it
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

// a pane of the shell's liquid glass, over a backdrop
struct Glass<'a> {
    backdrop: &'a Pixmap,
    frost: &'a Pixmap,

    // the backdrop's size, which the frost is stretched to
    size: (u32, u32),
    palette: Palette,
    scale: f32,
}

impl Glass<'_> {
    // the glass over `body`, in logical pixels: frost, bent rim, tint and highlight
    fn draw(&self, pixmap: &mut Pixmap, body: Body) {
        let scale = self.scale;
        let Some(shape) = rounded(body, scale) else {
            return;
        };

        shadow(pixmap, body, scale);

        // a material but liquid glass: its tint over the backdrop, as the shell lays it
        if let Some(tint) = self.palette.tint {
            fill(pixmap, &shape, tint);
            self.highlight(pixmap, body);
            return;
        }

        // what is behind it, frosted
        let stretched = Transform::from_scale(
            self.size.0 as f32 / self.frost.width() as f32,
            self.size.1 as f32 / self.frost.height() as f32,
        );
        let mut paint = Paint {
            shader: Pattern::new(
                self.frost.as_ref(),
                SpreadMode::Pad,
                FilterQuality::Bilinear,
                1.0,
                stretched,
            ),
            anti_alias: true,
            ..Paint::default()
        };
        pixmap.fill_path(
            &shape,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );

        // its rim, what is just outside bent in, and how bright that is
        let light = self.rim(pixmap, body);

        // as `src/glass/pane.rs` tints liquid glass: brighter behind, more tint under light text
        let opacity = if self.palette.dark {
            0.10 + 0.25 * light
        } else {
            0.62 - 0.30 * light
        };
        let tint = if self.palette.dark {
            Color::rgba(18, 18, 24, 255)
        } else {
            Color::rgba(246, 246, 250, 255)
        };
        paint.shader = tiny_skia::Shader::SolidColor(tint.faded(opacity).skia());
        pixmap.fill_path(
            &shape,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );

        self.highlight(pixmap, body);
    }

    // the rim bent from the sharp backdrop around `body`, drawn; the backdrop's light around it
    fn rim(&self, pixmap: &mut Pixmap, body: Body) -> f32 {
        let scale = self.scale;
        let (width, height) = (self.backdrop.width() as i64, self.backdrop.height() as i64);

        // the captured area: the body and `MARGIN` around it, within the output
        let left = (((body.x - MARGIN) * scale).floor() as i64).clamp(0, width);
        let top = (((body.y - MARGIN) * scale).floor() as i64).clamp(0, height);
        let right = (((body.x + body.width + MARGIN) * scale).ceil() as i64).clamp(0, width);
        let bottom = (((body.y + body.height + MARGIN) * scale).ceil() as i64).clamp(0, height);
        if right <= left || bottom <= top {
            return 0.0;
        }

        let (across, down) = ((right - left) as usize, (bottom - top) as usize);
        let stride = width as usize * 4;
        let mut pixels = Vec::with_capacity(across * down * 4);
        for row in top as usize..bottom as usize {
            let start = row * stride + left as usize * 4;
            pixels.extend_from_slice(&self.backdrop.data()[start..start + across * 4]);
        }

        let capture = Capture {
            pixels: &pixels,
            width: across,
            height: down,
            body_x: body.x - left as f32 / scale,
            body_y: body.y - top as f32 / scale,
            scale: (scale, scale),
        };
        let (rim_width, rim_height, rgba, light) = refract::rim(&capture, body);

        let rim = Picture {
            width: rim_width,
            height: rim_height,
            rgba,
        };
        if let Some(rim) = rim.pixmap() {
            pixmap.draw_pixmap(
                (body.x * scale).round() as i32,
                (body.y * scale).round() as i32,
                rim.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }

        light
    }

    // `src/glass.wgsl` at rest, with no pointer light: the glare, Fresnel lift, bevel and sheen
    fn highlight(&self, pixmap: &mut Pixmap, body: Body) {
        let scale = self.scale;
        let dark = if self.palette.dark { 1.0 } else { 0.0 };
        let highlight = self.palette.highlight;
        let (width, height) = (pixmap.width() as i64, pixmap.height() as i64);

        let left = ((body.x * scale).floor() as i64).clamp(0, width);
        let top = ((body.y * scale).floor() as i64).clamp(0, height);
        let right = (((body.x + body.width) * scale).ceil() as i64).clamp(0, width);
        let bottom = (((body.y + body.height) * scale).ceil() as i64).clamp(0, height);

        let half = (body.width / 2.0, body.height / 2.0);
        let radius = body.radius.min(half.0).min(half.1);
        let sun = normalize((-0.5, -0.85));
        let small = mix(0.6, 1.0, smoothstep(32.0, 120.0, body.height));
        let band = 14.0f32.min(half.0.min(half.1) * 0.6);
        let stride = width as usize * 4;
        let data = pixmap.data_mut();

        for row in top..bottom {
            for column in left..right {
                // the point in the body, from its middle, in logical pixels
                let x = (column as f32 + 0.5) / scale - body.x;
                let y = (row as f32 + 0.5) / scale - body.y;
                let p = (x - half.0, y - half.1);
                let d = rounded_distance(p, half, radius);

                // outside, but for the edge's antialiasing
                let coverage = (0.5 - d * scale).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    continue;
                }

                let inside = (-d).max(0.0);
                let e = 0.75;
                let n = normalize((
                    rounded_distance((p.0 + e, p.1), half, radius)
                        - rounded_distance((p.0 - e, p.1), half, radius)
                        + 1e-5,
                    rounded_distance((p.0, p.1 + e), half, radius)
                        - rounded_distance((p.0, p.1 - e), half, radius)
                        + 1e-5,
                ));
                let facing = n.0 * sun.0 + n.1 * sun.1;

                let near = (facing.max(0.0) * 1.2).clamp(0.0, 1.0).powf(1.1);
                let far = ((-facing).max(0.0) * 1.2).clamp(0.0, 1.0).powf(1.1)
                    * mix(0.3, 0.4, 1.0 - dark);
                let lit = 0.12 + 0.88 * (near + far).clamp(0.0, 1.0);

                let geo = (1.2 - 0.185 * inside).max(0.0).powi(5).clamp(0.0, 1.0);
                let hairline = (1.0 - smoothstep(0.0, 1.0, inside)) * lit;
                let glare = geo * (near + far) * mix(0.70, 0.45, dark);
                let fresnel = geo * mix(0.08, 0.05, dark);

                let edge = 1.0 - (inside / band).clamp(0.0, 1.0);
                let concave = 1.0 - (1.0 - smoothstep(0.0, 1.0, edge).powi(3)).max(0.0).sqrt();
                let bevel = concave * facing * 0.10;

                let (u, v) = (x / body.width, y / body.height);
                let top_sheen = 1.0 - smoothstep(0.03, 0.42, v);
                let slant = u + (1.0 - v) * 0.65;
                let diagonal =
                    smoothstep(0.18, 0.64, slant) * (1.0 - smoothstep(0.64, 1.16, slant));
                let sheen = top_sheen * mix(0.07, 0.03, dark) + diagonal * 0.025;

                let white =
                    ((glare + fresnel + bevel.max(0.0) + sheen + hairline * mix(0.45, 0.20, dark))
                        * highlight
                        * small)
                        .clamp(0.0, 1.0);
                let black = ((-bevel).max(0.0) * 0.4 * highlight * small
                    + (1.0 - dark) * (1.0 - smoothstep(0.0, 0.5, inside)) * 0.08)
                    .clamp(0.0, 1.0);

                let alpha = (white + black).clamp(0.0, 1.0) * coverage;
                if alpha <= 0.0 {
                    continue;
                }
                let shade = if white + black > 1e-4 {
                    white / (white + black)
                } else {
                    0.0
                };

                // over, premultiplied
                let at = row as usize * stride + column as usize * 4;
                for channel in 0..3 {
                    let under = f32::from(data[at + channel]);
                    data[at + channel] =
                        (shade * 255.0 * alpha + under * (1.0 - alpha)).round() as u8;
                }
                let under = f32::from(data[at + 3]);
                data[at + 3] = (255.0 * alpha + under * (1.0 - alpha)).round() as u8;
            }
        }
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

// signed distance to a rounded rectangle centered on 0, negative inside, as the shader's
fn rounded_distance(p: (f32, f32), half: (f32, f32), radius: f32) -> f32 {
    let q = (p.0.abs() - half.0 + radius, p.1.abs() - half.1 + radius);
    let outside = (q.0.max(0.0).powi(2) + q.1.max(0.0).powi(2)).sqrt();

    outside + q.0.max(q.1).min(0.0) - radius
}

fn normalize((x, y): (f32, f32)) -> (f32, f32) {
    let length = (x * x + y * y).sqrt().max(1e-6);
    (x / length, y / length)
}

fn mix(from: f32, to: f32, by: f32) -> f32 {
    from + (to - from) * by
}

fn smoothstep(low: f32, high: f32, value: f32) -> f32 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
