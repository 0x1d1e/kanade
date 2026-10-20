/*
 * the web, as the Launcher's last resort (ADR 0130): a typed query nothing answers offers to search
 * Google for it, and to open it in the default browser. It is the one answer that sends what was
 * typed off the machine, so it is offered only when nothing else answers, each row says where the
 * text goes, and neither the query nor the address is ever logged or kept (ADR 0019). The browser is
 * the default one, through `xdg-open` alone, and the search's address is fixed in the binary
 */

use std::process::{Command, Stdio};
use std::thread;

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

use crate::icon::Icon;
use crate::sources::wake;

use super::provider::{Action, Answer, Fit, Mark};

// what opens the default browser, and the only way this module opens anything
pub const OPEN: &str = "xdg-open";

// where a search goes: fixed in the binary, never a configurable endpoint
const GOOGLE: &str = "https://www.google.com/search?q=";

// what a row is called while the program it needs is missing
const MISSING: &str = "Needs xdg-open";

// the fewest letters the web is offered for: one is a stray keystroke, not a search
const FROM: usize = 2;

// whether `query` is one the web is offered for at all: a query of one letter is a stray keystroke,
// and `:`, `>` and `@` search something else
pub fn asked(query: &str) -> bool {
    let query = query.trim();

    query.chars().count() >= FROM
        && !query.starts_with(super::emoji::PREFIX)
        && !query.starts_with(super::history::PREFIX)
        && !query.starts_with(super::destinations::WALLPAPER)
}

// whether the browser can be opened: without `xdg-open` neither row can act, so neither is offered
pub fn openable() -> bool {
    wake::found(OPEN)
}

/*
 * the one row that stands in the two rows' place while `xdg-open` is missing: what a row's second
 * line says of a program it needs, as Controls' tiles do (`Needs <program>`), with why the query is
 * nowhere. A query the web is not offered for at all gets none
 */
pub fn refused(query: &str, openable: impl FnOnce() -> bool) -> Option<(&'static str, String)> {
    if !asked(query) || openable() {
        return None;
    }

    Some((
        MISSING,
        format!(
            "Nothing found for \u{201c}{}\u{201d}, and {OPEN} is missing, so the browser cannot open it",
            query.trim()
        ),
    ))
}

/*
 * the two rows a query nothing answers offers: the search first, then the address. The query goes
 * to Google escaped, so nothing in it reads as a second parameter of the search; the browser row
 * opens it as an address, with `https` where it names no scheme, so it never begins with a dash
 * `xdg-open` would read as an option of its own
 */
pub fn offers(query: &str, openable: impl FnOnce() -> bool) -> Vec<Answer> {
    let query = query.trim();

    if !asked(query) || !openable() {
        return Vec::new();
    }

    let search = format!("{GOOGLE}{}", utf8_percent_encode(query, NON_ALPHANUMERIC));

    vec![
        Answer {
            fit: Fit::Answer,
            title: format!("Search Google for \u{201c}{query}\u{201d}"),
            detail: String::from("Sends it to Google in the default browser"),
            mark: Mark::Icon(Icon::Search),
            action: Action::Browse(search),
        },
        Answer {
            fit: Fit::Answer,
            title: format!("Open \u{201c}{query}\u{201d} in the browser"),
            detail: String::from("The default browser opens it as an address"),
            mark: Mark::Icon(Icon::Forward),
            action: Action::Browse(address(query)),
        },
    ]
}

// the query as an address: one that names its scheme as it is, anything else as `https`
fn address(query: &str) -> String {
    match query.split_once("://") {
        Some((scheme, _))
            if !scheme.is_empty()
                && scheme.chars().all(|letter| {
                    letter.is_ascii_alphanumeric() || matches!(letter, '+' | '-' | '.')
                }) =>
        {
            query.to_owned()
        }
        _ => format!("https://{query}"),
    }
}

