//! Today's focused time (ADR 0138). The list is the `ScreenTime` Service, which the worker posts.
//! This view never writes that Service and never writes the state file.

use std::time::Instant;

use kanade_runtime::service::Service;
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Column, Key, Padding, Parent, Rectangle, Row, Scroll, Stack, Start, Text, Widget,
    children,
};

use crate::clock;
use crate::icon::Icon;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::sources::screen_time::{Row as App, ScreenTime};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, radius};
use crate::view;

use super::visit::Visited;
use super::{Outline, store};

const HEADER: f32 = 36.0;
const FOOTER: f32 = 28.0;
const GAP: f32 = 12.0;
const ROW: f32 = 36.0;
const ITEM_GAP: f32 = 2.0;
const ROW_INSET: f32 = 10.0;
const WHEEL: f32 = 40.0;

fn width() -> f32 {
    view::largest().width - 2.0 * INSET
}

fn room() -> f32 {
    view::largest().height - 2.0 * INSET - HEADER - FOOTER - 2.0 * GAP
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lookup {
    visit: u64,
    selected: usize,
    offset: f32,
}

impl Service for Lookup {
    fn new() -> Self {
        Lookup {
            visit: 0,
            selected: 0,
            offset: 0.0,
        }
    }

    fn listen() {}
}

impl Visited for Lookup {
    fn visit(&self) -> u64 {
        self.visit
    }

    fn begin(visit: u64) -> Self {
        Lookup {
            visit,
            selected: 0,
            offset: 0.0,
        }
    }
}

impl Lookup {
    fn bounded(mut self, count: usize, content: f32) -> Lookup {
        self.selected = self.selected.min(count.saturating_sub(1));
        self.offset = self.offset.clamp(0.0, most(content));

        self
    }

    fn step(mut self, key: Key, count: usize, content: f32) -> Option<Lookup> {
        let last = count.saturating_sub(1);

        match key {
            Key::Enter | Key::Left | Key::Right => return Some(self),
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => self.selected = (self.selected + 1).min(last),
            Key::Tab if self.selected < last => self.selected += 1,
            Key::Tab | Key::Home => self.selected = 0,
            Key::End => self.selected = last,
            _ => return None,
        }

        self.offset = reveal(self.offset, self.selected, content);

        Some(self)
    }

    fn wheel(mut self, lines: f32, count: usize, content: f32) -> Lookup {
        self.offset = (self.offset + lines * WHEEL).clamp(0.0, most(content));

        if count > 0 {
            let first = (self.offset / (ROW + ITEM_GAP)).floor() as usize;
            let last = ((self.offset + room()) / (ROW + ITEM_GAP) - 1.0)
                .ceil()
                .max(0.0) as usize;

            self.selected = self.selected.clamp(first, last.min(count - 1));
        }

        self
    }
}

fn most(content: f32) -> f32 {
    (content - room()).max(0.0)
}

fn reveal(offset: f32, selected: usize, content: f32) -> f32 {
    let top = selected as f32 * (ROW + ITEM_GAP);

    offset
        .max(top + ROW - room())
        .min(top)
        .clamp(0.0, most(content))
}

pub fn surface(_monitor: &str, visit: u64) -> Rectangle {
    let now = clock::unix_seconds();
    let snap = ScreenTime::read().clone();
    let rows = shown(&snap, now);
    let lookup = Lookup::read()
        .of(visit)
        .bounded(rows.len(), content(rows.len()));
    let shape = view::largest();
    let total = rows.iter().map(|row| row.seconds).sum();

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::Region, "screen time"))
        .child(
            Column::new(vec![
                Box::new(view::rise(0, header(total))) as Box<dyn Widget>,
                Box::new(view::rise(1, list(&rows, &lookup))),
                Box::new(view::rise(2, footer(rows.len()))),
            ])
            .width(width())
            .gap(GAP),
        )
}

pub fn key(monitor: &str, key: Key) -> bool {
    let visit = {
        let island = IslandService::read();

        if island.presentation(monitor) != Presentation::Expanded(Surface::ScreenTime) {
            return false;
        }

        island.visit()
    };

    let count = shown(&ScreenTime::read(), clock::unix_seconds()).len();
    let content = content(count);
    let Some(lookup) = Lookup::read()
        .of(visit)
        .bounded(count, content)
        .step(key, count, content)
    else {
        return false;
    };

    store(lookup);
    IslandService::write().attend(monitor, Instant::now());

    true
}

fn wheel(lines: f32) {
    let visit = IslandService::read().visit();
    let count = shown(&ScreenTime::read(), clock::unix_seconds()).len();
    let content = content(count);

    store(
        Lookup::read()
            .of(visit)
            .bounded(count, content)
            .wheel(lines, count, content),
    );
}

