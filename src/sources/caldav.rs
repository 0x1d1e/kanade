//! CalDAV accounts (ADR 0044, docs/design.md Calendar): Nextcloud, Fastmail, iCloud with an app
//! password, Radicale and other CalDAV servers, synced read-only into files the `calendar` Module
//! reads beside the local ones, as Google's are (ADR 0016).
//!
//! Each account is a `[[calendar.caldav]]` entry: a name, the server's address and the username.
//! Its password is kept only in the Secret Service (`secrets`), under the address and username, so
//! it goes to no other server even if the entry is edited. `kanade caldav login <name>` reads it
//! from the terminal unseen, checks it with the server, then keeps it, all in the `kanade` process
//! that asked; it never crosses Kanade's socket, a log or a file.
//!
//! The sync runs on this source's thread. Discovery asks the address for the user's principal
//! (`current-user-principal`), else `/.well-known/caldav`, then the principal for its
//! `calendar-home-set` and the home for its calendars that hold events. Each calendar's events
//! from `PAST` before now until `AHEAD` after come in one `calendar-query` REPORT, unexpanded, so
//! a recurring event keeps its rule and its alarms, and are written as one `.ics` file per calendar
//! under `$XDG_CACHE_HOME/kanade/caldav/<account>/`. Again every `INTERVAL`, on `kanade caldav
//! sync` and on a config reload; a failed sync tries again after `RETRY`, doubling up to
//! `INTERVAL`. A file whose events did not change is not written.
//!
//! The password goes only over HTTPS, or plain HTTP to this machine, as Basic authentication. A
//! redirect to another server is not followed; an address a server names for its own calendars,
//! as iCloud does, is, over HTTPS. With no account configured, the thread waits on its queue and
//! nothing touches the network or the keyring. A problem keeps the events synced before, and the
//! Calendar Surface and `status` say it.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::DateTime;
use kanade_runtime::{queue::Queue, service::Service};
use ring::digest;
use roxmltree::{Document, Node};
use ureq::Agent;
use ureq::http::{self, Method, Uri};
use zeroize::Zeroizing;

use super::{calendar, secrets};
use crate::clock;
use crate::config::{self, CalDav};

// a sync's interval, and the longest wait after failures
const INTERVAL: Duration = Duration::from_secs(15 * 60);

// the first wait after a failed sync, doubling with each failure in a row
const RETRY: Duration = Duration::from_secs(60);

// the events synced: a year back, as Google's, to two ahead
const PAST: i64 = 366 * 24 * 60 * 60;
const AHEAD: i64 = 2 * 366 * 24 * 60 * 60;

// an answer's largest, as the calendar reads no larger file
const BODY: u64 = 32 * 1024 * 1024;

// a request's longest, start to end
const PATIENCE: Duration = Duration::from_secs(60);

// the redirects followed on one server
const HOPS: usize = 5;

// the calendars of one account synced at most
const CALENDARS: usize = 100;

const DIRECTORY: &str = "kanade/caldav";

const DAV: &str = "DAV:";
const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";

// why an account does not sync
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    // no password kept for it
    LoggedOut,

    // the server refused the username or password
    Refused,

    // the server could not be reached, in ureq's words
    Offline(String),

    // the server answered what is no calendar, in Kanade's words with its status
    Server(String),

    // the Secret Service has no password to give, in its words
    Keyring(String),

    // the synced events could not be written
    Disk(String),
}

impl Problem {
    // in a few words, for the Calendar Surface
    fn brief(&self) -> &'static str {
        match self {
            Problem::LoggedOut => "log in",
            Problem::Refused => "wrong password",
            Problem::Offline(_) => "offline",
            Problem::Server(_) | Problem::Disk(_) => "sync failed",
            Problem::Keyring(_) => "keyring unavailable",
        }
    }

    // in full, for `status` and `kanade caldav login`, of the account `name`
    pub fn says(&self, name: &str) -> String {
        match self {
            Problem::LoggedOut => {
                format!("no password; log in with `kanade caldav login {name}`")
            }
            Problem::Refused => format!(
                "the server refused the username or password; log in again with \
                 `kanade caldav login {name}`"
            ),
            Problem::Offline(why) => format!("the server cannot be reached: {why}"),
            Problem::Server(why) => why.clone(),
            Problem::Keyring(why) => format!("no password: {why}"),
            Problem::Disk(why) => format!("cannot write the synced events: {why}"),
        }
    }
}

// how one account stands
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Standing {
    pub name: String,

    // when its events on disk were synced, in seconds since the epoch, and how many calendars
    pub synced: Option<(i64, usize)>,
    pub problem: Option<Problem>,
}

impl Standing {
    // a line for `status`, with when it last synced read by `local`
    fn status(&self, local: impl Fn(i64) -> Option<String>) -> String {
        let synced = self
            .synced
            .and_then(|(at, calendars)| Some((local(at)?, calendars)));

        let said = match (&self.problem, synced) {
            (None, None) => String::from("syncing"),
            (None, Some((at, calendars))) => format!("synced at {at}, {calendars} calendars"),
            (Some(problem), None) => problem.says(&self.name),
            (Some(problem), Some((at, _))) => {
                format!(
                    "{}; showing the events synced at {at}",
                    problem.says(&self.name)
                )
            }
        };

        format!("caldav {}: {said}", self.name)
    }
}

