//! D-Bus calls, properties, signals and incoming method calls over zbus (ADR 0027 step 1).
//!
//! A bus that cannot be reached, a program that is not running or a reply of the wrong kind all
//! answer with nothing rather than an error: a missing player or network service is normal, and
//! a view shows the empty value.

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use zbus::blocking::{Connection, MessageIterator};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::message::Type;
use zbus::zvariant::{ObjectPath, Structure, StructureBuilder, Value as Variant};
use zbus::{MatchRule, Message};

const RETRY_INITIAL: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(30);

static SESSION: LazyLock<Manager> = LazyLock::new(|| Manager::new(Connection::session));
static SYSTEM: LazyLock<Manager> = LazyLock::new(|| Manager::new(Connection::system));

type Connector = fn() -> zbus::Result<Connection>;

struct Manager {
    connector: Connector,
    state: Mutex<ManagerState>,
}

struct ManagerState {
    connection: Option<Connection>,
    generation: u64,
    failures: u32,
    retry_at: Option<Instant>,
}

impl Manager {
    fn new(connector: Connector) -> Self {
        Self {
            connector,
            state: Mutex::new(ManagerState {
                connection: None,
                generation: 0,
                failures: 0,
                retry_at: None,
            }),
        }
    }

    fn connection(&self) -> Option<(Connection, u64)> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(connection) = &state.connection {
            if !connection.is_closed() {
                return Some((connection.clone(), state.generation));
            }
            state.connection = None;
            state.generation = state.generation.wrapping_add(1);
            Self::schedule_retry(&mut state, Instant::now());
        }
        let now = Instant::now();
        if state.retry_at.is_some_and(|at| now < at) {
            return None;
        }
        match (self.connector)() {
            Ok(connection) => {
                state.generation = state.generation.wrapping_add(1);
                state.connection = Some(connection.clone());
                state.failures = 0;
                state.retry_at = None;
                Some((connection, state.generation))
            }
            Err(_) => {
                Self::schedule_retry(&mut state, now);
                None
            }
        }
    }

    fn schedule_retry(state: &mut ManagerState, now: Instant) {
        state.failures = state.failures.saturating_add(1);
        let shift = state.failures.saturating_sub(1).min(31);
        let delay = RETRY_INITIAL.saturating_mul(1_u32 << shift).min(RETRY_MAX);
        state.retry_at = Some(now + delay);
    }

    fn disconnected(&self, generation: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.generation == generation && state.connection.is_some() {
            state.connection = None;
            state.generation = state.generation.wrapping_add(1);
            Self::schedule_retry(&mut state, Instant::now());
        }
    }

    fn wait_for_retry(&self) {
        let wait = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state
                .retry_at
                .map(|at| at.saturating_duration_since(Instant::now()))
                .filter(|wait| !wait.is_zero())
                .unwrap_or(RETRY_INITIAL)
        };
        thread::sleep(wait);
    }
}

#[derive(Clone, Copy)]
enum WhichBus {
    Session,
    System,
}

impl WhichBus {
    fn manager(self) -> &'static Manager {
        match self {
            Self::Session => &SESSION,
            Self::System => &SYSTEM,
        }
    }
}

const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

#[derive(Clone, Copy)]
pub struct Bus {
    which: WhichBus,
}

/// What a reply holds, every number as one kind, object paths and signatures as text.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Value {
    #[default]
    Nothing,
    Bool(bool),
    Number(f64),
    Text(String),
    // arrays, and a struct's fields
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

/// What a call sends, each of the exact kind D-Bus checks.
#[derive(Debug, Clone, PartialEq)]
pub enum Argument {
    Bool(bool),
    Int(i32),
    Unsigned(u32),
    Unsigned64(u64),
    Long(i64),
    Float(f64),
    Text(String),
    Path(String),
    TextList(Vec<String>),
    Bytes(Vec<u8>),
    // string to variant, like a Wi-Fi scan's options
    Map(BTreeMap<String, Argument>),
    // groups of string to variant, like a NetworkManager connection's settings
    Groups(BTreeMap<String, BTreeMap<String, Argument>>),
    // fields in order, like a polkit subject's kind and details
    Struct(Vec<Argument>),
    Variant(Box<Argument>),
}

