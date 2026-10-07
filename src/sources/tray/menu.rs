//! An item's menu, over com.canonical.dbusmenu: its entries as the item lays them out, read whole
//! each time it opens and again when the item says its layout changed while it shows. Only one
//! menu shows at a time, so one is kept.

use std::collections::HashMap;
use std::thread;

use amane::Service;
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Value};

use super::{Item, connection};
use crate::island::presentation::Surface;
use crate::island::service::IslandService;

pub const INTERFACE: &str = "com.canonical.dbusmenu";

// how a check or radio entry shows whether it is on
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Toggle {
    #[default]
    None,
    Check(bool),
    Radio(bool),
}

// one entry of a menu, with the shown entries of its submenu
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    pub id: i32,

    // with its mnemonic's underscore taken out
    pub label: String,

    pub enabled: bool,

    // a line between entries, drawn above the next one
    pub separator: bool,

    pub toggle: Toggle,

    // it opens a submenu, which the item may fill only once asked to
    pub submenu: bool,

    pub children: Vec<Entry>,
}

impl Entry {
    // the entry `id` in this one's submenus, or this one
    pub fn find(&self, id: i32) -> Option<&Entry> {
        if self.id == id {
            return Some(self);
        }

        self.children.iter().find_map(|child| child.find(id))
    }
}

// the menu of the item whose key it is: none until the item answers, and why it did not
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Menu {
    pub item: Option<u64>,
    pub root: Option<Entry>,
    pub failed: bool,

    // tells this opening's reads from an earlier one's
    opening: u64,

    // where it is: the program that answered the item's read, and the object
    owner: String,
    path: String,
}

impl Service for Menu {
    fn new() -> Self {
        Menu::default()
    }

    fn listen() {}
}

impl Menu {
    // the menu of `item`'s, none while it is another's or not read yet
    pub fn of(&self, item: u64) -> Option<&Entry> {
        self.root.as_ref().filter(|_| self.item == Some(item))
    }

    // where a call about this opening goes, while it is `item`'s
    fn request(&self, item: u64) -> Option<Request> {
        (self.item == Some(item)).then(|| Request {
            opening: self.opening,
            owner: self.owner.clone(),
            path: self.path.clone(),
        })
    }
}

/*
 * a call about one opening of a menu, addressed when it is made: a reply or a worker that runs late
 * still reaches the menu it was for, never one opened since
 */
#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    opening: u64,
    owner: String,
    path: String,
}

// the calls a menu takes; a test stands in for D-Bus
trait Calls {
    // whether the item changed entry `id`'s submenu, so it is read again
    fn about_to_show(&self, to: &Request, id: i32) -> bool;

    fn layout(&self, to: &Request) -> zbus::Result<Layout>;
}

impl Calls for Connection {
    // an item without AboutToShow is read all the same
    fn about_to_show(&self, to: &Request, id: i32) -> bool {
        self.call_method(
            Some(to.owner.as_str()),
            to.path.as_str(),
            Some(INTERFACE),
            "AboutToShow",
            &(id,),
        )
        .and_then(|reply| reply.body().deserialize::<bool>())
        .unwrap_or(false)
    }

    fn layout(&self, to: &Request) -> zbus::Result<Layout> {
        self.call_method(
            Some(to.owner.as_str()),
            to.path.as_str(),
            Some(INTERFACE),
            "GetLayout",
            &(0_i32, -1_i32, Vec::<String>::new()),
        )
        .and_then(|reply| reply.body().deserialize::<Layout>())
    }
}

/*
 * reads `item`'s menu whole, off the caller's thread, telling it first that it is about to show so
 * it may fill it. What showed of another item's goes at once
 */
pub fn open(item: &Item) {
    let (Some(path), Some(connection)) = (item.menu.clone(), connection(item)) else {
        return;
    };

    let request = {
        let mut menu = Menu::write();

        *menu = Menu {
            item: Some(item.key()),
            opening: menu.opening + 1,
            owner: item.owner.clone(),
            path,
            ..Menu::default()
        };

        menu.request(item.key())
    };

    if let Some(request) = request {
        thread::spawn(move || fetch(&connection, &request, 0));
    }
}

// the submenu of entry `id` is about to show; an item that fills it then says so, and is read again
pub fn enter(item: &Item, id: i32) {
    let (Some(connection), Some(request)) = (connection(item), Menu::read().request(item.key()))
    else {
        return;
    };

    thread::spawn(move || fetch(&connection, &request, id));
}

// runs entry `id`'s action
pub fn click(item: &Item, id: i32) {
    let (Some(connection), Some(request)) = (connection(item), Menu::read().request(item.key()))
    else {
        return;
    };

    thread::spawn(move || {
        let Request { owner, path, .. } = request;
        let clicked = connection.call_method(
            Some(owner.as_str()),
            path.as_str(),
            Some(INTERFACE),
            "Event",
            &(id, "clicked", Value::I32(0), 0_u32),
        );

        if let Err(error) = clicked {
            eprintln!("kanade: tray menu {owner}{path} failed a click: {error}");
        }
    });
}

