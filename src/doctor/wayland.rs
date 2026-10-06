//! The one place Kanade speaks Wayland itself: `kanade doctor` lists the compositor's globals to name
//! the protocols Amane needs that are missing, before a shell start panics on the first one. It
//! opens no window and asks nothing past the registry's first roundtrip. Amane owns every other use
//! of Wayland, so `src/boundary.rs` keeps `wayland_client` inside `doctor/`.

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::{Connection, Dispatch, QueueHandle};

use super::Check;

// what Amane binds at start, or Kanade has no windows, input or monitors
const REQUIRED: &[&str] = &[
    "wl_compositor",
    "wl_shm",
    "wl_seat",
    "wl_output",
    "xdg_wm_base",
    "zwlr_layer_shell_v1",
];

// what Amane uses when offered, with what goes without it
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

    let missing: Vec<&str> = REQUIRED
        .iter()
        .copied()
        .filter(|name| !offered(name))
        .collect();

    let required = match missing.as_slice() {
        [] => Check::ok(format!("wayland: required {}", REQUIRED.join(", "))),
        missing => Check::fail(format!(
            "wayland: required {} missing, so the shell cannot start",
            missing.join(", ")
        )),
    };

    let optional = OPTIONAL.iter().map(|&(name, without)| match offered(name) {
        true => Check::ok(format!("wayland: optional {name} available")),
        false => Check::warn(format!("wayland: optional {name} missing: {without}")),
    });

    std::iter::once(required).chain(optional).collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::Verdict;

    fn globals(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| String::from(*name)).collect()
    }

    #[test]
    fn every_required_global_passes() {
        let all: Vec<&str> = REQUIRED
            .iter()
            .copied()
            .chain(OPTIONAL.iter().map(|&(name, _)| name))
            .collect();

        let checks = judge(&globals(&all));

        assert!(checks.iter().all(|check| check.verdict == Verdict::Ok));
        assert_eq!(
            checks[0].text,
            "wayland: required wl_compositor, wl_shm, wl_seat, wl_output, xdg_wm_base, zwlr_layer_shell_v1"
        );
    }

    #[test]
    fn a_missing_required_global_fails_and_an_optional_one_degrades() {
        let checks = judge(&globals(&[
            "wl_compositor",
            "wl_shm",
            "wl_seat",
            "wl_output",
            "xdg_wm_base",
            "wp_viewporter",
        ]));

        assert_eq!(
            checks,
            [
                Check::fail(String::from(
                    "wayland: required zwlr_layer_shell_v1 missing, so the shell cannot start"
                )),
                Check::warn(String::from(
                    "wayland: optional wp_fractional_scale_manager_v1 missing: windows draw at whole scales"
                )),
                Check::ok(String::from("wayland: optional wp_viewporter available")),
            ]
        );
    }
}
