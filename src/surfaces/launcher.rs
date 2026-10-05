//! The Launcher Surface (plan 7): a search over Amane's `Apps` and the apps it finds, best match
//! first. It opens only from IPC or a keybind, so it always holds the keyboard: typing searches,
//! the arrow keys move the selection, Enter starts the selected app and a click starts the one
//! clicked, and either closes the island. It says when the apps are still being found and when
//! none match.

use std::time::Instant;

use amane::{
    Apps, Center, Column, Cursor, DesktopApp, Image, Key, Padding, Parent, Rectangle, Row, Scroll,
    Service, Size, Stack, Start, Text, Widget, children,
};

use crate::island::geometry;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::theme;
use crate::view::{self, Icon};

const INSET: f32 = 20.0;

// the content's width, which every row fills
const WIDTH: f32 = geometry::EXPANDED_MAX.width - 2.0 * INSET;

// clear of the queued badge in the body's top right corner
const BADGE: f32 = 36.0;

const FIELD: f32 = 44.0;
const FIELD_INSET: f32 = 16.0;
const GAP: f32 = 12.0;

// what the rows scroll in, `ROWS` at a time
const LIST: f32 = geometry::EXPANDED_MAX.height - 2.0 * INSET - FIELD - GAP;

const ROWS: usize = 5;
const ROW_GAP: f32 = 6.0;
const ROW: f32 = (LIST - (ROWS - 1) as f32 * ROW_GAP) / ROWS as f32;
const ROW_INSET: f32 = 8.0;
const ROW_RADIUS: f32 = 12.0;

const ICON: f32 = 28.0;
const ICON_RADIUS: f32 = 7.0;
const ICON_GAP: f32 = 12.0;

// a pressable target never smaller than plan 7's 24 px
const TARGET: f32 = 24.0;

// the typed text's size, and the caret after it
const QUERY: f32 = 16.0;
const CARET: f32 = 1.5;

// pixels per wheel line
const WHEEL: f32 = 40.0;

/*
 * the query, the selection and the scroll for one visit of the Surface (`IslandService::visit`),
 * so every opening starts empty at the top. Written by input only, never by the view
 */
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Search {
    visit: u64,
    query: String,

    // in the apps the query finds
    selected: usize,

    // how far the rows scrolled, in pixels
    offset: f32,
}

impl Service for Search {
    fn new() -> Self {
        Search::default()
    }

    fn listen() {}
}

impl Search {
    // this visit's search; one kept from an earlier visit is over
    fn of(&self, visit: u64) -> Search {
        if self.visit == visit {
            self.clone()
        } else {
            Search {
                visit,
                ..Search::default()
            }
        }
    }

    /*
     * the search after a key and whether it starts the selected app, none when the key is not for
     * the Launcher; `count` is how many apps the query finds before the key
     */
    fn step(mut self, key: Key, count: usize) -> Option<(Search, bool)> {
        let last = count.saturating_sub(1);

        let selected = match key {
            Key::Character(letter) => return Some((self.typed(Some(letter)), false)),
            Key::Space => return Some((self.typed(Some(' ')), false)),
            Key::Backspace => return Some((self.typed(None), false)),
            Key::Enter => return Some((self, count > 0)),
            Key::Up => self.selected.saturating_sub(1),
            Key::Down => (self.selected + 1).min(last),
            Key::Home => 0,
            Key::End => last,
            _ => return None,
        };

        self.selected = selected;
        self.offset = reveal(self.offset.clamp(0.0, most(count)), selected);

        Some((self, false))
    }

    // a letter typed, or the last one erased; a new query finds new apps, so it starts at the top
    fn typed(mut self, letter: Option<char>) -> Search {
        match letter {
            Some(letter) => self.query.push(letter),
            None => {
                self.query.pop();
            }
        }

        self.selected = 0;
        self.offset = 0.0;

        self
    }

    // scrolled by the wheel, the selection kept on a row that shows whole
    fn wheel(mut self, lines: f32, count: usize) -> Search {
        let most = most(count);

        self.offset = (self.offset.clamp(0.0, most) + lines * WHEEL).clamp(0.0, most);

        let (first, last) = whole(self.offset, count);
        self.selected = self.selected.clamp(first, last);

        self
    }

