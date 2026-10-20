/*
 * The actions Ctrl+K opens over the selected answer (ADR 0129): pressing it, its app's Desktop
 * Actions, copying what names it, and the Dock's pin. A row that cannot act says why in its second
 * line and takes no press, rather than doing nothing.
 */

use crate::sources::apps::App;

use super::provider::{Action, Answer};

// what pressing a row asks for
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    // what Enter would do with the answer, or with one of its app's own actions
    Press(Action),

    // put this text on the clipboard
    Copy(String),

    // pin the app with this desktop file id to the Dock, or unpin it
    Pin { id: String, pinned: bool },
}

// one row of the panel
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub label: String,

    // what it acts on, or why it cannot act; empty where the label says it all
    pub detail: String,

    // none for a row that cannot act
    pub act: Option<Act>,
}

// the actions of one answer, drawn in the place of the list
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Panel {
    pub offers: Vec<Offer>,
}

/*
 * the actions of `answer`: pressing it, its app's own Desktop Actions where it has an app with
 * any, copying what names it, and the Dock's pin. `apps` and `pinned` are as they are now, so the
 * panel says the pin the Dock holds rather than the one it had when the Launcher opened
 */
pub fn of(answer: &Answer, apps: &[App], pinned: &[String]) -> Panel {
    let app = named(&answer.action).and_then(|id| apps.iter().find(|app| app.id == id));

    let mut offers = vec![pressing(answer)];

    if let Some(app) = app {
        for action in &app.actions {
            // the answer a desktop action already is takes no second row for it
            if matches!(&answer.action, Action::Perform(_, _, at) if *at == action.id) {
                continue;
            }

            offers.push(Offer {
                label: action.name.clone(),
                detail: String::new(),
                act: Some(Act::Press(Action::Perform(
                    app.launch.clone(),
                    app.id.clone(),
                    action.id.clone(),
                ))),
            });
        }
    }

    offers.push(copied(&answer.action));
    offers.push(to_dock(&answer.action, pinned));

    Panel { offers }
}

// what pressing the answer itself does, named as it does it
fn pressing(answer: &Answer) -> Offer {
    let label = match &answer.action {
        Action::Launch(..) => "Start",
        Action::Perform(..) => "Run this action",
        Action::Focus(_) => "Switch to",
        Action::Open(_) => "Open",
        Action::Run(_) => "Run",
        Action::Copy(_) => "Copy",
        Action::Paste(_) => "Paste",
        Action::Choose(_) => "Choose",
        Action::Browse(_) => "Open in the browser",
    };

    Offer {
        label: String::from(label),
        detail: answer.detail.clone(),
        act: Some(Act::Press(answer.action.clone())),
    }
}

// the path or id that names the answer, when one does
fn copied(action: &Action) -> Offer {
    let copies = |label: &str, text: String| Offer {
        label: String::from(label),
        detail: text.clone(),
        act: Some(Act::Copy(text)),
    };
    let cannot = |why: &str| Offer {
        label: String::from("Copy id"),
        detail: String::from(why),
        act: None,
    };

    match action {
        Action::Launch(_, id) | Action::Perform(_, id, _) => copies("Copy id", id.clone()),
        Action::Focus(id) => copies("Copy id", id.to_string()),
        Action::Run(verb) => copies("Copy command", verb.join(" ")),
        Action::Open(destination) => copies("Copy id", destination.name().to_owned()),
        Action::Copy(text) => copies("Copy", text.clone()),
        Action::Paste(_) => cannot("A copied entry has no path or id"),
        Action::Choose(_) => cannot("A choice has no path or id"),
        // the address is what a web row sends, and it is the query itself, so there is nothing of
        // the machine to copy out of it
        Action::Browse(_) => cannot("A web search copies nothing"),
    }
}