// the accounts configured, as the Calendar Surface and `status` read them
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Accounts {
    pub accounts: Vec<Standing>,
}

impl Service for Accounts {
    fn new() -> Self {
        Accounts::default()
    }

    fn listen() {}
}

// what `kanade caldav` asks
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    // asked of the terminal and kept by the `kanade` that runs it, never of the shell
    Login(String),
    Logout(String),

    // one account, or all
    Sync(Option<String>),
    Status,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            ["login", name] => Some(Request::Login((*name).to_owned())),
            ["logout", name] => Some(Request::Logout((*name).to_owned())),
            ["sync"] => Some(Request::Sync(None)),
            ["sync", name] => Some(Request::Sync(Some((*name).to_owned()))),
            ["status"] => Some(Request::Status),
            _ => None,
        }
    }
}

// what the sync thread is told
enum Message {
    // one account, its password asked again as after a login, or all
    Sync(Option<String>),

    // the config was read again
    Reread,
}

static QUEUE: LazyLock<Queue<Message>> = LazyLock::new(Queue::new);

// what the shell answers `kanade caldav`, at once, on the draw thread
pub fn request(request: Request) -> Result<String, String> {
    match request {
        Request::Login(_) | Request::Logout(_) => Err(String::from(
            "login and logout run in the terminal, with `kanade caldav login <name>`",
        )),
        Request::Sync(name) => {
            let accounts = config::get().caldav.clone();

            if accounts.is_empty() {
                return Err(String::from("no CalDAV account in calendar.caldav"));
            }
            if let Some(name) = &name {
                configured(&accounts, name)?;
            }

            QUEUE.send(Message::Sync(name));
            Ok(String::from("syncing"))
        }
        Request::Status => Ok(match status() {
            lines if lines.is_empty() => String::from("caldav: no accounts in calendar.caldav"),
            lines => lines.join("\n"),
        }),
    }
}

// a line per account for `status`
pub fn status() -> Vec<String> {
    Accounts::read()
        .accounts
        .iter()
        .map(|standing| {
            standing.status(|seconds| {
                Some(
                    clock::local_time(seconds)?
                        .format("%Y-%m-%d %H:%M")
                        .to_string(),
                )
            })
        })
        .collect()
}

// whether a `caldav status` line is of an account syncing as it should, for `kanade doctor`
pub fn healthy(line: &str) -> bool {
    line.split_once(": ")
        .is_some_and(|(_, said)| said.starts_with("synced at ") || said == "syncing")
}

/*
 * why the accounts do not sync, in a few words for the Calendar Surface; none when they all do.
 * Read on the draw thread
 */
pub fn brief() -> Option<String> {
    let accounts = Accounts::read();
    let mut failing = accounts
        .accounts
        .iter()
        .filter_map(|standing| Some((&standing.name, standing.problem.as_ref()?)));

    let (name, problem) = failing.next()?;
    match failing.count() {
        0 => Some(format!("CalDAV {name}: {}", problem.brief())),
        more => Some(format!("CalDAV: {} accounts not syncing", more + 1)),
    }
}

// the config was read again; a new or changed account syncs at once
pub fn reread() {
    QUEUE.send(Message::Reread);
}

// where the synced events of the configured accounts are, for the calendar to read
pub fn places() -> Vec<PathBuf> {
    let Some(dir) = directory() else {
        return Vec::new();
    };

    config::get()
        .caldav
        .iter()
        .map(|account| dir.join(key(account)))
        .filter(|dir| dir.is_dir())
        .collect()
}

// where all accounts' synced events are kept
fn directory() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok();

    config::base("XDG_CACHE_HOME", ".cache", home.as_deref()).map(|dir| dir.join(DIRECTORY))
}

/*
 * an account's own name on disk: a hash of its server and username, which its password is kept
 * under too, so another server's events are never shown as its own
 */
fn key(account: &CalDav) -> String {
    hex(&format!("{}\0{}", account.url, account.username))
}

// the first 8 bytes of the SHA-256 of `text`, in hex
fn hex(text: &str) -> String {
    digest::digest(&digest::SHA256, text.as_bytes()).as_ref()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn configured<'a>(accounts: &'a [CalDav], name: &str) -> Result<&'a CalDav, String> {
    accounts
        .iter()
        .find(|account| account.name == name)
        .ok_or_else(|| format!("no CalDAV account named {name} in calendar.caldav"))
}

// what finds an account's password in the Secret Service
fn attributes(account: &CalDav) -> [(&str, &str); 4] {
    [
        ("application", "kanade"),
        ("account", "caldav"),
        ("server", &account.url),
        ("username", &account.username),
    ]
}

fn password(account: &CalDav) -> Result<Option<Zeroizing<String>>, String> {
    secrets::load(&attributes(account), |bytes| {
        std::str::from_utf8(bytes)
            .ok()
            .map(|text| Zeroizing::new(text.to_owned()))
    })
}

/*
 * `kanade caldav login <name>`, in the `kanade` process: asks the terminal for the password,
 * checks it with the server, and keeps it. What it says is for the terminal
 */
