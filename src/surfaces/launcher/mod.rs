//! The Launcher Surface (plan 7): a search over its providers (#138), the apps `sources::apps`
//! lists, the calculator, emoji and, with the `wallpaper` Module on, wallpapers, best answer first.
//! It opens only from IPC or a keybind, so it always holds the keyboard: typing searches, the arrow
//! keys move the selection, Enter presses the selected answer and a click the one clicked. An app
//! starts and the island closes; a value or an emoji is copied, or a wallpaper set, and the island
//! closes once it is, or says it was not. It says when the apps are still being found and when
//! nothing matches.

use std::sync::Arc;
use std::thread;
use std::time::Instant;

use amane::{
    Center, Column, Cursor, Image, Key, Padding, Parent, Rectangle, Row, Scroll, Service, Size,
    Stack, Start, Text, Widget, children,
};

use crate::icon::Icon;
use crate::island::geometry;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::sources::apps::{App, Apps};
use crate::sources::wallpaper::Unset;
use crate::sources::{self, clipboard};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, radius};
use crate::{modules, view};

use super::Ring;

mod apps;
mod calculator;
mod emoji;
mod provider;
mod wallpaper;

use apps::Apps as AppsProvider;
use calculator::Calculator;
use emoji::Emoji;
use provider::{Action, Answer, Mark};
use wallpaper::Wallpapers;

// the content's width, which every row fills
const WIDTH: f32 = geometry::EXPANDED_MAX.width - 2.0 * INSET;

const FIELD: f32 = 44.0;
const FIELD_INSET: f32 = 16.0;
const GAP: f32 = 12.0;

// what the rows scroll in, `ROWS` at a time
const LIST: f32 = geometry::EXPANDED_MAX.height - 2.0 * INSET - FIELD - GAP;

const ROWS: usize = 5;
const ROW_GAP: f32 = 6.0;
const ROW: f32 = (LIST - (ROWS - 1) as f32 * ROW_GAP) / ROWS as f32;
const ROW_INSET: f32 = 8.0;

const ICON: f32 = 28.0;
const ICON_GAP: f32 = 12.0;

// the typed text's size, and the caret after it
const QUERY: f32 = theme::text::TITLE;
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

    // in the answers the query finds
    selected: usize,

    // how far the rows scrolled, in pixels
    offset: f32,

    pressing: Pressing,
}

/*
 * a copy or a wallpaper, from its press until it is done or failed, of the answer pressed; so the
 * row of that answer says it failed, whichever is selected by then
 */
#[derive(Debug, Clone, Default, PartialEq)]
enum Pressing {
    #[default]
    Idle,
    Waiting(Action),

    // with why
    Failed(Action, String),
}

impl Service for Search {
    fn new() -> Self {
        Search::default()
    }

    fn listen() {}
}

impl Search {
    // what its row says when pressing `action` failed, none when it did not
    fn failed(&self, action: &Action) -> Option<String> {
        let Pressing::Failed(failed, why) = &self.pressing else {
            return None;
        };

        if failed != action {
            return None;
        }

        match action {
            Action::Copy(_) => Some(String::from("Not copied")),
            Action::Wallpaper(_) => Some(format!("Not set: {why}")),
            Action::Launch(_) => None,
        }
    }

    /*
     * pressing `action` in `visit` failed, for `why`; nothing once that visit or that press is
     * over, so a stale worker never touches a later visit's Search
     */
    fn press_failed(&mut self, visit: u64, action: Action, why: String) {
        if self.visit == visit
            && matches!(&self.pressing, Pressing::Waiting(waiting) if *waiting == action)
        {
            self.pressing = Pressing::Failed(action, why);
        }
    }

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
     * the search after a key and whether it presses the selected answer, none when the key is not
     * for the Launcher; `count` is how many answers the query finds before the key
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

