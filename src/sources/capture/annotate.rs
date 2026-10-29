//! The screenshot annotation editor (`capture.annotate`, ADR 0142): after `capture screenshot`
//! saves a shot, it opens over the focused output with that file, the pointer draws rectangles or
//! arrows, and Save writes `<stem>-annotated.png` beside it. Escape closes without writing and
//! Ctrl+Z takes back the last shape. Its preview is the runtime's image and its own vector shapes;
//! only Save rasterizes, on a worker, never on a frame. No text, highlighter, crop or layer.
//!
//! It opens only for a path a `capture screenshot` asked for and niri then announced, so it never
//! races the file and never opens on a screenshot niri's own bind took. It is its own Overlay
//! window: no global grab and no second Wayland connection.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;

use kanade_runtime::Shape as _;
use kanade_runtime::service::Service;
use kanade_runtime::{
    Canvas, Cap, Center, Color, Cursor, Image, Key, Keyboard, Layer, LayerWindow, Line,
    MOST_PIXELS, Monitor, MutexExt, Padding, Parent, Point, Rectangle, Row, Stack, Text, Widget,
    Zone, decode_image,
};

use super::free;
use crate::atomic::{self, Mode};
use crate::config;
use crate::sources::live;
use crate::theme::{self, radius};

mod paint;

// the toolbar's height, and the room between the image and the output's edges
const BAR: f32 = 64.0;
const MARGIN: f32 = 32.0;

// the largest screenshot file the editor opens, before it even decodes it
const MOST_FILE: u64 = 64 << 20;

// how many screenshot paths `capture screenshot` may ask for and not yet hear of
const WAITING: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Rectangle,
    Arrow,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Tool::Rectangle => "Rectangle",
            Tool::Arrow => "Arrow",
        }
    }
}

// one mark, in the screenshot's own pixels
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub tool: Tool,
    pub start: (f32, f32),
    pub end: (f32, f32),
}

impl Shape {
    fn min_x(&self) -> f32 {
        self.start.0.min(self.end.0)
    }

    fn max_x(&self) -> f32 {
        self.start.0.max(self.end.0)
    }

    fn min_y(&self) -> f32 {
        self.start.1.min(self.end.1)
    }

    fn max_y(&self) -> f32 {
        self.start.1.max(self.end.1)
    }

    // a click or a nudge too small to be a mark, which a release then drops
    fn tiny(&self) -> bool {
        match self.tool {
            Tool::Rectangle => {
                self.max_x() - self.min_x() < 3.0 && self.max_y() - self.min_y() < 3.0
            }
            Tool::Arrow => {
                let (dx, dy) = (self.end.0 - self.start.0, self.end.1 - self.start.1);

                dx * dx + dy * dy < 6.0 * 6.0
            }
        }
    }
}

// the shapes fixed so far and the one the pointer is still drawing
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drawing {
    pub shapes: Vec<Shape>,
    pub drag: Option<Shape>,
}

impl Drawing {
    fn begin(&mut self, tool: Tool, at: (f32, f32)) {
        self.drag = Some(Shape {
            tool,
            start: at,
            end: at,
        });
    }

    fn extend(&mut self, at: (f32, f32)) {
        if let Some(drag) = &mut self.drag {
            drag.end = at;
        }
    }

    fn finish(&mut self) {
        if let Some(shape) = self.drag.take().filter(|shape| !shape.tiny()) {
            self.shapes.push(shape);
        }
    }

    fn undo(&mut self) {
        if self.drag.take().is_none() {
            self.shapes.pop();
        }
    }
}

// where the image stands in the editor's window: scaled to fit, letterboxed in its own room
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub image: (f32, f32),
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
}

impl Frame {
    // the room the image may fill: the output, inset on every side and above the toolbar
    fn room(width: f32, height: f32) -> (f32, f32, f32, f32) {
        (
            MARGIN,
            MARGIN,
            (width - 2.0 * MARGIN).max(1.0),
            (height - BAR - 2.0 * MARGIN).max(1.0),
        )
    }