/// A signal heard on the bus.
#[derive(Debug, Clone, PartialEq)]
pub enum SignalEvent {
    Reconnected,
    Signal(Signal),
}

/// A method subscription was attached on a connection, or a call arrived on it.
pub enum MethodEvent {
    Attached(Connection, u64),
    Method(Method),
}

/// A signal heard on the bus.
#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    // the unique name like :1.42, not a well-known one
    sender: String,
    path: String,
    arguments: Vec<Value>,
}

/// A call another program made to Kanade, waiting for its reply.
pub struct Method {
    connection: Connection,
    message: Message,
    name: String,
    arguments: Vec<Value>,
}

static NOTHING: Value = Value::Nothing;

impl Bus {
    /// The user's own programs: media players, notifications, the tray.
    pub fn session() -> Self {
        Self {
            which: WhichBus::Session,
        }
    }

    /// Programs every user shares: NetworkManager, UPower, logind.
    pub fn system() -> Self {
        Self {
            which: WhichBus::System,
        }
    }

    /// The generation of the live connection, without trying to reconnect.
    pub fn generation(&self) -> Option<u64> {
        let state = self
            .which
            .manager()
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .connection
            .as_ref()
            .filter(|connection| !connection.is_closed())
            .map(|_| state.generation)
    }

    /// One value back as itself, several as a list, a failed call as nothing.
    pub fn call(
        &self,
        destination: &str,
        path: &str,
        interface: &str,
        method: &str,
        arguments: &[Argument],
    ) -> Value {
        self.try_call(destination, path, interface, method, arguments)
            .unwrap_or_default()
    }

    /// As `call`, but says why a call failed in the callee's own words, for one whose failure is
    /// worth telling apart from its answer.
    pub fn try_call(
        &self,
        destination: &str,
        path: &str,
        interface: &str,
        method: &str,
        arguments: &[Argument],
    ) -> Result<Value, String> {
        let Some((connection, _)) = self.which.manager().connection() else {
            return Err(String::from("no bus"));
        };

        let reply = if arguments.is_empty() {
            connection.call_method(Some(destination), path, Some(interface), method, &())
        } else {
            connection.call_method(
                Some(destination),
                path,
                Some(interface),
                method,
                &body(arguments),
            )
        };

        let reply = reply.map_err(|error| error.to_string())?;
        let mut values = message_values(&reply);

        Ok(match values.len() {
            0 => Value::Nothing,
            1 => values.remove(0),
            _ => Value::List(values),
        })
    }

    pub fn property(&self, destination: &str, path: &str, interface: &str, name: &str) -> Value {
        self.call(
            destination,
            path,
            PROPERTIES,
            "Get",
            &[Argument::from(interface), Argument::from(name)],
        )
    }

    pub fn set_property(
        &self,
        destination: &str,
        path: &str,
        interface: &str,
        name: &str,
        value: Argument,
    ) {
        self.call(
            destination,
            path,
            PROPERTIES,
            "Set",
            &[
                Argument::from(interface),
                Argument::from(name),
                Argument::Variant(Box::new(value)),
            ],
        );
    }

    /// Each signal of `interface` named `name`, waited for; from a Source's thread, never a view.
    pub fn signals(&self, interface: &str, name: &str) -> impl Iterator<Item = Signal> + use<> {
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .interface(interface)
            .expect("failed to watch signals: bad interface name")
            .member(name)
            .expect("failed to watch signals: bad signal name")
            .build();

        SignalIterator::new(*self, rule.into_owned(), false).filter_map(|event| match event {
            SignalEvent::Signal(signal) => Some(signal),
            SignalEvent::Reconnected => None,
        })
    }