// the item at `path` of `sender` said its menu changed: read again while it shows
pub(super) fn changed(sender: &str, path: &str) {
    if IslandService::read().surface() != Some(Surface::Tray) {
        return;
    }

    let request = {
        let menu = Menu::read();

        if menu.owner != sender || menu.path != path {
            return;
        }

        let Some(request) = menu.item.and_then(|item| menu.request(item)) else {
            return;
        };

        request
    };

    let Some(connection) = super::reach()
        .as_ref()
        .map(|reach| reach.connection.clone())
    else {
        return;
    };

    thread::spawn(move || read(&connection, &request));
}

// asks AboutToShow of entry `id`, then reads the layout when it opens the menu (0) or it changed
fn fetch(calls: &impl Calls, request: &Request, id: i32) {
    let update = calls.about_to_show(request, id);

    if id == 0 || update {
        read(calls, request);
    }
}

// reads the whole layout into the menu, unless another opened since
fn read(calls: &impl Calls, request: &Request) {
    if Menu::read().opening != request.opening {
        return;
    }

    let read = calls.layout(request);

    if let Err(error) = &read {
        let Request { owner, path, .. } = request;
        eprintln!("kanade: tray menu {owner}{path} could not be read: {error}");
    }

    let mut menu = Menu::write();

    if menu.opening != request.opening {
        return;
    }

    match read {
        Ok(layout) => {
            menu.root = Some(self::layout(&layout));
            menu.failed = false;
        }
        Err(_) => menu.failed = menu.root.is_none(),
    }
}

// GetLayout's answer: the revision, then the root as (id, properties, children)
pub type Layout = (u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>));

pub fn layout((_, (id, properties, children)): &Layout) -> Entry {
    let properties = properties
        .iter()
        .map(|(key, value)| (key.as_str(), unwrapped(value)))
        .collect();

    entry(*id, &properties, children.iter().map(|child| &**child))
}

// a child, (id, properties, children) in a variant
fn node(value: &Value) -> Option<Entry> {
    let Value::Structure(node) = unwrapped(value) else {
        return None;
    };

    let [
        Value::I32(id),
        Value::Dict(properties),
        Value::Array(children),
    ] = node.fields()
    else {
        return None;
    };

    let properties = properties
        .iter()
        .filter_map(|(key, value)| match key {
            Value::Str(key) => Some((key.as_str(), unwrapped(value))),
            _ => None,
        })
        .collect::<HashMap<_, _>>();

    // a hidden entry is left out, with its submenu
    if matches!(properties.get("visible"), Some(Value::Bool(false))) {
        return None;
    }

    Some(entry(*id, &properties, children.iter()))
}

fn entry<'a>(
    id: i32,
    properties: &HashMap<&str, &Value>,
    children: impl Iterator<Item = &'a Value<'a>>,
) -> Entry {
    let text = |key: &str| match properties.get(key) {
        Some(Value::Str(text)) => Some(text.as_str()),
        _ => None,
    };
    let flag = |key: &str| match properties.get(key) {
        Some(Value::Bool(flag)) => *flag,
        _ => true,
    };

    let on = matches!(properties.get("toggle-state"), Some(Value::I32(1)));
    let toggle = match text("toggle-type") {
        Some("checkmark") => Toggle::Check(on),
        Some("radio") => Toggle::Radio(on),
        _ => Toggle::None,
    };

    let children: Vec<Entry> = children.filter_map(node).collect();

    Entry {
        id,
        label: label(text("label").unwrap_or_default()),
        enabled: flag("enabled"),
        separator: text("type") == Some("separator"),
        toggle,
        submenu: text("children-display") == Some("submenu") || !children.is_empty(),
        children,
    }
}

// a property's value, out of the variant it comes in
fn unwrapped<'a>(value: &'a Value<'a>) -> &'a Value<'a> {
    match value {
        Value::Value(inner) => unwrapped(inner),
        value => value,
    }
}