    fn fit(image: (u32, u32), width: f32, height: f32) -> Frame {
        let (left, top, room_width, room_height) = Self::room(width, height);
        let (image_width, image_height) = (image.0.max(1) as f32, image.1.max(1) as f32);
        let scale = (room_width / image_width).min(room_height / image_height);
        let (width, height) = (image_width * scale, image_height * scale);

        Frame {
            image: (image_width, image_height),
            x: left + (room_width - width) / 2.0,
            y: top + (room_height - height) / 2.0,
            width,
            height,
            scale,
        }
    }

    // the image pixel under a point of the window, held inside the image
    fn to_image(self, point: Point) -> (f32, f32) {
        (
            ((point.x - self.x) / self.scale).clamp(0.0, self.image.0),
            ((point.y - self.y) / self.scale).clamp(0.0, self.image.1),
        )
    }

    // where an image pixel is drawn in the window
    fn to_window(self, (x, y): (f32, f32)) -> (f32, f32) {
        (self.x + x * self.scale, self.y + y * self.scale)
    }
}

// a screenshot being annotated: its file, its pixels' size, and the output it opened on
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shot {
    path: PathBuf,
    width: u32,
    height: u32,
    monitor: String,
}

pub struct Editor {
    shot: Option<Shot>,
    tool: Tool,
    drawing: Drawing,
    saving: bool,

    // why the last save was refused, in the editor's own words; never a path
    why: Option<String>,

    // so a save that finishes after the editor closed, or another shot opened, changes nothing
    serial: u64,
}

impl Service for Editor {
    fn new() -> Self {
        Self {
            shot: None,
            tool: Tool::Rectangle,
            drawing: Drawing::default(),
            saving: false,
            why: None,
            serial: 0,
        }
    }

    fn listen() {}
}

impl Editor {
    fn close(&mut self) {
        self.shot = None;
        self.drawing = Drawing::default();
        self.saving = false;
        self.why = None;
        self.serial = self.serial.wrapping_add(1);
    }

    // the picture the editor shows, for the output it opened on
    fn showing_on(&self, monitor: &str) -> Option<&Shot> {
        self.shot.as_ref().filter(|shot| shot.monitor == monitor)
    }

    fn status(&self) -> String {
        match &self.why {
            Some(why) => format!("{} \u{b7} {why}", self.tool.name()),
            None if self.saving => format!("{} \u{b7} saving", self.tool.name()),
            None => format!("{} \u{b7} unsaved", self.tool.name()),
        }
    }
}

/*
 * the paths a `capture screenshot` asked niri for: niri saves after it answers, so the editor opens
 * on its own announcement, when the file is there. A stale one goes after `WAITING`, as a picker
 * the user left never announces
 */
static ASKED: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

// `capture screenshot` took this path, for the editor to open on niri's word of it
pub fn asked(path: &str) {
    if !config::get().capture_annotate {
        return;
    }

    let mut asked = ASKED.locked();

    if asked.len() == WAITING {
        asked.pop_front();
    }

    asked.push_back(path.to_owned());
}

/*
 * niri saved a screenshot, the path or none when it only copied, on the niri thread. One a
 * `capture screenshot` asked for opens in the editor and its saved notice waits until the editor
 * closes; any other is not the editor's, and is shown at once
 */
pub fn heard(path: Option<String>) {
    let Some(path) = path else {
        live::screenshot_captured(None);

        return;
    };

    let asked = {
        let mut asked = ASKED.locked();

        asked
            .iter()
            .position(|waiting| *waiting == path)
            .and_then(|place| asked.remove(place))
    };

    if asked.is_some() {
        open(path);
    } else {
        live::screenshot_captured(Some(path));
    }
}

/*
 * opens the editor on `path`, on a thread of its own: its pixels are read to learn its size, which
 * waits on the disk, and niri's thread must not
 */
fn open(path: String) {
    let spawned = thread::Builder::new()
        .name(String::from("annotate-open"))
        .spawn(move || {
            let size = match read(Path::new(&path)) {
                Ok(size) => size,
                Err(why) => {
                    live::annotation_failed(&why);
                    live::screenshot_captured(Some(path));

                    return;
                }
            };
            let Some(monitor) = focused_monitor() else {
                live::screenshot_captured(Some(path));

                return;
            };
            let mut editor = Editor::write();

            editor.serial = editor.serial.wrapping_add(1);
            editor.shot = Some(Shot {
                path: PathBuf::from(path),
                width: size.0,
                height: size.1,
                monitor,
            });
            editor.tool = Tool::Rectangle;
            editor.drawing = Drawing::default();
            editor.saving = false;
            editor.why = None;
        });

    if let Err(error) = spawned {
        eprintln!("capture: annotate: {error}");
    }
}

