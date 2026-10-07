//! A StatusNotifierItem as it describes itself, read from its properties. Every field is optional
//! to the reader: one of the wrong kind, or a pixmap whose bytes do not fit its size, is left out
//! and the rest still stands. Only an answer that is no dictionary at all describes nothing.

use std::collections::HashMap;

use zbus::names::BusName;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

// where an item without a path of its own serves, as the spec has it
pub const DEFAULT_PATH: &str = "/StatusNotifierItem";

// a side bigger than this is no tray icon; it is left out rather than drawn
const LARGEST: i32 = 1024;

// where an item lives on the bus: the name it registered with, or its caller's, and its object
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Address {
    pub name: String,
    pub path: String,
}

impl Address {
    /*
     * from what an item registered with: an object path of the caller (libappindicator), a bus
     * name serving at the default path (Qt, Electron), or the two joined as watchers list them
     */
    pub fn registered(service: &str, caller: Option<&str>) -> Option<Address> {
        let (name, path) = match service.find('/') {
            Some(0) => (caller?, service),
            Some(at) => service.split_at(at),
            None => (service, DEFAULT_PATH),
        };

        BusName::try_from(name).ok()?;
        ObjectPath::try_from(path).ok()?;

        Some(Address {
            name: name.to_owned(),
            path: path.to_owned(),
        })
    }

