//! `kanade ui` on the event loop's thread (ADR 0095): the windows' trees, and acts on their nodes
//! through the windows' own pointer and keyboard paths, as a real pointer and keyboard would.

use wayland_client::protocol::wl_surface::WlSurface;

use crate::input::{focus, released};
use crate::ui::{self, Act, Answer, Node, Request, Window};
use crate::{Button, Key, Modifiers, Scroll};

use super::WaylandState;

impl WaylandState {
    // answers what was asked since the loop last looked, from the windows as they were last drawn
    pub fn serve_ui(&mut self) {
        for (request, answer) in ui::take() {
            // the mode may have turned off since it was asked
            if !ui::enabled() {
                answer(Err(String::from("ui.describe is off")));

                continue;
            }

            answer(self.ui_answer(request));
        }
    }

    fn ui_answer(&mut self, request: Request) -> Result<Answer, String> {
        match request {
            Request::Describe { window } => self.describe(window.as_deref()).map(Answer::Windows),

            Request::Find { window, name } => Ok(Answer::Present(
                match self.locate(window.as_deref(), &name) {
                    Ok(_) => true,
                    Err((exists, _)) => exists,
                },
            )),

            Request::Act { window, name, act } => {
                let (at, node) = self
                    .locate(window.as_deref(), &name)
                    .map_err(|(_, why)| why)?;

                self.act(at, &node, act).map(Answer::Done)
            }
        }
    }

    // the names of the windows that are shown, a second of one name numbered: `kanade-dock#2`
    fn shown(&self) -> Vec<(usize, String)> {
        let mut taken: Vec<&str> = Vec::new();
        let mut shown = Vec::new();

        for (at, window) in self.windows.iter().enumerate() {
            if window.width == 0 || window.height == 0 || window.name.is_empty() {
                continue;
            }

            taken.push(window.name);

            let number = taken.iter().filter(|name| **name == window.name).count();

            shown.push((
                at,
                match number {
                    1 => String::from(window.name),
                    number => format!("{}#{number}", window.name),
                },
            ));
        }

        shown
    }

    // the windows a name asks for: `kanade-dock`, or `dock`
    fn shown_as(&self, window: Option<&str>) -> Result<Vec<(usize, String)>, String> {
        let shown = self.shown();

        let Some(asked) = window else {
            return Ok(shown);
        };

        let named: Vec<_> = shown
            .into_iter()
            .filter(|(_, name)| {
                name == asked
                    || name
                        .strip_prefix("kanade-")
                        .is_some_and(|rest| rest == asked)
            })
            .collect();

        if named.is_empty() {
            return Err(format!("no window named {asked} is shown"));
        }

        Ok(named)
    }

    fn describe(&self, window: Option<&str>) -> Result<Vec<Window>, String> {
        let windows = self.shown_as(window)?;

        Ok(windows
            .into_iter()
            .map(|(at, name)| {
                let window = &self.windows[at];

                Window {
                    name,
                    width: window.width as f32 / crate::ui_scale::ui_scale(),
                    height: window.height as f32 / crate::ui_scale::ui_scale(),
                    scale: window.scale,
                    nodes: nodes_of(window),
                }
            })
            .collect())
    }

    /*
     * the one node a name stands for, in the window it is in; the error says whether a node of
     * that name exists at all (it is only ambiguous), for `wait`
     */
    fn locate(&self, window: Option<&str>, name: &str) -> Result<(usize, Node), (bool, String)> {
        let windows = self.shown_as(window).map_err(|why| (false, why))?;

        let mut found = Vec::new();
        let mut ambiguous = None;

        for (at, window_name) in windows {
            match ui::resolve(&nodes_of(&self.windows[at]), name) {
                Ok(node) => found.push((at, window_name, node.clone())),
                Err(why) if why.starts_with("no node") => {}
                Err(why) => ambiguous = Some(format!("{window_name}: {why}")),
            }
        }

        match (found.len(), ambiguous) {
            (1, None) => {
                let (at, _, node) = found.remove(0);

                Ok((at, node))
            }
            (0, None) => Err((false, format!("no node named {name}"))),
            (0, Some(why)) => Err((true, why)),
            _ => {
                let names: Vec<_> = found.iter().map(|(_, window, _)| window.as_str()).collect();

                Err((
                    true,
                    format!(
                        "{name} is in several windows: {}; say which with --window",
                        names.join(", ")
                    ),
                ))
            }
        }
    }