// the focused output's name, else the first monitor niri lists; none with no monitor to show on
fn focused_monitor() -> Option<String> {
    let monitors = kanade_runtime::Monitors::read();
    let focused = crate::sources::niri::Niri::read()
        .seen
        .focused_output
        .clone();

    focused
        .filter(|name| monitors.has(name))
        .or_else(|| monitors.all().first().map(|monitor| monitor.name.clone()))
}

// the screenshot's size in pixels, refusing a file too large to read or an image too large to draw
fn read(path: &Path) -> Result<(u32, u32), String> {
    let meta = std::fs::metadata(path)
        .map_err(|_| String::from("the screenshot cannot be read, so nothing opens"))?;

    if !meta.is_file() {
        return Err(String::from(
            "the screenshot is not a file Kanade can annotate",
        ));
    }

    if meta.len() > MOST_FILE {
        return Err(String::from(
            "the screenshot is larger than the 64 MiB the editor opens",
        ));
    }

    decode_image(path, 64.0)
        .map(|(width, height, _)| (width, height))
        .ok_or_else(|| {
            format!(
                "the screenshot is not an image the editor draws, or is over {MOST_PIXELS} pixels"
            )
        })
}

// `<stem>-annotated.png` beside the original, and never over a file already there
fn save_path(original: &Path, taken: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let dir = original.parent()?;
    let stem = original.file_stem()?.to_str()?;

    Some(free(dir, &format!("{stem}-annotated"), "png", taken))
}

// the accent every mark is drawn in, the same in the preview and in the saved copy
fn colour() -> Color {
    theme::island().primary
}

/*
 * `path` with `shapes` drawn on it, as a png, written beside it; the new file's path. Runs on the
 * save worker, never on a frame, and keeps no path or pixel in an error
 */
fn compose(path: &Path, shapes: &[Shape], colour: Color) -> Result<PathBuf, String> {
    let (width, height, mut pixels) = decode_image(path, 64.0)
        .ok_or_else(|| String::from("the screenshot could not be read again to save it"))?;

    paint::paint(&mut pixels, width, height, shapes, colour);

    let bytes = paint::encode(width, height, &pixels)?;
    let target = save_path(path, |taken| taken.exists())
        .ok_or_else(|| String::from("the screenshot's name is not one Kanade can write beside"))?;

    atomic::write(&target, &bytes, Mode::Shared)
        .map_err(|_| String::from("the annotated copy could not be written"))?;

    Ok(target)
}

fn saving() {
    let colour = colour();
    let (serial, shot, shapes) = {
        let mut editor = Editor::write();
        let Some(shot) = editor.shot.clone() else {
            editor.quiet();

            return;
        };

        if editor.saving {
            editor.quiet();

            return;
        }

        editor.saving = true;
        editor.why = None;

        (editor.serial, shot, editor.drawing.shapes.clone())
    };

    let spawned = thread::Builder::new()
        .name(String::from("annotate-save"))
        .spawn(move || match compose(&shot.path, &shapes, colour) {
            Ok(target) => {
                // the saved notice names the copy, as niri's own does for a screenshot
                if let Some(path) = target.to_str() {
                    live::screenshot_captured(Some(path.to_owned()));
                }

                let mut editor = Editor::write();

                if editor.serial == serial {
                    editor.close();
                } else {
                    editor.quiet();
                }
            }
            Err(why) => {
                let mut editor = Editor::write();

                if editor.serial == serial {
                    editor.saving = false;
                    editor.why = Some(why);
                } else {
                    editor.quiet();
                }
            }
        });

    if let Err(error) = spawned {
        eprintln!("capture: annotate: {error}");
    }
}

fn choose(tool: Tool) {
    let mut editor = Editor::write();

    if editor.shot.is_none() || editor.saving {
        editor.quiet();

        return;
    }

    editor.tool = tool;
}

fn undo() {
    let mut editor = Editor::write();

    if editor.shot.is_none() || editor.saving {
        editor.quiet();

        return;
    }

    editor.drawing.undo();
}

