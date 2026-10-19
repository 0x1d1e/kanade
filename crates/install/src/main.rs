//! Install TUI. Three stages: scan, plan, the script. It does not install packages or the unit
//! itself. `scripts/install` does, after this binary execs it with `--yes`.

#![forbid(unsafe_code)]

use std::env;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

const USAGE: &str = "usage: kanade-install --script PATH [--shell-only] [--optional]";

/// Same sentence as `scripts/install`.
const NO_PACMAN: &str = "failed to find pacman, Kanade installs on Arch Linux and derivatives only";

struct Args {
    script: PathBuf,
    shell_only: bool,
    optional: bool,
}

struct Scan {
    lines: Vec<String>,
    /// Non-empty: do not install. The strings are the script's refusal lines.
    refusal: Vec<String>,
    helper: &'static str,
    config: PathBuf,
}

struct TermGuard;

impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

fn main() {
    let args = parse_args();
    let scan = scan();
    if !stdio_is_tty() {
        for line in &scan.lines {
            eprintln!("{line}");
        }
        for line in &scan.refusal {
            eprintln!("{line}");
        }
        if scan.refusal.is_empty() {
            eprintln!("failed to find a terminal");
        }
        std::process::exit(1);
    }

    let mut stdout = io::stdout();
    if enable_raw_mode().is_err() || execute!(stdout, EnterAlternateScreen).is_err() {
        eprintln!("failed to find a terminal");
        std::process::exit(1);
    }
    let _guard = TermGuard;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).unwrap_or_else(|err| {
        eprintln!("failed to open the terminal: {err}");
        std::process::exit(1);
    });

    let mut stage = Stage::Scan;
    let mut focus = 0usize;
    let mut shell_only = args.shell_only;
    let mut optional = args.optional;
    let mut run: Option<Run> = None;

    loop {
        let height = terminal.size().map(|s| s.height).unwrap_or(24);
        if let Err(err) = terminal.draw(|frame| {
            draw(
                frame,
                &Ui {
                    scan: &scan,
                    stage,
                    focus,
                    shell_only,
                    optional,
                    run: run.as_ref(),
                    script: &args.script,
                },
            );
        }) {
            eprintln!("failed to draw: {err}");
            std::process::exit(1);
        }

        if let Some(active) = run.as_ref() {
            let _ = active.finished();
        }
        if !event::poll(Duration::from_millis(50)).unwrap_or(false) {
            continue;
        }
        let Ok(happened) = event::read() else {
            continue;
        };
        if let Event::Resize(cols, rows) = happened {
            if let Some(active) = run.as_ref() {
                active.resize(cols, rows);
            }
            continue;
        }
        let Event::Key(key) = happened else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if let Some(active) = run.as_ref() {
            if active.finished() {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        let code = active.exit_code().unwrap_or(1);
                        let _ = disable_raw_mode();
                        let _ = execute!(io::stdout(), LeaveAlternateScreen);
                        std::process::exit(code);
                    }
                    _ => {}
                }
                continue;
            }
            active.forward(key.code, key.modifiers);
            continue;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            std::process::exit(quit_code(&scan));
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => std::process::exit(quit_code(&scan)),
            KeyCode::Up | KeyCode::BackTab => {
                if stage == Stage::Plan {
                    focus = focus.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                if stage == Stage::Plan && focus < 1 {
                    focus += 1;
                }
            }
            KeyCode::Char(' ') => {
                if stage == Stage::Plan {
                    if focus == 0 {
                        shell_only = !shell_only;
                    } else {
                        optional = !optional;
                    }
                }
            }
            KeyCode::Enter => match stage {
                Stage::Scan if scan.refusal.is_empty() => stage = Stage::Plan,
                Stage::Scan => {}
                Stage::Plan => match start_script(&args.script, shell_only, optional, height) {
                    Ok(started) => {
                        run = Some(started);
                        stage = Stage::Running;
                    }
                    Err(err) => {
                        let _ = disable_raw_mode();
                        let _ = execute!(io::stdout(), LeaveAlternateScreen);
                        eprintln!("failed to run the install script: {err}");
                        std::process::exit(1);
                    }
                },
                Stage::Running => {}
            },
            _ => {}
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Scan,
    Plan,
    Running,
}

#[derive(Clone, Copy)]
struct Ui<'a> {
    scan: &'a Scan,
    stage: Stage,
    focus: usize,
    shell_only: bool,
    optional: bool,
    run: Option<&'a Run>,
    script: &'a Path,
}

