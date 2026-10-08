//! Signing in to Google (ADR 0016): OAuth 2.0 for an installed app, with the user's own client.
//! The browser asks the user, then sends the code to a listener of Kanade's on the loopback
//! address; PKCE (S256) and a random state tie that code to this sign-in. The code becomes a
//! refresh token, which the Secret Service keeps, and each sync trades it for an access token
//! held only in memory. Only read access to calendars is asked for.

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::Value;
use ureq::Agent;

use super::Problem;
use super::secret::Credentials;

const AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";
const REVOKE: &str = "https://oauth2.googleapis.com/revoke";

// the calendars and their events, read only
pub const SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";

// how long the browser has to come back, for a user who reads the consent screen
pub const PATIENCE: Duration = Duration::from_secs(5 * 60);

// a browser's request line and headers, well past what it sends to the loopback address
const REQUEST: u64 = 16 * 1024;

// the OAuth client from the JSON file Google Cloud gives for a Desktop app
#[derive(Clone, PartialEq, Eq)]
pub struct Client {
    pub id: String,
    pub secret: String,
}

impl Client {
    // the client in a file Google Cloud gave, which says why when it is not one
    pub fn read(path: &Path) -> Result<Client, String> {
        let text =
            fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;

        Client::parse(&text).ok_or_else(|| {
            format!(
                "{}: no Desktop app OAuth client in it; download its JSON from Google Cloud",
                path.display()
            )
        })
    }

    fn parse(text: &str) -> Option<Client> {
        let value: Value = serde_json::from_str(text).ok()?;
        let installed = value.get("installed")?;
        let field = |name: &str| {
            Some(installed.get(name)?.as_str()?.to_owned()).filter(|value| !value.is_empty())
        };

        Some(Client {
            id: field("client_id")?,
            secret: field("client_secret")?,
        })
    }
}

// one sign-in waiting for the browser
pub struct Pending {
    pub client: Client,
    pub address: String,
    listener: TcpListener,
    redirect: String,
    verifier: String,
    state: String,
}

impl Pending {
    // listens on a free loopback port and makes the address the browser opens
    pub fn start(client: Client) -> Result<Pending, String> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|error| format!("cannot listen for the browser: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("cannot listen for the browser: {error}"))?
            .port();

        let verifier = random(32)?;
        let state = random(16)?;
        let redirect = format!("http://127.0.0.1:{port}");
        let challenge =
            URL_SAFE_NO_PAD.encode(digest::digest(&digest::SHA256, verifier.as_bytes()));

        let address = authorize(&client.id, &redirect, &challenge, &state);

        Ok(Pending {
            client,
            address,
            listener,
            redirect,
            verifier,
            state,
        })
    }

    /*
     * the code the browser brings back by `deadline`, its tab waiting to be told how the sign-in
     * went; other requests, like for a favicon, are turned away and waited past. `cancelled` is
     * asked between them, so a newer sign-in or a sign-out ends this one
     */
    pub fn wait(
        &self,
        deadline: Instant,
        cancelled: impl Fn() -> bool,
    ) -> Result<Returned, String> {
        self.listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;

        loop {
            if cancelled() {
                return Err(String::from("cancelled"));
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the browser did not come back within {} minutes",
                    PATIENCE.as_secs() / 60
                ));
            }

            match self.listener.accept() {
                Ok((stream, _)) => {
                    if let Some(came) = self.answer(stream) {
                        return came;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }

    // what one request to the listener says, none for one that is not the browser coming back
    fn answer(&self, mut stream: TcpStream) -> Option<Result<Returned, String>> {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));

        let mut line = String::new();
        BufReader::new((&stream).take(REQUEST))
            .read_line(&mut line)
            .ok()?;

        match returned(&line, &self.state) {
            None => {
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                );
                None
            }
            Some(Ok(code)) => Some(Ok(Returned { code, stream })),
            Some(Err(why)) => {
                Returned {
                    code: String::new(),
                    stream,
                }
                .tell(Err(&why));
                Some(Err(why))
            }
        }
    }

    // the credentials the code is worth
    pub fn exchange(&self, agent: &Agent, code: &str) -> Result<Credentials, String> {
        let answer = post(
            agent,
            TOKEN,
            &[
                ("code", code),
                ("client_id", &self.client.id),
                ("client_secret", &self.client.secret),
                ("redirect_uri", &self.redirect),
                ("grant_type", "authorization_code"),
                ("code_verifier", &self.verifier),
            ],
        )
        .map_err(|problem| problem.to_string())?;

        let refresh_token = answer
            .get("refresh_token")
            .and_then(Value::as_str)
            .ok_or("Google gave no refresh token")?;

        Ok(Credentials {
            client_id: self.client.id.clone(),
            client_secret: self.client.secret.clone(),
            refresh_token: refresh_token.to_owned(),
        })
    }
}