// Escape: closes without writing, and the screenshot's saved notice shows then, not before
fn dismiss() {
    let path = {
        let mut editor = Editor::write();

        let Some(path) = editor.shot.as_ref().map(|shot| shot.path.clone()) else {
            editor.quiet();

            return;
        };

        // a save already under way finishes and names its copy; Escape does not race it
        if editor.saving {
            editor.quiet();

            return;
        }

        editor.close();

        path
    };

    // the original is untouched and still on disk, so its notice is still true
    if let Some(path) = path.to_str() {
        live::screenshot_captured(Some(path.to_owned()));
    }
}

// a key while the editor shows: Escape closes without writing, Ctrl+Z takes the last shape back
fn press(key: Key) {
    match key {
        Key::Escape => dismiss(),
        Key::Shortcut('z') => undo(),
        _ => {}
    }
}

// the pointer pressed or moved while drawing
fn drag(monitor: &str, size: (f32, f32), point: Point) {
    let mut editor = Editor::write();

    let Some(frame) = editor
        .showing_on(monitor)
        .map(|shot| Frame::fit((shot.width, shot.height), size.0, size.1))
    else {
        editor.quiet();

        return;
    };

    if editor.saving {
        editor.quiet();

        return;
    }

    let at = frame.to_image(point);
    let tool = editor.tool;

    if editor.drawing.drag.is_some() {
        editor.drawing.extend(at);
    } else {
        editor.drawing.begin(tool, at);
    }
}

// the pointer released: the shape drawn is fixed, unless it is too small to be one
fn finished(monitor: &str) {
    let mut editor = Editor::write();

    if editor.showing_on(monitor).is_none() || editor.saving {
        editor.quiet();

        return;
    }

    editor.drawing.finish();
}

// the marks as vectors over the image, the fixed ones and the one still drawn
fn marks(editor: &Editor, frame: &Frame, colour: Color) -> Vec<Box<dyn kanade_runtime::Shape>> {
    let thickness =
        (paint::stroke(frame.image.0 as u32, frame.image.1 as u32) * frame.scale).max(1.0);

    let mut marks: Vec<Box<dyn kanade_runtime::Shape>> = Vec::new();

    for shape in editor
        .drawing
        .shapes
        .iter()
        .chain(editor.drawing.drag.as_ref())
    {
        let start = frame.to_window(shape.start);
        let end = frame.to_window(shape.end);

        match shape.tool {
            Tool::Rectangle => {
                let left = frame.x + shape.min_x() * frame.scale;
                let top = frame.y + shape.min_y() * frame.scale;
                let right = frame.x + shape.max_x() * frame.scale;
                let bottom = frame.y + shape.max_y() * frame.scale;

                marks.push(Box::new(
                    kanade_runtime::Path::new()
                        .move_to(left, top)
                        .line_to(right, top)
                        .line_to(right, bottom)
                        .line_to(left, bottom)
                        .close()
                        .stroke(thickness, colour),
                ));
            }
            Tool::Arrow => {
                marks.push(Box::new(
                    Line::new()
                        .from(start.0, start.1)
                        .to(end.0, end.1)
                        .stroke(thickness, colour)
                        .cap(Cap::Round),
                ));

                if let Some([tip, left, right]) =
                    paint::arrow_head(shape.start, shape.end, thickness / frame.scale)
                {
                    let tip = frame.to_window(tip);
                    let left = frame.to_window(left);
                    let right = frame.to_window(right);

                    marks.push(Box::new(
                        kanade_runtime::Path::new()
                            .move_to(tip.0, tip.1)
                            .line_to(left.0, left.1)
                            .line_to(right.0, right.1)
                            .close()
                            .fill(colour),
                    ));
                }
            }
        }
    }

    marks
}

fn button(label: &str, width: f32, active: bool, press: impl Fn() + 'static) -> Box<dyn Widget> {
    let roles = theme::island();

    let (fill, ink) = if active {
        (roles.primary, roles.on_primary)
    } else {
        (theme::REGION_BADGE_BG, theme::REGION_BADGE_TEXT)
    };

    Box::new(
        Rectangle::new()
            .width(width)
            .height(40.0)
            .radius(radius::ROW)
            .fill(fill)
            .border(1.0, theme::REGION_BADGE_BORDER)
            .cursor(Cursor::Pointer)
            .align_child(Center, Center)
            .on_click(move |_| press())
            .child(
                Text::new(label)
                    .size(theme::text::LABEL_SMALL)
                    .color(ink)
                    .weight(theme::text::MEDIUM),
            ),
    )
}

