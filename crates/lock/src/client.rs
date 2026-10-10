//! The lock's own Wayland client, on its own thread: it holds `ext-session-lock`, makes a lock
//! surface for every output as soon as a lock is asked, and commits a frame to each the moment
//! niri sizes it, so niri never waits on one nor shows red. Backdrops and faces are worked out
//! by a worker as scenes arrive, unlocked too, so the first frame shows them.

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, FrameCallbackData};
use smithay_client_toolkit::output::{OutputHandler, OutputInfo, OutputState};
use smithay_client_toolkit::reexports::calloop::channel::{self, Channel, Event};
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::calloop::{EventLoop, LoopHandle, RegistrationToken};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
};
use smithay_client_toolkit::seat::pointer::cursor_shape::CursorShapeManager;
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::session_lock::{
    SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface,
    SessionLockSurfaceConfigure,
};
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_registry, registry_handlers};
use tiny_skia::Pixmap;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_keyboard::WlKeyboard;
use wayland_client::protocol::wl_output::{Transform, WlOutput};
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_shm::Format;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    Shape, WpCursorShapeDeviceV1,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::{self, WpViewport};
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use zeroize::Zeroize;

use crate::paint::{self, Faces, Field, Look};
use crate::picture::{self, Prepared};
use crate::stage::{Stage, Step};
use crate::text::Font;
use crate::{Ask, Backdrop, Decode, Fonts, Image, News, Report, Scene, Submit};

// how long the caret shows, then hides
const BLINK: Duration = Duration::from_millis(530);

// the most bytes a password typed holds, so its buffer never grows and leaves a copy behind
const LONGEST: usize = 1024;

// how large a face is kept, decoded, before it is cut round at each output's size
const FACE: u32 = 256;

// a lock surface, on one output
struct Screen {
    output: WlOutput,
    surface: SessionLockSurface,
    viewport: Option<WpViewport>,

    // its size, in logical pixels, once niri said it
    size: Option<(u32, u32)>,

    // a frame was committed that niri has not shown yet, and another is wanted after it
    shown: bool,
    owed: bool,
}

// what a backdrop is worked out for: the backdrop, at a size in physical pixels and a scale
#[derive(Debug, Clone, PartialEq)]
struct Key {
    backdrop: Backdrop,
    size: (u32, u32),
    scale: f32,
}

// the worker's tasks and what came of them
enum Job {
    Backdrop { output: String, key: Key },
    Face(Image),
}

enum Done {
    Backdrop {
        output: String,
        key: Key,
        prepared: Option<Prepared>,
    },
    Face(Image, Option<Pixmap>),
}

// how a frame's bytes are laid out in shared memory
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    // red, green, blue, alpha in memory, as tiny-skia draws
    Rgba,
    Bgra,
}

struct State {
    connection: Connection,
    queue: QueueHandle<State>,
    events: LoopHandle<'static, State>,

    registry: RegistryState,
    outputs: OutputState,
    seats: SeatState,
    compositor: CompositorState,
    shm: Shm,
    locks: SessionLockState,
    viewporter: Option<WpViewporter>,
    cursors: Option<CursorShapeManager>,
    pool: Option<SlotPool>,
    order: Order,

    // the lock asked for or held, there while `stage` is not unlocked
    stage: Stage,
    lock: Option<SessionLock>,
    screens: Vec<Screen>,

    keyboard: Option<WlKeyboard>,
    pointer: Option<(WlPointer, Option<WpCursorShapeDeviceV1>)>,
    control: bool,
    caps_lock: bool,

    // the password typed, as UTF-8, zeroed whenever it empties
    password: Vec<u8>,

    // the caret shows, and the timer blinking it while locked
    caret: bool,
    blink: Option<RegistrationToken>,

    // the last password refused shakes the field from then
    refused: Option<u64>,
    shake: Option<Instant>,

