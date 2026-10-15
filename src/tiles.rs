//! The Controls Surface's tile layout (ADR 0067): which switches show, in what order and at what
//! size, and how they pack into rows. Pure: the config reads and writes it, and the Surface draws
//! it and edits a draft of it.

use std::fmt;

// the Surface has room for this many rows of switches (ADR 0030)
pub const ROWS: usize = 3;

/*
 * a row holds two halves' worth of room, and the round buttons that fit beside them: three beside a
 * full tile, but one beside a half, or a half is under 160 px of the 400 and its words go unread
 */
const ROOM: u32 = 2;
const MOST_ICONS: usize = 3;
const MOST_ICONS_BESIDE_HALF: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Wifi,
    Bluetooth,
    Caffeine,
    Dnd,
    NightLight,
    Focus,
    ChargeLimit,
}

impl Tile {
    pub const ALL: [Tile; 7] = [
        Tile::Wifi,
        Tile::Bluetooth,
        Tile::Caffeine,
        Tile::Dnd,
        Tile::NightLight,
        Tile::Focus,
        Tile::ChargeLimit,
    ];

    // as the config names it
    pub fn name(self) -> &'static str {
        match self {
            Tile::Wifi => "wifi",
            Tile::Bluetooth => "bluetooth",
            Tile::Caffeine => "caffeine",
            Tile::Dnd => "dnd",
            Tile::NightLight => "night-light",
            Tile::Focus => "focus",
            Tile::ChargeLimit => "charge-limit",
        }
    }

    // as the Surface and Settings word it
    pub fn label(self) -> &'static str {
        match self {
            Tile::Wifi => "Wi-Fi",
            Tile::Bluetooth => "Bluetooth",
            Tile::Caffeine => "Caffeine",
            Tile::Dnd => "Do Not Disturb",
            Tile::NightLight => "Night Light",
            Tile::Focus => "Focus",
            Tile::ChargeLimit => "Limit Charge",
        }
    }

    // a radio opens its sub-surface by a chevron, which a round button has no room for
    pub fn sizes(self) -> &'static [Size] {
        match self {
            Tile::Wifi | Tile::Bluetooth => &[Size::Half, Size::Full],
            _ => &[Size::Half, Size::Full, Size::Icon],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    // two of them side by side
    Half,

    // the whole row's width
    Full,

    // a round button
    Icon,
}

impl Size {
    pub fn name(self) -> &'static str {
        match self {
            Size::Half => "half",
            Size::Full => "full",
            Size::Icon => "icon",
        }
    }

    fn weight(self) -> u32 {
        match self {
            Size::Half => 1,
            Size::Full => 2,
            Size::Icon => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub tile: Tile,
    pub size: Size,
}

// `name`, or `name:size` for a size other than half
impl fmt::Display for Placed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.size {
            Size::Half => f.write_str(self.tile.name()),
            size => write!(f, "{}:{}", self.tile.name(), size.name()),
        }
    }
}

impl Placed {
    pub fn parse(text: &str) -> Result<Placed, String> {
        let (name, size) = match text.split_once(':') {
            Some((name, size)) => (name, Some(size)),
            None => (text, None),
        };

        let tile = Tile::ALL
            .into_iter()
            .find(|tile| tile.name() == name)
            .ok_or_else(|| {
                let names: Vec<&str> = Tile::ALL.into_iter().map(Tile::name).collect();

                format!(
                    "unknown tile \"{name}\", expected one of {}",
                    names.join(", ")
                )
            })?;

        let size = match size {
            None | Some("half") => Size::Half,
            Some("full") => Size::Full,
            Some("icon") => Size::Icon,
            Some(other) => {
                return Err(format!(
                    "unknown size \"{other}\" for {name}, expected half, full or icon"
                ));
            }
        };

        if !tile.sizes().contains(&size) {
            return Err(format!("{name} has no {} size", size.name()));
        }

        Ok(Placed { tile, size })
    }
}

pub const DEFAULT: [Placed; 6] = [
    Placed {
        tile: Tile::Wifi,
        size: Size::Half,
    },
    Placed {
        tile: Tile::Bluetooth,
        size: Size::Half,
    },
    Placed {
        tile: Tile::Caffeine,
        size: Size::Half,
    },
    Placed {
        tile: Tile::Dnd,
        size: Size::Half,
    },
    Placed {
        tile: Tile::NightLight,
        size: Size::Half,
    },
    Placed {
        tile: Tile::Focus,
        size: Size::Half,
    },
];

