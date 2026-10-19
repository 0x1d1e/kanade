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

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

const USAGE: &str = "usage: kanade-install --script PATH [--shell-only] [--optional]";

/// Same sentence as `scripts/install`.
const NO_PACMAN: &str = "failed to find pacman, Kanade installs on Arch Linux and derivatives only";

/// Copper `#c4a484`. Name, focus mark, and a refusal. Nothing else.
const COPPER: Color = Color::Rgb(0xc4, 0xa4, 0x84);

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
        restore_terminal();
    }
}

fn main() {
    let code = run();
    std::process::exit(code);
}

fn run() -> i32 {
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
        return 1;
    }

    let mut stdout = io::stdout();
    if enter_terminal(&mut stdout).is_err() {
        restore_terminal();
        eprintln!("failed to find a terminal");
        return 1;
    }
    let guard = TermGuard;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(err) => {
            drop(guard);
            eprintln!("failed to open the terminal: {err}");
            return 1;
        }
    };
    if let Err(err) = terminal.clear() {
        drop(terminal);
        drop(guard);
        eprintln!("failed to open the terminal: {err}");
        return 1;
    }
    let _ = terminal.hide_cursor();

    let mut stage = Stage::Scan;
    let mut focus = 0usize;
    let mut shell_only = args.shell_only;
    let mut optional = args.optional;
    let mut run_state: Option<Run> = None;

    let outcome = loop {
        if let Some(active) = run_state.as_ref() {
            let _ = active.finished();
        }
        let ui = Ui {
            scan: &scan,
            stage,
            focus,
            shell_only,
            optional,
            run: run_state.as_ref(),
        };
        if let Err(err) = terminal.draw(|frame| draw(frame, &ui)) {
            break Outcome::Fail(format!("failed to draw: {err}"));
        }

        if !event::poll(Duration::from_millis(50)).unwrap_or(false) {
            continue;
        }
        let Ok(happened) = event::read() else {
            continue;
        };
        if let Event::Resize(cols, rows) = happened {
            if let Some(active) = run_state.as_ref() {
                active.resize(cols, rows.saturating_sub(3).max(1));
            }
            let _ = terminal.clear();
            continue;
        }
        let Event::Key(key) = happened else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if let Some(active) = run_state.as_ref() {
            if active.finished() {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        break Outcome::Code(active.exit_code().unwrap_or(1));
                    }
                    _ => {}
                }
                continue;
            }
            active.forward(key.code, key.modifiers);
            continue;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            break Outcome::Code(quit_code(&scan));
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => break Outcome::Code(quit_code(&scan)),
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
                Stage::Plan => {
                    let rows = terminal.size().map(|size| size.height).unwrap_or(24);
                    match start_script(
                        &args.script,
                        shell_only,
                        optional,
                        rows.saturating_sub(3).max(1),
                    ) {
                        Ok(started) => {
                            run_state = Some(started);
                            stage = Stage::Running;
                        }
                        Err(err) => {
                            break Outcome::Fail(format!(
                                "failed to run the install script: {err}"
                            ));
                        }
                    }
                }
                Stage::Running => {}
            },
            _ => {}
        }
    };

    drop(terminal);
    drop(guard);
    match outcome {
        Outcome::Code(code) => code,
        Outcome::Fail(message) => {
            eprintln!("{message}");
            1
        }
    }
}

enum Outcome {
    Code(i32),
    Fail(String),
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
}

