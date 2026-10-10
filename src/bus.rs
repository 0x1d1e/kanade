//! D-Bus calls, properties, signals and incoming method calls over zbus (ADR 0027 step 1).
//!
//! A bus that cannot be reached, a program that is not running or a reply of the wrong kind all
//! answer with nothing rather than an error: a missing player or network service is normal, and
//! a view shows the empty value.

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use zbus::blocking::{Connection, MessageIterator};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::message::Type;
use zbus::zvariant::{ObjectPath, Structure, StructureBuilder, Value as Variant};
use zbus::{MatchRule, Message};

// one connection per bus, shared by every thread; tried once, so a bus started later needs a restart
static SESSION: LazyLock<Option<Connection>> = LazyLock::new(|| Connection::session().ok());
static SYSTEM: LazyLock<Option<Connection>> = LazyLock::new(|| Connection::system().ok());

const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

#[derive(Clone, Copy)]
pub struct Bus {
    connection: Option<&'static Connection>,
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
    Variant(Box<Argument>),
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
    connection: &'static Connection,
    message: Message,
    name: String,
    arguments: Vec<Value>,
}

static NOTHING: Value = Value::Nothing;

impl Bus {
    /// The user's own programs: media players, notifications, the tray.
    pub fn session() -> Self {
        Self {
            connection: SESSION.as_ref(),
        }
    }

    /// Programs every user shares: NetworkManager, UPower, logind.
    pub fn system() -> Self {
        Self {
            connection: SYSTEM.as_ref(),
        }
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
        let Some(connection) = self.connection else {
            return Value::Nothing;
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

        let Ok(reply) = reply else {
            return Value::Nothing;
        };

        let mut values = message_values(&reply);

        match values.len() {
            0 => Value::Nothing,
            1 => values.remove(0),
            _ => Value::List(values),
        }
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

        self.messages(rule.into_owned()).map(|message| {
            let header = message.header();

            Signal {
                sender: header.sender().map(ToString::to_string).unwrap_or_default(),
                path: header.path().map(ToString::to_string).unwrap_or_default(),
                arguments: message_values(&message),
            }
        })
    }

    /// Each call to `path` and `interface`, waited for; watch before taking the name, so no
    /// early call is missed.
    pub fn methods(&self, path: &str, interface: &str) -> impl Iterator<Item = Method> + use<> {
        let rule = MatchRule::builder()
            .msg_type(Type::MethodCall)
            .path(path)
            .expect("failed to watch method calls: bad path")
            .interface(interface)
            .expect("failed to watch method calls: bad interface name")
            .build();

        let connection = self.connection;

        self.messages(rule.into_owned()).filter_map(move |message| {
            Some(Method {
                connection: connection?,
                name: message
                    .header()
                    .member()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                arguments: message_values(&message),
                message,
            })
        })
    }

    /// A signal from Kanade's object at `path`, to whoever listens.
    pub fn emit(&self, path: &str, interface: &str, name: &str, arguments: &[Argument]) {
        let Some(connection) = self.connection else {
            return;
        };

        let _ = if arguments.is_empty() {
            connection.emit_signal(None::<&str>, path, interface, name, &())
        } else {
            connection.emit_signal(None::<&str>, path, interface, name, &body(arguments))
        };
    }

    /// False when another program already owns the name; Kanade never takes it from them.
    pub fn own(&self, name: &str) -> bool {
        let Some(connection) = self.connection else {
            return false;
        };

        let reply = connection.request_name_with_flags(name, RequestNameFlags::DoNotQueue.into());

        matches!(
            reply,
            Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner)
        )
    }

    // none without a bus; a message that fails to read is skipped
    fn messages(self, rule: MatchRule<'static>) -> impl Iterator<Item = Message> + use<> {
        self.connection
            .and_then(|connection| MessageIterator::for_match_rule(rule, connection, None).ok())
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
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
        ]);

        let read: Vec<Value> = sent.fields().iter().map(value).collect();

        assert_eq!(read[0].text(), "wlan0");
        assert_eq!(read[1].text(), "/org/freedesktop/NetworkManager");
        assert_eq!(read[2].number(), 7.0);
        assert_eq!(read[3].get("ssids").list().len(), 2);
        assert_eq!(read[3].get("missing"), &Value::Nothing);
    }
}
