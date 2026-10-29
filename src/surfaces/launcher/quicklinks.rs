// Quicklinks: user-configured shortcuts that surface in the Launcher as a provider.
// Each entry has a title and a target: a URL opened with `xdg-open` (no shell) or an argv list
// spawned directly (no shell). Titles and URLs are never logged (ADR 0019) and are kept out of
// the frecency state file, so quicklinks list in config order within a Fit (ADR 0127).

use std::process::{Command, Stdio};
use std::thread;

use toml::Value;

use crate::icon::Icon;

use super::provider::{self, Action, Answer, Fit, LauncherProvider, Mark};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quicklink {
    pub title: String,
    pub target: Target,
    // the typed query is one more argv word; only an argv target takes one
    pub passes_query: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Url(String),
    Argv(Vec<String>),
}

pub struct Quicklinks<'a>(pub &'a [Quicklink]);

impl LauncherProvider for Quicklinks<'_> {
    fn find(&self, query: &str) -> Vec<Answer> {
        self.0
            .iter()
            .filter_map(|link| {
                let fit = if link.passes_query {
                    // the query is the argument, so the row takes any text; its name still
                    // ranks it when the text fits, else it waits at the end
                    if query.is_empty() {
                        return None;
                    }
                    provider::fit(query, &link.title).unwrap_or(Fit::Anywhere)
                } else if query.is_empty() {
                    Fit::Prefix
                } else {
                    provider::fit(query, &link.title)?
                };

                Some(answer(link, fit, query))
            })
            .collect()
    }
}

fn answer(link: &Quicklink, fit: Fit, query: &str) -> Answer {
    let (detail, action) = match &link.target {
        Target::Url(url) => (String::new(), Action::Browse(url.clone())),
        Target::Argv(argv) => (
            String::new(),
            Action::Quicklink(command(argv, link.passes_query, query)),
        ),
    };

    Answer {
        fit,
        title: link.title.clone(),
        detail,
        mark: Mark::Icon(Icon::Forward),
        action,
    }
}

// the argv a row runs: a passing row's typed query is one more word, never split
fn command(argv: &[String], passes_query: bool, query: &str) -> Vec<String> {
    let mut words = argv.to_vec();

    if passes_query && !query.is_empty() {
        words.push(query.to_owned());
    }

    words
}

// runs the argv directly, no shell, off the view thread
pub fn run(argv: &[String]) -> Result<(), String> {
    if argv.is_empty() {
        return Err(String::from("empty argv"));
    }

    let mut command = Command::new(&argv[0]);
    if argv.len() > 1 {
        command.args(&argv[1..]);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = command
        .spawn()
        .map_err(|error| format!("{}: {error}", argv[0]))?;

    thread::Builder::new()
        .name("launcher-quicklink".into())
        .spawn(move || match child.wait_with_output() {
            Ok(output) if !output.status.success() => {
                eprintln!("launcher: quicklink failed ({})", output.status)
            }
            Ok(_) => {}
            Err(error) => eprintln!("launcher: quicklink: {error}"),
        })
        .map(drop)
        .map_err(|error| format!("quicklink: {error}"))
}

// reads a list of quicklink tables from a TOML value
pub fn read_all(value: &Value) -> Result<Vec<Quicklink>, String> {
    let list = value
        .as_array()
        .ok_or("expected a list of quicklink tables")?;

    let mut links = Vec::new();

    for (at, entry) in list.iter().enumerate() {
        let table = entry
            .as_table()
            .ok_or_else(|| format!("quicklink {}: expected a table", at + 1))?;

        let title = table
            .get("title")
            .and_then(Value::as_str)
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| format!("quicklink {}: expected a non-empty \"title\"", at + 1))?
            .trim()
            .to_owned();

        let has_url = table.contains_key("url");
        let has_argv = table.contains_key("argv");

        if has_url && has_argv {
            return Err(format!(
                "quicklink {}: set \"url\" or \"argv\", not both",
                at + 1
            ));
        }

        let target = if let Some(url) = table.get("url") {
            let url = url
                .as_str()
                .filter(|u| !u.trim().is_empty())
                .ok_or_else(|| format!("quicklink {}: expected a non-empty \"url\"", at + 1))?
                .trim()
                .to_owned();
            Target::Url(url)
        } else if let Some(argv) = table.get("argv") {
            let argv = argv.as_array().ok_or_else(|| {
                format!(
                    "quicklink {}: expected a list of strings for \"argv\"",
                    at + 1
                )
            })?;

            if argv.is_empty() {
                return Err(format!("quicklink {}: \"argv\" must not be empty", at + 1));
            }

            let words: Vec<String> = argv
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    v.as_str().map(String::from).ok_or_else(|| {
                        format!("quicklink {}: argv[{}]: expected a string", at + 1, i)
                    })
                })
                .collect::<Result<_, _>>()?;

            Target::Argv(words)
        } else {
            return Err(format!(
                "quicklink {}: needs a \"url\" or an \"argv\"",
                at + 1
            ));
        };

        let passes_query = match table.get("passes_query") {
            Some(value) => value.as_bool().ok_or_else(|| {
                format!(
                    "quicklink {}: \"passes_query\" must be true or false",
                    at + 1
                )
            })?,
            None => false,
        };

        if passes_query && !matches!(target, Target::Argv(_)) {
            return Err(format!(
                "quicklink {}: \"passes_query\" needs an \"argv\"",
                at + 1
            ));
        }

        // refuse unknown keys
        for key in table.keys() {
            if key != "title" && key != "url" && key != "argv" && key != "passes_query" {
                return Err(format!("quicklink {}: unknown key {key:?}", at + 1));
            }
        }

        links.push(Quicklink {
            title,
            target,
            passes_query,
        });
    }

    Ok(links)
}