/*
 * a list from the config: each tile once, the first kept, in sizes it has, and no more than the
 * Surface has rows for
 */
pub fn parse(values: &[String]) -> Result<Vec<Placed>, String> {
    let mut list: Vec<Placed> = Vec::new();

    for (at, value) in values.iter().enumerate() {
        let placed = Placed::parse(value).map_err(|why| format!("entry {}: {why}", at + 1))?;

        if list.iter().all(|kept| kept.tile != placed.tile) {
            list.push(placed);
        }
    }

    if list.is_empty() {
        return Err(String::from("at least one tile"));
    }

    if !fits(&list) {
        return Err(format!("these do not fit in {ROWS} rows"));
    }

    Ok(list)
}

// the tiles, packed left to right into rows, a new row when the next does not fit the last
pub fn pack(list: &[Placed]) -> Vec<Vec<Placed>> {
    let mut rows: Vec<Vec<Placed>> = Vec::new();

    for &placed in list {
        match rows.last_mut() {
            Some(row) if room(row, placed) => row.push(placed),
            _ => rows.push(vec![placed]),
        }
    }

    rows
}

fn room(row: &[Placed], next: Placed) -> bool {
    let all = || row.iter().chain(std::iter::once(&next));
    let weight: u32 = all().map(|placed| placed.size.weight()).sum();
    let icons = all().filter(|placed| placed.size == Size::Icon).count();
    let halves = all().any(|placed| placed.size == Size::Half);
    let most = if halves {
        MOST_ICONS_BESIDE_HALF
    } else {
        MOST_ICONS
    };

    weight <= ROOM && icons <= most
}

// whether all of `list` has a place
pub fn fits(list: &[Placed]) -> bool {
    pack(list).len() <= ROWS
}

// the rows that show: what does not fit the Surface is left out
pub fn rows(list: &[Placed]) -> Vec<Vec<Placed>> {
    let mut rows = pack(list);

    rows.truncate(ROWS);
    rows
}

/*
 * the width of each tile of a row `total` wide with `gap` between: a round button is `icon` wide,
 * and what is left is shared as two halves, so a lone half keeps its width
 */
pub fn widths(row: &[Placed], total: f32, gap: f32, icon: f32) -> Vec<f32> {
    let icons = row
        .iter()
        .filter(|placed| placed.size == Size::Icon)
        .count();
    let weight: u32 = row.iter().map(|placed| placed.size.weight()).sum();

    let room = total - gap * row.len().saturating_sub(1) as f32 - icon * icons as f32;
    let unit = room / weight.max(ROOM) as f32;

    row.iter()
        .map(|placed| match placed.size {
            Size::Icon => icon,
            size => unit * size.weight() as f32,
        })
        .collect()
}

// the tiles not in `list`
pub fn missing(list: &[Placed]) -> Vec<Tile> {
    Tile::ALL
        .into_iter()
        .filter(|tile| list.iter().all(|placed| placed.tile != *tile))
        .collect()
}

// `text`'s tile in its next size, none for text that is no tile; for Settings' size button
pub fn cycled(text: &str) -> Option<String> {
    let placed = Placed::parse(text).ok()?;
    let sizes = placed.tile.sizes();
    let at = sizes.iter().position(|&size| size == placed.size)?;
    let size = sizes[(at + 1) % sizes.len()];

    Some(Placed { size, ..placed }.to_string())
}

// the edits an editor makes: each gives the new list, or none when it would not fit or not apply

// the tile at `at` swapped with its neighbor, `by` places away
pub fn moved(list: &[Placed], at: usize, by: isize) -> Option<Vec<Placed>> {
    let to = at.checked_add_signed(by).filter(|&to| to < list.len())?;
    let mut list = list.to_vec();

    list.swap(at, to);
    fits(&list).then_some(list)
}

pub fn resized(list: &[Placed], at: usize, size: Size) -> Option<Vec<Placed>> {
    let mut list = list.to_vec();
    let placed = list.get_mut(at)?;

    if !placed.tile.sizes().contains(&size) || placed.size == size {
        return None;
    }

    placed.size = size;
    fits(&list).then_some(list)
}

// the tile at `at` taken out, unless it is the last
pub fn removed(list: &[Placed], at: usize) -> Option<Vec<Placed>> {
    if list.len() < 2 || at >= list.len() {
        return None;
    }

    let mut list = list.to_vec();

    list.remove(at);
    fits(&list).then_some(list)
}

