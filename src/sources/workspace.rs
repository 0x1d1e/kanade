//! Workspace switch Transients (ADR 0007): a switch shows the workspace on the island of
//! the output it happened on, as its primary until it expires, then the one before returns. It is
//! hidden while niri's overview is open, which shows the workspaces itself.
//!
//! niri.rs follows niri's EventStream and says which workspace is focused; this watches that and
//! decides what the island makes of it.

use std::sync::Mutex;
use std::time::Instant;

use kanade_runtime::{MutexExt, service::Service};

use super::niri::{Niri, Seen, UrgentWorkspace};
use crate::config;
use crate::island::activity::{
    Activity, Detail, Id, Interrupt, Kind, Lifetime, Priority, Scope, Workspace,
};
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
    let before = SEEN.locked().replace(seen.clone()).unwrap_or_default();
    let mut changes = Vec::new();

    if let Some(change) = change(&before, &seen) {
        changes.push(change);
    }

    changes.extend(urgencies(&before.urgent, &seen.urgent));

    if changes.is_empty() {
        return;
    }

    let now = Instant::now();
    let mut island = IslandService::write();

    for change in changes {
        match change {
            Change::Post(activity) => island.post(activity, now),
            Change::Withdraw(id) => island.withdraw(&id, now),
        }
    }
}

/*
 * Show only workspace changes on the previously focused output; focus moves and renumbering do not
 * count. Overview open/close, switches within it, and overviews between reads suppress the switch.
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

// post what became urgent, withdraw what cleared; a workspace already urgent and unchanged is quiet
pub fn urgencies(before: &[UrgentWorkspace], now: &[UrgentWorkspace]) -> Vec<Change> {
    let mut changes = Vec::new();

    for gone in before {
        if !now.iter().any(|urgent| urgent.id == gone.id) {
            changes.push(Change::Withdraw(urgent_id(gone.id)));
        }
    }

    for urgent in now {
        if before
            .iter()
            .any(|before| before.id == urgent.id && before == urgent)
        {
            continue;
        }

        changes.push(Change::Post(urgent_activity(urgent)));
    }

    changes
}

fn urgent_id(id: u64) -> Id {
    Id::new(Kind::Workspace, format!("urgent-{id}"))
}

// Ongoing, so the Arbiter can place it as a Satellite; Interrupt::None, so it preempts nothing
fn urgent_activity(urgent: &UrgentWorkspace) -> Activity {
    Activity::new(
        urgent_id(urgent.id),
        Priority::Ongoing,
        Lifetime::Persistent,
        Scope::Global,
        Interrupt::None,
    )
    .expect("a persistent Activity is valid")
    .on_output(urgent.output.clone())
    .with_detail(Detail::Workspace(Workspace {
        index: urgent.index,
        count: urgent.count,
        name: urgent.name.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::activity::Workspace;
    use crate::sources::niri::{Focused, UrgentWorkspace};

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

    fn marked(id: u64, output: &str) -> UrgentWorkspace {
        UrgentWorkspace {
            id,
            output: output.to_owned(),
            index: 2,
            count: 3,
            name: None,
        }
    }

    #[test]
    fn urgency_is_a_satellite_on_that_output_until_it_clears() {
        let hdmi = marked(3, "HDMI-A-1");
        let posted = urgencies(&[], std::slice::from_ref(&hdmi));

        let Change::Post(activity) = &posted[0] else {
            panic!("expected a post");
        };

        assert_eq!(activity.scope(), Scope::Output);
        assert_eq!(activity.output(), Some("HDMI-A-1"));
        assert_eq!(activity.priority(), Priority::Ongoing);
        assert_eq!(activity.interrupt(), Interrupt::None);
        assert!(matches!(activity.lifetime(), Lifetime::Persistent));

        assert_eq!(
            urgencies(std::slice::from_ref(&hdmi), std::slice::from_ref(&hdmi)),
            []
        );
        assert!(matches!(
            urgencies(&[hdmi], &[]).as_slice(),
            [Change::Withdraw(_)]
        ));
    }
}