    /// As `signals`, only those the object at `path` sends, so other objects' never wake the thread.
    pub fn signals_from(
        &self,
        path: &str,
        interface: &str,
        name: &str,
    ) -> impl Iterator<Item = Signal> + use<> {
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .path(path)
            .expect("failed to watch signals: bad path")
            .interface(interface)
            .expect("failed to watch signals: bad interface name")
            .member(name)
            .expect("failed to watch signals: bad signal name")
            .build();

        SignalIterator::new(*self, rule.into_owned(), false).filter_map(|event| match event {
            SignalEvent::Signal(signal) => Some(signal),
            SignalEvent::Reconnected => None,
        })
    }

    /// As `signals_from`, with a notification after each successful signal subscription.
    pub fn signals_from_with_reconnect(
        &self,
        path: &str,
        interface: &str,
        name: &str,
    ) -> impl Iterator<Item = SignalEvent> + use<> {
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .path(path)
            .expect("failed to watch signals: bad path")
            .interface(interface)
            .expect("failed to watch signals: bad interface name")
            .member(name)
            .expect("failed to watch signals: bad signal name")
            .build();

        SignalIterator::new(*self, rule.into_owned(), true)
    }

    /// Each call to `path` and `interface`, waited for; watch before taking the name, so no
    /// early call is missed. Reattaches after bus reconnects and signals each new subscription.
    pub fn methods_with_reconnect(
        &self,
        path: &str,
        interface: &str,
    ) -> Box<dyn Iterator<Item = MethodEvent> + Send> {
        let rule = MatchRule::builder()
            .msg_type(Type::MethodCall)
            .path(path)
            .expect("failed to watch method calls: bad path")
            .interface(interface)
            .expect("failed to watch method calls: bad interface name")
            .build();

        Box::new(MethodIterator::new(*self, rule.into_owned()))
    }

    /// A signal from Kanade's object at `path`, to whoever listens.
    pub fn emit(&self, path: &str, interface: &str, name: &str, arguments: &[Argument]) {
        self.send(None, path, interface, name, arguments);
    }

    /// A signal from Kanade's object at `path`, to the one program at the unique name `to`.
    pub fn emit_to(
        &self,
        to: &str,
        path: &str,
        interface: &str,
        name: &str,
        arguments: &[Argument],
    ) {
        self.send(Some(to), path, interface, name, arguments);
    }

    fn send(
        &self,
        to: Option<&str>,
        path: &str,
        interface: &str,
        name: &str,
        arguments: &[Argument],
    ) {
        let Some((connection, _)) = self.which.manager().connection() else {
            return;
        };

        let _ = if arguments.is_empty() {
            connection.emit_signal(to, path, interface, name, &())
        } else {
            connection.emit_signal(to, path, interface, name, &body(arguments))
        };
    }

    /// False when another program already owns the name; Kanade never takes it from them. Claim
    /// it on the method subscription's connection, so calls sent to the name arrive on that server.
    pub fn own_on(&self, connection: &Connection, name: &str) -> bool {
        let reply = connection.request_name_with_flags(name, RequestNameFlags::DoNotQueue.into());

        matches!(
            reply,
            Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner)
        )
    }
}

struct MethodIterator {
    bus: Bus,
    rule: MatchRule<'static>,
    messages: Option<MessageIterator>,
    connection: Option<Connection>,
    generation: Option<u64>,
    attached_pending: bool,
}

impl MethodIterator {
    fn new(bus: Bus, rule: MatchRule<'static>) -> Self {
        let mut iterator = Self {
            bus,
            rule,
            messages: None,
            connection: None,
            generation: None,
            attached_pending: false,
        };
        iterator.subscribe_once();
        iterator
    }

