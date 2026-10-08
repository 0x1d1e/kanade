//! The tray (#134, ADR 0011): the StatusNotifierItems apps show, hosted by Kanade. Kanade serves the
//! StatusNotifierWatcher items register with, unless another program already does; then it hosts
//! that one's items, and takes over the name once that program quits. Each item's properties are
//! read again when it says they changed, off this thread, so a slow or dead item stalls only its
//! own read. An item that quits, or has nothing at its address, is dropped; one that answers with
//! something else stays as last read, hidden if never read, until a change reads it whole. Losing
//! the bus ends the run, its items withdrawn, and it connects again, as items register again with
//! a watcher that comes back.
//!
//! Over zbus, not Amane's `Bus`: an item may register with only its object path, and the bus name
//! it lives at is then the caller's, which Amane's `Method` does not give.

mod item;
pub mod menu;
mod watcher;

use std::collections::HashMap;
use std::convert::Infallible;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use amane::Service;
use enumflags2::BitFlags;
use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::{Connection, MessageIterator, connection};
use zbus::fdo::RequestNameReply;
use zbus::message::Type;
use zbus::names::BusName;
use zbus::zvariant::OwnedValue;

use super::icons;

pub use item::Status;
use item::{Address, Described};
use watcher::Watcher;

use crate::island::service::IslandService;
use crate::{raster, supervise};

const ITEM: &str = "org.kde.StatusNotifierItem";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

// how long an item may take to answer before its read, or an action on it, is given up
const TIMEOUT: Duration = Duration::from_secs(2);

// how long until a run that ended connects again, doubling up to `LAST_RETRY` while it keeps
// ending, back to `FIRST_RETRY` once one lasted `HEALTHY`
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LAST_RETRY: Duration = Duration::from_secs(60);
const HEALTHY: Duration = Duration::from_secs(60);

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

impl Role {
    /*
     * whether what a watcher said of an item counts. Kanade's own watcher (none) always: it takes
     * only calls the bus routed by the watcher's name, so Kanade held it then, whatever the role
     * says yet. Another only while it holds the name, so any program's signal is not taken for it
     */
    fn heeds(&self, watcher: Option<&str>) -> bool {
        match (self, watcher) {
            (_, None) => true,
            (Role::Foreign(owner), Some(watcher)) => owner == watcher,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    address: Address,

    // the unique name that answered its read, which actions go to: a well-known name may move to
    // another program
    owner: String,

    // its entry's, never reused, so an old row cannot reach what replaced it
    serial: u64,

    pub title: String,
    pub status: Status,
    pub icon: Icon,

    // the object its menu is at
    menu: Option<String>,

    // it only shows its menu, so a click opens that rather than activating it
    pub is_menu: bool,
}

impl Item {
    // which item it is, for as long as it is shown as read
    pub fn key(&self) -> u64 {
        self.serial
    }

    pub fn has_menu(&self) -> bool {
        self.menu.is_some()
    }

    // what its primary press asks: one that is only a menu shows it, the item drawing it without one
    pub fn press(&self) -> Press {
        match (self.is_menu, self.has_menu()) {
            (true, true) => Press::Menu,
            (true, false) => Press::ContextMenu,
            (false, _) => Press::Activate,
        }
    }
}

// what a primary press on an item does
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    // its menu, shown by Kanade
    Menu,

    // `ContextMenu`, for an item that is only a menu but has none Kanade can show
    ContextMenu,

    Activate,
}

// what to draw: the file its icon name stands for, else its own pixels, else neither
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Icon {
    pub file: Option<PathBuf>,

    // its pixels, written as a png
    pub pixmap: Option<PathBuf>,
}

// which way a scroll turns
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Vertical,
    Horizontal,
}

// what the tray thread hears, from the bus or a read it started
#[derive(Debug)]
enum Event {
    // an item registered, with Kanade's watcher when none, else the unique name of the one that
    // said so
    Registered {
        address: Address,
        watcher: Option<String>,
    },

    // the watcher of that unique name dropped this item
    Unregistered {
        address: Address,
        watcher: String,
    },

    // an item said one of its properties changed: who said it, at which object
    Changed {
        sender: String,
        path: String,
    },

    // an item said its menu changed: who said it, at which object
    MenuChanged {
        sender: String,
        path: String,
    },