// the browser back with the code, its tab left loading until it is told how the sign-in went
pub struct Returned {
    pub code: String,
    stream: TcpStream,
}

impl Returned {
    // a page in the tab that says whether Kanade is signed in, and why not
    pub fn tell(mut self, signed: Result<(), &str>) {
        let page = match signed {
            Ok(()) => {
                String::from("Kanade is signed in to Google Calendar. You can close this tab.")
            }
            Err(why) => format!(
                "Kanade was not signed in to Google Calendar: {}.",
                why.trim_end_matches('.')
            ),
        };

        let _ = write!(
            self.stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\
             Content-Length: {}\r\n\r\n{page}",
            page.len()
        );
    }
}

// the address the browser opens: Google's consent screen, coming back to `redirect`
fn authorize(client: &str, redirect: &str, challenge: &str, state: &str) -> String {
    let query: Vec<String> = [
        ("client_id", client),
        ("redirect_uri", redirect),
        ("response_type", "code"),
        ("scope", SCOPE),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
        // a refresh token, every time, so a second sign-in gets one too
        ("access_type", "offline"),
        ("prompt", "consent"),
    ]
    .into_iter()
    .map(|(name, value)| format!("{name}={}", utf8_percent_encode(value, NON_ALPHANUMERIC)))
    .collect();

    format!("{AUTHORIZE}?{}", query.join("&"))
}

/*
 * what a request line says of the sign-in: the code, why there is none, or none for a request that
 * is not the browser coming back with this sign-in's state
 */
fn returned(line: &str, state: &str) -> Option<Result<String, String>> {
    let target = line.strip_prefix("GET ")?.split(' ').next()?;
    let query = target.strip_prefix("/?")?;

    let fields: Vec<(&str, String)> = query
        .split('&')
        .filter_map(|field| {
            let (name, value) = field.split_once('=')?;
            let value = percent_decode_str(&value.replace('+', " "))
                .decode_utf8()
                .ok()?
                .into_owned();

            Some((name, value))
        })
        .collect();
    let field = |wanted: &str| {
        fields
            .iter()
            .find_map(|(name, value)| (*name == wanted).then_some(value.as_str()))
    };

    if field("state")? != state {
        return None;
    }

    Some(match (field("code"), field("error")) {
        (Some(code), None) => Ok(code.to_owned()),
        (_, Some("access_denied")) => Err(String::from("access was not granted")),
        (_, Some(error)) => Err(format!("Google said {error}")),
        (None, None) => Err(String::from("Google sent no code")),
    })
}

// `bytes` random bytes, base64url
fn random(bytes: usize) -> Result<String, String> {
    let mut buffer = vec![0; bytes];

    SystemRandom::new()
        .fill(&mut buffer)
        .map_err(|_| String::from("no randomness for the sign-in"))?;

    Ok(URL_SAFE_NO_PAD.encode(buffer))
}

// an access token and when it runs out
pub struct Access {
    pub token: String,
    pub until: Instant,
}

// an access token for the credentials; a revoked or expired grant is `Problem::Revoked`
pub fn refresh(agent: &Agent, credentials: &Credentials) -> Result<Access, Problem> {
    let answer = post(
        agent,
        TOKEN,
        &[
            ("client_id", &credentials.client_id),
            ("client_secret", &credentials.client_secret),
            ("refresh_token", &credentials.refresh_token),
            ("grant_type", "refresh_token"),
        ],
    )?;

    let token = answer
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| Problem::Refused(String::from("Google gave no access token")))?;
    let lasts = answer
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    Ok(Access {
        token: token.to_owned(),
        // a minute early, so one never runs out on the way
        until: Instant::now() + Duration::from_secs(lasts.saturating_sub(60)),
    })
}

// asks Google to end the grant; one already ended is fine
pub fn revoke(agent: &Agent, credentials: &Credentials) -> Result<(), Problem> {
    match post(agent, REVOKE, &[("token", &credentials.refresh_token)]) {
        Ok(_) | Err(Problem::Revoked) => Ok(()),
        Err(problem) => Err(problem),
    }
}