    scenes: HashMap<String, Scene>,
    // the fonts of the last scene, and their faces unless they did not read
    fonts: Option<(Fonts, Option<Faces>)>,
    font_files: HashMap<PathBuf, Arc<[u8]>>,

    // each output's backdrop worked out, and the one the worker was last asked for or failed on
    prepared: HashMap<String, (Key, Prepared)>,
    asked: HashMap<String, Key>,
    faces: HashMap<Image, Option<Pixmap>>,

    // each face cut round, at the size each output's scale draws it
    rounds: HashMap<(Image, u32), Option<Pixmap>>,

    work: mpsc::Sender<Job>,
    submit: Submit,
    report: Report,

    // the lock cannot go on, so its thread ends, saying it is gone
    dead: bool,
}

/*
 * the lock's thread: connects, says whether it could, then runs until the connection is gone. A
 * lock held then stays held by niri, and is asked again by the shell's next start
 */
pub(crate) fn run(
    asked: Channel<Ask>,
    decode: Decode,
    submit: Submit,
    report: Report,
    started: &mpsc::Sender<Result<(), String>>,
    up: &AtomicBool,
) {
    let (mut events, mut state) = match connect(asked, decode, submit, report) {
        Ok(connected) => connected,
        Err(error) => {
            let _ = started.send(Err(error));
            return;
        }
    };
    let _ = started.send(Ok(()));
    up.store(true, Ordering::Relaxed);

    while !state.dead && events.dispatch(None, &mut state).is_ok() {}

    eprintln!("kanade: the lock lost its connection to the compositor, or its worker");
    (state.report)(News::Gone);
}

fn connect(
    asked: Channel<Ask>,
    decode: Decode,
    submit: Submit,
    report: Report,
) -> Result<(EventLoop<'static, State>, State), String> {
    let connection = Connection::connect_to_env()
        .map_err(|error| format!("cannot connect to the compositor: {error}"))?;
    let (globals, queue) = registry_queue_init::<State>(&connection)
        .map_err(|error| format!("cannot list the compositor's globals: {error}"))?;
    let handle = queue.handle();

    let events: EventLoop<'static, State> = EventLoop::try_new()
        .map_err(|error| format!("cannot start the lock's event loop: {error}"))?;

    let has = |interface: &str| {
        globals
            .contents()
            .with_list(|list| list.iter().any(|global| global.interface == interface))
    };
    if !has("ext_session_lock_manager_v1") {
        return Err(String::from("the compositor has no ext-session-lock"));
    }

    let compositor = CompositorState::bind(&globals, &handle)
        .map_err(|error| format!("no wl_compositor: {error}"))?;
    let shm = Shm::bind(&globals, &handle).map_err(|error| format!("no wl_shm: {error}"))?;
    let viewporter = globals.bind::<WpViewporter, _, _>(&handle, 1..=1, ()).ok();
    let cursors = CursorShapeManager::bind(&globals, &handle).ok();

    WaylandSource::new(connection.clone(), queue)
        .insert(events.handle())
        .map_err(|error| format!("cannot listen to the compositor: {error}"))?;

    events
        .handle()
        .insert_source(asked, |event, _, state: &mut State| {
            if let Event::Msg(ask) = event {
                state.ask(ask);
            }
        })
        .map_err(|error| format!("cannot listen to the shell: {error}"))?;

    // the worker, which hands back what it worked out to this thread
    let (work, jobs) = mpsc::channel();
    let (done, finished) = channel::channel();
    thread::Builder::new()
        .name(String::from("kanade-lock-backdrops"))
        .spawn(move || prepare(&jobs, &done, decode))
        .map_err(|error| format!("cannot start the lock's worker: {error}"))?;
    events
        .handle()
        .insert_source(finished, |event, _, state: &mut State| match event {
            Event::Msg(done) => state.done(done),

            // the worker ended, as by a panic: no backdrop would be worked out again
            Event::Closed => state.dead = true,
        })
        .map_err(|error| format!("cannot listen to the lock's worker: {error}"))?;

    let state = State {
        connection,
        queue: handle.clone(),
        events: events.handle(),
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &handle),
        seats: SeatState::new(&globals, &handle),
        compositor,
        shm,
        locks: SessionLockState::new(&globals, &handle),
        viewporter,
        cursors,
        pool: None,
        order: Order::Bgra,
        stage: Stage::Unlocked,
        lock: None,
        screens: Vec::new(),
        keyboard: None,
        pointer: None,
        control: false,
        caps_lock: false,
        password: Vec::with_capacity(LONGEST),
        caret: true,
        blink: None,
        refused: None,
        shake: None,
        scenes: HashMap::new(),
        fonts: None,
        font_files: HashMap::new(),
        prepared: HashMap::new(),
        asked: HashMap::new(),
        faces: HashMap::new(),
        rounds: HashMap::new(),
        work,
        submit,
        report,
        dead: false,
    };

    Ok((events, state))
}