    // what the view draws; a query no longer finding as many leaves the selection on the last one
    fn bounded(mut self, count: usize) -> Search {
        self.selected = self.selected.min(count.saturating_sub(1));
        self.offset = self.offset.clamp(0.0, most(count));

        self
    }
}

// how far `count` rows can scroll: none while they fit
fn most(count: usize) -> f32 {
    (content(count) - LIST).max(0.0)
}

fn content(count: usize) -> f32 {
    count as f32 * ROW + count.saturating_sub(1) as f32 * ROW_GAP
}

fn top(row: usize) -> f32 {
    row as f32 * (ROW + ROW_GAP)
}

// the least scroll from `offset` that shows `row` whole
fn reveal(offset: f32, row: usize) -> f32 {
    let top = top(row);

    offset.min(top).max(top + ROW - LIST)
}

// the first and last rows that show whole at `offset`
fn whole(offset: f32, count: usize) -> (usize, usize) {
    let step = ROW + ROW_GAP;
    let first = (offset / step).ceil() as usize;
    let last = ((offset + LIST + ROW_GAP) / step).floor() as usize;

    (
        first.min(count.saturating_sub(1)),
        last.saturating_sub(1).min(count.saturating_sub(1)),
    )
}

/*
 * how well an app answers the query, lower first, none when it does not: its name starting with
 * the query, a word in it starting so, the query in the name, every word of the query in the name,
 * then every word anywhere in its name, description or command
 */
fn rank(query: &str, name: &str, description: &str, exec: &str) -> Option<u8> {
    let name = name.to_lowercase();

    if name.starts_with(query) {
        return Some(0);
    }

    let word = name
        .match_indices(query)
        .any(|(at, _)| !name[..at].ends_with(char::is_alphanumeric));

    if word {
        return Some(1);
    }

    if name.contains(query) {
        return Some(2);
    }

    let terms = || query.split_whitespace();

    if terms().all(|term| name.contains(term)) {
        return Some(3);
    }

    let anywhere = format!("{name} {} {}", description, exec).to_lowercase();

    terms().all(|term| anywhere.contains(term)).then_some(4)
}

/*
 * the apps the query finds, best first and by name within a rank, since Amane sorts them by name;
 * a query of only spaces finds every app
 */
fn found(apps: &[DesktopApp], query: &str) -> Vec<DesktopApp> {
    let query = query.trim().to_lowercase();

    let mut ranked: Vec<(u8, &DesktopApp)> = apps
        .iter()
        .filter_map(|app| {
            let rank = rank(
                &query,
                app.name(),
                app.description().unwrap_or_default(),
                app.exec(),
            )?;

            Some((rank, app))
        })
        .collect();

    ranked.sort_by_key(|&(rank, _)| rank);

    ranked.into_iter().map(|(_, app)| app.clone()).collect()
}

// the view's own read of the visit, so this never reads IslandService again
pub fn surface(monitor: &str, visit: u64) -> Rectangle {
    let apps = Apps::read();
    let search = Search::read().of(visit);
    let found = found(apps.list(), &search.query);
    let search = search.bounded(found.len());

    let list: Box<dyn Widget> = if apps.list().is_empty() {
        // Amane scans every icon theme first, which takes a few seconds after Kanade starts
        Box::new(state("Finding apps", ""))
    } else if found.is_empty() {
        Box::new(state(
            "No matching apps",
            &format!("Nothing found for \u{201c}{}\u{201d}", search.query.trim()),
        ))
    } else {
        Box::new(list(monitor, &found, &search))
    };

    let shape = geometry::EXPANDED_MAX;

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .child(
            Column::new(vec![
                Box::new(field(&search.query)) as Box<dyn Widget>,
                list,
            ])
            .width(WIDTH)
            .gap(GAP),
        )
}

/*
 * a search icon, what is typed with the caret after it, or what to type faded; a query wider than
 * the field shows its end, where the typing is
 */