// a form posted to Google's OAuth endpoints, and its JSON answer
fn post(agent: &Agent, url: &str, form: &[(&str, &str)]) -> Result<Value, Problem> {
    let mut response = agent
        .post(url)
        .send_form(form.iter().copied())
        .map_err(super::unreached)?;

    let status = response.status().as_u16();
    let body: Value = response
        .body_mut()
        .read_to_string()
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);

    if status == 200 {
        return Ok(body);
    }

    Err(refusal(status, &body))
}

/*
 * why the token endpoint said no: its `error` code, and its description, which names no secret.
 * `invalid_grant` is a grant revoked, expired or for a changed password; `invalid_token` is
 * the revoke endpoint's for one already gone
 */
fn refusal(status: u16, body: &Value) -> Problem {
    let error = body.get("error").and_then(Value::as_str);
    let description = body.get("error_description").and_then(Value::as_str);

    match (error, description) {
        (Some("invalid_grant" | "invalid_token"), _) => Problem::Revoked,
        (Some(error), Some(description)) => {
            Problem::Refused(format!("Google refused it: {error}, {description}"))
        }
        (Some(error), None) => Problem::Refused(format!("Google refused it: {error}")),
        (None, _) => Problem::Refused(format!("Google answered {status}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_client_file_is_read() {
        let file = r#"{"installed":{"client_id":"1-a.apps.googleusercontent.com",
            "project_id":"p","auth_uri":"https://accounts.google.com/o/oauth2/auth",
            "token_uri":"https://oauth2.googleapis.com/token","client_secret":"GOCSPX-s",
            "redirect_uris":["http://localhost"]}}"#;

        assert!(
            Client::parse(file)
                == Some(Client {
                    id: String::from("1-a.apps.googleusercontent.com"),
                    secret: String::from("GOCSPX-s"),
                })
        );

        // a Web app's client cannot come back to the loopback address
        assert!(Client::parse(r#"{"web":{"client_id":"a","client_secret":"b"}}"#).is_none());
        assert!(Client::parse(r#"{"installed":{"client_id":"a"}}"#).is_none());
        assert!(Client::parse("{").is_none());
    }

    #[test]
    fn the_address_asks_for_read_only_calendars_with_pkce() {
        let address = authorize("id", "http://127.0.0.1:5555", "chal-lenge_", "st");

        assert!(address.starts_with("https://accounts.google.com/o/oauth2/v2/auth?client_id=id&"));
        assert!(address.contains("&redirect_uri=http%3A%2F%2F127%2E0%2E0%2E1%3A5555&"));
        assert!(
            address.contains(
                "&scope=https%3A%2F%2Fwww%2Egoogleapis%2Ecom%2Fauth%2Fcalendar%2Ereadonly&"
            )
        );
        assert!(address.contains("&code_challenge=chal%2Dlenge%5F&code_challenge_method=S256&"));
        assert!(address.contains("&state=st&access_type=offline&prompt=consent"));
    }

    #[test]
    fn the_browser_coming_back_brings_the_code() {
        assert_eq!(
            returned("GET /?state=st&code=4%2F0Ab&scope=x HTTP/1.1\r\n", "st"),
            Some(Ok(String::from("4/0Ab")))
        );
        assert_eq!(
            returned("GET /?error=access_denied&state=st HTTP/1.1\r\n", "st"),
            Some(Err(String::from("access was not granted")))
        );

        // another sign-in's, a favicon, or anything else is waited past
        assert_eq!(returned("GET /?state=old&code=c HTTP/1.1\r\n", "st"), None);
        assert_eq!(returned("GET /favicon.ico HTTP/1.1\r\n", "st"), None);
        assert_eq!(returned("POST /?state=st&code=c HTTP/1.1\r\n", "st"), None);
    }

    #[test]
    fn a_revoked_grant_says_so() {
        let body: Value = serde_json::from_str(
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
        )
        .unwrap_or_default();

        assert_eq!(refusal(400, &body), Problem::Revoked);
        assert_eq!(
            refusal(401, &serde_json::from_str(r#"{"error":"invalid_client","error_description":"The OAuth client was not found."}"#).unwrap_or_default()),
            Problem::Refused(String::from(
                "Google refused it: invalid_client, The OAuth client was not found."
            ))
        );
        assert_eq!(
            refusal(503, &Value::Null),
            Problem::Refused(String::from("Google answered 503"))
        );
    }

    #[test]
    fn verifiers_are_long_enough_and_differ() {
        let (Ok(one), Ok(two)) = (random(32), random(32)) else {
            panic!("no randomness");
        };

        assert_eq!(one.len(), 43);
        assert_ne!(one, two);
    }
}