/*
 * the worker: decodes wallpapers and faces and works out each output's backdrop, only the newest
 * asked of each output when several wait. The last wallpaper stays decoded, for the next output
 * showing it
 */
fn prepare(jobs: &mpsc::Receiver<Job>, done: &channel::Sender<Done>, decode: Decode) {
    // the last wallpaper decoded, from the file as it was then written
    let mut decoded: Option<(Image, Option<Pixmap>)> = None;

    while let Ok(first) = jobs.recv() {
        let mut waiting = vec![first];
        waiting.extend(jobs.try_iter());

        // only the newest backdrop of each output
        let mut newest: HashMap<String, usize> = HashMap::new();
        for (index, job) in waiting.iter().enumerate() {
            if let Job::Backdrop { output, .. } = job {
                newest.insert(output.clone(), index);
            }
        }

        for (index, job) in waiting.into_iter().enumerate() {
            let finished = match job {
                Job::Backdrop { output, key } => {
                    if newest.get(&output) != Some(&index) {
                        continue;
                    }

                    let wallpaper = match &key.backdrop {
                        Backdrop::Wallpaper { image, .. } => {
                            if decoded.as_ref().is_none_or(|(kept, _)| kept != image) {
                                // the last one let go of first, so two are never held at once
                                drop(decoded.take());
                                let pixmap = guarded(|| {
                                    decode(&image.path).and_then(|picture| picture.pixmap())
                                });
                                decoded = Some((image.clone(), pixmap));
                            }
                            decoded.as_ref().and_then(|(_, pixmap)| pixmap.as_ref())
                        }
                        Backdrop::Solid(_) => None,
                    };

                    let prepared =
                        guarded(|| picture::prepare(&key.backdrop, wallpaper, key.size, key.scale));
                    Done::Backdrop {
                        output,
                        key,
                        prepared,
                    }
                }
                Job::Face(image) => {
                    let face = guarded(|| {
                        decode(&image.path)
                            .and_then(|picture| picture.pixmap())
                            .and_then(|pixmap| picture::cover(&pixmap, FACE, FACE))
                    });
                    Done::Face(image, face)
                }
            };

            if done.send(finished).is_err() {
                return;
            }
        }
    }
}

// a black frame `width` by `height`
fn black((width, height): (u32, u32)) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(width.max(1), height.max(1))?;
    pixmap.fill(tiny_skia::Color::BLACK);
    Some(pixmap)
}

/*
 * `work`, or none if it panicked: a picture that crashes its decoder or the drawing only goes
 * undrawn, never ends the lock, which would crash again on the same picture as the shell restarts
 */
fn guarded<T>(work: impl FnOnce() -> Option<T>) -> Option<T> {
    panic::catch_unwind(AssertUnwindSafe(work)).ok().flatten()
}

// whether an output turned a quarter, so its mode is the other way round from its logical size
fn turned(transform: Transform) -> bool {
    matches!(
        transform,
        Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270
    )
}

/*
 * physical pixels per logical one on an output: its mode over its logical size, which a
 * fractional scale needs; else its whole scale
 */
