// the apps `sources::apps` lists, as a Launcher provider; pressing one starts it

use crate::sources::apps::App;

use super::provider::{self, Action, Answer, Fit, LauncherProvider, Mark};
use super::{emoji, wallpaper};

// the apps, and whether `@` searches wallpapers instead
pub struct Apps<'a>(pub &'a [App], pub bool);

impl LauncherProvider for Apps<'_> {
    // by name within a fit, since the list is sorted so; an empty query finds every app
    fn find(&self, query: &str) -> Vec<Answer> {
        // an emoji or wallpaper search, which no app answers
        if query.starts_with(emoji::PREFIX) || (self.1 && query.starts_with(wallpaper::PREFIX)) {
            return Vec::new();
        }

        let query = query.to_lowercase();

        self.0
            .iter()
            .filter_map(|app| {
                let fit = fit(
                    &query,
                    &app.name,
                    app.description.as_deref().unwrap_or_default(),
                    app.command(),
                )?;

                Some(answer(app, fit))
            })
            .collect()
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
fn answer(app: &App, fit: Fit) -> Answer {
    let mark = match &app.icon_file {
        Some(path) => Mark::Picture(path.clone()),
        None => Mark::Tile(
            app.name
                .chars()
                .next()
                .map_or_else(String::new, |initial| initial.to_uppercase().collect()),
        ),
    };

    Answer {
        fit,
        title: app.name.clone(),
        detail: app.description.clone().unwrap_or_default(),
        mark,
        action: Action::Launch(app.launch.clone()),
    }
}