// the hidden window of an output the editor does not stand on, taking nothing
fn nothing() -> LayerWindow {
    LayerWindow::new()
        .width(kanade_runtime::Full)
        .height(kanade_runtime::Full)
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace("kanade-annotate")
        .visible(false)
        .click_through()
        .child(Rectangle::new().width(Parent).height(Parent))
}

/*
 * the editor on this output: the screenshot scaled to fit over a dim backdrop, the marks over it,
 * and a toolbar of the two tools, Undo and Save with a status line. Only the output it opened on
 * gets it, and only that one takes the keyboard
 */
pub fn window(monitor: &Monitor) -> LayerWindow {
    let editor = Editor::read();

    let Some(shot) = editor.showing_on(&monitor.name) else {
        return nothing();
    };

    let (width, height) = (monitor.width.max(1) as f32, monitor.height.max(1) as f32);
    let frame = Frame::fit((shot.width, shot.height), width, height);
    let colour = colour();
    let status = editor.status();
    let tool = editor.tool;

    let layers: Vec<Box<dyn Widget>> = vec![
        Box::new(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .fill(theme::REGION_SCRIM),
        ),
        Box::new(
            Rectangle::new()
                .width(frame.width)
                .height(frame.height)
                .translate(frame.x, frame.y)
                .fill(Image::contain(&shot.path)),
        ),
        Box::new(
            Canvas::new()
                .width(Parent)
                .height(Parent)
                .shapes(marks(&editor, &frame, colour)),
        ),
        Box::new(
            Rectangle::new()
                .width(Parent)
                .height((height - BAR).max(1.0))
                .cursor(Cursor::Crosshair)
                .on_drag({
                    let name = monitor.name.clone();
                    let size = (width, height);

                    move |point| drag(&name, size, point)
                })
                .on_drag_end({
                    let name = monitor.name.clone();

                    move || finished(&name)
                }),
        ),
        Box::new(
            Rectangle::new()
                .width(Parent)
                .height(BAR)
                .translate(0.0, (height - BAR).max(0.0))
                .fill(theme::REGION_BADGE_BG)
                .padding(Padding {
                    top: 12.0,
                    bottom: 12.0,
                    left: 24.0,
                    right: 24.0,
                })
                .child(
                    Row::new(vec![
                        button("Rectangle", 116.0, tool == Tool::Rectangle, || {
                            choose(Tool::Rectangle)
                        }),
                        button("Arrow", 92.0, tool == Tool::Arrow, || choose(Tool::Arrow)),
                        Box::new(
                            Text::new(status)
                                .size(theme::text::LABEL_SMALL)
                                .color(theme::REGION_BADGE_TEXT),
                        ),
                        button("Undo", 84.0, false, undo),
                        button("Save", 84.0, false, saving),
                    ])
                    .width(Parent)
                    .height(40.0)
                    .justify(kanade_runtime::SpaceBetween)
                    .align(Center),
                ),
        ),
    ];

    drop(editor);

    LayerWindow::new()
        .width(kanade_runtime::Full)
        .height(kanade_runtime::Full)
        .layer(Layer::Overlay)
        .keyboard(Keyboard::Exclusive)
        .space(Zone::Ignore)
        .namespace("kanade-annotate")
        .on_key(press)
        .child(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .child(Stack::new(layers).width(Parent).height(Parent)),
        )
}

