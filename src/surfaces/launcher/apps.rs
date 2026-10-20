// the apps `sources::apps` lists, as a Launcher provider; pressing one starts it

use crate::clock;
use crate::sources::apps::App;
use crate::sources::recents::{self, Usage};

use super::provider::{self, Action, Answer, Fit, LauncherProvider, Mark};
use super::{destinations, emoji, history};

// the apps, and whether `@` opens Wallpapers instead
pub struct Apps<'a>(pub &'a [App], pub bool);

impl LauncherProvider for Apps<'_> {
    fn find(&self, query: &str) -> Vec<Answer> {
        let now = clock::unix_seconds();

        recents::with(|usage| self.found(query, usage, now))
    }
}

impl Apps<'_> {
    /*
     * by name within a fit, the higher score first, since the list is sorted so; an empty query
     * finds every app, the highest scores before the rest. A query that started an app before
     * lifts it again while it is remembered. The score orders only apps that fit alike, so it never
     * lifts one over a better fit
     */
    fn found(&self, query: &str, usage: &Usage, now: i64) -> Vec<Answer> {
        // an emoji search or the Wallpapers destination, which no app answers
        if query.starts_with(emoji::PREFIX)
            || query.starts_with(history::PREFIX)
            || (self.1 && query.starts_with(destinations::WALLPAPER))
        {
            return Vec::new();
        }

        let query = query.to_lowercase();

        // with nothing typed, the apps with the highest scores come first
        let recent = if query.is_empty() {
            usage.recent(now)
        } else {
            Vec::new()
        };

        let mut answers: Vec<(u8, usize, f64, Answer)> = self
            .0
            .iter()
            .filter_map(|app| {
                let fit = fit(
                    &query,
                    &app.name,
                    app.description.as_deref().unwrap_or_default(),
                    app.command(),
                )?;
                let key = recents::app(&app.id);
                let place = recent.iter().position(|recent| *recent == key.as_str());

                Some((
                    usage.learned(&key, &query, now),
                    place.unwrap_or(usize::MAX),
                    usage.score(&key, now),
                    answer(app, place.map_or(fit, |_| Fit::Recent)),
                ))
            })
            .collect();

        // stable, so the rest stay by name
        answers.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then(left.1.cmp(&right.1))
                .then(right.2.total_cmp(&left.2))
        });

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
pub(super) fn mark(app: &App) -> Mark {
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

pub(super) fn answer(app: &App, fit: Fit) -> Answer {
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
        let now = clock::unix_seconds();

        recents::with(|usage| self.found(query, usage, now))
    }
}

impl Actions<'_> {
    /*
     * an action's name by `provider::fit`, else every word in the app's name and its own; none
     * for an empty or short query, which lists apps, nor for what other providers answer. The
     * higher its app's score, the sooner within a fit
     */
    fn found(&self, query: &str, usage: &Usage, now: i64) -> Vec<Answer> {
        if !self.1
            || query.chars().count() < FROM
            || query.starts_with(emoji::PREFIX)
            || query.starts_with(history::PREFIX)
        {
            return Vec::new();
        }

        let query = query.to_lowercase();
        let mut answers: Vec<(f64, Answer)> = Vec::new();

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
                    usage.score(&recents::app(&app.id), now),
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
        answers.sort_by(|left, right| right.0.total_cmp(&left.0));

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
    // nothing typed the higher scores come first, and they fade as the score halves
    #[test]
    fn usage_never_outranks_a_better_fit() {
        let apps = [app("Files"), app("Profiles"), app("Filezilla")];
        let mut usage = Usage::default();
        let now = clock::unix_seconds();

        for _ in 0..5 {
            usage.start(&recents::app("Profiles.desktop"), "", now);
        }
        usage.start(&recents::app("Filezilla.desktop"), "", now);

        let names = |usage: &Usage, query: &str, now: i64| {
            let mut found = Apps(&apps, false).found(query, usage, now);

            // as `provider::ranked` orders, stable
            found.sort_by_key(|answer| answer.fit);
            found
                .into_iter()
                .map(|answer| answer.title)
                .collect::<Vec<_>>()
        };

        assert_eq!(names(&usage, "files", now), ["Files", "Profiles"]);
        assert_eq!(
            names(&usage, "file", now),
            ["Filezilla", "Files", "Profiles"]
        );

        // five starts beat one, and a start a month later beats those five
        assert_eq!(names(&usage, "", now), ["Profiles", "Filezilla", "Files"]);

        let later = now + 30 * 86_400;
        usage.start(&recents::app("Filezilla.desktop"), "", later);

        assert_eq!(names(&usage, "", later), ["Filezilla", "Profiles", "Files"]);
    }
}