    fn act(&mut self, at: usize, node: &Node, act: Act) -> Result<String, String> {
        if node.disabled {
            return Err(format!("{} is disabled", node.id));
        }

        let surface = self.windows[at].role.wl_surface().clone();

        let done = match act {
            Act::Press => self.press(at, node)?,

            Act::Focus => {
                if node.role != ui::Role::TextInput {
                    return Err(format!("{} is not a text input", node.id));
                }

                self.press(at, node)?
            }

            Act::Scroll(lines) => self.scroll(at, node, lines)?,

            Act::Type(text) => {
                let keys = ui::typed(&text)?;

                self.keys(
                    at,
                    node,
                    &surface,
                    keys.into_iter()
                        .map(|key| (key, Modifiers::default()))
                        .collect(),
                )?
            }

            Act::Key(words) => {
                let key = ui::parse_key(&words)?;

                self.keys(at, node, &surface, vec![key])?
            }
        };

        self.request_frame_on(&surface);

        Ok(format!("{done} {}", node.id))
    }

    // a press at the middle of the node, if a click or drag there reaches the node or what is in it
    fn press(&mut self, at: usize, node: &Node) -> Result<&'static str, String> {
        let (x, y) = middle(node)?;
        let pointer = &mut self.windows[at].pointer;

        let reached = pointer
            .pressed_at(x, y)
            .ok_or_else(|| format!("nothing at {} takes a click", node.id))?;

        let end = pointer.targets()[node.target].end.max(node.target + 1);

        if !(node.target..end).contains(&reached) {
            return Err(format!("{} is covered or does not take a click", node.id));
        }

        let (ox, oy, inside) = pointer.position();

        // the same calls the compositor's events make, from press to release
        pointer.move_to(x, y);
        pointer.press();
        pointer.start_drag();
        pointer.stop_drag(true);
        pointer.release(Button::Left);

        restore(&mut self.windows[at].pointer, (ox, oy, inside));

        Ok("pressed")
    }

    fn scroll(&mut self, at: usize, node: &Node, lines: i32) -> Result<&'static str, String> {
        let (x, y) = middle(node)?;
        let pointer = &mut self.windows[at].pointer;

        let (ox, oy, inside) = pointer.position();

        pointer.move_to(x, y);

        let scrolled = pointer.scroll(Scroll {
            x: 0.0,
            y: lines as f32,
        });

        restore(pointer, (ox, oy, inside));

        if !scrolled {
            return Err(format!("nothing at {} scrolls", node.id));
        }

        Ok("scrolled")
    }

    /*
     * keys for a node: a text input of the runtime gets the keyboard first, as a click would give it,
     * and its keys go to it; the window's own on_key takes them for any other node
     */
    fn keys(
        &mut self,
        at: usize,
        node: &Node,
        surface: &WlSurface,
        keys: Vec<(Key, Modifiers)>,
    ) -> Result<&'static str, String> {
        let input = self.windows[at].pointer.targets()[node.target]
            .semantics
            .as_ref()
            .and_then(|semantics| semantics.input);

        if let Some(id) = input
            && !focus::has(id)
        {
            self.press(at, node)?;
        }

        let to_input = input.is_some_and(focus::has);

        for (key, modifiers) in keys {
            self.deliver_to(surface, key, modifiers, to_input);

            // a sent key is a press and its release, so no press that counts only while the
            // key is held (a hold to confirm) can be kept by it
            released::send();
        }

        Ok("sent keys to")
    }
}

fn nodes_of(window: &super::surface::OpenWindow) -> Vec<Node> {
    let pointer = &window.pointer;

    ui::nodes(pointer.targets(), pointer.pressed_index(), ui::mode())
}

// where to click a node, which has to be on the screen
fn middle(node: &Node) -> Result<(f32, f32), String> {
    let [x, y, width, height] = node.bounds;

    if width <= 0.0 || height <= 0.0 {
        return Err(format!("{} is not on the screen", node.id));
    }

    Ok((x + width / 2.0, y + height / 2.0))
}

// the real pointer back where it was, so what hovers is as the user left it
fn restore(pointer: &mut crate::input::Pointer, (x, y, inside): (f32, f32, bool)) {
    if inside {
        pointer.move_to(x, y);
    } else {
        pointer.leave();
    }
}
