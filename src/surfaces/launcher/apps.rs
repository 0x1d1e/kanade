// the apps `sources::apps` lists, as a Launcher provider; pressing one starts it

use crate::sources::apps::App;
use crate::sources::recents::{self, Usage};

use super::provider::{self, Action, Answer, Fit, LauncherProvider, Mark};
use super::{destinations, emoji, history};

// the apps, and whether `@` opens Wallpapers instead
pub struct Apps<'a>(pub &'a [App], pub bool);

impl LauncherProvider for Apps<'_> {
    fn find(&self, query: &str) -> Vec<Answer> {
        recents::with(|usage| self.found(query, usage))
    }
}

impl Apps<'_> {
    /*
     * by name within a fit, the more started first, since the list is sorted so; an empty query
     * finds every app, those started last before the rest. A count orders only apps that fit alike,
     * so it never lifts an app over a better fit
     */
    fn found(&self, query: &str, usage: &Usage) -> Vec<Answer> {
        // an emoji search or the Wallpapers destination, which no app answers
        if query.starts_with(emoji::PREFIX)
            || query.starts_with(history::PREFIX)
            || (self.1 && query.starts_with(destinations::WALLPAPER))
        {
            return Vec::new();
        }

        let query = query.to_lowercase();

        // with nothing typed, the apps started lately come first, newest first
        let recent = if query.is_empty() {
            usage.recent()
        } else {
            Vec::new()
        };

        let mut answers: Vec<(usize, u32, Answer)> = self
            .0
            .iter()
            .filter_map(|app| {
                let fit = fit(
                    &query,
                    &app.name,
                    app.description.as_deref().unwrap_or_default(),
                    app.command(),
                )?;
                let place = recent.iter().position(|id| *id == app.id);

                Some((
                    place.unwrap_or(usize::MAX),
                    usage.count(&app.id),
                    answer(app, place.map_or(fit, |_| Fit::Recent)),
                ))
            })
            .collect();

        // stable, so the rest stay by name
        answers.sort_by_key(|&(place, count, _)| (place, std::cmp::Reverse(count)));

        answers.into_iter().map(|(.., answer)| answer).collect()
    }
}

/*
 * how well an app answers the lowercase query, as `provider::fit` says of its name, then every
 * word anywhere in its name, description or command
 */
fn fit(query: &str, name: &str, description: &str, exec: &str) -> Option<Fit> {
    let name = name.to_lowercase();

    if let Some(fit) = provider::fit(query, &name) {
        return Some(fit);
    }

    let anywhere = format!("{name} {description} {exec}").to_lowercase();

    query
        .split_whitespace()
        .all(|term| anywhere.contains(term))
        .then_some(Fit::Anywhere)
}

// the app's icon, or its name's initial for one without
fn mark(app: &App) -> Mark {
    match &app.icon_file {
        Some(path) => Mark::Picture(path.clone()),
        None => Mark::Tile(
            app.name
                .chars()
                .next()
                .map_or_else(String::new, |initial| initial.to_uppercase().collect()),
        ),
    }
}

fn answer(app: &App, fit: Fit) -> Answer {
    Answer {
        fit,
        title: app.name.clone(),
        detail: String::new(),
        mark: mark(app),
        action: Action::Launch(app.launch.clone(), app.id.clone()),
    }
}

// the fewest letters an app's action takes: one would list the actions of every app
const FROM: usize = 2;

// the apps' own actions, like a browser's New Private Window, with the switch `launcher.actions`
pub struct Actions<'a>(pub &'a [App], pub bool);

impl LauncherProvider for Actions<'_> {
    fn find(&self, query: &str) -> Vec<Answer> {
        recents::with(|usage| self.found(query, usage))
    }
}

impl Actions<'_> {
    /*
     * an action's name by `provider::fit`, else every word in the app's name and its own; none
     * for an empty or short query, which lists apps, nor for what other providers answer. The
     * more started its app, the sooner within a fit
     */
    fn found(&self, query: &str, usage: &Usage) -> Vec<Answer> {
        if !self.1
            || query.chars().count() < FROM
            || query.starts_with(emoji::PREFIX)
            || query.starts_with(history::PREFIX)
        {
            return Vec::new();
        }

        let query = query.to_lowercase();
        let mut answers: Vec<(u32, Answer)> = Vec::new();

        for app in self.0 {
            for action in &app.actions {
                let name = action.name.to_lowercase();
                let anywhere = format!("{} {name}", app.name.to_lowercase());

                let Some(fit) = provider::fit(&query, &name).or_else(|| {
                    query
                        .split_whitespace()
                        .all(|term| anywhere.contains(term))
                        .then_some(Fit::Anywhere)
                }) else {
                    continue;
                };

                answers.push((
                    usage.count(&app.id),
                    Answer {
                        fit,
                        title: action.name.clone(),
                        detail: app.name.clone(),
                        mark: mark(app),
                        action: Action::Perform(
                            action.launch.clone(),
                            app.id.clone(),
                            action.id.clone(),
                        ),
                    },
                ));
            }
        }

        // stable, so the rest stay by app name
        answers.sort_by_key(|&(count, _)| std::cmp::Reverse(count));

        answers.into_iter().map(|(_, answer)| answer).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::launch::{Fields, Launch};
    use std::path::Path;

    fn app(name: &str) -> App {
        let id = format!("{name}.desktop");

        App {
            launch: Launch::of(&Fields {
                id: &id,
                name,
                icon: None,
                file: Path::new("/usr/share/applications/x.desktop"),
                exec: Some(name),
                path: None,
                terminal: false,
                dbus_activatable: false,
            })
            .unwrap(),
            id,
            name: name.into(),
            description: None,
            icon: None,
            icon_file: None,
            actions: Vec::new(),
        }
    }

    // usage orders apps that fit a query alike and never lifts one over a better fit; with
    // nothing typed the apps started lately come first
    #[test]
    fn usage_never_outranks_a_better_fit() {
        let apps = [app("Files"), app("Profiles"), app("Filezilla")];
        let mut usage = Usage::default();

        for _ in 0..5 {
            usage.start("Profiles.desktop");
        }
        usage.start("Filezilla.desktop");

        let names = |query| {
            let mut found = Apps(&apps, false).found(query, &usage);

            // as `provider::ranked` orders, stable
            found.sort_by_key(|answer| answer.fit);
            found
                .into_iter()
                .map(|answer| answer.title)
                .collect::<Vec<_>>()
        };

        assert_eq!(names("files"), ["Files", "Profiles"]);
        assert_eq!(names("file"), ["Filezilla", "Files", "Profiles"]);
        assert_eq!(names(""), ["Filezilla", "Profiles", "Files"]);
    }
}
