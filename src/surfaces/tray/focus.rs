//! Where the Tray Surface is for one visit: the items, or the menu of one of them a submenu deep,
//! and the keyboard's ring on it. Written by input only, never by the view. The arrows move the
//! ring, Enter or Space presses it, Right enters the submenu it is on, and Escape or Left goes back
//! a level.

use kanade_runtime::Key;
use kanade_runtime::service::Service;

use crate::surfaces::grid::moved;
use crate::surfaces::ring::Ring;

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
    ring: Ring<At>,

    // the item whose menu shows, by its key; none is the items
    pub item: Option<u64>,

    // the submenus entered, by their entries' ids, outermost first
    pub entered: Vec<i32>,

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
        if self.ring.current(visit) {
            self.clone()
        } else {
            Focus {
                ring: Ring::start(visit, held),
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

    // what the ring shows on in `grid`, none while it hides
    pub fn ring(&self, grid: &[Vec<(At, f32)>]) -> Option<At> {
        self.ring.on(grid, self.header())
    }

    // the row of the list the ring shows in, so the list scrolls to show it
    pub fn row(&self, grid: &[Vec<(At, f32)>]) -> Option<usize> {
        self.ring
            .row(grid, self.header())?
            .checked_sub(self.header())
    }

    // into `item`'s menu, at its top
    pub fn into_menu(self, item: u64) -> Focus {
        Focus {
            ring: self.ring.top(),
            item: Some(item),
            entered: Vec::new(),
            offset: 0.0,
        }
    }

    // into the submenu of entry `id`, at its top
    pub fn into_submenu(mut self, id: i32) -> Focus {
        self.entered.push(id);

        Focus {
            ring: self.ring.top(),
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
            ring: self.ring.onto(at),
            offset: 0.0,
            ..self
        }
    }

    // the menu's submenus entered down to `depth`, the rest since gone
    pub fn within(mut self, depth: usize) -> Focus {
        if depth < self.entered.len() {
            self.entered.truncate(depth);
            self.ring = self.ring.top();
            self.offset = 0.0;
        }

        self
    }

    // back at the items, the menu's item gone
    pub fn without_menu(self) -> Focus {
        Focus {
            ring: self.ring.top(),
            item: None,
            entered: Vec::new(),
            offset: 0.0,
        }
    }

    // the ring hides, the pointer being what moves now
    pub fn hidden(self) -> Focus {
        Focus {
            ring: self.ring.hidden(),
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

        let Some(place) = self.ring.place(grid, self.header()) else {
            // a menu with nothing in it yet still goes back
            return (menu && key == Key::Left).then(|| (self.out(), None));
        };

        let at = grid[place.row][place.column].0.clone();

        let act = match (key, &at) {
            (Key::Enter | Key::Space, _) => Some(Act::Press(at.clone())),
            (Key::Right, At::Entry(id)) => Some(Act::Enter(*id)),
            _ => None,
        };

        // a menu's rows are one target wide, so Left goes back a level
        let back = menu && grid[place.row].len() == 1 && key == Key::Left;
        let to = moved(place, key, grid);

        if act.is_none() && !back && to.is_none() {
            return None;
        }

        if let Some(ring) = self.ring.revealed(grid, self.header()) {
            return Some((Focus { ring, ..self }, None));
        }

        if act.is_some() {
            return Some((self, act));
        }

        if back {
            return Some((self.out(), None));
        }

        let to = to?;

        Some((
            Focus {
                ring: self.ring.onto(grid[to.row][to.column].0.clone()),
                ..self
            },
            None,
        ))
    }
}