fn copper() -> Style {
    Style::default().fg(COPPER)
}

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn draw(frame: &mut ratatui::Frame, ui: &Ui<'_>) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    if area.width == 0 || area.height == 0 {
        return;
    }

    let head_rows = match ui.stage {
        Stage::Scan => 3,
        Stage::Plan | Stage::Running => 2,
    };
    let [head, body, foot] = Layout::vertical([
        Constraint::Length(head_rows),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    frame.render_widget(Paragraph::new(header(ui)), head);
    draw_body(frame, body, ui);
    frame.render_widget(Paragraph::new(footer(ui)), foot);
}

fn header(ui: &Ui<'_>) -> Vec<Line<'static>> {
    let name = Line::from(Span::styled("Kanade", copper()));
    match ui.stage {
        Stage::Scan => vec![name, Line::from("A shell for niri."), Line::from("")],
        Stage::Plan => vec![name, Line::from("")],
        Stage::Running => {
            let status = match ui.run.and_then(Run::exit_code) {
                Some(code) => format!("exited {code}"),
                None => "running".to_string(),
            };
            vec![name, Line::from(status)]
        }
    }
}

fn draw_body(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, ui: &Ui<'_>) {
    match ui.stage {
        Stage::Scan => draw_scan(frame, area, ui.scan),
        Stage::Plan => {
            frame.render_widget(
                Paragraph::new(plan_lines(ui)).wrap(Wrap { trim: false }),
                area,
            );
        }
        Stage::Running => {
            let lines = running_lines(ui.run, area.height);
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
        }
    }
}

fn draw_scan(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, scan: &Scan) {
    let mut lines = fact_lines(scan);
    if !scan.refusal.is_empty() {
        let needed = lines.len() as u16 + 1 + scan.refusal.len() as u16;
        if needed > area.height && area.height > 0 {
            let keep = area.height.saturating_sub(1 + scan.refusal.len() as u16) as usize;
            lines.truncate(keep);
        }
        lines.push(Line::from(""));
        for line in &scan.refusal {
            lines.push(Line::from(Span::styled(line.clone(), copper())));
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn fact_lines(scan: &Scan) -> Vec<Line<'static>> {
    let pairs: Vec<(&str, &str)> = scan.lines.iter().map(|line| split_fact(line)).collect();
    let width = pairs
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    pairs
        .into_iter()
        .map(|(label, value)| fact_line(label, value, width))
        .collect()
}

fn split_fact(line: &str) -> (&str, &str) {
    line.split_once(": ").unwrap_or((line, ""))
}

fn fact_line(label: &str, value: &str, label_width: usize) -> Line<'static> {
    let pad = label_width.saturating_sub(label.chars().count()) + 3;
    Line::from(vec![
        Span::styled(label.to_string(), dim()),
        Span::raw(" ".repeat(pad)),
        Span::raw(value.to_string()),
    ])
}

fn plan_lines(ui: &Ui<'_>) -> Vec<Line<'static>> {
    let mut lines = vec![
        choice_line(ui.focus == 0, "shell-only", ui.shell_only),
        choice_line(ui.focus == 1, "optional packages", ui.optional),
        Line::from(""),
    ];
    if ui.shell_only {
        lines.push(dim_line("niri config is not installed with shell-only"));
    } else {
        let path = ui.scan.config.display();
        lines.push(dim_line(format!(
            "niri config is installed only when {path} is missing, and never replaced"
        )));
    }
    lines.push(dim_line(format!("helper: {}", ui.scan.helper)));
    if ui.scan.helper == "none" {
        lines.push(dim_line(
            "paru is installed with pacman, then used for packages",
        ));
    }
    lines
}

fn choice_line(focused: bool, label: &str, on: bool) -> Line<'static> {
    let style = if focused { Style::default() } else { dim() };
    let state = if on { "on" } else { "off" };
    let pad = "optional packages".len().saturating_sub(label.len()) + 3;
    Line::from(vec![
        Span::styled(
            if focused { ">" } else { " " },
            if focused { copper() } else { Style::default() },
        ),
        Span::styled("  ", style),
        Span::styled(label.to_string(), style),
        Span::styled(" ".repeat(pad), style),
        Span::styled(state.to_string(), style),
    ])
}

fn dim_line(text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(text.into(), dim()))
}

fn running_lines(run: Option<&Run>, rows: u16) -> Vec<Line<'static>> {
    let Some(run) = run else {
        return Vec::new();
    };
    let stored = run
        .lines
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let shown = stored.tail(rows.max(1) as usize);
    drop(stored);
    shown.into_iter().map(Line::from).collect()
}

fn footer(ui: &Ui<'_>) -> Line<'static> {
    match ui.stage {
        Stage::Scan if ui.scan.refusal.is_empty() => {
            keys(&[("enter: plan", true), ("q: quit", false)])
        }
        Stage::Scan => keys(&[("q: quit", true)]),
        Stage::Plan => keys(&[
            ("space: toggle", false),
            ("enter: run the script", true),
            ("q: quit", false),
        ]),
        Stage::Running if ui.run.is_some_and(Run::finished) => keys(&[("q: close", true)]),
        Stage::Running => Line::from(""),
    }
}

fn keys(items: &[(&str, bool)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, (text, primary)) in items.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("    ", dim()));
        }
        let style = if *primary { Style::default() } else { dim() };
        spans.push(Span::styled((*text).to_string(), style));
    }
    Line::from(spans)
}