pub fn value(links: &[Quicklink]) -> Value {
    Value::Array(
        links
            .iter()
            .map(|link| {
                let mut table = toml::map::Map::new();
                table.insert("title".into(), Value::String(link.title.clone()));

                match &link.target {
                    Target::Url(url) => {
                        table.insert("url".into(), Value::String(url.clone()));
                    }
                    Target::Argv(argv) => {
                        table.insert(
                            "argv".into(),
                            Value::Array(argv.iter().map(|a| Value::String(a.clone())).collect()),
                        );
                    }
                }

                if link.passes_query {
                    table.insert("passes_query".into(), Value::Boolean(true));
                }

                Value::Table(table)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link_url(title: &str, url: &str) -> Quicklink {
        Quicklink {
            title: title.into(),
            target: Target::Url(url.into()),
            passes_query: false,
        }
    }

    fn link_argv(title: &str, argv: &[&str]) -> Quicklink {
        Quicklink {
            title: title.into(),
            target: Target::Argv(argv.iter().map(|s| String::from(*s)).collect()),
            passes_query: false,
        }
    }

    fn script(title: &str, argv: &[&str]) -> Quicklink {
        Quicklink {
            title: title.into(),
            target: Target::Argv(argv.iter().map(|s| String::from(*s)).collect()),
            passes_query: true,
        }
    }

    // a document layer, as the config file parsed it
    fn document(text: &str) -> Value {
        Value::Table(text.parse().unwrap())
    }

    #[test]
    fn parse_url_quicklink() {
        let toml = r#"[[launcher.quicklinks]]
title = "GitHub"
url = "https://github.com"
"#;
        let value = document(toml);
        let links = read_all(value.get("launcher").unwrap().get("quicklinks").unwrap()).unwrap();

        assert_eq!(links, [link_url("GitHub", "https://github.com")]);
    }

    #[test]
    fn parse_argv_quicklink() {
        let toml = r#"[[launcher.quicklinks]]
title = "Editor"
argv = ["code", "--new-window"]
"#;
        let value = document(toml);
        let links = read_all(value.get("launcher").unwrap().get("quicklinks").unwrap()).unwrap();

        assert_eq!(links, [link_argv("Editor", &["code", "--new-window"])]);
    }

    #[test]
    fn empty_list_is_fine() {
        let value = Value::Array(Vec::new());
        let links = read_all(&value).unwrap();

        assert!(links.is_empty());
    }

    #[test]
    fn both_url_and_argv_is_refused() {
        let toml = r#"[[links]]
title = "Bad"
url = "https://example.com"
argv = ["ls"]
"#;
        let value = document(toml);
        let err = read_all(value.get("links").unwrap()).unwrap_err();

        assert!(err.contains("not both"), "{err}");
    }

    #[test]
    fn missing_target_is_refused() {
        let toml = r#"[[links]]
title = "No target"
"#;
        let value = document(toml);
        let err = read_all(value.get("links").unwrap()).unwrap_err();

        assert!(err.contains("needs a \"url\" or an \"argv\""), "{err}");
    }

    #[test]
    fn unknown_key_is_refused() {
        let toml = r#"[[links]]
title = "Bad"
url = "https://example.com"
icon = "star"
"#;
        let value = document(toml);
        let err = read_all(value.get("links").unwrap()).unwrap_err();

        assert!(err.contains("unknown key"), "{err}");
    }

    #[test]
    fn empty_argv_is_refused() {
        let toml = r#"[[links]]
title = "Bad"
argv = []
"#;
        let value = document(toml);
        let err = read_all(value.get("links").unwrap()).unwrap_err();

        assert!(err.contains("must not be empty"), "{err}");
    }

    // the provider finds by title, same Fit scale as every other provider
    #[test]
    fn find_filters_by_title() {
        let links = [
            link_url("GitHub", "https://github.com"),
            link_url("GitLab", "https://gitlab.com"),
            link_argv("Editor", &["code"]),
        ];

        let provider = Quicklinks(&links);
        let found = provider.find("git");

        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|a| a.title.starts_with("Git")));
    }

    #[test]
    fn empty_query_finds_all() {
        let links = [
            link_url("GitHub", "https://github.com"),
            link_argv("Editor", &["code"]),
        ];

        let provider = Quicklinks(&links);
        let found = provider.find("");

        assert_eq!(found.len(), 2);
    }

    #[test]
    fn empty_links_finds_nothing() {
        let provider = Quicklinks(&[]);

        assert!(provider.find("").is_empty());
        assert!(provider.find("anything").is_empty());
    }

    // a url target produces an Action::Browse; pressing it goes through the same xdg-open path
    #[test]
    fn url_target_produces_browse_action() {
        let links = [link_url("GitHub", "https://github.com")];
        let provider = Quicklinks(&links);
        let found = provider.find("");

        assert_eq!(found.len(), 1);
        assert!(matches!(&found[0].action, Action::Browse(url) if url == "https://github.com"));
    }

    // an argv target produces an Action::Quicklink
    #[test]
    fn argv_target_produces_quicklink_action() {
        let links = [link_argv("Editor", &["code", "--new-window"])];
        let provider = Quicklinks(&links);
        let found = provider.find("");

        assert_eq!(found.len(), 1);
        assert!(
            matches!(&found[0].action, Action::Quicklink(argv) if argv == &["code", "--new-window"])
        );
    }

    // a passing row appends the typed query as one final argv word, unchanged
    #[test]
    fn a_passing_row_appends_the_query_as_one_word() {
        let argv = [String::from("lookup")];

        assert_eq!(
            command(&argv, true, "two words $(x)"),
            ["lookup", "two words $(x)"]
        );
    }

    // a plain row keeps its words, and a passing row with nothing typed has no word to add
    #[test]
    fn command_leaves_a_plain_or_empty_query_alone() {
        let argv = [String::from("code")];

        assert_eq!(command(&argv, false, "two words"), ["code"]);
        assert_eq!(command(&argv, true, ""), ["code"]);
    }

    // with nothing typed a passing row is hidden rather than run without its argument
    #[test]
    fn a_passing_row_is_hidden_with_no_query() {
        let links = [script("Look up", &["lookup"])];
        let provider = Quicklinks(&links);

        assert!(provider.find("").is_empty());
    }

    // a typed query reaches a passing row whole, even when it does not fit its title
    #[test]
    fn a_typed_query_reaches_the_action_whole() {
        let links = [script("Look up", &["lookup"])];
        let provider = Quicklinks(&links);
        let found = provider.find("two words $(x)");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fit, Fit::Anywhere);
        assert!(matches!(&found[0].action, Action::Quicklink(argv)
                if argv == &["lookup", "two words $(x)"]));
    }

    // a query that fits a passing row's title ranks it as that fit
    #[test]
    fn a_fitting_query_keeps_its_fit() {
        let links = [script("Look up", &["lookup"])];
        let provider = Quicklinks(&links);
        let found = provider.find("look");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fit, Fit::Prefix);
    }

    // a plain argv keeps its words when a query is typed
    #[test]
    fn a_plain_argv_ignores_the_query() {
        let links = [link_argv("Editor", &["code"])];
        let provider = Quicklinks(&links);
        let found = provider.find("Editor");

        assert_eq!(found.len(), 1);
        assert!(matches!(&found[0].action, Action::Quicklink(argv) if argv == &["code"]));
    }

    #[test]
    fn passes_query_needs_an_argv() {
        let toml = r#"[[links]]
title = "Bad"
url = "https://example.com"
passes_query = true
"#;
        let value = document(toml);
        let err = read_all(value.get("links").unwrap()).unwrap_err();

        assert!(err.contains("needs an \"argv\""), "{err}");
    }

    #[test]
    fn parse_passing_row() {
        let toml = r#"[[links]]
title = "Look up"
argv = ["lookup"]
passes_query = true
"#;
        let value = document(toml);
        let links = read_all(value.get("links").unwrap()).unwrap();

        assert_eq!(links, [script("Look up", &["lookup"])]);
    }

    // value round-trips through read_all
    #[test]
    fn value_round_trips() {
        let links = vec![
            link_url("GitHub", "https://github.com"),
            link_argv("Editor", &["code", "--new-window"]),
            script("Look up", &["lookup"]),
        ];
        let val = value(&links);
        let parsed = read_all(&val).unwrap();

        assert_eq!(parsed, links);
    }
}
