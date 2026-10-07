//! Where the Tray Surface is for one visit: the items, or the menu of one of them a submenu deep,
//! and the keyboard's ring on it. Written by input only, never by the view. The arrows move the
//! ring, Enter or Space presses it, Right enters the submenu it is on, and Escape or Left goes back
//! a level.

use amane::{Key, Service};

use crate::surfaces::grid::{Place, find, moved};

// what the ring can be on, by what it is, so an item or entry moving keeps it
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum At {
    // a menu's back chevron
    Back,

    // an item, by its key: the row, which presses it, then its chevron, which opens its menu
    Item(u64),
    Menu(u64),

    // an entry of the menu, by its id
    Entry(i32),
}

// what a key asks for beyond moving
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    Press(At),

    // into the submenu of the entry, if it opens one
    Enter(i32),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Focus {
    visit: u64,

    // the item whose menu shows, by its key; none is the items
    pub item: Option<u64>,

    // the submenus entered, by their entries' ids, outermost first
    pub entered: Vec<i32>,

    // none is the level's first target
    at: Option<At>,

    // the ring shows: from the start when opened from the keyboard, else from the first key
    pub shown: bool,

    // how far the rows scrolled, in pixels
    pub offset: f32,
}

impl Service for Focus {
    fn new() -> Self {
        Focus::default()
    }

    fn listen() {}
}

impl Focus {
    // this visit's focus; one kept from an earlier visit is over
    pub fn of(&self, visit: u64, held: bool) -> Focus {
        if self.visit == visit {
            self.clone()
        } else {
            Focus {
                visit,
                shown: held,
                ..Focus::default()
            }
        }
    }

    // the rows above the list: a menu's back chevron
    pub fn header(&self) -> usize {
        usize::from(self.item.is_some())
    }

    // the level's targets: the menu's back chevron above `rows`, which scroll
    pub fn grid(&self, rows: Vec<Vec<(At, f32)>>) -> Vec<Vec<(At, f32)>> {
        if self.item.is_none() {
            return rows;
        }

        let mut grid = vec![vec![(At::Back, 0.0)]];

        grid.extend(rows);
        grid
    }

    // where the ring is in `grid`: on its target, or the first row when that is gone or unchosen
    fn place(&self, grid: &[Vec<(At, f32)>]) -> Option<Place> {
        let row = if grid.len() > self.header() {
            self.header()
        } else {
            0
        };

        self.found(grid)
            .or_else(|| (!grid.is_empty()).then_some(Place { row, column: 0 }))
    }

    fn found(&self, grid: &[Vec<(At, f32)>]) -> Option<Place> {
        find(grid, self.at.as_ref()?)
    }

    // its target gone, the ring hides until a key puts it on what took its place
    fn lost(&self, grid: &[Vec<(At, f32)>]) -> bool {
        self.at.is_some() && self.found(grid).is_none()
    }

    // what the ring shows on in `grid`, none while it hides
    pub fn ring(&self, grid: &[Vec<(At, f32)>]) -> Option<At> {
        (self.shown && !self.lost(grid))
            .then(|| self.place(grid))
            .flatten()
            .map(|place| grid[place.row][place.column].0.clone())
    }

    // the row of the list the ring shows in, so the list scrolls to show it
    pub fn row(&self, grid: &[Vec<(At, f32)>]) -> Option<usize> {
        (self.shown && !self.lost(grid))
            .then(|| self.place(grid))
            .flatten()
            .and_then(|place| place.row.checked_sub(self.header()))
    }

    // into `item`'s menu, at its top
    pub fn into_menu(self, item: u64) -> Focus {
        Focus {
            item: Some(item),
            entered: Vec::new(),
            at: None,
            offset: 0.0,
            ..self
        }
    }

    // into the submenu of entry `id`, at its top
    pub fn into_submenu(mut self, id: i32) -> Focus {
        self.entered.push(id);

        Focus {
            at: None,
            offset: 0.0,
            ..self
        }
    }

    // a level up, the ring on what entered it
    pub fn out(mut self) -> Focus {
        let at = match (self.entered.pop(), self.item) {
            (Some(id), _) => At::Entry(id),
            (None, Some(item)) => {
                self.item = None;
                At::Menu(item)
            }
            (None, None) => return self,
        };

        Focus {
            at: Some(at),
            offset: 0.0,
            ..self
        }
    }

    // the menu's submenus entered down to `depth`, the rest since gone
    pub fn within(mut self, depth: usize) -> Focus {
        if depth < self.entered.len() {
            self.entered.truncate(depth);
            self.at = None;
            self.offset = 0.0;
        }

        self
    }

    // back at the items, the menu's item gone
    pub fn without_menu(self) -> Focus {
        Focus {
            item: None,
            entered: Vec::new(),
            at: None,
            offset: 0.0,
            ..self
        }
    }

    // the ring hides, the pointer being what moves now
    pub fn hidden(self) -> Focus {
        Focus {
            shown: false,
            ..self
        }
    }