fn draw(frame: &mut ratatui::Frame, ui: &Ui<'_>) {
    let Ui {
        scan,
        stage,
        focus,
        shell_only,
        optional,
        run,
        script,
    } = *ui;
    let area = frame.area();
    let title = match stage {
        Stage::Scan => "kanade install  scan",
        Stage::Plan => "kanade install  plan",
        Stage::Running => "kanade install  running",
    };
    let mut lines = vec![Line::from(Span::styled(
        title,
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    lines.push(Line::from(""));

    match stage {
        Stage::Scan => {
            for line in &scan.lines {
                lines.push(Line::from(line.as_str()));
            }
            lines.push(Line::from(""));
            if scan.refusal.is_empty() {
                lines.push(Line::from("enter: plan    q: quit"));
            } else {
                for line in &scan.refusal {
                    lines.push(Line::from(Span::styled(
                        line.as_str(),
                        Style::default().add_modifier(Modifier::BOLD),
                    )));
                }
                lines.push(Line::from("q: quit"));
            }
        }
        Stage::Plan => {
            lines.push(toggle_line(focus == 0, "shell-only", shell_only));
            lines.push(toggle_line(focus == 1, "optional packages", optional));
            lines.push(Line::from(""));
            if shell_only {
                lines.push(Line::from("niri config is not installed with shell-only"));
            } else {
                let path = scan.config.display();
                lines.push(Line::from(format!(
                    "niri config is installed only when {path} is missing, and never replaced"
                )));
            }
            lines.push(Line::from(format!("helper: {}", scan.helper)));
            if scan.helper == "none" {
                lines.push(Line::from(
                    "paru is installed with pacman, then used for packages",
                ));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(
                "space: toggle    enter: run the script    q: quit",
            ));
        }
        Stage::Running => {
            let command = script_command(script, shell_only, optional);
            lines.push(Line::from(command));
            lines.push(Line::from(
                "the script is running. this does not roll back.",
            ));
            lines.push(Line::from(""));
            if let Some(run) = run {
                let stored = run.lines.lock().unwrap_or_else(|p| p.into_inner());
                let shown = stored.tail(area.height.saturating_sub(8) as usize);
                for line in shown {
                    lines.push(Line::from(line));
                }
                drop(stored);
                lines.push(Line::from(""));
                if let Some(code) = run.exit_code() {
                    lines.push(Line::from(format!("exited {code}    q: close")));
                }
            }
        }
    }

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn toggle_line(focused: bool, label: &str, on: bool) -> Line<'static> {
    let mark = if on { "[x]" } else { "[ ]" };
    let text = format!("{mark} {label}");
    let style = if focused {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };
    let prefix = if focused { "> " } else { "  " };
    Line::from(vec![Span::raw(prefix), Span::styled(text, style)])
}

fn parse_args() -> Args {
    let mut script = None;
    let mut shell_only = false;
    let mut optional = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--shell-only" => shell_only = true,
            "--optional" => optional = true,
            "--script" => {
                let Some(path) = args.next() else {
                    eprintln!("{USAGE}");
                    std::process::exit(2);
                };
                script = Some(PathBuf::from(path));
            }
            other if !other.starts_with('-') && script.is_none() => {
                script = Some(PathBuf::from(other));
            }
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let Some(script) = script else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    if !script.is_file() {
        eprintln!("failed to find install script {}", script.display());
        std::process::exit(1);
    }
    Args {
        script,
        shell_only,
        optional,
    }
}

fn scan() -> Scan {
    let mut lines = Vec::new();
    let mut refusal = Vec::new();

    if command_exists("pacman") {
        lines.push("pacman: found".to_string());
    } else {
        lines.push("pacman: missing".to_string());
        refusal.push(NO_PACMAN.to_string());
    }

    let helper = if command_exists("paru") {
        "paru"
    } else if command_exists("yay") {
        "yay"
    } else {
        "none"
    };
    lines.push(format!("helper: {helper}"));

    if Path::new("/run/systemd/system").exists() {
        lines.push("systemd: /run/systemd/system".to_string());
    } else {
        lines.push("systemd: /run/systemd/system missing".to_string());
    }

    let machine = uname_m();
    lines.push(format!("machine: {machine}"));
    if machine != "x86_64" {
        refusal.push(format!(
            "failed: this machine is {machine}, Kanade's install binary is x86_64 only"
        ));
    }

    if command_exists("niri") {
        lines.push("niri: on PATH".to_string());
    } else {
        lines.push("niri: not on PATH".to_string());
    }

    let config = config_home().join("niri/config.kdl");
    if config.exists() {
        lines.push(format!("niri config: {} exists", config.display()));
    } else {
        lines.push(format!("niri config: {} missing", config.display()));
    }

    if cargo_bin_on_path() {
        lines.push("~/.cargo/bin: on PATH".to_string());
    } else {
        lines.push("~/.cargo/bin: not on PATH".to_string());
    }

    lines.push(notification_line());

    Scan {
        lines,
        refusal,
        helper,
        config,
    }
}

fn notification_line() -> String {
    if !command_exists("busctl") {
        return "notifications: busctl not found".to_string();
    }
    let output = Command::new("busctl")
        .args(["--user", "status", "org.freedesktop.Notifications"])
        .output();
    let Ok(output) = output else {
        return "notifications: busctl failed".to_string();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if let Some(owner) = line.strip_prefix("Comm=") {
            if owner.is_empty() {
                return "notifications: no owner".to_string();
            }
            return format!("notifications: {owner}");
        }
    }
    "notifications: no owner".to_string()
}

fn command_exists(name: &str) -> bool {
    Command::new("sh")
        .args([
            "-c",
            "command -v \"$1\" >/dev/null 2>&1",
            "kanade-install",
            name,
        ])
        .status()
        .is_ok_and(|status| status.success())
}

fn uname_m() -> String {
    let Ok(output) = Command::new("uname").arg("-m").output() else {
        return "unknown".to_string();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let machine = text.trim();
    if machine.is_empty() {
        "unknown".to_string()
    } else {
        machine.to_string()
    }
}

fn config_home() -> PathBuf {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME")
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }
    home().join(".config")
}

fn home() -> PathBuf {
    env::var_os("HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn cargo_bin_on_path() -> bool {
    // the same containment test as scripts/install, not a canonicalized path
    let Ok(path) = env::var("PATH") else {
        return false;
    };
    let Ok(home) = env::var("HOME") else {
        return false;
    };
    format!(":{path}:").contains(&format!(":{home}/.cargo/bin:"))
}

fn quit_code(scan: &Scan) -> i32 {
    if scan.refusal.is_empty() { 0 } else { 1 }
}

fn stdio_is_tty() -> bool {
    io::IsTerminal::is_terminal(&io::stdin())
}

struct Run {
    master: Box<dyn MasterPty + Send>,
    writer: Mutex<Box<dyn Write + Send>>,
    lines: Arc<Mutex<LineBuf>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    exit: Mutex<Option<i32>>,
}

impl Run {
    fn finished(&self) -> bool {
        if self
            .exit
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
        {
            return true;
        }
        let mut child = self.child.lock().unwrap_or_else(|p| p.into_inner());
        match child.try_wait() {
            Ok(Some(status)) => {
                let code = i32::try_from(status.exit_code()).unwrap_or(1);
                *self.exit.lock().unwrap_or_else(|p| p.into_inner()) = Some(code);
                true
            }
            _ => false,
        }
    }

    fn exit_code(&self) -> Option<i32> {
        *self.exit.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn resize(&self, cols: u16, rows: u16) {
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    fn forward(&self, code: KeyCode, modifiers: KeyModifiers) {
        let bytes = key_bytes(code, modifiers);
        if bytes.is_empty() {
            return;
        }
        let mut writer = self.writer.lock().unwrap_or_else(|p| p.into_inner());
        let _ = writer.write_all(&bytes);
        let _ = writer.flush();
    }
}

fn start_script(script: &Path, shell_only: bool, optional: bool, rows: u16) -> io::Result<Run> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: rows.max(24),
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|err| io::Error::other(err.to_string()))?;
    let mut cmd = CommandBuilder::new("sh");
    cmd.arg(script);
    cmd.arg("--yes");
    if shell_only {
        cmd.arg("--shell-only");
    }
    if optional {
        cmd.arg("--optional");
    }
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|err| io::Error::other(err.to_string()))?;
    drop(pair.slave);
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|err| io::Error::other(err.to_string()))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|err| io::Error::other(err.to_string()))?;
    let lines = Arc::new(Mutex::new(LineBuf::default()));
    let sink = Arc::clone(&lines);
    thread::spawn(move || read_pty(reader, sink));
    Ok(Run {
        master: pair.master,
        writer: Mutex::new(writer),
        lines,
        child: Mutex::new(child),
        exit: Mutex::new(None),
    })
}

fn read_pty(mut reader: Box<dyn Read + Send>, lines: Arc<Mutex<LineBuf>>) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let text = String::from_utf8_lossy(&buf[..n]);
                let clean = strip_ansi(&text);
                if let Ok(mut lines) = lines.lock() {
                    lines.push(&clean);
                }
            }
        }
    }
}