fn scale(info: &OutputInfo) -> f32 {
    let mode = info.modes.iter().find(|mode| mode.current);

    match (mode, info.logical_size) {
        (Some(mode), Some((width, _))) if width > 0 => {
            let across = if turned(info.transform) {
                mode.dimensions.1
            } else {
                mode.dimensions.0
            };
            across as f32 / width as f32
        }
        _ => info.scale_factor.max(1) as f32,
    }
}

impl State {
    fn ask(&mut self, ask: Ask) {
        match ask {
            Ask::Lock(request) => self.lock(request),
            Ask::Unlock => {
                let step = self.stage.unlock();
                self.step(step);
            }
            Ask::Scene(output, scene) => self.scene(output, *scene),
        }
    }

    fn lock(&mut self, request: u64) {
        let step = self.stage.lock(request);
        self.step(step);
    }

    // does what the stage said
    fn step(&mut self, step: Step) {
        match step {
            Step::Nothing => {}
            Step::Tell(news) => (self.report)(news),
            Step::Ask => self.ask_lock(),
            Step::End(news) => {
                // `unlock` ends a lock niri holds; `destroy` after `locked` is a protocol error
                if let Some(lock) = self.lock.take() {
                    lock.unlock();
                }
                self.forget();
                let _ = self.connection.flush();
                (self.report)(news);
            }
        }
    }

    // asks niri for a lock, with a surface on every output
    fn ask_lock(&mut self) {
        let lock = match self.locks.lock(&self.queue) {
            Ok(lock) => lock,
            Err(error) => {
                eprintln!("kanade: niri offers no session lock, so the lock is refused: {error}");
                let step = self.stage.refused();
                self.step(step);
                return;
            }
        };

        let outputs: Vec<WlOutput> = self.outputs.outputs().collect();
        for output in outputs {
            self.cover(&lock, output);
        }

        self.lock = Some(lock);
        self.clear();
        self.caret = true;
        self.shake = None;
        self.start_blinking();
        let _ = self.connection.flush();
    }

    // a lock surface on `output`, unless it has one: a second is a protocol error
    fn cover(&mut self, lock: &SessionLock, output: WlOutput) {
        if self.screens.iter().any(|screen| screen.output == output) {
            return;
        }

        let surface = self.compositor.create_surface(&self.queue);
        let viewport = self
            .viewporter
            .as_ref()
            .map(|viewporter| viewporter.get_viewport(&surface, &self.queue, ()));
        let surface = lock.create_lock_surface(surface, &output, &self.queue);

        self.screens.push(Screen {
            output,
            surface,
            viewport,
            size: None,
            shown: false,
            owed: false,
        });
    }

    // drops the lock surfaces and what was typed in them
    fn forget(&mut self) {
        for screen in self.screens.drain(..) {
            if let Some(viewport) = screen.viewport {
                viewport.destroy();
            }
        }

        // the frames' memory too, until the next lock
        self.pool = None;

        self.clear();
        if let Some(blink) = self.blink.take() {
            self.events.remove(blink);
        }
    }

    fn start_blinking(&mut self) {
        if self.blink.is_some() {
            return;
        }

        let blinking =
            self.events
                .insert_source(Timer::from_duration(BLINK), |_, _, state: &mut State| {
                    state.caret = !state.caret;
                    state.redraw();
                    TimeoutAction::ToDuration(BLINK)
                });
        self.blink = blinking.ok();
    }

