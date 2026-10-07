//! The tray (#134, ADR 0011): the StatusNotifierItems apps show, hosted by Kanade. Kanade serves the
//! StatusNotifierWatcher items register with, unless another program already does; then it hosts
//! that one's items, and takes over the name once that program quits. Each item's properties are
//! read again when it says they changed, off this thread, so a slow or dead item stalls only its
//! own read. An item that quits, or has nothing at its address, is dropped; one that answers with
//! something else stays as last read, hidden if never read, until a change reads it whole.
//!
//! Over zbus, not Amane's `Bus`: an item may register with only its object path, and the bus name
//! it lives at is then the caller's, which Amane's `Method` does not give.

mod item;
mod watcher;

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::Duration;

use amane::Service;
use enumflags2::BitFlags;
use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::{Connection, MessageIterator, connection};
use zbus::fdo::RequestNameReply;
use zbus::message::Type;
use zbus::names::BusName;
use zbus::zvariant::OwnedValue;

pub use item::Status;
use item::{Address, Described};
use watcher::Watcher;

use crate::{raster, supervise};

const ITEM: &str = "org.kde.StatusNotifierItem";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

// how long an item may take to answer before its read, or an action on it, is given up
const TIMEOUT: Duration = Duration::from_secs(2);

// what reading an item finds: it is gone for good only when nothing is at its address
const GONE: [&str; 5] = [
    "org.freedesktop.DBus.Error.ServiceUnknown",
    "org.freedesktop.DBus.Error.NameHasNoOwner",
    "org.freedesktop.DBus.Error.UnknownObject",
    "org.freedesktop.DBus.Error.UnknownInterface",
    "org.freedesktop.DBus.Error.UnknownMethod",
];

// the items to show, in the order they registered, and whose watcher they registered with
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tray {
    pub items: Vec<Item>,
    pub watcher: Role,
}

impl Service for Tray {
    fn new() -> Self {
        Tray::default()
    }

    fn listen() {}
}

