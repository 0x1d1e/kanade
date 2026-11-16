//! What every Surface's keyboard shares: its targets as rows, each with where its middle is across
//! the Surface from 0 to 1, and how the arrows and Tab move a ring between them.

use kanade_runtime::Key;

// a place in a grid
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    pub row: usize,
    pub column: usize,
}

// where `at` is in `grid`, none when it is gone
pub fn find<A: PartialEq>(grid: &[Vec<(A, f32)>], at: &A) -> Option<Place> {
    grid.iter().enumerate().find_map(|(row, targets)| {
        let column = targets.iter().position(|(target, _)| target == at)?;

        Some(Place { row, column })
    })
}

/*
 * the place a key moves to, none for a key that does not move or a move that stays put. Up and Down
 * go to the target in the next row nearest across, Home and End to the first and last. Tab reads
 * on, row by row, and wraps to the first after the last; it is always taken, even when that is
 * where the ring already is, so it still shows a hidden ring
 */
pub fn moved<A>(place: Place, key: Key, grid: &[Vec<(A, f32)>]) -> Option<Place> {
    let nearest = |row: usize| nearest(place, row, grid);

    let to = match key {
        Key::Up => nearest(place.row.checked_sub(1)?)?,
        Key::Down if place.row + 1 < grid.len() => nearest(place.row + 1)?,
        Key::Left => Place {
            column: place.column.checked_sub(1)?,
            ..place
        },
        Key::Right if place.column + 1 < grid[place.row].len() => Place {
            column: place.column + 1,
            ..place
        },
        Key::Tab if place.column + 1 < grid[place.row].len() => Place {
            column: place.column + 1,
            ..place
        },
        Key::Tab if place.row + 1 < grid.len() => Place {
            row: place.row + 1,
            column: 0,
        },
        Key::Tab | Key::Home => Place { row: 0, column: 0 },
        Key::End => {
            let row = grid.len() - 1;

            Place {
                row,
                column: grid[row].len() - 1,
            }
        }
        _ => return None,
    };

    (key == Key::Tab || to != place).then_some(to)
}

// the target in `row` nearest across to `place`, as Up and Down take it
fn nearest<A>(place: Place, row: usize, grid: &[Vec<(A, f32)>]) -> Option<Place> {
    let across = grid[place.row][place.column].1;

    let column = grid[row]
        .iter()
        .enumerate()
        .min_by(|(_, (_, a)), (_, (_, b))| (a - across).abs().total_cmp(&(b - across).abs()))
        .map(|(column, _)| column)?;

    Some(Place { row, column })
}

/*
 * the place a page key moves to: `page` rows at a time, never past either end and never around to
 * the other, so a page at the bottom stays there rather than wrapping to the top. The target in the
 * new row nearest across is taken, as Up and Down take it, and no key but PageUp or PageDown moves
 */
pub fn paged<A>(place: Place, key: Key, page: usize, grid: &[Vec<(A, f32)>]) -> Option<Place> {
    let page = page.max(1);
    let last = grid.len().saturating_sub(1);

    let row = match key {
        Key::PageUp => place.row.saturating_sub(page),
        Key::PageDown => (place.row + page).min(last),
        _ => return None,
    };

    (row != place.row)
        .then(|| nearest(place, row, grid))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    // a lone target, as a Tray with one item and no menu
    #[test]
    fn tab_is_taken_on_the_only_target() {
        let grid = vec![vec![((), 0.5)]];
        let place = Place { row: 0, column: 0 };

        assert_eq!(moved(place, Key::Tab, &grid), Some(place));
        assert_eq!(moved(place, Key::Right, &grid), None);
    }

    // a page moves a screenful of rows in the key's direction, stops at either end rather than
    // wrapping, and keeps the target nearest across where the rows differ in length
    #[test]
    fn a_page_jumps_a_screenful_and_clamps_without_wrapping() {
        let grid: Vec<Vec<((), f32)>> = (0..10).map(|_| vec![((), 0.5)]).collect();
        let at = |row| Place { row, column: 0 };

        assert_eq!(paged(at(0), Key::PageDown, 3, &grid), Some(at(3)));
        assert_eq!(paged(at(3), Key::PageDown, 3, &grid), Some(at(6)));
        assert_eq!(paged(at(8), Key::PageDown, 3, &grid), Some(at(9)));
        assert_eq!(paged(at(9), Key::PageDown, 3, &grid), None);

        assert_eq!(paged(at(9), Key::PageUp, 3, &grid), Some(at(6)));
        assert_eq!(paged(at(1), Key::PageUp, 3, &grid), Some(at(0)));
        assert_eq!(paged(at(0), Key::PageUp, 3, &grid), None);

        // a page shorter than a row still moves one, so the key is never stuck
        assert_eq!(paged(at(0), Key::PageDown, 0, &grid), Some(at(1)));

        // the shorter last row takes the target nearest the column it came from
        let mut uneven = grid.clone();
        uneven[3] = vec![((), 0.1), ((), 0.6)];
        assert_eq!(
            paged(Place { row: 0, column: 0 }, Key::PageDown, 3, &uneven),
            Some(Place { row: 3, column: 1 })
        );

        assert_eq!(paged(at(4), Key::Down, 3, &grid), None);
    }
}