    fn scene(&mut self, output: String, scene: Scene) {
        if self.scenes.get(&output) == Some(&scene) {
            return;
        }

        // a password refused since the last scene shakes the field
        if self.refused.is_some_and(|refused| scene.refused > refused) {
            self.shake = Some(Instant::now());
        }
        self.refused = Some(scene.refused);

        if self
            .fonts
            .as_ref()
            .is_none_or(|(fonts, _)| *fonts != scene.fonts)
        {
            self.read_fonts(&scene.fonts);
        }

        if let Some(face) = &scene.face
            && !self.faces.contains_key(face)
        {
            // none until it decodes, so it is asked once
            self.faces.insert(face.clone(), None);
            let _ = self.work.send(Job::Face(face.clone()));
        }

        self.scenes.insert(output.clone(), scene);

        // only the faces a scene shows stay decoded
        let shown: Vec<&Image> = self
            .scenes
            .values()
            .filter_map(|scene| scene.face.as_ref())
            .collect();
        self.faces.retain(|image, _| shown.contains(&image));
        self.rounds.retain(|(image, _), _| shown.contains(&image));

        self.ahead(&output);
        self.redraw();
    }

    fn read_fonts(&mut self, fonts: &Fonts) {
        let mut read = |file: &crate::FontFile| {
            let data = match self.font_files.get(&file.path) {
                Some(data) => data.clone(),
                None => {
                    let data: Arc<[u8]> = std::fs::read(&file.path).ok()?.into();
                    self.font_files.insert(file.path.clone(), data.clone());
                    data
                }
            };
            Font::new(file, data)
        };

        let faces = (|| {
            Some(Faces {
                regular: read(&fonts.regular)?,
                medium: read(&fonts.medium)?,
                semibold: read(&fonts.semibold)?,
                display: read(&fonts.display)?,
                fallback: fonts.fallback.as_ref().and_then(&mut read),
            })
        })();

        if faces.is_none() {
            eprintln!("kanade: the lock screen's fonts do not read, so it shows no text");
        }
        self.fonts = Some((fonts.clone(), faces));

        // only the files the faces use stay read
        let used: Vec<&Path> = [
            &fonts.regular,
            &fonts.medium,
            &fonts.semibold,
            &fonts.display,
        ]
        .into_iter()
        .chain(&fonts.fallback)
        .map(|file| file.path.as_path())
        .collect();
        self.font_files
            .retain(|path, _| used.contains(&path.as_path()));
    }

    // the output named `output` and how its lock surface is drawn: physical size and scale
    fn measure(&self, output: &str) -> Option<((u32, u32), f32)> {
        self.outputs.outputs().find_map(|wl_output| {
            let info = self.outputs.info(&wl_output)?;
            if info.name.as_deref() != Some(output) {
                return None;
            }

            // the size niri gave its lock surface, which it draws at, else the output's
            let configured = self
                .screens
                .iter()
                .find(|screen| screen.output == wl_output)
                .and_then(|screen| screen.size);
            let (width, height) = match configured {
                Some(size) => size,
                None => {
                    let (width, height) = info.logical_size?;
                    (u32::try_from(width).ok()?, u32::try_from(height).ok()?)
                }
            };
            let scale = self.drawn_scale(&info);

            Some((physical((width, height), scale), scale))
        })
    }

    // the scale a lock surface is drawn at: any with the viewporter, else a whole one
    fn drawn_scale(&self, info: &OutputInfo) -> f32 {
        if self.viewporter.is_some() {
            scale(info)
        } else {
            info.scale_factor.max(1) as f32
        }
    }

    // asks the worker for `output`'s backdrop at its size now, unless it has it or was asked
    fn ahead(&mut self, output: &str) {
        if let Some((size, scale)) = self.measure(output) {
            self.ensure(output, size, scale);
        }
    }

    fn ensure(&mut self, output: &str, size: (u32, u32), scale: f32) {
        let Some(scene) = self.scenes.get(output) else {
            return;
        };

        let key = Key {
            backdrop: scene.backdrop.clone(),
            size,
            scale,
        };
        let have = self
            .prepared
            .get(output)
            .is_some_and(|(have, _)| *have == key);
        let asked = self.asked.get(output) == Some(&key);

        if !have && !asked {
            self.asked.insert(output.to_owned(), key.clone());
            let _ = self.work.send(Job::Backdrop {
                output: output.to_owned(),
                key,
            });
        }
    }