impl Tray {
    // for `kanade status`
    pub fn status(&self) -> String {
        let watcher = match &self.watcher {
            Role::Starting => String::from("starting"),
            Role::Own => String::from("watcher Kanade"),
            Role::Foreign(owner) => format!("watcher {owner}, not Kanade"),
        };

        format!("tray: {watcher}, {} items", self.items.len())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Role {
    #[default]
    Starting,

    // Kanade holds the watcher's name
    Own,

    // another program does, by its unique name; Kanade hosts its items
    Foreign(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    address: Address,

    pub title: String,
    pub status: Status,
    pub icon: Icon,
}

// what to draw: the named icon if the theme, or the item's own folder, has it, else its pixels
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Icon {
    // a theme icon name, or an absolute path
    pub name: String,

    // a folder of the item's own icons, before the user's theme
    pub themes: Option<PathBuf>,

    // its pixels, written as a png
    pub pixmap: Option<PathBuf>,
}

// which way a scroll turns
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(dead_code, reason = "the tray's view (#135) scrolls items")]
pub enum Orientation {
    Vertical,
    Horizontal,
}

// what the tray thread hears, from the bus or a read it started
#[derive(Debug)]
enum Event {
    // an item registered, with Kanade's watcher or another one
    Registered(Address),

    // another watcher dropped this item
    Unregistered(Address),

    // an item said one of its properties changed: who said it, at which object
    Changed { sender: String, path: String },

    // a bus name has a new owner, none when empty
    Owner { name: String, new: String },

    // what reading an entry found, by its serial
    Read { serial: u64, outcome: Outcome },
}

#[derive(Debug)]
enum Outcome {
    // the unique name that answered, and what it said
    Read(String, Described),

    // nothing is at its address
    Gone,

    // it answered with something else, or not in time
    Failed(String),
}

// one registered item, shown once read
struct Entry {
    address: Address,

    // tells this entry's reads from those of one at the same address before it
    serial: u64,

    // the unique name that last answered for it
    owner: Option<String>,

    item: Option<Item>,

    reading: bool,

    // it changed while being read, so it is read again after
    again: bool,

    // a failed read was logged, so the next is not
    failed: bool,
}

// the connection actions on items go over, the tray thread's
static CONNECTION: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();

pub fn follow() {
    supervise::spawn("tray", || {
        if let Err(error) = run() {
            let why = format!("cannot host items: {error}");

            eprintln!("kanade: tray {why}");
            supervise::stopped("tray", why);
        }
    });
}

fn run() -> zbus::Result<()> {
    let connection = connection::Builder::session()?
        .method_timeout(TIMEOUT)
        .build()?;

    // the bus thread keeps a clone, so the names are let go of when this run ends, a panic too
    let _names = Names(connection.clone());

    let own = connection
        .unique_name()
        .map(|name| name.to_string())
        .unwrap_or_default();

    let (events, heard) = mpsc::channel();

    // everything the bus sends, from before the names are asked for so no change is missed
    let messages = MessageIterator::from(&connection);

    let bus = DBusProxy::new(&connection)?;
    for rule in [
        format!("type='signal',interface='{ITEM}'"),
        format!("type='signal',interface='{}'", watcher::INTERFACE),
        String::from(
            "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged'",
        ),
    ] {
        bus.add_match_rule(rule.as_str().try_into()?)?;
    }

    {
        let events = events.clone();
        let own = own.clone();
        thread::Builder::new()
            .name(String::from("tray-bus"))
            .spawn(move || forward(messages, &own, &events))?;
    }

    let registered = Arc::new(Mutex::new(Vec::new()));
    connection.object_server().at(
        watcher::PATH,
        Watcher {
            events: events.clone(),
            registered: Arc::clone(&registered),
        },
    )?;

    connection.request_name(host())?;

    /*
     * waits in line behind another watcher, which then lists the items, and neither takes the name
     * from one nor gives it up to one; zbus's default flags would do both
     */
    let reply = connection.request_name_with_flags(watcher::NAME, BitFlags::empty())?;

    let role = match reply {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Role::Own,
        RequestNameReply::InQueue | RequestNameReply::Exists => {
            let owner = bus
                .get_name_owner(BusName::try_from(watcher::NAME)?)
                .map(|owner| owner.to_string())
                .unwrap_or_default();

            Role::Foreign(owner)
        }
    };

    *CONNECTION
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(connection.clone());

    let mut tray = Hosting {
        connection,
        own,
        events,
        registered,
        role: Role::Starting,
        entries: Vec::new(),
        serials: 0,
    };

    tray.assume(role);
    tray.publish();
    tray.listen(heard);

    Ok(())
}

// a host's name, as the spec has hosts take one
fn host() -> String {
    format!("org.kde.StatusNotifierHost-{}-1", process::id())
}

struct Names(Connection);

impl Drop for Names {
    fn drop(&mut self) {
        let _ = self.0.release_name(watcher::NAME);
        let _ = self.0.release_name(host());
    }
}

// turns what the bus sends into events; it only sends, so it never falls behind
fn forward(messages: MessageIterator, own: &str, events: &Sender<Event>) {
    for message in messages {
        let Ok(message) = message else { continue };

        let header = message.header();

        if header.message_type() != Type::Signal {
            continue;
        }

        let (Some(interface), Some(member), Some(sender)) =
            (header.interface(), header.member(), header.sender())
        else {
            continue;
        };

        let body = message.body();

        let event = match (interface.as_str(), member.as_str()) {
            ("org.freedesktop.DBus", "NameOwnerChanged") => {
                let Ok((name, _, new)) = body.deserialize::<(String, String, String)>() else {
                    continue;
                };

                Event::Owner { name, new }
            }

            // what Kanade's own watcher says, it already knows
            (watcher::INTERFACE, _) if sender.as_str() == own => continue,
            (watcher::INTERFACE, "StatusNotifierItemRegistered") => {
                let Ok((service,)) = body.deserialize::<(String,)>() else {
                    continue;
                };
                let Some(address) = Address::registered(&service, None) else {
                    continue;
                };

                Event::Registered(address)
            }
            (watcher::INTERFACE, "StatusNotifierItemUnregistered") => {
                let Ok((service,)) = body.deserialize::<(String,)>() else {
                    continue;
                };
                let Some(address) = Address::registered(&service, None) else {
                    continue;
                };

                Event::Unregistered(address)
            }

            (ITEM, _) => {
                let Some(path) = header.path() else { continue };

                Event::Changed {
                    sender: sender.to_string(),
                    path: path.to_string(),
                }
            }

            _ => continue,
        };

        if events.send(event).is_err() {
            return;
        }
    }
}

// the tray thread's state
struct Hosting {
    connection: Connection,

    // Kanade's unique name
    own: String,

    // for reads to answer on
    events: Sender<Event>,

    // what Kanade's watcher lists
    registered: Arc<Mutex<Vec<String>>>,

    role: Role,
    entries: Vec<Entry>,
    serials: u64,
}

impl Hosting {
    fn listen(&mut self, heard: Receiver<Event>) {
        for event in heard {
            match event {
                Event::Registered(address) => self.register(address),
                Event::Unregistered(address) => {
                    if matches!(self.role, Role::Foreign(_)) {
                        self.drop_where(|entry| entry.address == address);
                    }
                }
                Event::Changed { sender, path } => {
                    let changed: Vec<usize> = (0..self.entries.len())
                        .filter(|&at| {
                            let entry = &self.entries[at];

                            entry.address.path == path
                                && (entry.address.name == sender
                                    || entry.owner.as_ref().is_none_or(|owner| *owner == sender))
                        })
                        .collect();

                    for at in changed {
                        self.read(at);
                    }
                }
                Event::Owner { name, new } => self.owner(&name, &new),
                Event::Read { serial, outcome } => self.answered(serial, outcome),
            }

            self.publish();
        }
    }

    fn assume(&mut self, role: Role) {
        if role == self.role {
            return;
        }

        self.role = role.clone();

        match role {
            Role::Own => self.list(),
            Role::Foreign(_) => {
                // off this thread: another watcher may be slow to answer
                let connection = self.connection.clone();
                let events = self.events.clone();
                let host = host();

                thread::spawn(move || {
                    for address in host_with(&connection, &host) {
                        if events.send(Event::Registered(address)).is_err() {
                            return;
                        }
                    }
                });
            }
            Role::Starting => {}
        }
    }

    fn register(&mut self, address: Address) {
        if self.entries.iter().any(|entry| entry.address == address) {
            return;
        }

        self.serials += 1;

        self.entries.push(Entry {
            address: address.clone(),
            serial: self.serials,
            owner: None,
            item: None,
            reading: false,
            again: false,
            failed: false,
        });

        self.list();
        self.signal("StatusNotifierItemRegistered", &address);

        self.read(self.entries.len() - 1);
    }

    fn owner(&mut self, name: &str, new: &str) {
        if name == watcher::NAME {
            let role = match new {
                "" => Role::Starting,
                new if new == self.own => Role::Own,
                new => Role::Foreign(new.to_owned()),
            };

            self.assume(role);
            return;
        }

        if new.is_empty() {
            self.drop_where(|entry| {
                entry.address.name == name || entry.owner.as_deref() == Some(name)
            });
            return;
        }

        // a well-known name moved to another program, which is read afresh
        let moved: Vec<usize> = (0..self.entries.len())
            .filter(|&at| self.entries[at].address.name == name)
            .collect();

        for at in moved {
            self.read(at);
        }
    }

    // reads an entry's properties on its own thread, or once more after the one under way
    fn read(&mut self, at: usize) {
        let entry = &mut self.entries[at];

        if entry.reading {
            entry.again = true;
            return;
        }

        entry.reading = true;

        let connection = self.connection.clone();
        let events = self.events.clone();
        let address = entry.address.clone();
        let serial = entry.serial;

        thread::spawn(move || {
            let outcome = read(&connection, &address);
            let _ = events.send(Event::Read { serial, outcome });
        });
    }

    fn answered(&mut self, serial: u64, outcome: Outcome) {
        // a read of an entry since dropped
        let Some(at) = self.entries.iter().position(|entry| entry.serial == serial) else {
            return;
        };

        let entry = &mut self.entries[at];
        entry.reading = false;

        match outcome {
            Outcome::Read(owner, described) => {
                entry.owner = Some(owner);
                entry.failed = false;

                let item = shown(entry.address.clone(), described);
                let old = entry.item.replace(item);

                self.forget(old);
            }
            Outcome::Gone => {
                self.drop_where(|entry| entry.serial == serial);
                return;
            }
            Outcome::Failed(why) => {
                if !entry.failed {
                    eprintln!(
                        "kanade: tray item {} could not be read: {why}",
                        entry.address.listed()
                    );
                }

                entry.failed = true;
            }
        }

        let entry = &mut self.entries[at];
        if entry.again {
            entry.again = false;
            self.read(at);
        }
    }

    fn drop_where(&mut self, gone: impl Fn(&Entry) -> bool) {
        let (dropped, kept) = self.entries.drain(..).partition(|entry| gone(entry));
        self.entries = kept;

        let dropped: Vec<Entry> = dropped;

        if dropped.is_empty() {
            return;
        }

        self.list();

        for entry in dropped {
            self.signal("StatusNotifierItemUnregistered", &entry.address);
            self.forget(entry.item);
        }
    }

    // removes the png of an item no longer shown, unless another item draws the same
    fn forget(&self, item: Option<Item>) {
        let Some(pixmap) = item.and_then(|item| item.icon.pixmap) else {
            return;
        };

        let drawn = self.entries.iter().any(|entry| {
            entry
                .item
                .as_ref()
                .is_some_and(|item| item.icon.pixmap.as_ref() == Some(&pixmap))
        });

        if !drawn {
            let _ = fs::remove_file(pixmap);
        }
    }

    // what Kanade's watcher lists
    fn list(&self) {
        *self
            .registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = self
            .entries
            .iter()
            .map(|entry| entry.address.listed())
            .collect();
    }

    // tells other hosts, while Kanade is the watcher
    fn signal(&self, member: &str, address: &Address) {
        if self.role != Role::Own {
            return;
        }

        let sent = self.connection.emit_signal(
            None::<&str>,
            watcher::PATH,
            watcher::INTERFACE,
            member,
            &(address.listed(),),
        );

        if let Err(error) = sent {
            eprintln!("kanade: tray could not send {member}: {error}");
        }
    }

    // a write wakes every window, so only a change writes
    fn publish(&self) {
        let tray = Tray {
            items: self
                .entries
                .iter()
                .filter_map(|entry| entry.item.clone())
                .collect(),
            watcher: self.role.clone(),
        };

        if *Tray::read() != tray {
            *Tray::write() = tray;
        }
    }
}

// registers as a host with another watcher, and what it lists
fn host_with(connection: &Connection, host: &str) -> Vec<Address> {
    let registered = connection.call_method(
        Some(watcher::NAME),
        watcher::PATH,
        Some(watcher::INTERFACE),
        "RegisterStatusNotifierHost",
        &(host,),
    );

    if let Err(error) = registered {
        eprintln!("kanade: tray could not register with the watcher: {error}");
    }

    let listed = connection
        .call_method(
            Some(watcher::NAME),
            watcher::PATH,
            Some(PROPERTIES),
            "Get",
            &(watcher::INTERFACE, "RegisteredStatusNotifierItems"),
        )
        .and_then(|reply| reply.body().deserialize::<OwnedValue>())
        .and_then(|value| Ok(Vec::<String>::try_from(value)?));

    match listed {
        Ok(listed) => listed
            .iter()
            .filter_map(|service| Address::registered(service, None))
            .collect(),
        Err(error) => {
            eprintln!("kanade: tray could not list the watcher's items: {error}");
            Vec::new()
        }
    }
}

fn read(connection: &Connection, address: &Address) -> Outcome {
    let properties = connection
        .call_method(
            Some(address.name.as_str()),
            address.path.as_str(),
            Some(PROPERTIES),
            "GetAll",
            &(ITEM,),
        )
        .and_then(|reply| {
            let owner = reply
                .header()
                .sender()
                .map(|sender| sender.to_string())
                .unwrap_or_else(|| address.name.clone());

            let properties = reply.body().deserialize::<HashMap<String, OwnedValue>>()?;

            Ok((owner, properties))
        });

    match properties {
        Ok((owner, properties)) => Outcome::Read(owner, item::describe(&properties)),
        Err(zbus::Error::MethodError(name, ..)) if GONE.contains(&name.as_str()) => Outcome::Gone,
        Err(error) => Outcome::Failed(error.to_string()),
    }
}

// an item as the views see it, its pixels written for Amane to draw
fn shown(address: Address, described: Described) -> Item {
    let pixmap = described.picture.pixmap.map(|pixmap| {
        raster::write(
            "tray",
            "png",
            &raster::png(pixmap.width, pixmap.height, &pixmap.rgba),
        )
    });

    Item {
        address,
        title: described.title,
        status: described.status,
        icon: Icon {
            name: described.picture.name,
            themes: described.themes.map(PathBuf::from),
            pixmap,
        },
    }
}

/*
 * what clicking or scrolling an item asks of it, x and y where on screen, sent off the caller's
 * thread; an item that fails or does not answer is logged, nothing else
 */
#[expect(dead_code, reason = "the tray's view (#135) clicks items")]
pub fn activate(item: &Item, x: i32, y: i32) {
    call(item, "Activate", (x, y));
}

#[expect(dead_code, reason = "the tray's view (#135) clicks items")]
pub fn secondary_activate(item: &Item, x: i32, y: i32) {
    call(item, "SecondaryActivate", (x, y));
}

#[expect(dead_code, reason = "the tray's view (#135) scrolls items")]
pub fn scroll(item: &Item, delta: i32, orientation: Orientation) {
    let orientation = match orientation {
        Orientation::Vertical => "vertical",
        Orientation::Horizontal => "horizontal",
    };

    call(item, "Scroll", (delta, orientation.to_owned()));
}

fn call<B>(item: &Item, method: &'static str, body: B)
where
    B: serde::Serialize + zbus::zvariant::DynamicType + Send + 'static,
{
    let Some(connection) = CONNECTION.get().and_then(|connection| {
        connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }) else {
        return;
    };

    let address = item.address.clone();

    thread::spawn(move || {
        let called = connection.call_method(
            Some(address.name.as_str()),
            address.path.as_str(),
            Some(ITEM),
            method,
            &body,
        );

        if let Err(error) = called {
            eprintln!(
                "kanade: tray item {} failed {method}: {error}",
                address.listed()
            );
        }
    });
}