// `tile` at the end, at half width, else as an icon, whichever fits first
pub fn added(list: &[Placed], tile: Tile) -> Option<Vec<Placed>> {
    if list.iter().any(|placed| placed.tile == tile) {
        return None;
    }

    [Size::Half, Size::Icon].into_iter().find_map(|size| {
        let mut next = list.to_vec();

        next.push(Placed { tile, size });
        (tile.sizes().contains(&size) && fits(&next)).then_some(next)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(text: &[&str]) -> Vec<String> {
        text.iter().map(|text| (*text).to_owned()).collect()
    }

    #[test]
    fn the_default_fits_and_reads_back() {
        assert!(fits(&DEFAULT));

        let text: Vec<String> = DEFAULT.iter().map(Placed::to_string).collect();

        assert_eq!(parse(&text).unwrap(), DEFAULT);
    }

    #[test]
    fn a_list_is_refused_that_the_surface_cannot_show() {
        assert!(parse(&list(&[])).is_err());
        assert!(parse(&list(&["nope"])).is_err());
        assert!(parse(&list(&["focus:huge"])).is_err());
        assert!(parse(&list(&["wifi:icon"])).is_err());

        // four full rows
        assert!(
            parse(&list(&[
                "wifi:full",
                "bluetooth:full",
                "dnd:full",
                "focus:full"
            ]))
            .is_err()
        );

        // a tile twice is kept once
        assert_eq!(parse(&list(&["focus", "focus:icon"])).unwrap().len(), 1);
    }

    #[test]
    fn tiles_pack_by_room() {
        let packed = pack(
            &parse(&list(&[
                "wifi",
                "bluetooth",
                "dnd",
                "focus:icon",
                "caffeine:icon",
            ]))
            .unwrap(),
        );
        let sizes: Vec<usize> = packed.iter().map(Vec::len).collect();

        assert_eq!(sizes, [2, 2, 1]);

        let packed = pack(&parse(&list(&["wifi:full", "dnd", "focus"])).unwrap());

        assert_eq!(packed.len(), 2);
    }

    // a half beside round buttons keeps the width its words need, the radios' too
    #[test]
    fn a_half_keeps_room_for_its_words() {
        let list = parse(&list(&[
            "wifi",
            "bluetooth",
            "caffeine:icon",
            "dnd:icon",
            "focus:icon",
            "night-light:icon",
        ]))
        .unwrap();

        for row in pack(&list) {
            for (placed, width) in row.iter().zip(widths(&row, 400.0, 8.0, 48.0)) {
                assert!(placed.size != Size::Half || width >= 160.0);
            }
        }

        // the three icons of a row with no half
        assert_eq!(pack(&list).last().map(Vec::len), Some(3));
    }

    #[test]
    fn a_size_button_walks_the_sizes_a_tile_has() {
        assert_eq!(cycled("focus").as_deref(), Some("focus:full"));
        assert_eq!(cycled("focus:icon").as_deref(), Some("focus"));
        assert_eq!(cycled("wifi:full").as_deref(), Some("wifi"));
        assert_eq!(cycled("nope"), None);
    }

    #[test]
    fn a_row_fills_its_width() {
        let row =
            pack(&parse(&list(&["night-light", "charge-limit", "focus:icon"])).unwrap()).remove(0);
        let widths = widths(&row, 400.0, 8.0, 48.0);

        assert_eq!(widths.iter().sum::<f32>() + 16.0, 400.0);
    }

    #[test]
    fn an_edit_that_would_overflow_is_refused() {
        let full = parse(&list(&[
            "wifi:full",
            "bluetooth:full",
            "dnd:full",
            "caffeine:icon",
            "focus:icon",
            "night-light:icon",
        ]))
        .unwrap();

        assert!(added(&full, Tile::ChargeLimit).is_none());
        assert!(removed(&full, 0).is_some());
        assert!(removed(&full[..1], 0).is_none());

        let halves = parse(&list(&[
            "wifi",
            "bluetooth",
            "dnd",
            "focus",
            "caffeine",
            "night-light",
        ]))
        .unwrap();

        assert!(resized(&halves, 0, Size::Full).is_none());
        assert!(moved(&halves, 0, -1).is_none());
        assert!(moved(&halves, 0, 1).is_some());
    }
}