    fn done(&mut self, done: Done) {
        match done {
            Done::Backdrop {
                output,
                key,
                prepared,
            } => {
                match prepared {
                    Some(prepared) => {
                        if self.asked.get(&output) == Some(&key) {
                            self.asked.remove(&output);
                        }
                        self.prepared.insert(output, (key, prepared));
                    }
                    // stays asked, so it is not asked again until the backdrop or size changes
                    None => {
                        eprintln!(
                            "kanade: the lock screen's backdrop for {output} cannot be worked \
                             out, too large or failed, so it shows its color"
                        )
                    }
                }
            }
            Done::Face(image, face) => {
                if face.is_none() {
                    eprintln!(
                        "kanade: the lock screen cannot draw the face {}",
                        image.path.display()
                    );
                }
                self.rounds.retain(|(round, _), _| *round != image);
                self.faces.insert(image, face);
            }
        }

        self.redraw();
    }

    // every lock surface drawn again, each as soon as niri showed its last frame
    fn redraw(&mut self) {
        for index in 0..self.screens.len() {
            if self.screens[index].shown {
                self.screens[index].owed = true;
            } else {
                self.draw(index);
            }
        }
    }

    fn draw(&mut self, index: usize) {
        let screen = &self.screens[index];
        let Some((width, height)) = screen.size else {
            return;
        };
        let Some(info) = self.outputs.info(&screen.output) else {
            return;
        };

        let scale = self.drawn_scale(&info);
        let size = physical((width, height), scale);
        let name = info.name.clone().unwrap_or_default();
        self.ensure(&name, size, scale);

        let now = Instant::now();
        let shaken = self
            .shake
            .and_then(|since| paint::shaken(now.duration_since(since).as_secs_f32()));
        if shaken.is_none() {
            self.shake = None;
        }

        // the face cut round once for this size
        let face = self.scenes.get(&name).and_then(|scene| scene.face.clone());
        let side = paint::avatar(scale);
        if let Some(face) = face.filter(|face| !self.rounds.contains_key(&(face.clone(), side)))
            && let Some(decoded) = self.faces.get(&face)
            && let Some(decoded) = decoded
        {
            let round = picture::round(decoded, side);
            self.rounds.insert((face, side), round);
        }

        let pixmap = match self.scenes.get(&name) {
            Some(scene) => {
                let look = Look {
                    scene,
                    faces: self.fonts.as_ref().and_then(|(_, faces)| faces.as_ref()),
                    prepared: self.prepared.get(&name).map(|(_, prepared)| prepared),
                    face: scene
                        .face
                        .clone()
                        .and_then(|face| self.rounds.get(&(face, side))?.as_ref()),
                    field: Field {
                        typed: std::str::from_utf8(&self.password)
                            .map_or(0, |typed| typed.chars().count()),
                        caps_lock: self.caps_lock,
                        caret: self.caret,
                    },
                    shaken: shaken.unwrap_or(0.0),
                    size: (width as f32, height as f32),
                    scale,
                };
                guarded(|| paint::paint(&look, size)).or_else(|| black(size))
            }

            // no scene yet: black, as niri's own lock screen
            None => black(size),
        };
        let Some(pixmap) = pixmap else {
            eprintln!("kanade: the lock screen of {name} is too large to draw");
            return;
        };

        self.commit(index, &pixmap, (width, height), scale);
    }