fn content(count: usize) -> f32 {
    if count == 0 {
        0.0
    } else {
        count as f32 * ROW + (count - 1) as f32 * ITEM_GAP
    }
}

fn shown(snap: &ScreenTime, now: i64) -> Vec<App> {
    let mut rows = snap.rows.clone();
    let extra = snap
        .open_since
        .filter(|since| now > *since)
        .map(|since| (now - since) as u64)
        .unwrap_or(0);

    if let Some(id) = &snap.open_id {
        if let Some(row) = rows.iter_mut().find(|row| row.id == *id) {
            row.seconds = row.seconds.saturating_add(extra);
            if row.title.is_none() {
                row.title = snap.open_title.clone();
            }
        } else {
            rows.push(App {
                id: id.clone(),
                seconds: extra,
                title: snap.open_title.clone(),
            });
        }
    }

    rows.sort_by(|one, other| other.seconds.cmp(&one.seconds).then(one.id.cmp(&other.id)));
    rows
}

fn header(total: u64) -> Rectangle {
    Rectangle::new()
        .width(width())
        .height(HEADER)
        .align_child(Start, Center)
        .child(
            Row::new(vec![
                Box::new(Icon::Stopwatch.draw(18.0)) as Box<dyn Widget>,
                Box::new(
                    Text::new(format!("{} today", spent(total)))
                        .size(theme::text::TITLE)
                        .color(theme::island().on_surface)
                        .weight(theme::text::MEDIUM),
                ),
            ])
            .gap(8.0)
            .align(Center),
        )
}

fn footer(count: usize) -> Rectangle {
    let line = if count == 0 {
        String::from("No focused time today")
    } else {
        format!("{count} apps")
    };

    Rectangle::new()
        .width(width())
        .height(FOOTER)
        .align_child(Start, Center)
        .child(
            Text::new(line)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant),
        )
}

fn list(rows: &[App], lookup: &Lookup) -> Stack {
    if rows.is_empty() {
        return Stack::new(children![
            Rectangle::new()
                .width(width())
                .height(room())
                .align_child(Center, Center)
                .child(
                    Text::new("No focused time today")
                        .size(theme::text::BODY)
                        .color(theme::island().on_surface_variant),
                )
        ]);
    }

    let content = content(rows.len());
    let from = lookup.offset;
    let to = lookup.offset + room();
    let shown: Vec<Box<dyn Widget>> = rows
        .iter()
        .enumerate()
        .map(|(at, row)| (at, row, at as f32 * (ROW + ITEM_GAP)))
        .filter(|(_, _, top)| top + ROW > from && *top < to)
        .map(|(at, row, top)| {
            let fade = super::edge_fade(top, ROW, lookup.offset, room());

            Box::new(
                line(row, at == lookup.selected)
                    .opacity(fade)
                    .translate(0.0, top - lookup.offset),
            ) as Box<dyn Widget>
        })
        .collect();

    let listed = content.min(room());
    let viewport = Rectangle::new()
        .width(width())
        .height(listed)
        .clip()
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::List, "apps"))
        .on_scroll(|Scroll { y, .. }| wheel(y))
        .child(Stack::new(shown).width(width()).height(listed));

    let mut layers = children![viewport];
    let most = most(content);

    if most > 0.0 {
        let length = (room() * room() / content).max(TARGET);
        let at = (room() - length) * lookup.offset / most;

        layers.push(Box::new(
            Rectangle::new()
                .width(3.0)
                .height(length)
                .radius(radius::HAIRLINE)
                .fill(theme::island().surface_container_high)
                .translate(width() + 7.0, at),
        ));
    }

    Stack::new(layers).width(width()).height(listed)
}

fn line(row: &App, ring: bool) -> Rectangle {
    let name = row.title.as_deref().unwrap_or(row.id.as_str());

    Rectangle::new()
        .width(width())
        .height(ROW)
        .radius(radius::ROW)
        .identity(&row.id)
        .padding(Padding {
            top: 0.0,
            right: ROW_INSET,
            bottom: 0.0,
            left: ROW_INSET,
        })
        .align_child(Start, Center)
        .border_if(ring)
        .child(
            Row::new(vec![
                Box::new(
                    Text::new(name)
                        .size(theme::text::BODY)
                        .color(theme::island().on_surface)
                        .elide(),
                ) as Box<dyn Widget>,
                Box::new(
                    Text::new(spent(row.seconds))
                        .size(theme::text::LABEL)
                        .color(theme::island().on_surface_variant)
                        .weight(theme::text::MEDIUM),
                ),
            ])
            .width(Parent)
            .gap(12.0)
            .align(Center),
        )
}

fn spent(seconds: u64) -> String {
    let minutes = seconds / 60;
    let hours = minutes / 60;

    if hours > 0 {
        format!("{hours}h {:02}m", minutes % 60)
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}