    /*
     * the focus after a key and what it asks for, none when the key is not for the Surface, as
     * Escape at the items, which closes it. A key while the ring hides only shows it where it is,
     * so nothing is pressed that the ring was not seen on
     */
    pub fn step(self, key: Key, grid: &[Vec<(At, f32)>]) -> Option<(Focus, Option<Act>)> {
        let menu = self.item.is_some();

        if key == Key::Escape {
            return menu.then(|| (self.out(), None));
        }

        let Some(place) = self.place(grid) else {
            // a menu with nothing in it yet still goes back
            return (menu && key == Key::Left).then(|| (self.out(), None));
        };

        let at = grid[place.row][place.column].0.clone();
        let single = grid[place.row].len() == 1;

        if !self.shown || self.lost(grid) {
            let shown = matches!(
                key,
                Key::Up
                    | Key::Down
                    | Key::Left
                    | Key::Right
                    | Key::Home
                    | Key::End
                    | Key::Enter
                    | Key::Space
            );

            if !shown {
                return None;
            }

            let focus = Focus {
                at: Some(at),
                shown: true,
                ..self
            };

            return Some((focus, None));
        }

        let act = match (key, &at) {
            (Key::Enter | Key::Space, _) => Some(Act::Press(at.clone())),
            (Key::Right, At::Entry(id)) => Some(Act::Enter(*id)),
            _ => None,
        };

        if act.is_some() {
            return Some((self, act));
        }

        // a menu's rows are one target wide, so Left goes back a level
        if menu && single && key == Key::Left {
            return Some((self.out(), None));
        }

        let to = moved(place, key, grid)?;

        Some((
            Focus {
                at: Some(grid[to.row][to.column].0.clone()),
                ..self
            },
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<Vec<(At, f32)>> {
        vec![
            vec![(At::Item(1), 0.45), (At::Menu(1), 0.97)],
            vec![(At::Item(2), 0.45)],
        ]
    }

    fn entries() -> Vec<Vec<(At, f32)>> {
        vec![vec![(At::Entry(3), 0.5)], vec![(At::Entry(4), 0.5)]]
    }

    fn shown() -> Focus {
        Focus {
            shown: true,
            ..Focus::default()
        }
    }

    fn step(focus: Focus, key: Key, rows: Vec<Vec<(At, f32)>>) -> (Focus, Option<Act>) {
        let grid = focus.grid(rows);

        focus.step(key, &grid).unwrap()
    }

    #[test]
    fn the_first_key_only_shows_the_ring() {
        let focus = Focus::default();
        let grid = focus.grid(items());

        assert_eq!(focus.ring(&grid), None);

        let (focus, act) = focus.step(Key::Enter, &grid).unwrap();

        assert_eq!(act, None);
        assert_eq!(focus.ring(&grid), Some(At::Item(1)));
    }

    #[test]
    fn the_chevron_opens_the_menu_and_escape_comes_back_to_it() {
        let (focus, _) = step(shown(), Key::Right, items());
        let (focus, act) = step(focus, Key::Enter, items());

        assert_eq!(act, Some(Act::Press(At::Menu(1))));

        let focus = focus.into_menu(1);
        let grid = focus.grid(entries());

        // a menu starts on its first entry, under the back chevron
        assert_eq!(focus.ring(&grid), Some(At::Entry(3)));
        assert_eq!(focus.row(&grid), Some(0));

        let (focus, act) = step(focus, Key::Escape, entries());

        assert_eq!(act, None);
        assert_eq!(focus.item, None);
        assert_eq!(focus.ring(&focus.grid(items())), Some(At::Menu(1)));

        // at the items Escape is not the Surface's, so it closes
        assert_eq!(focus.step(Key::Escape, &items()), None);
    }

    #[test]
    fn right_enters_a_submenu_and_left_goes_back_to_its_entry() {
        let focus = shown().into_menu(1);

        let (focus, _) = step(focus, Key::Down, entries());
        let (focus, act) = step(focus, Key::Right, entries());

        assert_eq!(act, Some(Act::Enter(4)));

        let focus = focus.into_submenu(4);
        assert_eq!(focus.entered, [4]);

        let inner = vec![vec![(At::Entry(5), 0.5)]];
        let (focus, act) = step(focus, Key::Left, inner);

        assert_eq!(act, None);
        assert!(focus.entered.is_empty());
        assert_eq!(focus.ring(&focus.grid(entries())), Some(At::Entry(4)));
    }

    #[test]
    fn an_empty_menu_still_goes_back() {
        let focus = shown().into_menu(1);
        let grid = focus.grid(Vec::new());

        // the back chevron is all there is
        let (focus, act) = focus.step(Key::Enter, &grid).unwrap();
        assert_eq!(act, Some(Act::Press(At::Back)));

        assert_eq!(focus.out().item, None);
    }

    #[test]
    fn a_submenu_since_gone_leaves_the_levels_above() {
        let focus = shown().into_menu(1).into_submenu(4).into_submenu(6);

        assert_eq!(focus.clone().within(2), focus);
        assert_eq!(focus.within(1).entered, [4]);
    }

    #[test]
    fn a_later_visit_starts_at_the_items() {
        let focus = Focus {
            visit: 1,
            ..shown().into_menu(1)
        };

        assert_eq!(focus.of(1, false).item, Some(1));
        assert_eq!(focus.of(2, true).item, None);
        assert!(focus.of(2, true).shown);
    }
}