    // `pixmap` shown on the lock surface `index`, `size` logical pixels big
    fn commit(&mut self, index: usize, pixmap: &Pixmap, size: (u32, u32), scale: f32) {
        let (width, height) = (pixmap.width(), pixmap.height());
        let stride = width * 4;

        if self.pool.is_none() {
            self.order = if self.shm.formats().contains(&Format::Abgr8888) {
                Order::Rgba
            } else {
                Order::Bgra
            };
            self.pool = SlotPool::new((stride * height) as usize, &self.shm).ok();
        }
        let Some(pool) = &mut self.pool else {
            eprintln!("kanade: the lock screen has no shared memory to draw in");
            return;
        };

        let format = match self.order {
            Order::Rgba => Format::Abgr8888,
            Order::Bgra => Format::Argb8888,
        };
        let (buffer, canvas) =
            match pool.create_buffer(width as i32, height as i32, stride as i32, format) {
                Ok(created) => created,
                Err(error) => {
                    eprintln!("kanade: the lock screen cannot draw a frame: {error}");
                    return;
                }
            };

        match self.order {
            Order::Rgba => canvas.copy_from_slice(pixmap.data()),
            Order::Bgra => {
                let (to, _) = canvas.as_chunks_mut::<4>();
                let (from, _) = pixmap.data().as_chunks::<4>();
                for (to, from) in to.iter_mut().zip(from) {
                    *to = [from[2], from[1], from[0], from[3]];
                }
            }
        }

        let screen = &mut self.screens[index];
        let surface = screen.surface.wl_surface();

        match &screen.viewport {
            Some(viewport) => {
                surface.set_buffer_scale(1);
                viewport.set_destination(size.0 as i32, size.1 as i32);
            }
            None => surface.set_buffer_scale(scale as i32),
        }

        if buffer.attach_to(surface).is_err() {
            return;
        }
        // `damage_buffer` came in version 4
        if surface.version() >= 4 {
            surface.damage_buffer(0, 0, width as i32, height as i32);
        } else {
            surface.damage(0, 0, i32::MAX, i32::MAX);
        }
        surface.frame(&self.queue, FrameCallbackData(surface.clone()));
        surface.commit();

        screen.shown = true;
        screen.owed = false;
        let _ = self.connection.flush();
    }

    // the password emptied, every byte it held zeroed first
    fn clear(&mut self) {
        self.password.zeroize();
    }

    fn key(&mut self, event: &KeyEvent) {
        if self.lock.is_none() {
            return;
        }

        match event.keysym {
            Keysym::Return | Keysym::KP_Enter => self.enter(),
            Keysym::Escape => self.clear(),
            Keysym::BackSpace if self.control => self.clear(),
            Keysym::BackSpace => {
                let end = self.password.len();
                let start = (0..end)
                    .rev()
                    .find(|&at| self.password[at] & 0xC0 != 0x80)
                    .unwrap_or(0);
                self.password[start..].zeroize();
                self.password.truncate(start);
            }
            Keysym::u if self.control => self.clear(),
            // sctk's key event keeps its own copy of the text, freed unzeroed; nothing here can
            // reach it
            _ => {
                let Some(text) = &event.utf8 else {
                    return;
                };
                if self.control || text.chars().any(char::is_control) {
                    return;
                }
                if self.password.len() + text.len() <= LONGEST {
                    self.password.extend_from_slice(text.as_bytes());
                }
            }
        }

        // the caret shows as a key is typed, and blinks again from then
        self.caret = true;
        if let Some(blink) = self.blink.take() {
            self.events.remove(blink);
        }
        self.start_blinking();
        self.redraw();
    }

    // the password goes to the shell, which empties the field if it took it
    fn enter(&mut self) {
        if self.password.is_empty() {
            return;
        }

        // typed from keysyms, so it is UTF-8; otherwise its copy is zeroed and nothing sent
        let password = match String::from_utf8(self.password.clone()) {
            Ok(password) => password,
            Err(error) => {
                error.into_bytes().zeroize();
                return;
            }
        };
        if (self.submit)(password) {
            self.clear();
        }
    }
}

// a logical size in physical pixels
fn physical((width, height): (u32, u32), scale: f32) -> (u32, u32) {
    (
        (width as f32 * scale).round().max(1.0) as u32,
        (height as f32 * scale).round().max(1.0) as u32,
    )
}

impl SessionLockHandler for State {
    fn locked(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        let step = self.stage.locked();
        self.step(step);
    }