    // the match is set up before the caller registers anything, so no early call is missed
    fn subscribe_once(&mut self) {
        let manager = self.bus.which.manager();
        if let Some((connection, generation)) = manager.connection() {
            match MessageIterator::for_match_rule(self.rule.clone(), &connection, None) {
                Ok(messages) => {
                    self.messages = Some(messages);
                    self.connection = Some(connection);
                    self.generation = Some(generation);
                    self.attached_pending = true;
                }
                Err(_) => manager.disconnected(generation),
            }
        }
    }
}

impl Iterator for MethodIterator {
    type Item = MethodEvent;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.attached_pending {
                self.attached_pending = false;

                if let (Some(connection), Some(generation)) = (&self.connection, self.generation) {
                    return Some(MethodEvent::Attached(connection.clone(), generation));
                }
            }

            if let Some(messages) = &mut self.messages {
                match messages.next() {
                    Some(Ok(message)) => {
                        let connection = self.connection.as_ref()?.clone();
                        return Some(MethodEvent::Method(Method {
                            connection,
                            name: message
                                .header()
                                .member()
                                .map(ToString::to_string)
                                .unwrap_or_default(),
                            arguments: message_values(&message),
                            message,
                        }));
                    }
                    Some(Err(_)) | None => {
                        self.messages = None;
                        self.connection = None;
                        if let Some(generation) = self.generation.take() {
                            self.bus.which.manager().disconnected(generation);
                        }
                    }
                }
            }

            let manager = self.bus.which.manager();
            if let Some((connection, generation)) = manager.connection() {
                match MessageIterator::for_match_rule(self.rule.clone(), &connection, None) {
                    Ok(messages) => {
                        self.messages = Some(messages);
                        self.connection = Some(connection);
                        self.generation = Some(generation);
                        self.attached_pending = true;
                    }
                    Err(_) => {
                        manager.disconnected(generation);
                        manager.wait_for_retry();
                    }
                }
            } else {
                manager.wait_for_retry();
            }
        }
    }
}

struct SignalIterator {
    bus: Bus,
    rule: MatchRule<'static>,
    messages: Option<MessageIterator>,
    generation: Option<u64>,
    notify_reconnect: bool,
    ever_subscribed: bool,
    reconnect_pending: bool,
}

impl SignalIterator {
    fn new(bus: Bus, rule: MatchRule<'static>, notify_reconnect: bool) -> Self {
        let mut iterator = Self {
            bus,
            rule,
            messages: None,
            generation: None,
            notify_reconnect,
            ever_subscribed: false,
            reconnect_pending: false,
        };
        iterator.subscribe_once();
        iterator
    }

    fn subscribe_once(&mut self) {
        let manager = self.bus.which.manager();
        if let Some((connection, generation)) = manager.connection() {
            match MessageIterator::for_match_rule(self.rule.clone(), &connection, None) {
                Ok(messages) => {
                    self.messages = Some(messages);
                    self.generation = Some(generation);
                    self.ever_subscribed = true;
                }
                Err(_) => manager.disconnected(generation),
            }
        }
    }
}

impl Iterator for SignalIterator {
    type Item = SignalEvent;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(messages) = &mut self.messages {
                match messages.next() {
                    Some(Ok(message)) => return Some(SignalEvent::Signal(signal(message))),
                    Some(Err(_)) | None => {
                        self.messages = None;
                        self.reconnect_pending = true;
                        if let Some(generation) = self.generation.take() {
                            self.bus.which.manager().disconnected(generation);
                        }
                    }
                }
            }

            let manager = self.bus.which.manager();
            if let Some((connection, generation)) = manager.connection() {
                match MessageIterator::for_match_rule(self.rule.clone(), &connection, None) {
                    Ok(messages) => {
                        self.messages = Some(messages);
                        self.generation = Some(generation);
                        let should_notify = self.notify_reconnect
                            && (!self.ever_subscribed || self.reconnect_pending);
                        self.ever_subscribed = true;
                        self.reconnect_pending = false;
                        if should_notify {
                            return Some(SignalEvent::Reconnected);
                        }
                    }
                    Err(_) => {
                        manager.disconnected(generation);
                        manager.wait_for_retry();
                    }
                }
            } else {
                manager.wait_for_retry();
            }
        }
    }
}