/*
 * opens `address` in the default browser. It goes as one argument of `xdg-open`, by argv with no
 * shell, so no word of the query is ever a command (ADR 0055, 0112). Asked, not waited on: an
 * opener may live as long as the browser it started, so the child is reaped on a thread of its own
 * and only the spawn decides whether it opened. The reason a failure gives never names the address
 */
pub fn browse(address: &str) -> Result<(), String> {
    let child = command(address)
        .spawn()
        .map_err(|error| format!("{OPEN}: {error}"))?;

    thread::Builder::new()
        .name("launcher-web".into())
        .spawn(move || match child.wait_with_output() {
            Ok(output) if !output.status.success() => {
                eprintln!("launcher: {OPEN} failed ({})", output.status)
            }
            Ok(_) => {}
            Err(error) => eprintln!("launcher: {OPEN}: {error}"),
        })
        .map(drop)
        .map_err(|error| format!("{OPEN}: {error}"))
}

// the one way this module runs anything: the opener, with the address as its only argument
fn command(address: &str) -> Command {
    let mut command = Command::new(OPEN);
    command
        .arg(address)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    command
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addresses(answers: &[Answer]) -> Vec<&str> {
        answers
            .iter()
            .map(|answer| match &answer.action {
                Action::Browse(address) => address.as_str(),
                other => panic!("a web row browses, not {other:?}"),
            })
            .collect()
    }

    // the search's address is Google's, fixed in the binary, with the query escaped so nothing in
    // it can add a parameter of its own
    #[test]
    fn the_search_sends_the_query_escaped_to_google() {
        let found = offers("rust & borrow", || true);

        assert_eq!(found.len(), 2);
        assert_eq!(
            found[0].title,
            "Search Google for \u{201c}rust & borrow\u{201d}"
        );
        assert_eq!(
            addresses(&found)[0],
            "https://www.google.com/search?q=rust%20%26%20borrow"
        );
    }

    // the browser row opens what was typed as an address, https where it names no scheme, so it
    // never begins with a dash the opener would read as one of its own options
    #[test]
    fn the_browser_row_opens_an_address() {
        assert_eq!(address("example.com"), "https://example.com");
        assert_eq!(
            address("https://example.com/a b"),
            "https://example.com/a b"
        );
        assert_eq!(
            address("mailto:a_20261008T080000Z@google.com"),
            "https://mailto:a_20261008T080000Z@google.com"
        );
        assert_eq!(address("--help"), "https://--help");
    }

    // whatever was typed, the opener gets it as one argument of its own and no shell sees it, so no
    // word of it can be a command (ADR 0055, 0112)
    #[test]
    fn the_opener_gets_the_query_as_one_argument() {
        let command = command(&address("; rm -rf / && echo"));
        let argv: Vec<_> = command.get_args().collect();

        assert_eq!(command.get_program(), OPEN);
        assert_eq!(argv, ["https://; rm -rf / && echo"]);
    }

    // a query of one letter, and the prefixes that search something else, send nothing anywhere
    #[test]
    fn only_a_real_query_is_offered() {
        assert!(!asked("x"));
        assert!(asked(" ab "));
        assert_eq!(offers("x", || true), Vec::new());
        assert!(
            refused("x", || false).is_none(),
            "nothing is refused for a query no row would act on"
        );

        for query in [":smile", ">invoice", "@wall"] {
            assert!(!asked(query), "{query}");
        }
    }

    // without `xdg-open` the two rows give way to one that says why, so a query nothing answers is
    // never a blank list; with it, there is nothing to refuse
    #[test]
    fn a_missing_opener_is_said_rather_than_offered() {
        assert!(refused("nowhere", || true).is_none());
        assert_eq!(offers("nowhere", || false), Vec::new());

        let (title, detail) =
            refused("nowhere", || false).expect("a refusal while the opener is missing");

        assert_eq!(title, MISSING);
        assert!(detail.contains("xdg-open"), "{detail}");
        assert!(
            detail.contains("nowhere"),
            "the row says what it found nothing for"
        );
    }
}