fn enter_terminal(stdout: &mut io::Stdout) -> io::Result<()> {
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, Hide)?;
    // The curl line can leave OSC 8 open, so its URL stays painted on the cells.
    wipe(stdout)?;
    Ok(())
}

fn wipe(stdout: &mut io::Stdout) -> io::Result<()> {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut buf = Vec::new();
    buf.extend_from_slice(b"\x1b[0m\x1b]8;;\x1b\\");
    buf.extend_from_slice(b"\x1b[2J\x1b[H");
    for row in 1..=rows {
        write!(buf, "\x1b[{row};1H")?;
        buf.extend(std::iter::repeat_n(b' ', cols as usize));
    }
    buf.extend_from_slice(b"\x1b[1;1H");
    stdout.write_all(&buf)?;
    stdout.flush()
}

fn restore_terminal() {
    let mut stdout = io::stdout();
    let _ = stdout.write_all(b"\x1b[0m\x1b]8;;\x1b\\");
    let _ = execute!(stdout, Show, LeaveAlternateScreen);
    let _ = disable_raw_mode();
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
            .unwrap_or_else(|poison| poison.into_inner())
            .is_some()
        {
            return true;
        }
        let mut child = self
            .child
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        match child.try_wait() {
            Ok(Some(status)) => {
                let code = i32::try_from(status.exit_code()).unwrap_or(1);
                *self
                    .exit
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()) = Some(code);
                true
            }
            _ => false,
        }
    }

    fn exit_code(&self) -> Option<i32> {
        *self
            .exit
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
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
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let _ = writer.write_all(&bytes);
        let _ = writer.flush();
    }
}

fn start_script(script: &Path, shell_only: bool, optional: bool, rows: u16) -> io::Result<Run> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: rows.max(1),
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
    /// A `\r` waiting to see if the next byte is `\n`. A pty writes `\r\n`.
    cr: bool,
}

impl LineBuf {
    fn push(&mut self, text: &str) {
        for ch in text.chars() {
            if self.cr {
                self.cr = false;
                if ch == '\n' {
                    self.commit();
                    continue;
                }
                self.cur.clear();
            }
            match ch {
                '\r' => self.cr = true,
                '\n' => self.commit(),
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

#[cfg(test)]
mod tests {
    use super::LineBuf;

    #[test]
    fn pty_crlf_keeps_the_line() {
        let mut buf = LineBuf::default();
        buf.push("script-output-line\r\n");
        buf.push("split\r");
        buf.push("\n");
        buf.push("bare\n");
        assert_eq!(
            buf.tail(10),
            vec![
                "script-output-line".to_string(),
                "split".to_string(),
                "bare".to_string(),
            ]
        );
    }
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if next.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        out.push(ch);
    }
    out
}