fn signal(message: Message) -> Signal {
    let header = message.header();

    Signal {
        sender: header.sender().map(ToString::to_string).unwrap_or_default(),
        path: header.path().map(ToString::to_string).unwrap_or_default(),
        arguments: message_values(&message),
    }
}

impl Signal {
    pub fn sender(&self) -> &str {
        &self.sender
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn arguments(&self) -> &[Value] {
        &self.arguments
    }
}

impl Method {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The caller's unique name like :1.42, empty when the bus gave none.
    pub fn sender(&self) -> String {
        self.message
            .header()
            .sender()
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    pub fn arguments(&self) -> &[Value] {
        &self.arguments
    }

    /// A reply that fails means the caller already left.
    pub fn reply(&self, arguments: &[Argument]) {
        let header = self.message.header();

        let _ = if arguments.is_empty() {
            self.connection.reply(&header, &())
        } else {
            self.connection.reply(&header, &body(arguments))
        };
    }

    /// A call Kanade cannot take, as a caller reads it rather than as an answer. The name is one
    /// of the bus's own, like `org.freedesktop.DBus.Error.InvalidArgs`.
    pub fn fail(&self, name: &str, message: &str) {
        let header = self.message.header();

        let _ = self.connection.reply_error(&header, name, &(message,));
    }
}

// each getter answers empty for the wrong kind
impl Value {
    pub fn bool(&self) -> bool {
        matches!(self, Value::Bool(true))
    }

    pub fn number(&self) -> f64 {
        match self {
            Value::Number(number) => *number,
            _ => 0.0,
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Value::Text(text) => text,
            _ => "",
        }
    }

    pub fn list(&self) -> &[Value] {
        match self {
            Value::List(list) => list,
            _ => &[],
        }
    }