// the editor closes if the session locks, so its exclusive keyboard never stands over the lock
pub fn spawn() {
    kanade_runtime::service::watch::<crate::lock::Said>(|| {
        if crate::lock::holds() {
            let mut editor = Editor::write();

            if editor.shot.is_some() {
                editor.close();
            } else {
                editor.quiet();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shape_is_added_on_release_and_undone() {
        let mut drawing = Drawing::default();

        drawing.begin(Tool::Rectangle, (10.0, 10.0));
        drawing.extend((40.0, 30.0));
        drawing.finish();
        drawing.begin(Tool::Arrow, (5.0, 5.0));
        drawing.extend((60.0, 50.0));
        drawing.finish();

        assert_eq!(drawing.shapes.len(), 2);
        assert_eq!(drawing.shapes[0].tool, Tool::Rectangle);
        assert_eq!(drawing.shapes[1].tool, Tool::Arrow);

        drawing.undo();
        assert_eq!(drawing.shapes.len(), 1);
        drawing.undo();
        assert!(drawing.shapes.is_empty());
        drawing.undo();
        assert!(drawing.shapes.is_empty());
    }

    #[test]
    fn a_shape_too_small_to_see_is_dropped() {
        let mut drawing = Drawing::default();

        drawing.begin(Tool::Rectangle, (10.0, 10.0));
        drawing.extend((11.0, 11.0));
        drawing.finish();

        drawing.begin(Tool::Arrow, (10.0, 10.0));
        drawing.extend((12.0, 12.0));
        drawing.finish();

        assert!(drawing.shapes.is_empty());
    }

    #[test]
    fn undo_takes_back_the_shape_still_being_drawn_before_a_fixed_one() {
        let mut drawing = Drawing::default();

        drawing.begin(Tool::Rectangle, (0.0, 0.0));
        drawing.extend((30.0, 30.0));
        drawing.finish();
        drawing.begin(Tool::Arrow, (0.0, 0.0));
        drawing.extend((40.0, 40.0));

        drawing.undo();

        assert!(drawing.drag.is_none());
        assert_eq!(drawing.shapes.len(), 1);
    }

    #[test]
    fn the_save_path_is_beside_the_original_and_never_over_it() {
        let original =
            Path::new("/home/me/Pictures/Screenshots/Screenshot from 2026-02-01 10-00-00.png");

        assert_eq!(
            save_path(original, |_| false),
            Some(PathBuf::from(
                "/home/me/Pictures/Screenshots/Screenshot from 2026-02-01 10-00-00-annotated.png"
            ))
        );

        // a copy already there takes the next free name, never the original's
        let taken = PathBuf::from(
            "/home/me/Pictures/Screenshots/Screenshot from 2026-02-01 10-00-00-annotated.png",
        );
        let next = save_path(original, |path| path == taken).expect("a free name");

        assert_eq!(
            next,
            PathBuf::from(
                "/home/me/Pictures/Screenshots/Screenshot from 2026-02-01 10-00-00-annotated (2).png"
            )
        );
        assert_ne!(next, original);
    }

    #[test]
    fn the_frame_maps_a_pointer_to_its_image_pixel() {
        // a 100 x 50 image in a 200 x 200 room: 200 x 100, centred
        let frame = Frame::fit((100, 50), 200.0 + 2.0 * MARGIN, 200.0 + BAR + 2.0 * MARGIN);

        assert_eq!((frame.width, frame.height), (200.0, 100.0));
        assert_eq!(frame.scale, 2.0);
        assert_eq!(frame.x, MARGIN);
        assert_eq!(frame.y, MARGIN + 50.0);

        let middle = frame.to_image(Point {
            x: frame.x + 100.0,
            y: frame.y + 50.0,
        });
        assert_eq!(middle, (50.0, 25.0));

        // outside the image holds to its edge
        assert_eq!(
            frame.to_image(Point {
                x: frame.x - 20.0,
                y: frame.y - 20.0,
            }),
            (0.0, 0.0)
        );
        assert_eq!(
            frame.to_image(Point {
                x: frame.x + 500.0,
                y: frame.y + 500.0,
            }),
            (100.0, 50.0)
        );

        // and back
        assert_eq!(
            frame.to_window((50.0, 25.0)),
            (frame.x + 100.0, frame.y + 50.0)
        );
    }

    // a png claiming `width` by `height`, with no pixels: its header is what a decode refuses
    fn header_only_png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();

        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);

            drop(encoder.write_header().expect("a png header"));
        }

        bytes
    }

    #[test]
    fn an_image_past_what_the_runtime_draws_is_refused() {
        let dir = std::env::temp_dir().join(format!("kanade-annotate-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        let wide = dir.join("wide.png");
        std::fs::write(&wide, header_only_png(7681, 4320)).expect("a header");

        assert!(read(&wide).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