fn script_command(script: &Path, shell_only: bool, optional: bool) -> String {
    let mut command = format!("sh {} --yes", script.display());
    if shell_only {
        command.push_str(" --shell-only");
    }
    if optional {
        command.push_str(" --optional");
    }
    command
}

fn key_bytes(code: KeyCode, modifiers: KeyModifiers) -> Vec<u8> {
    match code {
        KeyCode::Char(ch) if modifiers.contains(KeyModifiers::CONTROL) => {
            let byte = (ch as u8) & 0x1f;
            if byte == 0 { Vec::new() } else { vec![byte] }
        }
        KeyCode::Char(ch) => ch.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        _ => Vec::new(),
    }
}

#[derive(Default)]
struct LineBuf {
    done: Vec<String>,
    cur: String,
}

impl LineBuf {
    fn push(&mut self, text: &str) {
        for ch in text.chars() {
            match ch {
                '\n' => self.commit(),
                '\r' => self.cur.clear(),
                other if other.is_control() => {}
                other => self.cur.push(other),
            }
        }
        if self.done.len() > 2000 {
            let extra = self.done.len() - 2000;
            self.done.drain(0..extra);
        }
    }

    fn commit(&mut self) {
        self.done.push(std::mem::take(&mut self.cur));
    }

    fn tail(&self, rows: usize) -> Vec<String> {
        let mut all: Vec<String> = self.done.clone();
        if !self.cur.is_empty() {
            all.push(self.cur.clone());
        }
        let rows = rows.max(1);
        if all.len() > rows {
            all.split_off(all.len() - rows)
        } else {
            all
        }
    }
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}
