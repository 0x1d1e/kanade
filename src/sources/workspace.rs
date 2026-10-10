//! Workspace switch Transients (plan 5.3, ADR 0007): a switch shows the workspace on the island of
//! the output it happened on, as its primary until it expires, then the one before returns. It is
//! hidden while niri's overview is open, which shows the workspaces itself.
//!
//! niri.rs follows niri's EventStream and says which workspace is focused; this decides what the
//! island makes of it.

use super::niri::{Change, Seen};
use crate::config;
use crate::island::activity::{Activity, Detail, Id, Interrupt, Kind, Lifetime, Priority, Scope};

/*
 * what the island does about niri going from `before` to `now`. Only a switch on the output that
 * had the focus shows: focus moving to another output, by a window or the pointer, is no switch,
 * and a list that only renumbers is none either. The overview opening takes a shown switch away,
 * so it does not come back when the overview closes; switches inside it show nothing
 */
pub fn change(before: &Seen, now: &Seen) -> Option<Change> {
    if now.overview {
        return (!before.overview).then(|| Change::Withdraw(id()));
    }

    let (before, now) = (before.workspace.as_ref()?, now.workspace.as_ref()?);

    (now.id != before.id && now.output == before.output).then(|| {
        Change::Post(
            Activity::new(
                id(),
                Priority::Glance,
                Lifetime::Transient(config::get().osd),
                Scope::FocusedOutput,
                Interrupt::None,
            )
            .expect("the config bounds osd above zero")
            .with_detail(Detail::Workspace(now.workspace.clone())),
        )
    })
}

// one, so switch after switch extends one Transient
fn id() -> Id {
    Id::new(Kind::Workspace, "focused")
}