    fn finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        let step = self.stage.finished();
        self.step(step);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _: u32,
    ) {
        let Some(index) = self
            .screens
            .iter()
            .position(|screen| screen.surface.wl_surface() == surface.wl_surface())
        else {
            return;
        };

        self.screens[index].size = Some(configure.new_size);

        // drawn at once, whatever frame is pending: niri waits on it
        self.draw(index);
    }
}

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, _: u32) {
        let Some(index) = self
            .screens
            .iter()
            .position(|screen| screen.surface.wl_surface() == surface)
        else {
            return;
        };

        let screen = &mut self.screens[index];
        screen.shown = false;

        if screen.owed || self.shake.is_some() {
            self.draw(index);
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    // an output plugged in while locked gets a lock surface too; any gets its backdrop ahead
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        if let Some(lock) = self.lock.clone() {
            self.cover(&lock, output.clone());
            let _ = self.connection.flush();
        }

        if let Some(name) = self.outputs.info(&output).and_then(|info| info.name) {
            self.ahead(&name);
        }

        // a lock surface sized before its output's info came draws now, not at the next blink
        self.redraw();
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        if let Some(name) = self.outputs.info(&output).and_then(|info| info.name) {
            self.ahead(&name);
        }

        // a lock surface sized before its output's info came draws now, not at the next blink
        self.redraw();
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        self.screens.retain(|screen| {
            let kept = screen.output != output;
            if !kept && let Some(viewport) = &screen.viewport {
                viewport.destroy();
            }
            kept
        });

        if let Some(name) = self.outputs.info(&output).and_then(|info| info.name) {
            // its scene stays, as one for a monitor plugged in again may come first
            self.prepared.remove(&name);
            self.asked.remove(&name);
        }
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        queue: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            // held keys repeat at the compositor's rate, as Backspace held empties the field
            let keyboard = self.seats.get_keyboard_with_repeat(
                queue,
                &seat,
                None,
                self.events.clone(),
                Box::new(|state, _, event| state.key(&event)),
            );
            match keyboard {
                Ok(keyboard) => self.keyboard = Some(keyboard),
                Err(error) => eprintln!("kanade: the lock screen has no keyboard: {error}"),
            }
        }

        if capability == Capability::Pointer
            && self.pointer.is_none()
            && let Ok(pointer) = self.seats.get_pointer(queue, &seat)
        {
            let shape = self
                .cursors
                .as_ref()
                .map(|cursors| cursors.get_shape_device(&pointer, queue));
            self.pointer = Some((pointer, shape));
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(keyboard) = self.keyboard.take()
        {
            // `release` came in version 3; before it, the keyboard stays until the connection ends
            if keyboard.version() >= 3 {
                keyboard.release();
            }
        }

        if capability == Capability::Pointer
            && let Some((pointer, shape)) = self.pointer.take()
        {
            if let Some(shape) = shape {
                shape.destroy();
            }
            if pointer.version() >= 3 {
                pointer.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}

impl KeyboardHandler for State {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: &WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: &WlSurface,
        _: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(&event);
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(&event);
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.control = modifiers.ctrl;

        if self.caps_lock != modifiers.caps_lock {
            self.caps_lock = modifiers.caps_lock;
            self.redraw();
        }
    }
}

impl PointerHandler for State {
    // the arrow over a lock surface, as niri draws none of its own there
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlPointer,
        events: &[PointerEvent],
    ) {
        let Some((_, Some(shape))) = &self.pointer else {
            return;
        };

        for event in events {
            if let PointerEventKind::Enter { serial } = event.kind {
                shape.set_shape(serial, Shape::Default);
            }
        }
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState, SeatState];
}

delegate_noop!(State: WpViewporter);

impl Dispatch<WpViewport, ()> for State {
    fn event(
        _: &mut Self,
        _: &WpViewport,
        _: wp_viewport::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_registry!(State);
smithay_client_toolkit::delegate_dispatch2!(State);