fn field(query: &str) -> Rectangle {
    let width = WIDTH - BADGE;
    let room = width - 2.0 * FIELD_INSET - TARGET - ICON_GAP - CARET;

    let caret = Rectangle::new()
        .width(CARET)
        .height(QUERY + 4.0)
        .fill(theme::FG);

    let typed: Box<dyn Widget> = if query.is_empty() {
        Box::new(
            Row::new(children![
                caret,
                Text::new("Search apps")
                    .size(QUERY)
                    .color(theme::MUTED)
                    .weight(500),
            ])
            .align(Center),
        )
    } else {
        Box::new(Row::new(children![tail(query, room), caret]).align(Center))
    };

    Rectangle::new()
        .width(width)
        .height(FIELD)
        .radius(FIELD / 2.0)
        .fill(theme::CARD)
        .padding(Padding {
            top: 0.0,
            right: FIELD_INSET,
            bottom: 0.0,
            left: FIELD_INSET,
        })
        .align_child(Start, Center)
        .child(
            Row::new(vec![
                Box::new(Icon::Search.on(TARGET, theme::MUTED, theme::CARD)) as Box<dyn Widget>,
                typed,
            ])
            .gap(ICON_GAP)
            .align(Center),
        )
}

// the end of `query` that fits in `width`, an ellipsis before it when the start is cut
fn tail(query: &str, width: f32) -> Text {
    let text = |shown: &str| Text::new(shown).size(QUERY).color(theme::FG).weight(500);
    let fits = |text: &Text| matches!(text.width(), Size::Fixed(natural) if natural <= width);

    let whole = text(query);

    if fits(&whole) {
        return whole;
    }

    let letters: Vec<char> = query.chars().collect();

    (1..letters.len())
        .map(|cut| {
            text(&format!(
                "\u{2026}{}",
                letters[cut..].iter().collect::<String>()
            ))
        })
        .find(fits)
        .unwrap_or_else(|| text("\u{2026}"))
}

// what it means, in the middle of where the rows go
fn state(title: &str, detail: &str) -> Rectangle {
    let mut lines = children![
        Icon::Search.draw(28.0),
        Text::new(title).size(14.0).color(theme::FG).weight(600),
    ];

    if !detail.is_empty() {
        let text = Text::new(detail).size(12.0).color(theme::MUTED).weight(500);

        // centred while it fits, elided only when a long query does not, since elided text fills
        // its width
        lines.push(Box::new(match text.width() {
            Size::Fixed(natural) if natural <= WIDTH => text,
            _ => text.elide(),
        }));
    }

    Rectangle::new()
        .width(WIDTH)
        .height(LIST)
        .align_child(Center, Center)
        .child(Column::new(lines).gap(6.0).align(Center))
}

/*
 * the rows scrolled `offset` down, clipped to the list; a thumb in the inset says where while they
 * do not all fit
 */
fn list(monitor: &str, found: &[DesktopApp], search: &Search) -> Stack {
    let column = Column::new(
        found
            .iter()
            .enumerate()
            .map(|(index, app)| {
                Box::new(row(monitor, app, index == search.selected)) as Box<dyn Widget>
            })
            .collect(),
    )
    .width(WIDTH)
    .gap(ROW_GAP);

    let viewport = Rectangle::new()
        .width(WIDTH)
        .height(LIST)
        .clip()
        .align_child(Start, Start)
        .on_scroll(|Scroll { y, .. }| wheel(y))
        .child(
            Rectangle::new()
                .width(WIDTH)
                .height(content(found.len()))
                .align_child(Start, Start)
                .translate(0.0, -search.offset)
                .child(column),
        );

    let mut layers = children![viewport];

    let most = most(found.len());

    if most > 0.0 {
        let length = (LIST * LIST / content(found.len())).max(TARGET);
        let at = (LIST - length) * search.offset / most;

        layers.push(Box::new(
            Rectangle::new()
                .width(3.0)
                .height(length)
                .radius(1.5)
                .fill(theme::DOT)
                .translate(WIDTH + 7.0, at),
        ));
    }

    Stack::new(layers).width(WIDTH).height(LIST)
}

