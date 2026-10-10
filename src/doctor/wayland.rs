//! The one place Kanade speaks Wayland itself: `kanade doctor` lists the compositor's globals to name
//! the protocols the runtime needs that are missing, before a shell start panics on the first one. It
//! opens no window and asks nothing past the registry's first roundtrip. The runtime owns every other use
//! of Wayland, so `src/boundary.rs` keeps `wayland_client` inside `doctor/`.

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::{Connection, Dispatch, QueueHandle};

use super::Check;

const NO_START: &str = "the shell cannot start";

// what Kanade cannot work without: the runtime binds the ones that stop the start, the rest it lives without
const REQUIRED: &[(&str, &str)] = &[
    ("wl_compositor", NO_START),
    ("wl_shm", NO_START),
    ("wl_seat", "no pointer or keyboard reaches Kanade"),
    ("wl_output", "no monitor to draw on"),
    ("xdg_wm_base", NO_START),
    ("zwlr_layer_shell_v1", NO_START),
];

// what the runtime uses when offered, with what goes without it
const OPTIONAL: &[(&str, &str)] = &[
    (
        "wp_fractional_scale_manager_v1",
        "windows draw at whole scales",
    ),
    ("wp_viewporter", "windows draw at whole scales"),
];

pub fn check() -> Vec<Check> {
    match globals() {
        Ok(globals) => judge(&globals),
        Err(problem) => vec![Check::fail(format!("wayland: {problem}"))],
    }
}

// the interface of every global the compositor offers
fn globals() -> Result<Vec<String>, String> {
    let connection = Connection::connect_to_env().map_err(|error| error.to_string())?;
    let (globals, _) =
        registry_queue_init::<Registry>(&connection).map_err(|error| error.to_string())?;

    Ok(globals
        .contents()
        .clone_list()
        .into_iter()
        .map(|global| global.interface)
        .collect())
}

// required: present or failing; optional: present or degrading, each with what goes without it
fn judge(globals: &[String]) -> Vec<Check> {
    let offered = |name: &str| globals.iter().any(|global| global == name);

    let missing: Vec<Check> = REQUIRED
        .iter()
        .filter(|&&(name, _)| !offered(name))
        .map(|(name, without)| Check::fail(format!("wayland: required {name} missing: {without}")))
        .collect();

    let required = match missing.is_empty() {
        true => {
            let names: Vec<&str> = REQUIRED.iter().map(|&(name, _)| name).collect();
            vec![Check::ok(format!("wayland: required {}", names.join(", ")))]
        }
        false => missing,
    };

    let optional = OPTIONAL.iter().map(|&(name, without)| match offered(name) {
        true => Check::ok(format!("wayland: optional {name} available")),
        false => Check::warn(format!("wayland: optional {name} missing: {without}")),
    });

    required.into_iter().chain(optional).collect()
}

// the registry's events past the first roundtrip are never read
struct Registry;

impl Dispatch<WlRegistry, GlobalListContents> for Registry {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