    pub fn get(&self, key: &str) -> &Value {
        match self {
            Value::Map(map) => map.get(key).unwrap_or(&NOTHING),
            _ => &NOTHING,
        }
    }
}

impl From<bool> for Argument {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i32> for Argument {
    fn from(value: i32) -> Self {
        Self::Int(value)
    }
}

impl From<u32> for Argument {
    fn from(value: u32) -> Self {
        Self::Unsigned(value)
    }
}

impl From<u64> for Argument {
    fn from(value: u64) -> Self {
        Self::Unsigned64(value)
    }
}

impl From<i64> for Argument {
    fn from(value: i64) -> Self {
        Self::Long(value)
    }
}

impl From<f64> for Argument {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<&str> for Argument {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for Argument {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<Vec<String>> for Argument {
    fn from(value: Vec<String>) -> Self {
        Self::TextList(value)
    }
}

fn value(variant: &Variant) -> Value {
    match variant {
        Variant::Bool(value) => Value::Bool(*value),
        Variant::U8(number) => Value::Number(f64::from(*number)),
        Variant::I16(number) => Value::Number(f64::from(*number)),
        Variant::U16(number) => Value::Number(f64::from(*number)),
        Variant::I32(number) => Value::Number(f64::from(*number)),
        Variant::U32(number) => Value::Number(f64::from(*number)),
        Variant::I64(number) => Value::Number(*number as f64),
        Variant::U64(number) => Value::Number(*number as f64),
        Variant::F64(number) => Value::Number(*number),
        Variant::Str(text) => Value::Text(text.to_string()),
        Variant::Signature(signature) => Value::Text(signature.to_string()),
        Variant::ObjectPath(path) => Value::Text(path.to_string()),
        Variant::Value(inner) => value(inner),
        Variant::Array(array) => Value::List(array.inner().iter().map(value).collect()),
        Variant::Structure(structure) => {
            Value::List(structure.fields().iter().map(value).collect())
        }
        Variant::Dict(dict) => Value::Map(
            dict.iter()
                .map(|(key, entry)| (key_text(key), value(entry)))
                .collect(),
        ),
        _ => Value::Nothing,
    }
}

// keys are strings but for the odd number
fn key_text(key: &Variant) -> String {
    match value(key) {
        Value::Text(text) => text,
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

// one value per argument of the body; a body read as a structure reads any body
fn message_values(message: &Message) -> Vec<Value> {
    let body = message.body();

    if body.is_empty() {
        return Vec::new();
    }

    body.deserialize::<Structure>()
        .map(|structure| structure.fields().iter().map(value).collect())
        .unwrap_or_default()
}

fn body(arguments: &[Argument]) -> Structure<'static> {
    arguments
        .iter()
        .fold(StructureBuilder::new(), |builder, argument| {
            builder.append_field(variant(argument))
        })
        .build()
        .expect("failed to build D-Bus arguments")
}

fn variant(argument: &Argument) -> Variant<'static> {
    match argument {
        Argument::Bool(value) => Variant::Bool(*value),
        Argument::Int(number) => Variant::I32(*number),
        Argument::Unsigned(number) => Variant::U32(*number),
        Argument::Unsigned64(number) => Variant::U64(*number),
        Argument::Long(number) => Variant::I64(*number),
        Argument::Float(number) => Variant::F64(*number),
        Argument::Text(text) => Variant::from(text.clone()),
        Argument::Path(path) => Variant::ObjectPath(
            ObjectPath::try_from(path.clone())
                .expect("failed to send object path: it must look like /org/example"),
        ),
        Argument::TextList(texts) => Variant::from(texts.clone()),
        Argument::Bytes(bytes) => Variant::from(bytes.clone()),
        Argument::Map(map) => Variant::from(entries(map)),
        Argument::Groups(groups) => Variant::from(
            groups
                .iter()
                .map(|(name, group)| (name.clone(), entries(group)))
                .collect::<HashMap<_, _>>(),
        ),
        Argument::Struct(fields) => Variant::Structure(body(fields)),
        Argument::Variant(inner) => Variant::Value(Box::new(variant(inner))),
    }
}

// each entry keeps its own kind, sent as string to variant
fn entries(map: &BTreeMap<String, Argument>) -> HashMap<String, Variant<'static>> {
    map.iter()
        .map(|(key, argument)| (key.clone(), variant(argument)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_keep_their_kind_and_replies_read_back() {
        let mut options = BTreeMap::new();
        options.insert(String::from("ssids"), Argument::Bytes(vec![1, 2]));

        let sent = body(&[
            Argument::from("wlan0"),
            Argument::Path(String::from("/org/freedesktop/NetworkManager")),
            Argument::from(7_u32),
            Argument::Map(options),
            Argument::from(1_u64 << 40),
            // a struct's fields read back as a list, which is how polkit's subject is taken
            Argument::Struct(vec![
                Argument::from("unix-session"),
                Argument::Map(BTreeMap::from([(
                    String::from("session-id"),
                    Argument::from("3"),
                )])),
            ]),
        ]);

        assert_eq!(sent.fields()[5].value_signature().to_string(), "(sa{sv})");

        let read: Vec<Value> = sent.fields().iter().map(value).collect();

        assert_eq!(read[0].text(), "wlan0");
        assert_eq!(read[1].text(), "/org/freedesktop/NetworkManager");
        assert_eq!(read[2].number(), 7.0);
        assert_eq!(read[3].get("ssids").list().len(), 2);
        assert_eq!(read[3].get("missing"), &Value::Nothing);
        // a window id is a `t`, which a `u` is refused for
        assert_eq!(sent.fields()[4].value_signature().to_string(), "t");
        assert_eq!(read[4].number(), (1_u64 << 40) as f64);

        // a struct's fields read back as a list, in order
        assert_eq!(read[5].list()[0].text(), "unix-session");
        assert_eq!(read[5].list()[1].get("session-id").text(), "3");
    }
}
