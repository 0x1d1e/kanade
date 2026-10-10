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
    let across = grid[place.row][place.column].1;

    let nearest = |row: usize| {
        let column = grid[row]
            .iter()
            .enumerate()
            .min_by(|(_, (_, a)), (_, (_, b))| (a - across).abs().total_cmp(&(b - across).abs()))
            .map(|(column, _)| column)?;

        Some(Place { row, column })
    };

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
}