    // a letter typed, or the last one erased; a new query finds new answers, so it starts at the top
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

/*
 * the rows any part of shows at `offset`, as a range; only these are built, so a morph doesn't
 * lay out and draw every answer each frame (#36)
 */
fn shown(offset: f32, count: usize) -> (usize, usize) {
    let step = ROW + ROW_GAP;
    let first = (offset / step).floor() as usize;
    let end = ((offset + LIST) / step).ceil() as usize;

    (first.min(count), end.min(count))
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

// whether the query searches wallpapers, which it does only with their Module on
fn wallpapers(query: &str) -> bool {
    query.trim().starts_with(wallpaper::PREFIX) && modules::on("wallpaper")
}

// what the providers find for the query, best first
fn found(apps: &[App], query: &str) -> Vec<Answer> {
    // listed only for a wallpaper search, so no other query stats the directory
    let (images, current) = if wallpapers(query) {
        (sources::wallpaper::images(), sources::wallpaper::current())
    } else {
        (Arc::default(), None)
    };

    let wallpapers = Wallpapers {
        images: &images,
        current: current.as_deref(),
    };

    provider::ranked(
        &[
            &Calculator,
            &AppsProvider(apps, modules::on("wallpaper")),
            &Emoji,
            &wallpapers,
        ],
        query,
    )
}

// the view's own read of the visit, so this never reads IslandService again
pub fn surface(monitor: &str, visit: u64) -> Rectangle {
    let apps = Apps::read();
    let search = Search::read().of(visit);
    let found = found(apps.list(), &search.query);
    let search = search.bounded(found.len());

    let query = search.query.trim();
    let emoji = query.starts_with(emoji::PREFIX);
    let wallpapers = wallpapers(query);

    let list: Box<dyn Widget> = if !found.is_empty() {
        Box::new(list(monitor, &found, &search))
    } else if wallpapers && sources::wallpaper::images().is_empty() {
        Box::new(state(
            "No wallpapers",
            &format!("Add images to {}", directory()),
        ))
    } else if !apps.found() && !emoji && !wallpapers {
        // read once the Module starts, so only a Launcher opened at once waits
        Box::new(state("Finding apps", ""))
    } else {
        Box::new(state(
            if emoji {
                "No matching emoji"
            } else if wallpapers {
                "No matching wallpapers"
            } else {
                "No matching apps"
            },
            &format!("Nothing found for \u{201c}{query}\u{201d}"),
        ))
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

// where the wallpapers are looked for, the home directory as `~`
fn directory() -> String {
    let Some(directory) = sources::wallpaper::directory() else {
        return String::from("the wallpaper directory");
    };

    match std::env::var_os("HOME")
        .and_then(|home| directory.strip_prefix(home).ok().map(ToOwned::to_owned))
    {
        Some(rest) => format!("~/{}", rest.display()),
        None => directory.display().to_string(),
    }
}

/*
 * a search icon, what is typed with the caret after it, or what to type faded; a query wider than
 * the field shows its end, where the typing is
 */
fn field(query: &str) -> Rectangle {
    let room = WIDTH - 2.0 * FIELD_INSET - TARGET - ICON_GAP - CARET;

    let caret = Rectangle::new()
        .width(CARET)
        .height(QUERY + 4.0)
        .fill(theme::ISLAND.on_surface);

    let typed: Box<dyn Widget> = if query.is_empty() {
        Box::new(
            Row::new(children![
                caret,
                Text::new(placeholder())
                    .size(QUERY)
                    .color(theme::ISLAND.on_surface_variant)
                    .weight(theme::text::MEDIUM),
            ])
            .align(Center),
        )
    } else {
        Box::new(Row::new(children![tail(query, room), caret]).align(Center))
    };

    Rectangle::new()
        .width(WIDTH)
        .height(FIELD)
        .radius(FIELD / 2.0)
        .fill(theme::ISLAND.surface_container)
        .padding(Padding {
            top: 0.0,
            right: FIELD_INSET,
            bottom: 0.0,
            left: FIELD_INSET,
        })
        .align_child(Start, Center)
        .child(
            Row::new(vec![
                Box::new(Icon::Search.on(TARGET, theme::ISLAND.on_surface_variant))
                    as Box<dyn Widget>,
                typed,
            ])
            .gap(ICON_GAP)
            .align(Center),
        )
}

// what to type, wallpapers only with their Module on
fn placeholder() -> &'static str {
    if modules::on("wallpaper") {
        "Search apps, 2+2, :emoji or @wallpaper"
    } else {
        "Search apps, 2+2 or :emoji"
    }
}

// the end of `query` that fits in `width`, an ellipsis before it when the start is cut
fn tail(query: &str, width: f32) -> Text {
    let text = |shown: &str| {
        Text::new(shown)
            .size(QUERY)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::MEDIUM)
    };
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
        Text::new(title)
            .size(theme::text::BODY)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD),
    ];