pub fn login(name: &str) -> Result<String, String> {
    let (config, _) = config::read();
    let account = configured(&config.caldav, name)?;

    eprint!(
        "Password for {} at {} (an app password if the server asks for one): ",
        account.username, account.url
    );
    let typed = clock::hidden_line();
    eprintln!();

    let password = match typed {
        Ok(Some(password)) if !password.is_empty() => password,
        Ok(Some(_)) => return Err(String::from("no password given, none kept")),
        Ok(None) => return Err(String::from("cancelled, no password kept")),
        Err(error) => return Err(format!("the password could not be read: {error}")),
    };

    let calendars = Dav::new(&agent(), account, &password)
        .calendars()
        .map_err(|problem| match problem {
            Problem::Refused => String::from(
                "the server refused the username or password, none kept; a server with \
                 two-factor sign-in, as iCloud or Fastmail, needs an app password",
            ),
            problem => format!("{}; no password kept", problem.says(name)),
        })?;

    secrets::store(
        &attributes(account),
        &format!("Kanade: CalDAV {name}"),
        password.as_bytes(),
        "text/plain",
    )?;

    let names: Vec<&str> = calendars
        .iter()
        .map(|(_, calendar)| calendar.name.as_str())
        .collect();

    Ok(match names.as_slice() {
        [] => format!("logged in to {name}; it has no calendars of events yet"),
        names => format!("logged in to {name}: {}", names.join(", ")),
    })
}

/*
 * `kanade caldav logout <name>`, in the `kanade` process: forgets the password and deletes the
 * events synced with it
 */
pub fn logout(name: &str) -> Result<String, String> {
    let (config, _) = config::read();
    let account = configured(&config.caldav, name)?;

    secrets::delete(&attributes(account))?;

    if let Some(dir) = directory() {
        match fs::remove_dir_all(dir.join(key(account))) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                return Err(format!(
                    "the password is forgotten, but the synced events are not deleted: {error}"
                ));
            }
            _ => {}
        }
    }

    Ok(format!(
        "logged out of {name}; its password and synced events are gone"
    ))
}

fn agent() -> Agent {
    Agent::config_builder()
        .timeout_global(Some(PATIENCE))
        .http_status_as_error(false)
        .allow_non_standard_methods(true)
        .max_redirects(0)
        .build()
        .into()
}

/*
 * whether an account's address keeps its password safe: HTTPS, or plain HTTP to this machine,
 * as a Radicale run locally is. Why not, if not
 */
pub fn checked(url: &str) -> Result<Uri, String> {
    let uri: Uri = url
        .parse()
        .map_err(|_| format!("{url} is not an address"))?;

    let local = matches!(
        uri.host(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    match uri.scheme_str() {
        _ if uri.host().is_none_or(str::is_empty) => Err(format!("{url} names no server")),
        // a name and password in the address would go around the keyring, and can hide its host
        _ if uri
            .authority()
            .is_some_and(|authority| authority.as_str().contains('@')) =>
        {
            Err(format!(
                "{url} has a name in it; set `username` instead, the password goes in the keyring"
            ))
        }
        Some("https") => Ok(uri),
        Some("http") if local => Ok(uri),
        Some("http") => Err(format!(
            "{url} is plain HTTP, which would send the password unencrypted; use https://"
        )),
        _ => Err(format!("{url} is not an https:// address")),
    }
}

/*
 * whether the password may go to `there`, an address a server named: the account's own server,
 * or a sibling under the same parent domain, as iCloud's caldav.icloud.com sends each account to
 * its pNN-caldav.icloud.com. Never another domain, nor another address on this machine
 */
fn trusted(account: &Uri, there: &Uri) -> bool {
    let (Some(home), Some(host)) = (account.host(), there.host()) else {
        return false;
    };
    if account.scheme() != there.scheme() {
        return false;
    }
    if home == host {
        return true;
    }

    // an address by number has no parent domain
    let named = |host: &str| !host.starts_with('[') && host.parse::<std::net::IpAddr>().is_err();
    let parent = home.split_once('.').map(|(_, parent)| parent);

    named(home)
        && named(host)
        && parent
            .is_some_and(|parent| parent.contains('.') && host.ends_with(&format!(".{parent}")))
}

// `scheme://host:port` of an address
fn origin(uri: &Uri) -> Option<String> {
    Some(format!("{}://{}", uri.scheme_str()?, uri.authority()?))
}

/*
 * an `href` a server answered for `base`, as a whole address: one already whole, one from the
 * server's root, or one relative to `base`'s directory
 */
fn resolve(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    if href.starts_with("https://") || href.starts_with("http://") {
        return Some(href.to_owned());
    }

    let base: Uri = base.parse().ok()?;
    let origin = origin(&base)?;
    if href.starts_with('/') {
        return Some(format!("{origin}{href}"));
    }

    let path = base.path();
    let directory = &path[..=path.rfind('/')?];
    Some(format!("{origin}{directory}{href}"))
}

// a property with its namespace and name
type Name = (&'static str, &'static str);

const PRINCIPAL: Name = (DAV, "current-user-principal");
const HOME: Name = (CALDAV, "calendar-home-set");

fn is(node: Node<'_, '_>, (namespace, name): Name) -> bool {
    node.is_element()
        && node.tag_name().name() == name
        && node.tag_name().namespace() == Some(namespace)
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: Name) -> Option<Node<'a, 'input>> {
    node.children().find(|child| is(*child, name))
}

// a status line, like "HTTP/1.1 200 OK", of a success
fn succeeded(status: &str) -> bool {
    status
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .is_some_and(|code| (200..300).contains(&code))
}

/*
 * a WebDAV multistatus's responses, each with its href and the properties the server found. A
 * propstat of another status, as 404 for a property it lacks, holds none. No DTD is read, so an
 * answer cannot expand entities
 */
fn responses<'a>(document: &'a Document<'a>) -> Result<Vec<(String, Vec<Node<'a, 'a>>)>, String> {
    let root = document.root_element();
    if !is(root, (DAV, "multistatus")) {
        return Err(String::from("its answer is no WebDAV multistatus"));
    }

    Ok(root
        .children()
        .filter(|node| is(*node, (DAV, "response")))
        .map(|response| {
            let href = child(response, (DAV, "href"))
                .and_then(|href| href.text())
                .unwrap_or_default()
                .trim()
                .to_owned();
            let found = response
                .children()
                .filter(|node| is(*node, (DAV, "propstat")))
                .filter(|propstat| {
                    child(*propstat, (DAV, "status"))
                        .and_then(|status| status.text())
                        .is_none_or(succeeded)
                })
                .filter_map(|propstat| child(propstat, (DAV, "prop")))
                .flat_map(|prop| prop.children().filter(Node::is_element))
                .collect();

            (href, found)
        })
        .collect())
}

fn document(body: &str) -> Result<Document<'_>, String> {
    Document::parse(body).map_err(|error| format!("its answer is not XML: {error}"))
}