// the app's icon, name and what it is; the selected one stands out, and pressing one starts it
fn row(monitor: &str, app: &DesktopApp, selected: bool) -> Rectangle {
    let mut lines = children![
        Text::new(app.name())
            .size(14.0)
            .color(theme::FG)
            .weight(600)
            .elide()
    ];

    if let Some(description) = app.description().filter(|text| !text.is_empty()) {
        lines.push(Box::new(
            Text::new(description)
                .size(12.0)
                .color(theme::MUTED)
                .weight(500)
                .elide(),
        ));
    }

    let row = Rectangle::new()
        .width(WIDTH)
        .height(ROW)
        .radius(ROW_RADIUS)
        .padding(Padding {
            top: 0.0,
            right: ROW_INSET + 4.0,
            bottom: 0.0,
            left: ROW_INSET,
        })
        .align_child(Start, Center)
        .cursor(Cursor::Pointer)
        .child(
            Row::new(vec![
                icon(app),
                Box::new(Column::new(lines).width(Parent).gap(1.0)),
            ])
            .width(Parent)
            .gap(ICON_GAP)
            .align(Center),
        );

    let row = if selected { row.fill(theme::CARD) } else { row };

    let monitor = monitor.to_owned();
    let app = app.clone();

    row.on_click(super::on_left(move || {
        launch(&monitor, &app);
    }))
}

/*
 * the theme's icon, or the name's initial on the quiet tile for an app without one; nothing while
 * it decodes, so no row shifts
 */
fn icon(app: &DesktopApp) -> Box<dyn Widget> {
    let Some(path) = app.icon_path() else {
        let initial = app
            .name()
            .chars()
            .next()
            .map_or_else(String::new, |initial| initial.to_uppercase().collect());

        return Box::new(view::tile(None, &initial, ICON, ICON_RADIUS));
    };

    // decoded at twice its size, crisp at scale 2
    let pixels = (ICON * 2.0) as u32;

    Box::new(
        Rectangle::new()
            .width(ICON)
            .height(ICON)
            .fill(Image::contain(path).thumbnail(pixels, pixels)),
    )
}

/*
 * a key while this island shows the Launcher; false for one it does not use, which the window's
 * own keys then get. A key it uses keeps the held island open for another hold
 */
pub fn key(monitor: &str, key: Key) -> bool {
    let visit = {
        let island = IslandService::read();

        if island.presentation(monitor) != Presentation::Expanded(Surface::Launcher) {
            return false;
        }

        island.visit()
    };

    let search = Search::read().of(visit);
    let found = found(Apps::read().list(), &search.query);

    let Some((search, launches)) = search.bounded(found.len()).step(key, found.len()) else {
        return false;
    };

    let selected = found.get(search.selected).cloned();

    set(search);

    IslandService::write().attend(monitor, Instant::now());

    if launches && let Some(app) = selected {
        launch(monitor, &app);
    }

    true
}

// what starts is in the way of nothing, so the island closes
fn launch(monitor: &str, app: &DesktopApp) {
    app.launch();
    view::collapse(monitor);
}

// down scrolls further down the list
fn wheel(lines: f32) {
    let visit = IslandService::read().visit();

    let search = Search::read().of(visit);
    let count = found(Apps::read().list(), &search.query).len();

    set(search.bounded(count).wheel(lines, count));
}

