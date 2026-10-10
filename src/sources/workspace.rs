//! Workspace switch Transients (plan 5.3, ADR 0007): a switch shows the workspace on the island of
//! the output it happened on, as its primary until it expires, then the one before returns. It is
//! hidden while niri's overview is open, which shows the workspaces itself.
//!
//! niri.rs follows niri's EventStream and says which workspace is focused; this watches that and
//! decides what the island makes of it.

use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use kanade_runtime::service::Service;

use super::niri::{Niri, Seen};
use crate::config;
use crate::island::activity::{Activity, Detail, Id, Interrupt, Kind, Lifetime, Priority, Scope};
use crate::island::service::IslandService;

// what the island does about a change niri made
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Post(Activity),
    Withdraw(Id),
}

// what niri said when this last looked, which a switch is told from
static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

// a watcher of `Niri`: posts or withdraws the switch niri made since it last ran
pub fn follow() {
    let seen = Niri::read().seen.clone();
    let before = SEEN
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .replace(seen.clone())
        .unwrap_or_default();

    let now = Instant::now();

    match change(&before, &seen) {
        Some(Change::Post(activity)) => IslandService::write().post(activity, now),
        Some(Change::Withdraw(id)) => IslandService::write().withdraw(&id, now),
        None => {}
    }
}

/*
 * what the island does about niri going from `before` to `now`. Only a switch on the output that
 * had the focus shows: focus moving to another output, by a window or the pointer, is no switch,
 * and a list that only renumbers is none either. The overview opening takes a shown switch away,
 * so it does not come back when the overview closes; switches inside it show nothing, nor does
 * the one that closes it, which niri tells beside the close. An overview that opened and closed
 * between two looks is told by its count, and shows nothing either
 */
pub fn change(before: &Seen, now: &Seen) -> Option<Change> {
    if now.overview {
        return (!before.overview).then(|| Change::Withdraw(id()));
    }

    if now.overviews != before.overviews {
        return Some(Change::Withdraw(id()));
    }

    if before.overview {
        return None;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::activity::Workspace;
    use crate::sources::niri::Focused;

    // workspace `id` focused, the overview `opened` times so far and open now or not
    fn on(id: u64, opened: u32, overview: bool) -> Seen {
        Seen {
            overview,
            overviews: opened,
            workspace: Some(Focused {
                id,
                output: Some(String::from("eDP-1")),
                workspace: Workspace {
                    index: 1,
                    count: 2,
                    name: None,
                },
            }),
            ..Seen::default()
        }
    }

    // niri tells the workspace a click in the overview chose beside the overview closing, which
    // may reach this together
    #[test]
    fn closing_the_overview_on_another_workspace_shows_no_switch() {
        assert_eq!(change(&on(1, 1, true), &on(2, 1, false)), None);
        assert!(matches!(
            change(&on(1, 1, false), &on(2, 1, false)),
            Some(Change::Post(_))
        ));
    }

    // opened, a workspace chosen and closed before the next look: only the count shows it
    #[test]
    fn an_overview_opened_and_closed_between_looks_shows_no_switch() {
        assert!(matches!(
            change(&on(1, 0, false), &on(2, 1, false)),
            Some(Change::Withdraw(_))
        ));
    }
}