// the href inside the property `name` of the first response that has it, as for a principal
fn href_of(body: &str, name: Name) -> Result<Option<String>, String> {
    let document = document(body)?;

    Ok(responses(&document)?.into_iter().find_map(|(_, found)| {
        let property = found.into_iter().find(|property| is(*property, name))?;
        let href = child(property, (DAV, "href"))?.text()?.trim();

        (!href.is_empty()).then(|| href.to_owned())
    }))
}

// a calendar of the home, by its href and its name
#[derive(Debug, Clone, PartialEq, Eq)]
struct Collection {
    href: String,
    name: String,
}

// the calendars in a home's listing that hold events; one that says nothing of what it holds may
fn collections(body: &str) -> Result<Vec<Collection>, String> {
    let document = document(body)?;

    Ok(responses(&document)?
        .into_iter()
        .filter(|(_, found)| {
            let calendar = found
                .iter()
                .find(|property| is(**property, (DAV, "resourcetype")))
                .is_some_and(|kind| child(*kind, (CALDAV, "calendar")).is_some());
            let events = found
                .iter()
                .find(|property| is(**property, (CALDAV, "supported-calendar-component-set")))
                .is_none_or(|set| {
                    set.children().any(|comp| {
                        is(comp, (CALDAV, "comp")) && comp.attribute("name") == Some("VEVENT")
                    })
                });

            calendar && events
        })
        .filter(|(href, _)| !href.is_empty())
        .map(|(href, found)| {
            let name = found
                .iter()
                .find(|property| is(**property, (DAV, "displayname")))
                .and_then(|name| name.text())
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map_or_else(|| href.clone(), str::to_owned);

            Collection { href, name }
        })
        .collect())
}

// each event's iCalendar object in a calendar-query's answer
fn calendar_data(body: &str) -> Result<Vec<String>, String> {
    let document = document(body)?;

    Ok(responses(&document)?
        .into_iter()
        .flat_map(|(_, found)| found)
        .filter(|property| is(*property, (CALDAV, "calendar-data")))
        .filter_map(|data| {
            // the text may come in pieces, as CDATA beside escaped text
            let text: String = data.children().filter_map(|piece| piece.text()).collect();

            text.contains("BEGIN:VCALENDAR").then_some(text)
        })
        .collect())
}

const PRINCIPAL_BODY: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<D:propfind xmlns:D=\"DAV:\"><D:prop><D:current-user-principal/></D:prop></D:propfind>";

const HOME_BODY: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<D:propfind xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\
<D:prop><C:calendar-home-set/></D:prop></D:propfind>";

const CALENDARS_BODY: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<D:propfind xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\
<D:prop><D:resourcetype/><D:displayname/><C:supported-calendar-component-set/></D:prop>\
</D:propfind>";

// a calendar's events from `from` until `to`, each `YYYYMMDDTHHMMSSZ`
fn query_body(from: &str, to: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<C:calendar-query xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\
<D:prop><D:getetag/><C:calendar-data/></D:prop>\
<C:filter><C:comp-filter name=\"VCALENDAR\"><C:comp-filter name=\"VEVENT\">\
<C:time-range start=\"{from}\" end=\"{to}\"/>\
</C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"
    )
}

// a moment as CalDAV's time-range takes it
fn utc(seconds: i64) -> Option<String> {
    Some(
        DateTime::from_timestamp(seconds, 0)?
            .format("%Y%m%dT%H%M%SZ")
            .to_string(),
    )
}

// one account's server, with its password
struct Dav<'a> {
    agent: &'a Agent,
    url: &'a str,

    // `Basic <base64>`
    authorization: Zeroizing<String>,
}

