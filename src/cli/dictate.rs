//! `kanade dictate key|consent|forget` (ADR 0111): the setup that handles the Gemini API key and the
//! user's consent. It runs in this process, shell or none, because the key must go from the user's
//! terminal to the Secret Service without passing through argv, the socket, a log or the shell, and
//! because the consent is the user's own act, typed at a terminal: nothing a script, a keybind or the
//! shell can do for them.

use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use super::{Reply, complain, print};
use crate::sources::dictation::Request;
use crate::sources::dictation::gate::{self, Key};

// the most of a line read, far over any key
const LINE: u64 = 512;

pub fn run(request: Request) -> ExitCode {
    let done = match request {
        Request::Key => key(),
        Request::Consent => consent(),
        Request::Forget => forget(),
        // the shell's
        Request::Toggle | Request::Cancel | Request::Status => Err(String::from("not a setup")),
    };

    match done {
        Ok(said) => print(&said, ExitCode::SUCCESS),
        Err(why) => {
            complain(&format!("kanade: {why}"));
            ExitCode::FAILURE
        }
    }
}

/*
 * keeps the key read from stdin, a line: typed with no echo at a terminal, or piped from a file or a
 * password manager. Never an argument, which any process of the user's can read. Says nothing of it
 */
fn key() -> Result<String, String> {
    let stdin = io::stdin();
    let line = if stdin.is_terminal() {
        eprint!("Gemini API key (hidden): ");
        drop(io::stderr().flush());

        let line = hidden(read_secret);

        eprintln!();
        line?
    } else {
        read_line()?
    };

    let key = Key::new(&line).ok_or("that is not an API key: it is empty or not plain ASCII")?;

    gate::store_key(&key)?;

    let mut said = String::from("key stored in the Secret Service\n");

    if gate::decide(true, gate::read_consent().as_deref()) != gate::Gate::Ready {
        said.push_str("next: `kanade dictate consent`\n");
    }

    Ok(said)
}

fn read_line() -> Result<String, String> {
    let mut line = String::new();

    io::stdin()
        .lock()
        .take(LINE)
        .read_line(&mut line)
        .map_err(|error| format!("cannot read stdin: {error}"))?;

    Ok(line)
}

/*
 * runs `read` with the terminal not echoing, not turning Ctrl-C into a signal (it is read as a
 * key, so `read` can stop and the terminal is restored; a signal would leave it silent) and not
 * waiting for a line. The settings it had are put back after, even for a failed read.
 */
fn hidden<T>(read: impl FnOnce() -> T) -> T {
    let stty = |arguments: &[&str], output: Stdio| {
        Command::new("stty")
            .args(arguments)
            .stdin(Stdio::inherit())
            .stdout(output)
            .stderr(Stdio::null())
            .output()
    };

    // `stty -g` prints the settings in a form `stty` takes back
    let saved = Command::new("stty")
        .arg("-g")
        .stdin(Stdio::inherit())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|saved| saved.trim().to_owned())
        .filter(|saved| !saved.is_empty());

    let hidden = saved.is_some()
        && stty(
            &["-echo", "-isig", "-icanon", "min", "1", "time", "0"],
            Stdio::null(),
        )
        .is_ok_and(|output| output.status.success());

    if !hidden {
        eprintln!("(cannot hide the typing: stty failed)");
    }

    let read = read();

    if let Some(saved) = saved {
        drop(stty(&[saved.as_str()], Stdio::null()));
    }

    read
}

// a line typed with the terminal as `hidden` leaves it: Ctrl-C or Ctrl-D cancels, Backspace edits
fn read_secret() -> Result<String, String> {
    let mut line = Vec::new();
    let mut stdin = io::stdin().lock();
    let mut byte = [0u8; 1];

    loop {
        match stdin.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("cannot read stdin: {error}")),
        }

        match byte[0] {
            b'\n' | b'\r' => break,
            0x03 => return Err(String::from("cancelled")),
            0x04 if line.is_empty() => return Err(String::from("cancelled")),
            0x7f | 0x08 => drop(line.pop()),
            other if line.len() < LINE as usize => line.push(other),
            _ => {}
        }
    }

    String::from_utf8(line).map_err(|_| String::from("that is not an API key: not plain ASCII"))
}

/*
 * shows the disclosure and records consent once the user types `PHRASE` at a terminal. Piped input
 * is refused: a script must not accept it for them
 */
fn consent() -> Result<String, String> {
    if !io::stdin().is_terminal() {
        return Err(String::from(
            "consent is typed at a terminal; run `kanade dictate consent` in one",
        ));
    }

    eprintln!("{}", gate::DISCLOSURE);
    eprint!(
        "Type \"{}\" to accept, anything else to decline: ",
        gate::PHRASE
    );
    drop(io::stderr().flush());

    if read_line()?.trim() != gate::PHRASE {
        return Err(String::from("not accepted; nothing was recorded"));
    }

    gate::record_consent().map_err(|error| format!("cannot record the consent: {error}"))?;

    Ok(String::from("accepted\n"))
}

// deletes the key and takes the consent back, and asks a shell running to drop a dictation under way
fn forget() -> Result<String, String> {
    let key = gate::delete_key();
    let consent = gate::withdraw_consent();

    // best effort: the shell also checks the consent again just before sending, which is what
    // guarantees nothing is uploaded after this; no shell or a Module off is nothing to cancel
    let cancel = [String::from("dictate"), String::from("cancel")];
    let cancelled = matches!(
        super::call_within(&cancel, Duration::from_secs(2)),
        Ok(Reply::Done(said)) if said == "cancelled"
    );

    // both are tried, so one that fails does not leave the other behind
    match (key, consent) {
        (Ok(()), Ok(())) if cancelled => Ok(String::from(
            "key deleted and consent withdrawn; the dictation under way was cancelled\n",
        )),
        (Ok(()), Ok(())) => Ok(String::from("key deleted and consent withdrawn\n")),
        (key, consent) => Err([
            key.err().map(|why| format!("cannot delete the key: {why}")),
            consent
                .err()
                .map(|error| format!("cannot withdraw the consent: {error}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("; ")),
    }
}
