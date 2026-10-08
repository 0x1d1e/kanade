// the apps Amane's `Apps` finds, as a Launcher provider; pressing one starts it

use amane::DesktopApp;

use super::provider::{self, Action, Answer, Fit, LauncherProvider, Mark};
use super::{emoji, wallpaper};

// the apps, and whether `@` searches wallpapers instead
pub struct Apps<'a>(pub &'a [DesktopApp], pub bool);

impl LauncherProvider for Apps<'_> {
    // by name within a fit, since Amane sorts them so; an empty query finds every app
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
                    app.name(),
                    app.description().unwrap_or_default(),
                    app.exec(),
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
fn answer(app: &DesktopApp, fit: Fit) -> Answer {
    let mark = match app.icon_path() {
        Some(path) => Mark::Picture(path.to_owned()),
        None => Mark::Tile(
            app.name()
                .chars()
                .next()
                .map_or_else(String::new, |initial| initial.to_uppercase().collect()),
        ),
    };

    Answer {
        fit,
        title: app.name().to_owned(),
        detail: app.description().unwrap_or_default().to_owned(),
        mark,
        action: Action::Launch(app.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // what the query finds among (name, description, exec), best first, as `ranked` orders them
    fn find<'a>(query: &str, apps: &[(&'a str, &str, &str)]) -> Vec<&'a str> {
        let query = query.trim().to_lowercase();

        let mut ranked: Vec<(Fit, &str)> = apps
            .iter()
            .filter_map(|&(name, description, exec)| {
                Some((fit(&query, name, description, exec)?, name))
            })
            .collect();
        ranked.sort_by_key(|&(fit, _)| fit);

        ranked.into_iter().map(|(_, name)| name).collect()
    }

    const APPS: [(&str, &str, &str); 5] = [
        (
            "Files",
            "Access and organize files",
            "nautilus --new-window",
        ),
        ("Firefox", "Web Browser", "firefox"),
        (
            "GNU Image Manipulation Program",
            "Create images",
            "gimp-2.10",
        ),
        ("Kitty", "Terminal emulator", "kitty"),
        ("Visual Studio Code", "Code Editing. Redefined.", "code"),
    ];

    #[test]
    fn the_name_starting_with_the_query_comes_first() {
        // the description's "Redefined" finds Code too, after both names
        assert_eq!(
            find("fi", &APPS),
            ["Files", "Firefox", "Visual Studio Code"]
        );
        assert_eq!(find("FIRE", &APPS), ["Firefox"]);
    }

    #[test]
    fn the_whole_name_beats_its_start() {
        let apps = [("Files Manager", "", "fm"), ("Files", "", "files")];

        assert_eq!(find("files", &apps), ["Files", "Files Manager"]);
    }

    #[test]
    fn a_word_start_beats_the_middle_of_a_word() {
        let apps = [
            ("Tor Browser", "", "tor"),
            ("Monitor", "", "monitor"),
            ("Torrent", "", "torrent"),
        ];

        assert_eq!(find("tor", &apps), ["Tor Browser", "Torrent", "Monitor"]);
        assert_eq!(find("code", &APPS), ["Visual Studio Code"]);
    }

    #[test]
    fn every_word_may_match_apart_and_then_anywhere() {
        assert_eq!(
            find("visual code", &APPS),
            ["Visual Studio Code"],
            "both in the name"
        );
        assert_eq!(find("nautilus", &APPS), ["Files"], "the command");
        assert_eq!(find("browser", &APPS), ["Firefox"], "the description");
        assert_eq!(find("gimp", &APPS), ["GNU Image Manipulation Program"]);
        assert_eq!(find("zzz", &APPS), Vec::<&str>::new());
    }

    #[test]
    fn an_empty_query_finds_every_app_in_order() {
        let names: Vec<&str> = APPS.iter().map(|app| app.0).collect();

        assert_eq!(find("", &APPS), names);
        assert_eq!(find("   ", &APPS), names);
    }
}