    // a bus name has a new owner, none when empty
    Owner {
        name: String,
        new: String,
    },

    // what reading an entry found, by its serial
    Read {
        serial: u64,
        outcome: Outcome,
    },

    // the bus thread heard the last of the connection, and why
    Ended(zbus::Error),
}

#[derive(Debug)]
enum Outcome {
    // the unique name that answered, what it said, and the file its icon name stands for
    Read(String, Described, Option<PathBuf>),

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

impl Entry {
    /*
     * as if just registered, under a new serial so a read under way, or a row of the old item,
     * matches nothing; returns the old item
     */
    fn unread(&mut self) -> Option<Item> {
        self.serial = SERIALS.fetch_add(1, Ordering::Relaxed);
        self.owner = None;
        self.reading = false;
        self.again = false;
        self.failed = false;

        self.item.take()
    }
}

// what actions on items may reach, while a run is hosting
struct Reach {
    connection: Connection,

    // each shown item's serial and owner
    shown: Vec<(u64, String)>,
}

// whether a row is of an item shown now, read from the same program
fn reaches(shown: &[(u64, String)], item: &Item) -> bool {
    shown
        .iter()
        .any(|(serial, owner)| *serial == item.serial && *owner == item.owner)
}

static REACH: Mutex<Option<Reach>> = Mutex::new(None);

// an entry's serial, across runs
static SERIALS: AtomicU64 = AtomicU64::new(0);

// runs for good, connecting again after a run fails to start or loses the bus
pub fn follow() {
    supervise::spawn("tray", || {
        let mut wait = FIRST_RETRY;

        loop {
            let started = Instant::now();
            let Err(error) = run();

            wait = retry(wait, started.elapsed());
            eprintln!("kanade: tray ended ({error}), connecting again in {wait:?}");

            thread::sleep(wait);
            wait = (wait * 2).min(LAST_RETRY);
        }
    });
}

// how long until a run that lasted `ran` connects again, `wait` if it ended soon
fn retry(wait: Duration, ran: Duration) -> Duration {
    if ran >= HEALTHY { FIRST_RETRY } else { wait }
}

// hosts items until the connection ends
fn run() -> zbus::Result<Infallible> {
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
        format!("type='signal',interface='{}'", menu::INTERFACE),
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

    // withdrawn when `tray` drops
    *reach() = Some(Reach {
        connection: connection.clone(),
        shown: Vec::new(),
    });

    let mut tray = Hosting {
        connection,
        own,
        events,
        registered,
        role: Role::Starting,
        entries: Vec::new(),
    };

    tray.assume(role);
    tray.publish();

    Err(tray.listen(&heard))
}

fn reach() -> MutexGuard<'static, Option<Reach>> {
    REACH.lock().unwrap_or_else(PoisonError::into_inner)
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

// turns what the bus sends into events, the last its end; it only sends, so it never falls behind
fn forward(messages: MessageIterator, own: &str, events: &Sender<Event>) {
    let mut last = None;

    for message in messages {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                last = Some(error);
                continue;
            }
        };

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

