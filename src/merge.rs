//! The Island and the Dock where they share an edge and a side (ADR 0031, `dock.merge`): the
//! Island's window then draws both, as one shell with one pane. This lays the shell out;
//! `view::island` draws it and `dock::window` steps out of the way.

use kanade_runtime::Monitor;

use crate::config;
use crate::dock;
use crate::island::geometry::{self, Canvas, ROOM_WIDTH, Rect, TOP};
use crate::look::{Along, Anchor, Edge, Merge, Placement, ShellMode};
use crate::modules;
use crate::theme::radius;
use crate::view;

// how far the Island's body reaches into the Dock's plate under a Crown, where their panes flow together
pub const OVERLAP: f32 = 6.0;

// how far the two edges pull toward each other where they meet, in logical pixels
pub const BLEND: f32 = 14.0;

/*
 * how the Island and the Dock merge now, if they do: `dock.merge` is on, and both stand on the same
 * top or bottom edge and side, with the Dock showing an app and the Island not hiding. A Dock that
 * autohides merges as a Fold. Across a side edge they stay apart, as the Dock's row would stand in
 * the Island's way. Reading subscribes to the apps
 */
pub fn form() -> Option<Merge> {
    joined().map(|(form, _)| form)
}

// whether the Dock folds into the Island
pub fn folds() -> bool {
    form() == Some(Merge::Fold)
}

/*
 * how much further from its edge what hangs from the Island, as the Banners, and what is reserved
 * for it, stand than alone; zero unless merged. The Island's body stands further out, past the
 * Dock's plate, by the offset; and the Dock's strip reaches further than the body at rest, which
 * what hangs from the Island clears
 */
pub fn apart() -> f32 {
    joined().map_or(0.0, |(form, extent)| {
        offset_of(form, &extent) + clearance_of(form, &extent)
    })
}

fn chosen(config: &config::Config, dock: Placement) -> Option<Merge> {
    if config.shell.mode == ShellMode::Combined {
        Some(if dock.autohide {
            Merge::Fold
        } else {
            Merge::Keystone
        })
    } else {
        match config.merge {
            Merge::Off => None,
            _ if dock.autohide => Some(Merge::Fold),
            form => Some(form),
        }
    }
}

fn joined_visible(mode: ShellMode, island_hides: bool) -> bool {
    mode == ShellMode::Combined || !island_hides
}

fn joined() -> Option<(Merge, dock::Extent)> {
    let config = config::get();
    let (island, dock) = (config.island_placement(), config.dock_placement());

    let form = chosen(&config, dock)?;

    let merged = modules::on("dock")
        && island.anchor == dock.anchor
        && !island.anchor.sideways()
        && joined_visible(config.shell.mode, view::hides());

    merged
        .then(dock::extent)
        .flatten()
        .map(|extent| (form, extent))
}

fn offset_of(form: Merge, extent: &dock::Extent) -> f32 {
    match form {
        Merge::Crown => (extent.depth - OVERLAP - TOP).max(0.0),
        Merge::Keystone | Merge::Fold => depths(extent).0,
        Merge::Off => 0.0,
    }
}

fn clearance_of(form: Merge, extent: &dock::Extent) -> f32 {
    match form {
        Merge::Keystone | Merge::Fold => {
            let (island, strip) = depths(extent);

            (strip + extent.strip - (island + TOP + geometry::REST.height)).max(0.0)
        }
        Merge::Crown | Merge::Off => 0.0,
    }
}

/*
 * among the icons, the Island's body at rest stands in the middle of the strip's thickness. How far
 * its canvas then stands off the edge, and the strip; neither is nearer the edge than it would be
 * alone
 */
fn depths(extent: &dock::Extent) -> (f32, f32) {
    let edge = extent.depth - extent.strip;
    let island = edge + (extent.strip - geometry::REST.height) / 2.0 - TOP;

    if island >= 0.0 {
        (island, edge)
    } else {
        (0.0, edge - island)
    }
}

// where the Island's canvas and the Dock's lie in the Island's window, in whole pixels
#[derive(Clone)]
pub struct Shell {
    pub form: Merge,
    pub extent: dock::Extent,
    pub size: (f32, f32),
    pub island: (f32, f32),