impl<'a> Dav<'a> {
    fn new(agent: &'a Agent, account: &'a CalDav, password: &str) -> Self {
        let pair = Zeroizing::new(format!("{}:{password}", account.username));

        Dav {
            agent,
            url: &account.url,
            authorization: Zeroizing::new(format!("Basic {}", STANDARD.encode(pair.as_bytes()))),
        }
    }

    /*
     * `method` on `url` with `body`, following redirects on the same server; the answer's status,
     * the address it came from, and its body
     */
    fn ask(
        &self,
        method: &str,
        url: &str,
        depth: &str,
        body: &str,
    ) -> Result<(u16, String, String), Problem> {
        let method = Method::from_bytes(method.as_bytes())
            .map_err(|error| Problem::Server(error.to_string()))?;
        let mut url = url.to_owned();

        let account = checked(self.url).map_err(Problem::Server)?;

        for _ in 0..=HOPS {
            let uri = checked(&url).map_err(Problem::Server)?;
            if !trusted(&account, &uri) {
                return Err(Problem::Server(format!(
                    "{} names another server, {url}; the password goes only to its own",
                    self.url
                )));
            }
            let request = http::Request::builder()
                .method(method.clone())
                .uri(uri.clone())
                .header("Depth", depth)
                .header("Content-Type", "application/xml; charset=utf-8")
                .header("Authorization", self.authorization.as_str())
                .body(body)
                .map_err(|error| Problem::Server(error.to_string()))?;

            let mut response = self.agent.run(request).map_err(unreached)?;
            let status = response.status().as_u16();

            if response.status().is_redirection() {
                let Some(location) = response
                    .headers()
                    .get("Location")
                    .and_then(|location| location.to_str().ok())
                    .and_then(|location| resolve(&url, location))
                else {
                    return Err(Problem::Server(format!(
                        "{url} answered {status} with nowhere to go"
                    )));
                };

                let there = location.parse::<Uri>().ok();
                if there.as_ref().and_then(origin) != origin(&uri) {
                    return Err(Problem::Server(format!(
                        "{url} sends to another server, {location}; set that as its url"
                    )));
                }

                url = location;
                continue;
            }

            let text = response
                .body_mut()
                .with_config()
                .limit(BODY)
                .read_to_string()
                .map_err(unreached)?;

            return match status {
                401 | 403 => Err(Problem::Refused),
                _ => Ok((status, url, text)),
            };
        }

        Err(Problem::Server(format!(
            "{} redirects over {HOPS} times",
            self.url
        )))
    }

    // `ask` for a multistatus, any other answer a problem
    fn multistatus(
        &self,
        method: &str,
        url: &str,
        depth: &str,
        body: &str,
    ) -> Result<(String, String), Problem> {
        match self.ask(method, url, depth, body)? {
            (207, url, text) => Ok((url, text)),
            (status, url, _) => Err(Problem::Server(format!("{url} answered {status}"))),
        }
    }

    // the user's principal, from the address given, else from the server's well-known one
    fn principal(&self) -> Result<String, Problem> {
        let mut tried = Vec::new();
        let base = checked(self.url).map_err(Problem::Server)?;
        let well_known = origin(&base).map(|origin| format!("{origin}/.well-known/caldav"));

        for url in std::iter::once(self.url.to_owned()).chain(well_known) {
            match self.multistatus("PROPFIND", &url, "0", PRINCIPAL_BODY) {
                Ok((answered, text)) => {
                    let href = href_of(&text, PRINCIPAL)
                        .map_err(|why| Problem::Server(format!("{answered}: {why}")))?;

                    if let Some(principal) = href.and_then(|href| resolve(&answered, &href)) {
                        return Ok(principal);
                    }
                    tried.push(format!("{answered} names no principal"));
                }
                Err(Problem::Server(why)) => tried.push(why),
                Err(problem) => return Err(problem),
            }
        }

        Err(Problem::Server(format!(
            "no CalDAV account found: {}",
            tried.join("; ")
        )))
    }

    // the calendars that hold events, found from the address given
    fn calendars(&self) -> Result<Vec<(String, Collection)>, Problem> {
        let principal = self.principal()?;

        let (answered, text) = self.multistatus("PROPFIND", &principal, "0", HOME_BODY)?;
        let home = href_of(&text, HOME)
            .map_err(|why| Problem::Server(format!("{answered}: {why}")))?
            .and_then(|href| resolve(&answered, &href))
            .ok_or_else(|| Problem::Server(format!("{answered} names no calendar home")))?;

        let (answered, text) = self.multistatus("PROPFIND", &home, "1", CALENDARS_BODY)?;
        let found =
            collections(&text).map_err(|why| Problem::Server(format!("{answered}: {why}")))?;

        Ok(found
            .into_iter()
            .filter_map(|collection| Some((resolve(&answered, &collection.href)?, collection)))
            .take(CALENDARS)
            .collect())
    }