    if !detail.is_empty() {
        let text = Text::new(detail)
            .size(theme::text::LABEL_SMALL)
            .color(theme::ISLAND.on_surface_variant)
            .weight(theme::text::MEDIUM);

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
fn list(monitor: &str, found: &[Answer], search: &Search) -> Stack {
    let (first, end) = shown(search.offset, found.len());

    let column = Column::new(
        found[first..end]
            .iter()
            .zip(first..)
            .map(|(answer, index)| {
                let selected = index == search.selected;
                let failed = search.failed(&answer.action);

                Box::new(row(
                    monitor,
                    search.visit,
                    answer,
                    selected,
                    failed.as_deref(),
                )) as Box<dyn Widget>
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
                .height(content(end - first))
                .align_child(Start, Start)
                .translate(0.0, top(first) - search.offset)
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
                .radius(radius::HAIRLINE)
                .fill(theme::ISLAND.surface_container_high)
                .translate(WIDTH + 7.0, at),
        ));
    }

    Stack::new(layers).width(WIDTH).height(LIST)
}

/*
 * the answer's mark, title and what it is, or that pressing it failed; the selected one stands
 * out, and pressing one does what it does
 */
fn row(
    monitor: &str,
    visit: u64,
    answer: &Answer,
    selected: bool,
    failed: Option<&str>,
) -> Rectangle {
    let mut lines = children![
        Text::new(&answer.title)
            .size(theme::text::BODY)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD)
            .elide()
    ];

    let detail = failed.unwrap_or(&answer.detail);

    if !detail.is_empty() {
        lines.push(Box::new(
            Text::new(detail)
                .size(theme::text::LABEL_SMALL)
                .color(theme::ISLAND.on_surface_variant)
                .weight(theme::text::MEDIUM)
                .elide(),
        ));
    }

    let row = Rectangle::new()
        .width(WIDTH)
        .height(ROW)
        .radius(radius::ROW)
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
                mark(&answer.mark),
                Box::new(Column::new(lines).width(Parent).gap(1.0)),
            ])
            .width(Parent)
            .gap(ICON_GAP)
            .align(Center),
        );

    let row = if selected {
        row.fill(theme::ISLAND.surface_container)
    } else {
        row
    };
    let row = row.border_if(selected);

    let monitor = monitor.to_owned();
    let action = answer.action.clone();

    row.on_click(super::on_left(move || {
        press(&monitor, visit, action.clone());
    }))
}

/*
 * an app's icon, a wallpaper, a letter or sign on the quiet tile, or an emoji; nothing while an
 * image decodes, so no row shifts
 */
fn mark(mark: &Mark) -> Box<dyn Widget> {
    // decoded at twice its size, crisp at scale 2
    let pixels = (ICON * 2.0) as u32;

    match mark {
        Mark::Picture(path) => Box::new(
            Rectangle::new()
                .width(ICON)
                .height(ICON)
                .fill(Image::contain(path).thumbnail(pixels, pixels)),
        ),
        Mark::Photo(path) => Box::new(
            Rectangle::new()
                .width(ICON)
                .height(ICON)
                .radius(radius::ICON)
                .fill(Image::cover(path).thumbnail(pixels, pixels)),
        ),
        Mark::Tile(sign) => Box::new(view::tile(None, sign, ICON, radius::ICON, &theme::ISLAND)),
        Mark::Glyph(glyph) => Box::new(
            Rectangle::new()
                .width(ICON)
                .height(ICON)
                .align_child(Center, Center)
                .child(Text::new(*glyph).size(ICON * 0.8)),
        ),
    }
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

    let Some((mut search, presses)) = search.bounded(found.len()).step(key, found.len()) else {
        return false;
    };

    let selected = found
        .get(search.selected)
        .map(|answer| answer.action.clone());

    // a failed press is said until the next key
    if matches!(search.pressing, Pressing::Failed(..)) {
        search.pressing = Pressing::Idle;
    }

    set(search);

    IslandService::write().attend(monitor, Instant::now());

    if presses && let Some(action) = selected {
        press(monitor, visit, action);
    }

    true
}

/*
 * what starts, is copied or set is in the way of nothing, so the island closes; a copy or a
 * wallpaper once it is done. One of those at a time
 */
fn press(monitor: &str, visit: u64, action: Action) {
    if let Action::Launch(launch) = &action {
        let launch = launch.clone();

        // off the view thread, as niri's or the bus's answer may take its patience
        thread::spawn(move || {
            if let Err(error) = launch.run() {
                eprintln!("launcher: {launch:?}: {error}");
            }
        });

        view::collapse(monitor);
        return;
    }

    let search = Search::read().of(visit);

    if matches!(search.pressing, Pressing::Waiting(_)) {
        return;
    }

    set(Search {
        pressing: Pressing::Waiting(action.clone()),
        ..search
    });

    let done = {
        let (monitor, action) = (monitor.to_owned(), action.clone());
        move |done| pressed(&monitor, visit, action, done)
    };

    let started = match &action {
        Action::Copy(text) => copy(text.clone(), done),
        Action::Wallpaper(path) => sources::wallpaper::set(
            path.clone(),
            Some(Box::new(move |set: Result<(), Unset>| {
                done(set.map_err(|unset| String::from(unset.brief())))
            })),
        )
        .map(drop)
        .map_err(|unset| String::from(unset.brief())),
        Action::Launch(_) => Ok(()),
    };

    if let Err(why) = started {
        pressed(monitor, visit, action, Err(why));
    }
}

