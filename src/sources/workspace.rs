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
                Priority::Osd,
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

    fn on(output: &str, id: u64, index: u32) -> Seen {
        Seen {
            focused_output: Some(output.into()),
            overview: false,
            workspace: Some(Focused {
                id,
                output: Some(output.into()),
                workspace: Workspace {
                    index,
                    count: 3,
                    name: None,
                },
            }),
            casting: false,
        }
    }

    fn overview(seen: Seen) -> Seen {
        Seen {
            overview: true,
            ..seen
        }
    }

    fn shown(change: Option<Change>) -> Option<Workspace> {
        match change? {
            Change::Post(activity) => {
                assert_eq!(activity.id(), &id());
                assert_eq!(activity.lifetime(), Lifetime::Transient(config::get().osd));
                assert_eq!(activity.priority(), Priority::Osd);
                assert_eq!(activity.scope(), Scope::FocusedOutput);
                assert_eq!(activity.interrupt(), Interrupt::None);

                match activity.detail() {
                    Detail::Workspace(workspace) => Some(workspace.clone()),
                    detail => panic!("not a workspace: {detail:?}"),
                }
            }
            Change::Withdraw(_) => None,
        }
    }

    #[test]
    fn a_switch_on_the_focused_output_shows_the_new_workspace() {
        let (one, two) = (on("eDP-1", 1, 1), on("eDP-1", 2, 2));

        assert_eq!(
            shown(change(&one, &two)),
            two.workspace.map(|focused| focused.workspace)
        );
    }

    // the first list niri sends, and a lost stream, are no switch
    #[test]
    fn unknown_on_either_side_shows_nothing() {
        let one = on("eDP-1", 1, 1);

        assert_eq!(change(&Seen::default(), &one), None);
        assert_eq!(change(&one, &Seen::default()), None);
    }

    #[test]
    fn focus_moving_to_another_output_shows_nothing() {
        assert_eq!(change(&on("eDP-1", 1, 1), &on("HDMI-A-1", 3, 1)), None);
    }

    // a workspace that came or went, or the focused one moving to another output, keeps its id
    #[test]
    fn the_same_workspace_renumbered_or_moved_shows_nothing() {
        let one = on("eDP-1", 1, 1);

        assert_eq!(change(&one, &on("eDP-1", 1, 2)), None);
        assert_eq!(change(&one, &on("HDMI-A-1", 1, 1)), None);
    }

    #[test]
    fn the_overview_takes_a_switch_away_and_shows_none() {
        let (one, two) = (on("eDP-1", 1, 1), on("eDP-1", 2, 2));

        assert_eq!(
            change(&one, &overview(one.clone())),
            Some(Change::Withdraw(id()))
        );
        assert_eq!(change(&overview(one), &overview(two.clone())), None);

        // picking a workspace closes the overview on it, which shows it already
        assert_eq!(change(&overview(two.clone()), &two), None);
    }
}