// the Dock's pin, which only an app has
fn to_dock(action: &Action, pinned: &[String]) -> Offer {
    let Some(id) = named(action) else {
        return Offer {
            label: String::from("Pin to the Dock"),
            detail: String::from("Only an app is pinned to the Dock"),
            act: None,
        };
    };

    let pinned = pinned.iter().any(|each| each == id);

    Offer {
        label: String::from(match pinned {
            true => "Unpin from the Dock",
            false => "Pin to the Dock",
        }),
        detail: id.to_owned(),
        act: Some(Act::Pin {
            id: id.to_owned(),
            pinned,
        }),
    }
}

// the desktop file id of an answer that is an app or one of its actions
fn named(action: &Action) -> Option<&str> {
    match action {
        Action::Launch(_, id) | Action::Perform(_, id, _) => Some(id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::desktop;
    use crate::sources::launch::{Fields, Launch};
    use std::path::Path;

    fn launch(id: &str) -> Launch {
        Launch::of(&Fields {
            id,
            name: "Firefox",
            icon: None,
            file: Path::new("/usr/share/applications/firefox.desktop"),
            exec: Some("firefox"),
            path: None,
            terminal: false,
            dbus_activatable: false,
        })
        .unwrap()
    }

    fn app() -> App {
        let id = "firefox.desktop";

        App {
            id: String::from(id),
            name: String::from("Firefox"),
            description: None,
            icon: None,
            icon_file: None,
            launch: launch(id),
            actions: vec![desktop::Action {
                id: String::from("private-window"),
                name: String::from("New Private Window"),
                launch: launch(id),
            }],
        }
    }

    fn answer(action: Action) -> Answer {
        Answer {
            fit: crate::surfaces::launcher::provider::Fit::Exact,
            title: String::from("Firefox"),
            detail: String::new(),
            mark: crate::surfaces::launcher::provider::Mark::Tile(String::from("F")),
            action,
        }
    }

    fn app_answer() -> Answer {
        answer(Action::Launch(
            launch("firefox.desktop"),
            String::from("firefox.desktop"),
        ))
    }

    // an app's panel offers pressing it, its own actions, the id that names it and the Dock's pin
    #[test]
    fn an_apps_actions_list_its_own_and_what_names_it() {
        let apps = [app()];
        let panel = of(&app_answer(), &apps, &[]);

        let labels: Vec<&str> = panel
            .offers
            .iter()
            .map(|offer| offer.label.as_str())
            .collect();
        assert_eq!(
            labels,
            ["Start", "New Private Window", "Copy id", "Pin to the Dock"]
        );

        assert_eq!(
            panel.offers[1].act,
            Some(Act::Press(Action::Perform(
                launch("firefox.desktop"),
                String::from("firefox.desktop"),
                String::from("private-window"),
            )))
        );
        assert_eq!(
            panel.offers[2].act,
            Some(Act::Copy(String::from("firefox.desktop")))
        );
        assert_eq!(
            panel.offers[3].act,
            Some(Act::Pin {
                id: String::from("firefox.desktop"),
                pinned: false,
            })
        );

        let unpin = of(&app_answer(), &apps, &[String::from("firefox.desktop")]);
        assert_eq!(unpin.offers[3].label, "Unpin from the Dock");
    }

    // the answer a desktop action already is takes no second row for it
    #[test]
    fn the_action_an_answer_is_takes_no_second_row() {
        let panel = of(
            &answer(Action::Perform(
                launch("firefox.desktop"),
                String::from("firefox.desktop"),
                String::from("private-window"),
            )),
            &[app()],
            &[],
        );

        assert_eq!(panel.offers.len(), 3);
        assert_eq!(panel.offers[0].label, "Run this action");
    }

    // a row that cannot act says why, and carries nothing to press
    #[test]
    fn a_row_that_cannot_act_says_why() {
        let panel = of(
            &answer(Action::Paste(7)),
            &[app()],
            &[String::from("firefox.desktop")],
        );

        let copy = &panel.offers[1];
        let pin = &panel.offers[2];

        assert_eq!(copy.act, None);
        assert!(!copy.detail.is_empty());
        assert_eq!(pin.act, None);
        assert_eq!(pin.detail, "Only an app is pinned to the Dock");
    }
}
