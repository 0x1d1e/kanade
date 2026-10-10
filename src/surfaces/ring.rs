//! The keyboard ring of a Surface for one visit: what it is on and whether it shows. Each
//! Surface's `Focus` keeps one beside its own state, so the ring reads and reveals the same
//! in Session, Tray and Controls. Notifications keeps its own `At::place` but the same reveal rule.

use super::grid::{Place, find};

#[derive(Debug, Clone, PartialEq)]
pub struct Ring<A> {
    visit: u64,

    // none is the level's first target
    pub at: Option<A>,

    // the ring shows: from the start when opened from the keyboard, else from the first key
    pub shown: bool,
}

impl<A> Default for Ring<A> {
    fn default() -> Self {
        Ring {
            visit: 0,
            at: None,
            shown: false,
        }
    }
}

impl<A: Clone + PartialEq> Ring<A> {
    // a ring for `visit`, shown at once when `held`
    pub fn start(visit: u64, held: bool) -> Self {
        Ring {
            visit,
            at: None,
            shown: held,
        }
    }

    // whether it is this visit's; one kept from an earlier visit is over
    pub fn current(&self, visit: u64) -> bool {
        self.visit == visit
    }

    // the ring on `at`
    pub fn onto(self, at: A) -> Self {
        Ring {
            at: Some(at),
            ..self
        }
    }

    // the ring back on the level's first target
    pub fn top(self) -> Self {
        Ring { at: None, ..self }
    }

    // the ring hides, the pointer being what moves now
    pub fn hidden(self) -> Self {
        Ring {
            shown: false,
            ..self
        }
    }

    /*
     * where the ring is in `grid`: on its target, or on the first row after `header` rows when that
     * is gone or none was chosen
     */
    pub fn place(&self, grid: &[Vec<(A, f32)>], header: usize) -> Option<Place> {
        let row = if grid.len() > header { header } else { 0 };

        self.found(grid)
            .or_else(|| (!grid.is_empty()).then_some(Place { row, column: 0 }))
    }

    // where its target is in `grid`, none when it has none or that is gone
    fn found(&self, grid: &[Vec<(A, f32)>]) -> Option<Place> {
        find(grid, self.at.as_ref()?)
    }

    /*
     * its target gone from `grid`, as a network or device leaves while the ring is on it. The ring
     * hides until a key puts it on what took its place, so nothing is pressed it was not seen on
     */
    fn lost(&self, grid: &[Vec<(A, f32)>]) -> bool {
        self.at.is_some() && self.found(grid).is_none()
    }

    // what the ring is on in `grid`, shown or not, none on a level without targets
    pub fn target(&self, grid: &[Vec<(A, f32)>], header: usize) -> Option<A> {
        let place = self.place(grid, header)?;

        Some(grid[place.row][place.column].0.clone())
    }

    // what the ring shows on in `grid`, none while it hides
    pub fn on(&self, grid: &[Vec<(A, f32)>], header: usize) -> Option<A> {
        if self.shown && !self.lost(grid) {
            self.target(grid, header)
        } else {
            None
        }
    }

    // the row of `grid` the ring shows in, none while it hides
    pub fn row(&self, grid: &[Vec<(A, f32)>], header: usize) -> Option<usize> {
        if self.shown && !self.lost(grid) {
            Some(self.place(grid, header)?.row)
        } else {
            None
        }
    }

    /*
     * the ring a key shows, when it hides: on the target it was or would be on, which the key does
     * not press. Call it once the key is known to act or move; a key that does neither never
     * shows it. None when the ring already shows
     */
    pub fn revealed(&self, grid: &[Vec<(A, f32)>], header: usize) -> Option<Self> {
        if self.shown && !self.lost(grid) {
            return None;
        }

        Some(Ring {
            at: Some(self.target(grid, header)?),
            shown: true,
            ..self.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Vec<Vec<(u8, f32)>> {
        vec![vec![(1, 0.5)], vec![(2, 0.25), (3, 0.75)]]
    }

    #[test]
    fn a_hidden_ring_reveals_where_it_would_be() {
        let ring = Ring::<u8>::start(1, false);

        let revealed = ring.revealed(&grid(), 0).unwrap();

        assert!(revealed.shown);
        assert_eq!(revealed.at, Some(1));
        assert_eq!(revealed.revealed(&grid(), 0), None);
    }

    #[test]
    fn a_ring_after_the_header_starts_on_the_first_row_below() {
        let ring = Ring::<u8>::start(1, true);

        assert_eq!(ring.on(&grid(), 1), Some(2));
    }

    #[test]
    fn a_ring_whose_target_left_hides_until_a_key_reveals_it() {
        let ring = Ring {
            at: Some(9),
            ..Ring::<u8>::start(1, true)
        };

        assert_eq!(ring.on(&grid(), 0), None);
        assert_eq!(ring.row(&grid(), 0), None);

        let revealed = ring.revealed(&grid(), 0).unwrap();

        assert_eq!(revealed.on(&grid(), 0), Some(1));
    }
}