// "_Open" is "Open", "Save __as" is "Save _as"
fn label(label: &str) -> String {
    let mut shown = String::with_capacity(label.len());
    let mut letters = label.chars().peekable();

    while let Some(letter) = letters.next() {
        if letter == '_' {
            if letters.peek() == Some(&'_') {
                letters.next();
                shown.push('_');
            }

            continue;
        }

        shown.push(letter);
    }

    shown
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use zbus::zvariant::{Array, Dict, Signature, StructureBuilder};

    use super::*;

    // the calls made, each by method, owner, path and entry; every AboutToShow says it changed
    #[derive(Default)]
    struct Recorded(RefCell<Vec<(&'static str, String, String, i32)>>);

    impl Calls for Recorded {
        fn about_to_show(&self, to: &Request, id: i32) -> bool {
            let call = ("AboutToShow", to.owner.clone(), to.path.clone(), id);

            self.0.borrow_mut().push(call);
            true
        }

        fn layout(&self, to: &Request) -> zbus::Result<Layout> {
            let call = ("GetLayout", to.owner.clone(), to.path.clone(), 0);

            self.0.borrow_mut().push(call);
            Ok(root(Vec::new()))
        }
    }

    // #135: a submenu's read that runs after another item's menu opened asks only its own item
    #[test]
    fn a_late_submenu_read_reaches_only_its_own_item() {
        let opened = |item, opening, owner: &str, path: &str| Menu {
            item: Some(item),
            opening,
            owner: owner.to_owned(),
            path: path.to_owned(),
            ..Menu::default()
        };

        let a = opened(1, 1, ":1.1", "/A").request(1).unwrap();
        assert_eq!(opened(1, 1, ":1.1", "/A").request(2), None);

        *Menu::write() = opened(2, 2, ":1.2", "/B");

        let calls = Recorded::default();
        fetch(&calls, &a, 42);

        assert_eq!(
            *calls.0.borrow(),
            [("AboutToShow", String::from(":1.1"), String::from("/A"), 42)]
        );
        assert_eq!(*Menu::read(), opened(2, 2, ":1.2", "/B"));
    }

    fn properties(entries: &[(&str, Value<'static>)]) -> Dict<'static, 'static> {
        let mut dict = Dict::new(&Signature::Str, &Signature::Variant);

        for (key, value) in entries {
            dict.append(
                Value::from(key.to_string()),
                Value::Value(Box::new(value.try_clone().unwrap())),
            )
            .unwrap();
        }

        dict
    }

    fn child(
        id: i32,
        entries: &[(&str, Value<'static>)],
        children: Vec<Value<'static>>,
    ) -> Value<'static> {
        let mut array = Array::new(&Signature::Variant);

        for child in children {
            array.append(Value::Value(Box::new(child))).unwrap();
        }

        Value::Value(Box::new(Value::Structure(
            StructureBuilder::new()
                .add_field(id)
                .append_field(Value::Dict(properties(entries)))
                .append_field(Value::Array(array))
                .build()
                .unwrap(),
        )))
    }

    fn root(children: Vec<Value<'static>>) -> Layout {
        (
            7,
            (
                0,
                HashMap::from([(
                    String::from("children-display"),
                    Value::from("submenu").try_to_owned().unwrap(),
                )]),
                children
                    .into_iter()
                    .map(|child| child.try_to_owned().unwrap())
                    .collect(),
            ),
        )
    }

    #[test]
    fn a_layout_reads_its_entries_and_submenus() {
        let menu = layout(&root(vec![
            child(1, &[("label", Value::from("_Open"))], vec![]),
            child(2, &[("type", Value::from("separator"))], vec![]),
            child(
                3,
                &[
                    ("label", Value::from("More")),
                    ("children-display", Value::from("submenu")),
                ],
                vec![child(
                    4,
                    &[
                        ("label", Value::from("Save __as")),
                        ("toggle-type", Value::from("checkmark")),
                        ("toggle-state", Value::from(1_i32)),
                    ],
                    vec![],
                )],
            ),
            child(
                5,
                &[
                    ("label", Value::from("Off")),
                    ("enabled", Value::from(false)),
                    ("toggle-type", Value::from("radio")),
                    ("toggle-state", Value::from(0_i32)),
                ],
                vec![],
            ),
            child(
                6,
                &[
                    ("label", Value::from("Hidden")),
                    ("visible", Value::from(false)),
                ],
                vec![],
            ),
        ]));

        let labels: Vec<&str> = menu
            .children
            .iter()
            .map(|entry| entry.label.as_str())
            .collect();
        assert_eq!(labels, ["Open", "", "More", "Off"]);

        assert!(menu.submenu);
        assert!(menu.children[1].separator);
        assert!(menu.children[0].enabled && !menu.children[0].submenu);
        assert!(!menu.children[3].submenu);

        let more = &menu.children[2];
        assert!(more.submenu);
        assert_eq!(
            more.children[0],
            Entry {
                id: 4,
                label: String::from("Save _as"),
                enabled: true,
                separator: false,
                toggle: Toggle::Check(true),
                submenu: false,
                children: Vec::new(),
            }
        );
        assert_eq!(menu.find(4), Some(&more.children[0]));

        let off = &menu.children[3];
        assert!(!off.enabled);
        assert_eq!(off.toggle, Toggle::Radio(false));
    }

    // a submenu the item fills only once asked to, and children that are not entries
    #[test]
    fn an_empty_submenu_still_opens_and_malformed_children_are_left_out() {
        let menu = layout(&root(vec![
            child(
                1,
                &[
                    ("label", Value::from("Later")),
                    ("children-display", Value::from("submenu")),
                ],
                vec![],
            ),
            Value::from(3_i32),
        ]));

        assert_eq!(menu.children.len(), 1);
        assert!(menu.children[0].submenu);
        assert!(menu.children[0].children.is_empty());
    }

    #[test]
    fn a_menu_is_only_its_items() {
        let menu = Menu {
            item: Some(4),
            root: Some(Entry::default()),
            ..Menu::default()
        };

        assert!(menu.of(4).is_some());
        assert!(menu.of(5).is_none());
    }
}