    // as a watcher lists it, like ":1.42/org/ayatana/NotificationItem/nm_applet"
    pub fn listed(&self) -> String {
        format!("{}{}", self.name, self.path)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Status {
    // nothing to show for now; a host may leave it out
    Passive,

    #[default]
    Active,

    NeedsAttention,
}

// rgba pixels with plain alpha
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

// a picture by icon name, by pixels, or both; either may be missing
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Picture {
    // a theme icon name, or an absolute path
    pub name: String,

    // the largest the item gave whose bytes fit its size
    pub pixmap: Option<Pixmap>,
}

impl Picture {
    fn is_empty(&self) -> bool {
        self.name.is_empty() && self.pixmap.is_none()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Described {
    pub title: String,
    pub status: Status,

    // what to draw now: the attention picture while it needs attention and has one
    pub picture: Picture,

    // a folder the item's own icon names are in, before the user's theme
    pub themes: Option<String>,

    // the object its com.canonical.dbusmenu menu is at, none when it has none
    pub menu: Option<String>,

    // it only shows its menu, so a click opens that rather than activating it
    pub is_menu: bool,
}

// the answer to Properties.GetAll on org.kde.StatusNotifierItem
pub fn describe(properties: &HashMap<String, OwnedValue>) -> Described {
    let text = |key: &str| {
        properties
            .get(key)
            .and_then(|value| match &**value {
                Value::Str(text) => Some(text.as_str().to_owned()),
                _ => None,
            })
            .unwrap_or_default()
    };

    let status = match text("Status").as_str() {
        "Passive" => Status::Passive,
        "NeedsAttention" => Status::NeedsAttention,
        _ => Status::Active,
    };

    let picture = |name: &str, pixmap: &str| Picture {
        name: text(name),
        pixmap: properties.get(pixmap).and_then(|value| largest(value)),
    };

    let normal = picture("IconName", "IconPixmap");
    let attention = picture("AttentionIconName", "AttentionIconPixmap");

    let picture = if status == Status::NeedsAttention && !attention.is_empty() {
        attention
    } else {
        normal
    };

    let title = [
        text("Title"),
        tooltip(properties.get("ToolTip")),
        text("Id"),
    ]
    .into_iter()
    .find(|title| !title.trim().is_empty())
    .unwrap_or_default();

    let themes = Some(text("IconThemePath")).filter(|path| path.starts_with('/'));

    // "/" is how some say they have none, "/NO_DBUSMENU" how Qt does
    let menu = properties
        .get("Menu")
        .and_then(|value| match &**value {
            Value::ObjectPath(path) => Some(path.as_str().to_owned()),
            _ => None,
        })
        .filter(|path| path != "/" && path != "/NO_DBUSMENU");

    let is_menu = matches!(
        properties.get("ItemIsMenu").map(|value| &**value),
        Some(Value::Bool(true))
    );

    Described {
        title,
        status,
        picture,
        themes,
        menu,
        is_menu,
    }
}

// the title of a tooltip, (icon name, pixmaps, title, description)
fn tooltip(tooltip: Option<&OwnedValue>) -> String {
    let Some(Value::Structure(tooltip)) = tooltip.map(|value| &**value) else {
        return String::new();
    };

    match tooltip.fields().get(2) {
        Some(Value::Str(title)) => title.as_str().to_owned(),
        _ => String::new(),
    }
}

// the largest of a(iiay) whose bytes fit its size, ARGB32 in network byte order, made rgba
fn largest(pixmaps: &Value) -> Option<Pixmap> {
    let Value::Array(pixmaps) = pixmaps else {
        return None;
    };

    pixmaps
        .iter()
        .filter_map(pixmap)
        .max_by_key(|pixmap| pixmap.width * pixmap.height)
}

fn pixmap(pixmap: &Value) -> Option<Pixmap> {
    let Value::Structure(pixmap) = pixmap else {
        return None;
    };

    let [Value::I32(width), Value::I32(height), Value::Array(bytes)] = pixmap.fields() else {
        return None;
    };

    if !(1..=LARGEST).contains(width) || !(1..=LARGEST).contains(height) {
        return None;
    }

    let argb: Vec<u8> = bytes
        .iter()
        .map(|byte| match byte {
            Value::U8(byte) => Some(*byte),
            _ => None,
        })
        .collect::<Option<_>>()?;

    let (width, height) = (*width as u32, *height as u32);

    if argb.len() != (width * height * 4) as usize {
        return None;
    }

    let rgba = argb
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[a, r, g, b]| [r, g, b, a])
        .collect();

    Some(Pixmap {
        width,
        height,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(value: Value<'_>) -> OwnedValue {
        value.try_to_owned().unwrap()
    }

    fn properties(entries: Vec<(&str, Value<'_>)>) -> HashMap<String, OwnedValue> {
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), owned(value)))
            .collect()
    }

    fn pixmap_value(width: i32, height: i32, bytes: Vec<u8>) -> (i32, i32, Vec<u8>) {
        (width, height, bytes)
    }

    fn pixmaps(list: Vec<(i32, i32, Vec<u8>)>) -> Value<'static> {
        Value::from(list)
    }

    #[test]
    fn an_item_reads_its_title_status_and_icon() {
        let described = describe(&properties(vec![
            ("Id", Value::from("nm-applet")),
            ("Title", Value::from("Network")),
            ("Status", Value::from("Active")),
            ("IconName", Value::from("nm-signal-75")),
            ("IconThemePath", Value::from("/usr/share/nm-applet/icons")),
        ]));

        assert_eq!(
            described,
            Described {
                title: String::from("Network"),
                status: Status::Active,
                picture: Picture {
                    name: String::from("nm-signal-75"),
                    pixmap: None,
                },
                themes: Some(String::from("/usr/share/nm-applet/icons")),
                menu: None,
                is_menu: false,
            }
        );
    }

    #[test]
    fn an_item_reads_its_menu() {
        let path = |path: &'static str| Value::from(ObjectPath::try_from(path).unwrap());

        let described = describe(&properties(vec![
            ("Menu", path("/MenuBar")),
            ("ItemIsMenu", Value::from(true)),
        ]));
        assert_eq!(described.menu.as_deref(), Some("/MenuBar"));
        assert!(described.is_menu);

        // none, as some say it, and a menu by any other kind
        for none in [path("/"), path("/NO_DBUSMENU"), Value::from("/MenuBar")] {
            let described = describe(&properties(vec![("Menu", none)]));
            assert_eq!(described.menu, None);
        }
    }

    #[test]
    fn a_pixmap_becomes_rgba() {
        let described = describe(&properties(vec![(
            "IconPixmap",
            pixmaps(vec![pixmap_value(1, 1, vec![0x80, 0x10, 0x20, 0x30])]),
        )]));

        assert_eq!(
            described.picture.pixmap,
            Some(Pixmap {
                width: 1,
                height: 1,
                rgba: vec![0x10, 0x20, 0x30, 0x80],
            })
        );
    }

    #[test]
    fn the_largest_pixmap_that_fits_its_size_wins() {
        let described = describe(&properties(vec![(
            "IconPixmap",
            pixmaps(vec![
                pixmap_value(1, 1, vec![0; 4]),
                // says 4x4, carries one pixel
                pixmap_value(4, 4, vec![0; 4]),
                pixmap_value(2, 2, vec![0; 16]),
                pixmap_value(0, 0, vec![]),
                pixmap_value(-2, -2, vec![0; 16]),
                pixmap_value(LARGEST + 1, 1, vec![0; (LARGEST as usize + 1) * 4]),
            ]),
        )]));

        let pixmap = described.picture.pixmap.unwrap();
        assert_eq!((pixmap.width, pixmap.height), (2, 2));
    }

    #[test]
    fn attention_shows_its_own_picture_only_when_it_has_one() {
        let attention = |extra: Vec<(&'static str, Value<'static>)>| {
            let mut entries = vec![
                ("Status", Value::from("NeedsAttention")),
                ("IconName", Value::from("mail")),
            ];
            entries.extend(extra);
            describe(&properties(entries))
        };

        assert_eq!(
            attention(vec![("AttentionIconName", Value::from("mail-unread"))])
                .picture
                .name,
            "mail-unread"
        );
        assert_eq!(attention(vec![]).picture.name, "mail");

        let passive = describe(&properties(vec![
            ("Status", Value::from("Passive")),
            ("IconName", Value::from("mail")),
            ("AttentionIconName", Value::from("mail-unread")),
        ]));
        assert_eq!(passive.status, Status::Passive);
        assert_eq!(passive.picture.name, "mail");
    }

    #[test]
    fn a_missing_title_falls_back_to_the_tooltip_then_the_id() {
        let tooltip = ("", Vec::<(i32, i32, Vec<u8>)>::new(), "Volume 40%", "");

        let described = describe(&properties(vec![
            ("Id", Value::from("pasystray")),
            ("Title", Value::from(" ")),
            ("ToolTip", Value::from(tooltip)),
        ]));
        assert_eq!(described.title, "Volume 40%");

        let described = describe(&properties(vec![("Id", Value::from("pasystray"))]));
        assert_eq!(described.title, "pasystray");
    }

    // each field of the wrong kind is left out, the rest stands
    #[test]
    fn malformed_fields_are_left_out() {
        let described = describe(&properties(vec![
            ("Title", Value::from(7_i32)),
            ("Id", Value::from("app")),
            ("Status", Value::from("Sleeping")),
            ("IconName", Value::from(true)),
            ("IconPixmap", Value::from("not pixels")),
            ("AttentionIconPixmap", pixmaps(vec![])),
            ("ToolTip", Value::from(3_u32)),
            ("IconThemePath", Value::from("relative/icons")),
            ("Menu", Value::from(1_i32)),
            ("ItemIsMenu", Value::from("yes")),
        ]));

        assert_eq!(
            described,
            Described {
                title: String::from("app"),
                status: Status::Active,
                picture: Picture::default(),
                themes: None,
                menu: None,
                is_menu: false,
            }
        );

        // pixmap fields of the wrong kinds
        let wrong = vec![(1_u32, 1_i32, vec![0_u8; 4])];
        let described = describe(&properties(vec![("IconPixmap", Value::from(wrong))]));
        assert_eq!(described.picture.pixmap, None);

        assert_eq!(describe(&HashMap::new()), Described::default());
    }

    #[test]
    fn a_registration_names_where_the_item_lives() {
        let address = |name: &str, path: &str| {
            Some(Address {
                name: name.to_owned(),
                path: path.to_owned(),
            })
        };

        // libappindicator: its path, served by the caller
        assert_eq!(
            Address::registered("/org/ayatana/NotificationItem/nm_applet", Some(":1.42")),
            address(":1.42", "/org/ayatana/NotificationItem/nm_applet")
        );
        // Qt and Electron: a name, at the default path
        assert_eq!(
            Address::registered("org.kde.StatusNotifierItem-1234-1", Some(":1.42")),
            address("org.kde.StatusNotifierItem-1234-1", DEFAULT_PATH)
        );
        // as a watcher lists it
        assert_eq!(
            Address::registered(":1.42/org/ayatana/NotificationItem/x", None),
            address(":1.42", "/org/ayatana/NotificationItem/x")
        );
        assert_eq!(address(":1.42", "/org/x").unwrap().listed(), ":1.42/org/x");
    }

    #[test]
    fn a_malformed_registration_names_nothing() {
        assert_eq!(Address::registered("", Some(":1.42")), None);
        assert_eq!(Address::registered("/path", None), None);
        assert_eq!(Address::registered("/bad//path", Some(":1.42")), None);
        assert_eq!(Address::registered("not a name", Some(":1.42")), None);
        assert_eq!(Address::registered("org.example/bad path", None), None);
        assert_eq!(Address::registered(".org.example", None), None);
    }
}