    // the Dock's window, which lies at the origin of the shell when it is among the icons
    pub dock: (f32, f32),

    // among the icons, where the strip starts down the window
    pub strip: f32,
}

// the Dock's plate under the Island's body, and its corners' radius
#[derive(Clone, Copy)]
pub struct Plate {
    pub rect: Rect,
    pub radius: f32,
}

impl Shell {
    // how far the Island canvas stands from the shell window's edge, along the edge it hangs from
    pub fn edge_offset(&self, canvas: Canvas, edge: geometry::Edge) -> f32 {
        match edge {
            geometry::Edge::Top => self.island.1,
            geometry::Edge::Bottom => self.size.1 - self.island.1 - canvas.height,
            geometry::Edge::Left => self.island.0,
            geometry::Edge::Right => self.size.0 - self.island.0 - canvas.width,
        }
        .max(0.0)
    }

    // whether the Island is among the Dock's icons rather than above its plate
    pub fn among(&self) -> bool {
        matches!(self.form, Merge::Keystone | Merge::Fold)
    }

    /*
     * how far out the icons are, 0 to 1: always for a Keystone; folded, as far as the pointer holds
     * them, and not while the Island is larger than a small form, which folds them away as it grows
     */
    pub fn unfold(&self, held: f32, height: f32) -> f32 {
        if self.form != Merge::Fold {
            return 1.0;
        }

        let calm = geometry::COMPACT.height.max(geometry::SPLIT.height);
        let small = 1.0 - ((height - calm) / (geometry::PEEK.height - calm)).clamp(0.0, 1.0);

        (held * small).clamp(0.0, 1.1)
    }

    /*
     * the plate around the Island's `body` (in the window, with its corner `radius`) among its icons,
     * `unfold` of the way out of it: the whole strip when they are out, the body alone when they
     * are in. Icons grown under the pointer have pushed each side out by `swelled`
     */
    pub fn plate(&self, body: Rect, body_radius: f32, unfold: f32, swelled: (f32, f32)) -> Plate {
        let extent = &self.extent;
        let full = Rect {
            x: body.x - extent.before - swelled.0 - extent.inset,
            y: self.strip,
            width: extent.before
                + swelled.0
                + body.width
                + extent.after
                + swelled.1
                + 2.0 * extent.inset,
            height: extent.strip,
        };
        let toward = |from: f32, to: f32| from + unfold * (to - from);
        let (left, top) = (toward(body.x, full.x), toward(body.y, full.y));
        let (right, bottom) = (
            toward(body.right(), full.right()),
            toward(body.bottom(), full.bottom()),
        );

        Plate {
            rect: Rect {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            },
            radius: toward(body_radius, radius::CARD),
        }
    }
}

pub fn shell(monitor: &Monitor) -> Option<Shell> {
    let (form, extent) = joined()?;
    let anchor = config::get().island_placement().anchor;

    Some(arrange(form, anchor, view::canvas(monitor), extent))
}

fn arrange(form: Merge, anchor: Anchor, canvas: Canvas, extent: dock::Extent) -> Shell {
    match form {
        Merge::Keystone | Merge::Fold => among(form, anchor, canvas, extent),
        Merge::Crown | Merge::Off => crown(form, anchor, canvas, extent),
    }
}

fn crown(form: Merge, anchor: Anchor, canvas: Canvas, extent: dock::Extent) -> Shell {
    let offset = offset_of(form, &extent);

    // the Dock's plate lines up with the Island's body along the side it sits against
    let side = match anchor.along {
        Along::Center => 0.0,
        Along::Left | Along::Right => TOP,
    };
    let width = canvas.width.max(extent.width + side).ceil();
    let height = (canvas.height + offset).max(extent.height).ceil();

    let across = |size: f32, gap: f32| match anchor.along {
        Along::Left => gap,
        Along::Center => ((width - size) / 2.0).round(),
        Along::Right => width - size - gap,
    };
    let (island_y, dock_y) = match anchor.edge {
        Edge::Bottom => (height - canvas.height - offset, height - extent.height),
        _ => (offset, 0.0),
    };

    Shell {
        form,
        extent,
        size: (width, height),
        island: (across(canvas.width, 0.0), island_y),
        dock: (across(extent.width, side), dock_y),
        strip: 0.0,
    }
}