                Event::Registered {
                    address,
                    watcher: Some(sender.to_string()),
                }
            }
            (watcher::INTERFACE, "StatusNotifierItemUnregistered") => {
                let Ok((service,)) = body.deserialize::<(String,)>() else {
                    continue;
                };
                let Some(address) = Address::registered(&service, None) else {
                    continue;
                };

                Event::Unregistered {
                    address,
                    watcher: sender.to_string(),
                }
            }

            (ITEM, _) => {
                let Some(path) = header.path() else { continue };

                Event::Changed {
                    sender: sender.to_string(),
                    path: path.to_string(),
                }
            }

            (menu::INTERFACE, "LayoutUpdated" | "ItemsPropertiesUpdated") => {
                let Some(path) = header.path() else { continue };

                Event::MenuChanged {
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

    let why = last.unwrap_or_else(|| zbus::Error::Failure(String::from("the bus closed")));
    let _ = events.send(Event::Ended(why));
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
}

impl Hosting {
    // until the bus thread hears the connection end, and why
    fn listen(&mut self, heard: &Receiver<Event>) -> zbus::Error {
        loop {
            // never closed: `self` keeps a sender
            let Ok(event) = heard.recv() else {
                return zbus::Error::Failure(String::from("no events"));
            };

            match event {
                Event::Registered { address, watcher } => {
                    if self.role.heeds(watcher.as_deref()) {
                        self.register(address);
                    }
                }
                Event::Unregistered { address, watcher } => {
                    if self.role.heeds(Some(&watcher)) {
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
                Event::MenuChanged { sender, path } => menu::changed(&sender, &path),
                Event::Owner { name, new } => self.owner(&name, &new),
                Event::Read { serial, outcome } => self.answered(serial, outcome),
                Event::Ended(why) => return why,
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
            Role::Foreign(watcher) => {
                // off this thread: another watcher may be slow to answer
                let connection = self.connection.clone();
                let events = self.events.clone();
                let host = host();

                thread::spawn(move || {
                    for address in host_with(&connection, &host) {
                        let registered = Event::Registered {
                            address,
                            watcher: Some(watcher.clone()),
                        };

                        if events.send(registered).is_err() {
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

        self.entries.push(Entry {
            address: address.clone(),
            serial: SERIALS.fetch_add(1, Ordering::Relaxed),
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

        /*
         * a well-known name moved to another program: what the last one said is withdrawn at once,
         * so its row neither stays shown nor reaches it, and the new one hidden until read
         */
        let moved: Vec<usize> = (0..self.entries.len())
            .filter(|&at| {
                let entry = &self.entries[at];
                entry.address.name == name && entry.owner.as_deref() != Some(new)
            })
            .collect();

        for at in moved {
            let old = self.entries[at].unread();
            self.forget(old);
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
            Outcome::Read(owner, described, file) => {
                entry.owner = Some(owner.clone());
                entry.failed = false;

                let item = shown(entry.address.clone(), owner, serial, described, file);
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

        if let Some(reach) = reach().as_mut() {
            reach.shown = tray
                .items
                .iter()
                .map(|item| (item.serial, item.owner.clone()))
                .collect();
        }

        if *Tray::read() != tray {
            *Tray::write() = tray;
        }

        // the strip shows those that are not passive
        let count = self
            .entries
            .iter()
            .filter_map(|entry| entry.item.as_ref())
            .filter(|item| item.status != Status::Passive)
            .count();

        if IslandService::read().tray() != count {
            IslandService::write().set_tray(count, Instant::now());
        }
    }
}

// a run that ends, a panic too, withdraws what it showed and what actions may reach
impl Drop for Hosting {
    fn drop(&mut self) {
        *reach() = None;

        for entry in self.entries.drain(..) {
            if let Some(pixmap) = entry.item.and_then(|item| item.icon.pixmap) {
                let _ = fs::remove_file(pixmap);
            }
        }

        self.role = Role::Starting;
        self.publish();
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
        Ok((owner, properties)) => {
            let described = item::describe(&properties);
            let file = icons::find(
                &described.picture.name,
                described.themes.as_deref().map(Path::new),
            );

            Outcome::Read(owner, described, file)
        }
        Err(zbus::Error::MethodError(name, ..)) if GONE.contains(&name.as_str()) => Outcome::Gone,
        Err(error) => Outcome::Failed(error.to_string()),
    }
}

// an item as the views see it, its pixels written for Amane to draw
fn shown(
    address: Address,
    owner: String,
    serial: u64,
    described: Described,
    file: Option<PathBuf>,
) -> Item {
    let pixmap = described.picture.pixmap.map(|pixmap| {
        raster::write(
            "tray",
            "png",
            &raster::png(pixmap.width, pixmap.height, &pixmap.rgba),
        )
    });

    Item {
        address,
        owner,
        serial,
        title: described.title,
        status: described.status,
        icon: Icon { file, pixmap },
        menu: described.menu,
        is_menu: described.is_menu,
    }
}

/*
 * what clicking or scrolling an item asks of it, x and y where on screen, sent off the caller's
 * thread to the program that answered its read, while it is still shown as read; an item that
 * fails or does not answer is logged, nothing else
 */
pub fn activate(item: &Item, x: i32, y: i32) {
    call(item, "Activate", (x, y));
}

pub fn secondary_activate(item: &Item, x: i32, y: i32) {
    call(item, "SecondaryActivate", (x, y));
}

pub fn scroll(item: &Item, delta: i32, orientation: Orientation) {
    let orientation = match orientation {
        Orientation::Vertical => "vertical",
        Orientation::Horizontal => "horizontal",
    };

    call(item, "Scroll", (delta, orientation.to_owned()));
}

// for an item without a menu of its own: it shows its menu itself
pub fn context_menu(item: &Item, x: i32, y: i32) {
    call(item, "ContextMenu", (x, y));
}

fn call<B>(item: &Item, method: &'static str, body: B)
where
    B: serde::Serialize + zbus::zvariant::DynamicType + Send + 'static,
{
    let Some(connection) = connection(item) else {
        return;
    };

    let address = item.address.clone();
    let owner = item.owner.clone();

    thread::spawn(move || {
        let called = connection.call_method(
            Some(owner.as_str()),
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

// what reaches `item`, while it is shown as read
fn connection(item: &Item) -> Option<Connection> {
    reach()
        .as_ref()
        .filter(|reach| reaches(&reach.shown, item))
        .map(|reach| reach.connection.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(serial: u64, owner: &str) -> Item {
        Item {
            address: Address::registered("org.test.Moving", None).unwrap(),
            owner: owner.to_owned(),
            serial,
            title: String::new(),
            status: Status::Active,
            icon: Icon::default(),
            menu: None,
            is_menu: false,
        }
    }

    // #135: an item that is only a menu never gets Activate, as the spec asks
    #[test]
    fn a_press_on_an_item_that_is_only_a_menu_shows_a_menu() {
        let plain = item(1, ":1.1");
        let menu = Item {
            menu: Some(String::from("/MenuBar")),
            ..plain.clone()
        };

        assert_eq!(plain.press(), Press::Activate);
        assert_eq!(menu.press(), Press::Activate);

        assert_eq!(
            Item {
                is_menu: true,
                ..menu
            }
            .press(),
            Press::Menu
        );
        assert_eq!(
            Item {
                is_menu: true,
                ..plain
            }
            .press(),
            Press::ContextMenu
        );
    }

    #[test]
    fn only_the_foreign_watcher_holding_the_name_is_heeded() {
        let foreign = Role::Foreign(String::from(":1.5"));

        assert!(foreign.heeds(Some(":1.5")));
        assert!(!foreign.heeds(Some(":1.9")));
        assert!(!Role::Own.heeds(Some(":1.5")));
        assert!(!Role::Starting.heeds(Some(":1.5")));

        // Kanade's own watcher, called by the watcher's name: Kanade held it, the role may lag
        assert!(foreign.heeds(None));
        assert!(Role::Own.heeds(None));
    }

    #[test]
    fn a_row_reaches_only_the_program_that_answered_while_it_is_shown() {
        let shown = vec![(4, String::from(":1.7"))];

        assert!(reaches(&shown, &item(4, ":1.7")));

        // the name moved to another program, which was read again
        assert!(!reaches(&shown, &item(4, ":1.3")));

        // an entry since dropped, its address registered again
        assert!(!reaches(&shown, &item(2, ":1.7")));

        // no run is hosting
        assert!(!reaches(&[], &item(4, ":1.7")));
    }

    #[test]
    fn an_entry_unread_drops_what_its_last_owner_said() {
        let mut entry = Entry {
            address: Address::registered("org.test.Moving", None).unwrap(),
            serial: 4,
            owner: Some(String::from(":1.7")),
            item: Some(item(4, ":1.7")),
            reading: true,
            again: true,
            failed: true,
        };

        assert_eq!(entry.unread(), Some(item(4, ":1.7")));
        assert_ne!(entry.serial, 4);
        assert_eq!(entry.owner, None);
        assert_eq!(entry.item, None);
        assert!(!entry.reading && !entry.again && !entry.failed);
    }

    #[test]
    fn a_run_that_keeps_ending_waits_longer_until_one_lasts() {
        let mut wait = FIRST_RETRY;
        let mut waits = Vec::new();

        for _ in 0..8 {
            wait = retry(wait, Duration::from_secs(1));
            waits.push(wait.as_secs());
            wait = (wait * 2).min(LAST_RETRY);
        }

        assert_eq!(waits, [1, 2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(retry(LAST_RETRY, HEALTHY), FIRST_RETRY);
    }
}