// a write wakes the window even when nothing changed, so only write a real change
fn set(search: Search) {
    if *Search::read() != search {
        *Search::write() = search;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // what the query finds among (name, description, exec), best first, as `found` orders them
    fn find<'a>(query: &str, apps: &[(&'a str, &str, &str)]) -> Vec<&'a str> {
        let query = query.trim().to_lowercase();

        let mut ranked: Vec<(u8, &str)> = apps
            .iter()
            .filter_map(|&(name, description, exec)| {
                Some((rank(&query, name, description, exec)?, name))
            })
            .collect();
        ranked.sort_by_key(|&(rank, _)| rank);

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

    #[test]
    fn typing_edits_the_query_and_starts_at_the_top() {
        let search = Search {
            selected: 3,
            offset: 50.0,
            ..Search::default()
        };

        let (search, launches) = search.step(Key::Character('k'), 10).unwrap();
        assert_eq!(
            (search.query.as_str(), search.selected, search.offset),
            ("k", 0, 0.0)
        );
        assert!(!launches);

        let (search, _) = search.step(Key::Space, 10).unwrap();
        let (search, _) = search.step(Key::Backspace, 10).unwrap();
        let (search, _) = search.step(Key::Backspace, 10).unwrap();
        assert_eq!(search.query, "");

        // nothing left to erase is still the Launcher's key, not the window's
        assert!(search.step(Key::Backspace, 10).is_some());
    }

    #[test]
    fn arrows_move_the_selection_within_the_apps() {
        let at = |search: &Search| search.selected;
        let search = Search::default();

        let (search, _) = search.step(Key::Up, 10).unwrap();
        assert_eq!(at(&search), 0);

        let (search, _) = search.step(Key::Down, 10).unwrap();
        assert_eq!(at(&search), 1);

        let (search, _) = search.step(Key::End, 10).unwrap();
        assert_eq!(at(&search), 9);

        let (search, _) = search.clone().step(Key::Down, 10).unwrap();
        assert_eq!(at(&search), 9);

        let (search, _) = search.step(Key::Home, 10).unwrap();
        assert_eq!(at(&search), 0);

        let (search, _) = Search::default().step(Key::Down, 0).unwrap();
        assert_eq!(at(&search), 0);

        for key in [Key::Left, Key::Right, Key::Tab, Key::Escape, Key::Other] {
            assert_eq!(Search::default().step(key, 10), None, "{key:?}");
        }
    }

    #[test]
    fn enter_starts_the_selected_app_only_when_there_is_one() {
        assert!(Search::default().step(Key::Enter, 3).unwrap().1);
        assert!(!Search::default().step(Key::Enter, 0).unwrap().1);
    }

    #[test]
    fn the_selection_scrolls_into_view_and_no_further() {
        let (search, _) = Search::default().step(Key::Down, 20).unwrap();
        assert_eq!(search.offset, 0.0);

        // one past the last whole row: its bottom at the list's bottom
        let mut search = Search::default();
        for _ in 0..ROWS {
            search = search.step(Key::Down, 20).unwrap().0;
        }
        assert_eq!(search.offset, top(ROWS) + ROW - LIST);

        let (search, _) = search.step(Key::End, 20).unwrap();
        assert_eq!(search.offset, most(20));

        let (search, _) = search.step(Key::Home, 20).unwrap();
        assert_eq!(search.offset, 0.0);
    }

    #[test]
    fn five_rows_fill_the_list() {
        assert_eq!(content(ROWS), LIST);
        assert_eq!(most(ROWS), 0.0);
        const { assert!(ROW >= 40.0) };
    }

    #[test]
    fn the_wheel_keeps_the_selection_on_a_whole_row() {
        let search = Search::default().wheel(3.0, 20);
        let (first, last) = whole(search.offset, 20);

        assert_eq!(search.offset, 3.0 * WHEEL);
        assert_eq!(search.selected, first);
        assert!(top(first) >= search.offset);
        assert!(top(last) + ROW <= search.offset + LIST);
        assert!(top(last + 1) + ROW > search.offset + LIST);

        // far past the end stops there, the selection dragged to the first whole row
        let end = Search::default().wheel(100.0, 20);
        assert_eq!((end.offset, end.selected), (most(20), 20 - ROWS));
        assert_eq!(whole(end.offset, 20), (20 - ROWS, 19));

        // a selection still in view stays
        let kept = Search {
            selected: 17,
            ..end.clone()
        }
        .wheel(-0.5, 20);
        assert_eq!(kept.selected, 17);

        let fits = Search::default().wheel(3.0, 3);
        assert_eq!((fits.offset, fits.selected), (0.0, 0));
    }

    #[test]
    fn fewer_apps_found_leave_the_selection_on_the_last() {
        let search = Search {
            selected: 7,
            offset: 200.0,
            ..Search::default()
        };

        let bounded = search.bounded(2);
        assert_eq!((bounded.selected, bounded.offset), (1, 0.0));
        assert_eq!(Search::default().bounded(0).selected, 0);
    }

    #[test]
    fn a_new_visit_starts_empty() {
        let kept = Search {
            visit: 1,
            query: String::from("fire"),
            selected: 2,
            offset: 40.0,
        };

        assert_eq!(kept.of(1), kept);
        assert_eq!(
            kept.of(2),
            Search {
                visit: 2,
                ..Search::default()
            }
        );
    }
}