// puts `text` on the clipboard off the draw thread, then tells `done` how it went
fn copy(
    text: String,
    done: impl FnOnce(Result<(), String>) + Send + 'static,
) -> Result<(), String> {
    thread::Builder::new()
        .name("launcher-copy".into())
        .spawn(move || {
            let copied = clipboard::put(&text).map_err(|error| error.to_string());

            if let Err(why) = &copied {
                eprintln!("kanade: cannot copy a Launcher answer ({why})");
            }

            done(copied);
        })
        .map(drop)
        .map_err(|error| {
            eprintln!("kanade: cannot copy a Launcher answer ({error})");
            error.to_string()
        })
}

/*
 * how pressing `action` in `visit` went: closes the island, or says it failed. Nothing once that
 * visit is over, so it never closes another
 */
fn pressed(monitor: &str, visit: u64, action: Action, done: Result<(), String>) {
    match done {
        Ok(()) => IslandService::write().finish(monitor, visit, Surface::Launcher, Instant::now()),

        // checked and written under one guard, as a later visit may be writing its own Search
        Err(why) => Search::write().press_failed(visit, action, why),
    }
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
    use std::path::PathBuf;

    use super::*;

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
    fn only_the_rows_in_the_list_are_built() {
        let step = ROW + ROW_GAP;

        assert_eq!(shown(0.0, 200), (0, ROWS));
        assert_eq!(shown(0.0, 3), (0, 3));
        assert_eq!(shown(0.0, 0), (0, 0));

        // a row cut at either edge still shows
        assert_eq!(shown(step * 0.5, 200), (0, ROWS + 1));
        assert_eq!(shown(step * 2.0, 200), (2, ROWS + 2));
        assert_eq!(shown(most(7), 7), (2, 7));
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
    fn a_failed_press_marks_the_answer_pressed_not_the_selection() {
        let copy = Action::Copy(String::from("4"));
        let image = Action::Wallpaper(PathBuf::from("/w/sea.png"));

        let search = Search {
            selected: 0,
            pressing: Pressing::Failed(copy.clone(), String::from("no wl-copy")),
            ..Search::default()
        };

        assert_eq!(search.failed(&copy).as_deref(), Some("Not copied"));
        assert_eq!(search.failed(&Action::Copy(String::from("5"))), None);
        assert_eq!(search.failed(&image), None);

        let search = Search {
            pressing: Pressing::Failed(image.clone(), String::from("awww-daemon is not running")),
            ..Search::default()
        };
        assert_eq!(
            search.failed(&image).as_deref(),
            Some("Not set: awww-daemon is not running")
        );

        let waiting = Search {
            pressing: Pressing::Waiting(copy.clone()),
            ..Search::default()
        };
        assert_eq!(waiting.failed(&copy), None);
    }

    #[test]
    fn a_stale_failed_press_leaves_a_later_visit_alone() {
        let (four, five) = (
            Action::Copy(String::from("4")),
            Action::Copy(String::from("5")),
        );
        let later = Search {
            visit: 2,
            query: String::from("fire"),
            ..Search::default()
        };

        let mut search = later.clone();
        search.press_failed(1, four.clone(), String::new());
        assert_eq!(search, later);

        let mut search = Search {
            pressing: Pressing::Waiting(four.clone()),
            ..later.clone()
        };
        search.press_failed(2, five, String::new());
        assert_eq!(
            search.pressing,
            Pressing::Waiting(four.clone()),
            "another press"
        );

        search.press_failed(2, four.clone(), String::from("why"));
        assert_eq!(search.pressing, Pressing::Failed(four, String::from("why")));
        assert_eq!(search.query, "fire");
    }

    #[test]
    fn a_new_visit_starts_empty() {
        let kept = Search {
            visit: 1,
            query: String::from("fire"),
            selected: 2,
            offset: 40.0,
            pressing: Pressing::Failed(Action::Copy(String::from("4")), String::new()),
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