/*
 * the window holds the Island's canvas and, to either side of its body, the icons as far as they
 * reach with the body at its largest, where it stands nearest the canvas's sides; a Peek or a
 * Surface is not a body that outgrows it
 */
fn among(form: Merge, anchor: Anchor, canvas: Canvas, extent: dock::Extent) -> Shell {
    let (island_off, strip_off) = depths(&extent);

    // as far as the icons reach with the pointer growing them
    let (before, after) = (
        extent.before + extent.swell + extent.inset,
        extent.after + extent.swell + extent.inset,
    );
    let (near_left, near_right) = match anchor.along {
        Along::Left => (TOP, ROOM_WIDTH - TOP),
        Along::Center => (ROOM_WIDTH / 2.0, ROOM_WIDTH / 2.0),
        Along::Right => (ROOM_WIDTH - TOP, TOP),
    };
    let (left, right) = (
        (before - near_left).max(0.0).ceil(),
        (after - near_right).max(0.0).ceil(),
    );

    // centered, the window is as far out to one side as to the other, so the body stays in the middle
    let (left, right) = match anchor.along {
        Along::Center => (left.max(right), left.max(right)),
        Along::Left | Along::Right => (left, right),
    };

    let width = left + canvas.width + right;
    let height = (island_off + canvas.height)
        .max(strip_off + extent.strip + extent.rise)
        .ceil();
    let (island_y, strip) = match anchor.edge {
        Edge::Bottom => (
            height - island_off - canvas.height,
            height - strip_off - extent.strip,
        ),
        _ => (island_off, strip_off),
    };

    Shell {
        form,
        extent,
        size: (width, height),
        island: (left, island_y),
        dock: (0.0, 0.0),
        strip,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dock(autohide: bool) -> Placement {
        Placement {
            anchor: Anchor::default(),
            autohide,
            reserve: false,
        }
    }

    #[test]
    fn separate_mode_keeps_legacy_merge_choices() {
        let mut config = config::Config::default();
        config.shell.mode = ShellMode::Separate;

        for form in [Merge::Crown, Merge::Keystone, Merge::Fold, Merge::Off] {
            config.merge = form;
            assert_eq!(
                chosen(&config, dock(false)),
                (form != Merge::Off).then_some(form)
            );
            assert_eq!(
                chosen(&config, dock(true)),
                match form {
                    Merge::Off => None,
                    _ => Some(Merge::Fold),
                }
            );
        }
    }

    #[test]
    fn shell_edge_offset_accounts_for_top_and_bottom_canvas_insets() {
        let canvas = Canvas {
            width: 500.0,
            height: 500.0,
        };
        let extent = dock::Extent {
            width: 300.0,
            height: 50.0,
            depth: 54.0,
            strip: 32.0,
            inset: 0.0,
            before: 0.0,
            after: 0.0,
            swell: 0.0,
            rise: 0.0,
        };

        for edge in [Edge::Top, Edge::Bottom] {
            let shell = arrange(
                Merge::Keystone,
                Anchor {
                    edge,
                    along: Along::Center,
                },
                canvas,
                extent,
            );
            assert_eq!(
                shell.edge_offset(
                    canvas,
                    Anchor {
                        edge,
                        along: Along::Center
                    }
                    .hang()
                    .edge
                ),
                14.0
            );
        }
    }

    #[test]
    fn combined_stays_joined_when_the_island_hides() {
        assert!(joined_visible(ShellMode::Combined, true));
        assert!(joined_visible(ShellMode::Combined, false));
    }

    #[test]
    fn separate_mode_keeps_hiding_the_merge_with_the_island() {
        assert!(!joined_visible(ShellMode::Separate, true));
        assert!(joined_visible(ShellMode::Separate, false));
    }

    #[test]
    fn combined_mode_uses_its_form_without_rewriting_legacy_merge() {
        let mut config = config::Config::default();
        config.shell.mode = ShellMode::Combined;

        for legacy in [Merge::Crown, Merge::Keystone, Merge::Fold, Merge::Off] {
            config.merge = legacy;
            assert_eq!(chosen(&config, dock(false)), Some(Merge::Keystone));
            assert_eq!(chosen(&config, dock(true)), Some(Merge::Fold));
            assert_eq!(config.merge, legacy);
        }
    }
}