    // each calendar's events near now, as the name of its file and the file
    fn fetch(&self) -> Result<Vec<(String, String)>, Problem> {
        let now = now();
        let (Some(from), Some(to)) = (utc(now - PAST), utc(now + AHEAD)) else {
            return Err(Problem::Server(String::from("the clock is out of range")));
        };
        let query = query_body(&from, &to);

        let mut files = Vec::new();
        for (url, _) in self.calendars()? {
            let (answered, text) = self.multistatus("REPORT", &url, "1", &query)?;
            let events = calendar_data(&text)
                .map_err(|why| Problem::Server(format!("{answered}: {why}")))?;

            if events.is_empty() {
                continue;
            }

            // XML read each CR LF as LF; iCalendar's lines end in CR LF
            let mut file = String::new();
            for line in events.iter().flat_map(|event| event.trim().lines()) {
                file.push_str(line);
                file.push_str("\r\n");
            }
            files.push((format!("{}.ics", hex(&url)), file));
        }

        Ok(files)
    }
}

// why a request did not get an answer
fn unreached(error: ureq::Error) -> Problem {
    match error {
        ureq::Error::BodyExceedsLimit(limit) => Problem::Server(format!(
            "the server's answer was over {} MiB",
            limit / 1024 / 1024
        )),
        error => Problem::Offline(error.to_string()),
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

// how long after `failures` failed syncs in a row the next is tried
fn backoff(failures: u32) -> Duration {
    RETRY
        .saturating_mul(1 << failures.saturating_sub(1).min(16))
        .min(INTERVAL)
}

/*
 * syncs the accounts configured, on its own thread: at start, again every `INTERVAL` and when
 * asked; with none, it waits on its queue
 */
pub fn follow() {
    let queue = QUEUE.receiver();

    Worker {
        agent: agent(),
        dir: directory(),
        accounts: Vec::new(),
    }
    .run(&queue);
}

// an account as the sync thread keeps it
struct Synced {
    account: CalDav,
    password: Option<Zeroizing<String>>,
    due: Instant,
    failures: u32,
    standing: Standing,
}

struct Worker {
    agent: Agent,
    dir: Option<PathBuf>,
    accounts: Vec<Synced>,
}

impl Worker {
    fn run(&mut self, queue: &Receiver<Message>) {
        self.reread();

        loop {
            let now = Instant::now();
            let due: Vec<usize> = (0..self.accounts.len())
                .filter(|&index| self.accounts[index].due <= now)
                .collect();
            for index in due {
                self.sync(index);
            }
            self.publish();

            let next = self.accounts.iter().map(|synced| synced.due).min();
            let message = match next {
                Some(next) => {
                    match queue.recv_timeout(next.saturating_duration_since(Instant::now())) {
                        Ok(message) => message,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                None => match queue.recv() {
                    Ok(message) => message,
                    Err(_) => return,
                },
            };

            match message {
                Message::Sync(name) => {
                    let now = Instant::now();

                    for synced in &mut self.accounts {
                        if name
                            .as_ref()
                            .is_none_or(|name| *name == synced.account.name)
                        {
                            // asked again, as after a login or a logout
                            synced.password = None;
                            synced.due = now;
                        }
                    }
                }
                Message::Reread => self.reread(),
            }
        }
    }

    /*
     * the accounts as the config has them now: a new or changed one syncs at once, the events of
     * one gone are deleted
     */
    fn reread(&mut self) {
        let configured = config::get().caldav.clone();
        let now = Instant::now();
        let mut kept = std::mem::take(&mut self.accounts);

        self.accounts = configured
            .into_iter()
            .map(
                |account| match kept.iter().position(|synced| synced.account == account) {
                    Some(index) => kept.swap_remove(index),
                    None => Synced {
                        standing: Standing {
                            name: account.name.clone(),
                            ..Standing::default()
                        },
                        account,
                        password: None,
                        due: now,
                        failures: 0,
                    },
                },
            )
            .collect();

        if let Some(dir) = &self.dir
            && let Err(error) = self.forget(dir)
        {
            eprintln!("kanade: caldav: the events of an account gone are not deleted: {error}");
        }
        self.publish();
    }

    // deletes the synced events of accounts no longer configured
    fn forget(&self, dir: &Path) -> io::Result<()> {
        let entries = match fs::read_dir(dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            entries => entries?,
        };

        let mut deleted = false;
        for entry in entries {
            let entry = entry?;
            let ours = self
                .accounts
                .iter()
                .any(|synced| entry.file_name() == key(&synced.account).as_str());

            if !ours {
                fs::remove_dir_all(entry.path())?;
                deleted = true;
            }
        }

        if deleted {
            calendar::reread();
        }
        Ok(())
    }

    fn sync(&mut self, index: usize) {
        let Some(dir) = self.dir.clone() else {
            return;
        };
        let synced = &mut self.accounts[index];
        let account_dir = dir.join(key(&synced.account));

        let result = (|| {
            if synced.password.is_none() {
                synced.password = password(&synced.account).map_err(Problem::Keyring)?;
            }
            let Some(password) = &synced.password else {
                // logged out: what was synced with the password is gone with it
                match fs::remove_dir_all(&account_dir) {
                    Ok(()) => calendar::reread(),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => eprintln!(
                        "kanade: caldav {}: the synced events are not deleted: {error}",
                        synced.account.name
                    ),
                }
                synced.standing.synced = None;
                return Err(Problem::LoggedOut);
            };

            let files = Dav::new(&self.agent, &synced.account, password).fetch()?;
            let created =
                write(&account_dir, &files).map_err(|error| Problem::Disk(error.to_string()))?;

            Ok((files.len(), created))
        })();

        match result {
            Ok((calendars, created)) => {
                // a new directory is not watched yet
                if created {
                    calendar::reread();
                }
                synced.standing.synced = Some((now(), calendars));
                synced.standing.problem = None;
                synced.failures = 0;
                synced.due = Instant::now() + INTERVAL;
            }
            Err(problem) => {
                eprintln!(
                    "kanade: caldav {}: {}",
                    synced.account.name,
                    problem.says(&synced.account.name)
                );

                /*
                 * a wrong password is not tried again until a login, nor one not kept, nor a
                 * keyring whose prompt the user dismissed, which would ask again and again;
                 * `sync` asks again. A keyring not running yet is tried as a server is
                 */
                let waits = match &problem {
                    Problem::Refused | Problem::LoggedOut => true,
                    Problem::Keyring(why) => why == secrets::NOT_UNLOCKED,
                    _ => false,
                };
                if problem == Problem::Refused {
                    synced.password = None;
                }

                synced.failures = synced.failures.saturating_add(1);
                synced.due = Instant::now()
                    + if waits {
                        // a day, far enough not to matter, as a login asks again
                        Duration::from_secs(24 * 60 * 60)
                    } else {
                        backoff(synced.failures)
                    };
                synced.standing.problem = Some(problem);
            }
        }
    }

    // written only on a change, as a write wakes every window
    fn publish(&self) {
        let accounts = Accounts {
            accounts: self
                .accounts
                .iter()
                .map(|synced| synced.standing.clone())
                .collect(),
        };

        if *Accounts::read() != accounts {
            *Accounts::write() = accounts;
        }
    }
}

/*
 * the synced files in `dir`, each written only when it changed, and the files of calendars gone
 * deleted; true when it made `dir`. Only Kanade reads them, as they hold the user's events
 */
fn write(dir: &Path, files: &[(String, String)]) -> io::Result<bool> {
    let created = !dir.is_dir();

    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;

    for (name, text) in files {
        let path = dir.join(name);
        if fs::read_to_string(&path).is_ok_and(|kept| kept == *text) {
            continue;
        }

        // not `.ics`, so the calendar never reads one half written
        let part = dir.join(format!(".{name}.part"));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&part)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;

        fs::rename(&part, &path)?;
    }

    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let kept = path
            .file_name()
            .is_some_and(|name| files.iter().any(|(kept, _)| name == kept.as_str()));

        if !kept {
            fs::remove_file(path)?;
        }
    }

    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;

    // as Nextcloud answers, with a prefix of its own for DAV:
    const PRINCIPAL_ANSWER: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:s="http://sabredav.org/ns">
  <d:response>
    <d:href>/remote.php/dav/</d:href>
    <d:propstat>
      <d:prop>
        <d:current-user-principal><d:href>/remote.php/dav/principals/users/kanade/</d:href></d:current-user-principal>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

    #[test]
    fn a_principal_is_found_by_namespace_not_prefix() {
        assert_eq!(
            href_of(PRINCIPAL_ANSWER, PRINCIPAL),
            Ok(Some(String::from(
                "/remote.php/dav/principals/users/kanade/"
            )))
        );

        // the same name in another namespace is not it
        let other = PRINCIPAL_ANSWER.replace("xmlns:d=\"DAV:\"", "xmlns:d=\"urn:other\"");
        assert!(href_of(&other, PRINCIPAL).is_err());
    }

    #[test]
    fn a_property_the_server_lacks_is_none() {
        let answer = r#"<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <response>
    <href>/principals/kanade/</href>
    <propstat>
      <prop><C:calendar-home-set><href>/wrong/</href></C:calendar-home-set></prop>
      <status>HTTP/1.1 404 Not Found</status>
    </propstat>
  </response>
</multistatus>"#;

        assert_eq!(href_of(answer, HOME), Ok(None));
        assert_eq!(
            href_of(&answer.replace("404 Not Found", "200 OK"), HOME),
            Ok(Some(String::from("/wrong/")))
        );
    }

    // as Fastmail and iCloud answer a home's listing: the home, an inbox, a task list, calendars
    #[test]
    fn only_calendars_that_hold_events_are_synced() {
        let answer = r#"<?xml version="1.0" encoding="UTF-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:response>
    <D:href>/dav/calendars/user/me/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop>
      <D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/calendars/user/me/Default/</D:href>
    <D:propstat><D:prop>
      <D:resourcetype><D:collection/><C:calendar/></D:resourcetype>
      <D:displayname> Personal </D:displayname>
      <C:supported-calendar-component-set><C:comp name="VEVENT"/><C:comp name="VTODO"/></C:supported-calendar-component-set>
    </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/calendars/user/me/tasks/</D:href>
    <D:propstat><D:prop>
      <D:resourcetype><D:collection/><C:calendar/></D:resourcetype>
      <D:displayname>Tasks</D:displayname>
      <C:supported-calendar-component-set><C:comp name="VTODO"/></C:supported-calendar-component-set>
    </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/calendars/user/me/inbox/</D:href>
    <D:propstat><D:prop>
      <D:resourcetype><D:collection/><C:schedule-inbox/></D:resourcetype>
    </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/calendars/user/me/work/</D:href>
    <D:propstat><D:prop>
      <D:resourcetype><D:collection/><C:calendar/></D:resourcetype>
    </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
    <D:propstat><D:prop><D:displayname/><C:supported-calendar-component-set/></D:prop>
      <D:status>HTTP/1.1 404 Not Found</D:status></D:propstat>
  </D:response>
</D:multistatus>"#;

        assert_eq!(
            collections(answer),
            Ok(vec![
                Collection {
                    href: String::from("/dav/calendars/user/me/Default/"),
                    name: String::from("Personal"),
                },
                Collection {
                    href: String::from("/dav/calendars/user/me/work/"),
                    name: String::from("/dav/calendars/user/me/work/"),
                },
            ])
        );
    }

    #[test]
    fn each_event_comes_whole_from_a_query() {
        let answer = "<?xml version=\"1.0\"?>
<multistatus xmlns=\"DAV:\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\">
  <response>
    <href>/cal/a.ics</href>
    <propstat><prop><getetag>\"1\"</getetag>
      <cal:calendar-data>BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:Tea &amp; cake\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n</cal:calendar-data>
    </prop><status>HTTP/1.1 200 OK</status></propstat>
  </response>
  <response>
    <href>/cal/b.ics</href>
    <propstat><prop>
      <cal:calendar-data><![CDATA[BEGIN:VCALENDAR
BEGIN:VEVENT
SUMMARY:<b>
END:VEVENT
END:VCALENDAR
]]></cal:calendar-data>
    </prop><status>HTTP/1.1 200 OK</status></propstat>
  </response>
  <response>
    <href>/cal/gone.ics</href>
    <status>HTTP/1.1 404 Not Found</status>
  </response>
</multistatus>";

        let events = calendar_data(answer).unwrap();

        // XML reads a CR LF as LF; `fetch` ends the lines with CR LF again
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("SUMMARY:Tea & cake\n"));
        assert!(events[1].contains("SUMMARY:<b>\n"));
    }

    // an answer cannot make the parser expand entities without bound
    #[test]
    fn an_answer_with_a_dtd_or_no_multistatus_is_refused() {
        let bomb = r#"<?xml version="1.0"?>
<!DOCTYPE m [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">]>
<multistatus xmlns="DAV:"><response><href>&b;</href></response></multistatus>"#;

        assert!(collections(bomb).is_err());
        assert!(calendar_data("<html><body>Sign in</body></html>").is_err());
        assert!(calendar_data("not xml").is_err());
    }

    #[test]
    fn hrefs_resolve_against_the_address_that_answered() {
        let base = "https://cloud.example.org/remote.php/dav/";

        assert_eq!(
            resolve(base, "/remote.php/dav/principals/me/").as_deref(),
            Some("https://cloud.example.org/remote.php/dav/principals/me/")
        );
        assert_eq!(
            resolve(base, "calendars/me/").as_deref(),
            Some("https://cloud.example.org/remote.php/dav/calendars/me/")
        );
        assert_eq!(
            resolve(
                "https://caldav.icloud.com/",
                "https://p63-caldav.icloud.com:443/1/calendars/"
            )
            .as_deref(),
            Some("https://p63-caldav.icloud.com:443/1/calendars/")
        );
        assert_eq!(
            resolve("http://localhost:5232", "/kanade/").as_deref(),
            Some("http://localhost:5232/kanade/")
        );
        assert_eq!(resolve(base, "  "), None);
    }

    // a server cannot send the password elsewhere by naming another address in its answer
    #[test]
    fn the_password_goes_only_to_the_account_and_its_siblings() {
        let trusts = |account: &str, there: &str| {
            trusted(&account.parse().unwrap(), &there.parse().unwrap())
        };

        assert!(trusts(
            "https://caldav.icloud.com/",
            "https://p63-caldav.icloud.com:443/1/"
        ));
        assert!(trusts(
            "https://example.com/dav/",
            "https://example.com:443/dav/me/"
        ));
        assert!(trusts(
            "http://localhost:5232/",
            "http://localhost:5232/me/"
        ));

        assert!(!trusts("https://example.com/", "https://evil.com/"));
        assert!(!trusts("https://example.com/", "https://other.com/"));
        assert!(!trusts("https://dav.example.co.uk/", "https://evil.co.uk/"));
        assert!(!trusts(
            "https://caldav.icloud.com/",
            "https://icloud.com.evil.org/"
        ));
        assert!(!trusts(
            "https://caldav.icloud.com/",
            "http://p63-caldav.icloud.com/"
        ));
        assert!(!trusts("https://caldav.icloud.com/", "https://localhost/"));
        assert!(!trusts("https://10.0.0.5/", "https://1.0.0.5/"));
    }

    // the password goes over HTTPS, or to this machine
    #[test]
    fn only_a_safe_address_gets_the_password() {
        assert!(checked("https://caldav.fastmail.com").is_ok());
        assert!(checked("http://localhost:5232/").is_ok());
        assert!(checked("http://127.0.0.1:5232/").is_ok());
        assert!(checked("http://[::1]:5232/").is_ok());
        assert!(checked("http://cloud.example.org/").is_err());
        assert!(checked("http://localhost.example.org/").is_err());
        assert!(checked("ftp://cloud.example.org/").is_err());
        assert!(checked("cloud.example.org").is_err());
        assert!(checked("https://me:secret@cloud.example.org/").is_err());
        assert!(checked("https://cloud.example.org@evil.example/").is_err());
    }
}
