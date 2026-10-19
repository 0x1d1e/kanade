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
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

const USAGE: &str = "usage: kanade-install --script PATH [--shell-only] [--no-niri] [--niri] [--sddm] [--sddm-greeter keep|niri] [--optional]";

/// Same sentence as `scripts/install`.
const NO_PACMAN: &str = "failed to find pacman, Kanade installs on Arch Linux and derivatives only";

/// Copper `#c4a484`. Name, focus mark, and a refusal. Nothing else.
const COPPER: Color = Color::Rgb(0xc4, 0xa4, 0x84);

const INSET_X: u16 = 4;
const INSET_Y: u16 = 2;
const COLUMN: u16 = 72;
/// A full-width rule reads as a widget. This is ground under the name.
const RULE: u16 = 16;

struct Args {
    script: PathBuf,
    shell_only: bool,
    no_niri: bool,
    niri: bool,
    sddm: bool,
    /// `Some` when `--sddm-greeter` was passed. `true` is niri.
    greeter_niri: Option<bool>,
    optional: bool,
}

struct Scan {
    lines: Vec<String>,
    /// Non-empty: do not install. The strings are the script's refusal lines.
    refusal: Vec<String>,
    helper: &'static str,
    config: PathBuf,
    /// NVIDIA beside another device. The login screen row starts on niri.
    hybrid_nvidia: bool,
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
    let mut niri = niri_on(args.shell_only, args.no_niri, args.sddm, args.niri);
    let mut sddm = args.sddm;
    let mut login_niri = args.greeter_niri.unwrap_or(scan.hybrid_nvidia);
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
            niri,
            sddm,
            login_niri,
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
                active.resize(column_width(cols), body_rows(rows));
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
                if stage == Stage::Plan && focus + 1 < choice_count(sddm) {
                    focus += 1;
                }
            }
            KeyCode::Char(' ') => {
                if stage == Stage::Plan {
                    toggle(
                        focus,
                        &mut shell_only,
                        &mut niri,
                        &mut sddm,
                        &mut login_niri,
                        &mut optional,
                    );
                    if focus >= choice_count(sddm) {
                        focus = choice_count(sddm) - 1;
                    }
                }
            }
            KeyCode::Enter => match stage {
                Stage::Scan if scan.refusal.is_empty() => stage = Stage::Plan,
                Stage::Scan => {}
                Stage::Plan => {
                    let (cols, rows) = terminal
                        .size()
                        .map(|size| (size.width, size.height))
                        .unwrap_or((80, 24));
                    match start_script(
                        &args.script,
                        &Plan {
                            shell_only,
                            niri,
                            sddm,
                            login_niri,
                            optional,
                        },
                        column_width(cols),
                        body_rows(rows),
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
    niri: bool,
    sddm: bool,
    login_niri: bool,
    optional: bool,
    run: Option<&'a Run>,
}

/// shell-only, niri, sddm, then the login screen row only while sddm is on, then optional packages.
fn choice_count(sddm: bool) -> usize {
    if sddm { 5 } else { 4 }
}

fn niri_on(shell_only: bool, no_niri: bool, sddm: bool, requested: bool) -> bool {
    sddm || requested || (!shell_only && !no_niri)
}

fn toggle(
    focus: usize,
    shell_only: &mut bool,
    niri: &mut bool,
    sddm: &mut bool,
    login_niri: &mut bool,
    optional: &mut bool,
) {
    let optional_at = if *sddm { 4 } else { 3 };
    match focus {
        0 => {
            *shell_only = !*shell_only;
            if *shell_only {
                *niri = false;
                *sddm = false;
            }
        }
        1 => {
            *niri = !*niri;
            if !*niri {
                *sddm = false;
            }
        }
        2 => {
            *sddm = !*sddm;
            if *sddm {
                *niri = true;
            }
        }
        3 if *sddm => *login_niri = !*login_niri,
        at if at == optional_at => *optional = !*optional,
        _ => {}
    }
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

    let width = column_width(area.width);
    let x = area.x.saturating_add(inset_x(area.width));
    let lines = header(ui, width);
    let header_pref = u16::try_from(lines.len()).unwrap_or(0);
    let keys = key_line(ui);
    let want_keys = keys.is_some();
    let body_natural = body_natural_rows(ui, width);
    let refusal = match ui.stage {
        Stage::Scan => refusal_rows(ui.scan, width),
        Stage::Plan | Stage::Running => 0,
    };
    let y_off = top_inset(
        area.height,
        header_pref
            .saturating_add(refusal)
            .saturating_add(u16::from(want_keys)),
    );
    let top = area.y.saturating_add(y_off);
    let avail = area.height.saturating_sub(y_off);
    let page = place_page(
        avail,
        header_pref,
        body_natural,
        refusal,
        want_keys,
        wants_lead_gap(ui.stage),
    );

    if page.header > 0 {
        let shown: Vec<_> = lines.into_iter().take(usize::from(page.header)).collect();
        frame.render_widget(Paragraph::new(shown), Rect::new(x, top, width, page.header));
    }
    let body_y = top
        .saturating_add(page.header)
        .saturating_add(page.lead_gap);
    if page.body > 0 {
        draw_body(frame, Rect::new(x, body_y, width, page.body), ui);
    }
    if let Some(keys) = keys {
        let keys_y = if page.keys_pinned {
            top.saturating_add(avail.saturating_sub(1))
        } else {
            body_y
                .saturating_add(page.body)
                .saturating_add(page.key_gap)
        };
        frame.render_widget(Paragraph::new(keys), Rect::new(x, keys_y, width, 1));
    }
}

/// Keys follow the last content line. They take the last visible row only when the page does not fit.
struct Page {
    header: u16,
    lead_gap: u16,
    body: u16,
    key_gap: u16,
    keys_pinned: bool,
}

fn place_page(
    avail: u16,
    header_pref: u16,
    body_natural: u16,
    refusal: u16,
    want_keys: bool,
    want_lead_gap: bool,
) -> Page {
    let keys = u16::from(want_keys);
    let lead = u16::from(want_lead_gap && body_natural > 0);
    let key_gap = keys;
    let needed = header_pref
        .saturating_add(lead)
        .saturating_add(body_natural)
        .saturating_add(key_gap)
        .saturating_add(keys);
    if needed <= avail {
        return Page {
            header: header_pref,
            lead_gap: lead,
            body: body_natural,
            key_gap,
            keys_pinned: false,
        };
    }
    let keys = keys.min(avail);
    let rest = avail.saturating_sub(keys);
    let refusal = refusal.min(rest);
    let header = header_pref.min(rest.saturating_sub(refusal));
    Page {
        header,
        lead_gap: 0,
        body: rest.saturating_sub(header),
        key_gap: 0,
        keys_pinned: keys > 0,
    }
}

fn wants_lead_gap(stage: Stage) -> bool {
    matches!(stage, Stage::Scan | Stage::Running)
}

fn inset_x(cols: u16) -> u16 {
    INSET_X.min(cols.saturating_sub(1))
}

fn top_inset(rows: u16, min_page: u16) -> u16 {
    if rows <= 1 {
        return 0;
    }
    let preferred = INSET_Y.min(rows.saturating_sub(1));
    preferred.min(rows.saturating_sub(min_page))
}

fn column_width(cols: u16) -> u16 {
    cols.saturating_sub(inset_x(cols)).clamp(1, COLUMN)
}

fn body_rows(rows: u16) -> u16 {
    let head = u16::try_from(header_lines(Stage::Running, 1, "running").len()).unwrap_or(1);
    let inset = top_inset(rows, head);
    let avail = rows.saturating_sub(inset);
    avail.saturating_sub(head.min(avail)).max(1)
}

fn header(ui: &Ui<'_>, width: u16) -> Vec<Line<'static>> {
    let status = match ui.run.and_then(Run::exit_code) {
        Some(code) => format!("exited {code}"),
        None => "running".to_string(),
    };
    header_lines(ui.stage, width, &status)
}

fn header_lines(stage: Stage, width: u16, status: &str) -> Vec<Line<'static>> {
    let name = Line::from(Span::styled(
        "Kanade",
        copper().add_modifier(Modifier::BOLD),
    ));
    let rule_len = usize::from(RULE.min(width.max(1)));
    let rule = Line::from(Span::styled("\u{2500}".repeat(rule_len), dim()));
    match stage {
        Stage::Scan => vec![name, rule, Line::from("A shell for niri.")],
        Stage::Plan => vec![name, rule],
        Stage::Running => vec![name, rule, Line::from(status.to_string())],
    }
}

fn draw_body(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, ui: &Ui<'_>) {
    match ui.stage {
        Stage::Scan => draw_scan(frame, area, ui.scan),
        Stage::Plan => {
            frame.render_widget(Paragraph::new(plan_lines(ui, area.width)), area);
        }
        Stage::Running => {
            let lines = running_lines(ui.run, area.height, area.width);
            frame.render_widget(Paragraph::new(lines), area);
        }
    }
}

fn draw_scan(frame: &mut ratatui::Frame, area: Rect, scan: &Scan) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let pairs: Vec<(&str, &str)> = scan.lines.iter().map(|line| split_fact(line)).collect();
    let label_width = pairs
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    let label_w = u16::try_from(label_width)
        .unwrap_or(u16::MAX)
        .min(area.width);
    let value_x = label_w.saturating_add(3);
    let value_w = area.width.saturating_sub(value_x);

    let refusal_rows = refusal_rows(scan, area.width);
    let reserve = if refusal_rows == 0 {
        0
    } else {
        refusal_rows.saturating_add(1)
    };
    let facts_room = area.height.saturating_sub(reserve);

    let mut y = 0u16;
    for (label, value) in pairs {
        if y >= facts_room {
            break;
        }
        let rows = text_rows(value, value_w).min(facts_room - y);
        frame.render_widget(
            Paragraph::new(Span::styled(label.to_string(), dim())),
            Rect::new(area.x, area.y + y, label_w, 1),
        );
        if value_w > 0 && rows > 0 {
            frame.render_widget(
                Paragraph::new(value.to_string()).wrap(Wrap { trim: false }),
                Rect::new(area.x + value_x, area.y + y, value_w, rows),
            );
        }
        y = y.saturating_add(rows);
    }

    if scan.refusal.is_empty() || area.height == 0 {
        return;
    }
    let start = if y == 0 {
        0
    } else {
        y.saturating_add(1).min(area.height)
    };
    if start >= area.height {
        return;
    }
    let lines: Vec<Line> = scan
        .refusal
        .iter()
        .flat_map(|line| wrap_text(line, area.width))
        .map(|line| Line::from(Span::styled(line, copper())))
        .collect();
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(area.x, area.y + start, area.width, area.height - start),
    );
}

fn text_rows(text: &str, width: u16) -> u16 {
    let width = usize::from(width.max(1));
    let len = text.chars().count().max(1);
    u16::try_from(len.div_ceil(width))
        .unwrap_or(u16::MAX)
        .max(1)
}

fn split_fact(line: &str) -> (&str, &str) {
    line.split_once(": ").unwrap_or((line, ""))
}

fn plan_lines(ui: &Ui<'_>, width: u16) -> Vec<Line<'static>> {
    let lead = "Installs the shell, the unit, and the icon.";
    let mut lines: Vec<Line<'static>> =
        wrap_text(lead, width).into_iter().map(Line::from).collect();
    let mut push_choice = |focused, label: &str, state: &str| {
        lines.extend(choice_lines(focused, label, state, width));
    };
    push_choice(ui.focus == 0, "shell-only", on_off(ui.shell_only));
    push_choice(ui.focus == 1, "niri", on_off(ui.niri));
    push_choice(ui.focus == 2, "sddm", on_off(ui.sddm));
    let mut optional_at = 3;
    if ui.sddm {
        push_choice(
            ui.focus == 3,
            "login screen",
            if ui.login_niri { "niri" } else { "keep" },
        );
        optional_at = 4;
    }
    push_choice(
        ui.focus == optional_at,
        "optional packages",
        on_off(ui.optional),
    );
    lines.push(Line::from(""));
    let installed = if ui.niri {
        "niri is installed"
    } else {
        "niri is not installed"
    };
    lines.extend(wrap_text(installed, width).into_iter().map(dim_line));
    if ui.shell_only {
        lines.extend(
            wrap_text("Shell-only skips desktop packages and niri config; enabling niri or SDDM still installs niri.", width)
                .into_iter()
                .map(dim_line),
        );
    } else if ui.niri {
        let path = ui.scan.config.display();
        let niri =
            format!("niri config is installed only when {path} is missing, and never replaced");
        lines.extend(wrap_text(&niri, width).into_iter().map(dim_line));
    }
    if ui.sddm {
        lines.extend(
            wrap_text(
                "SDDM installs with niri, sets up the Kanade theme, and enables SDDM for the next boot only.",
                width,
            )
            .into_iter()
            .map(dim_line),
        );
    }
    lines.push(Line::from(""));
    lines.extend(
        wrap_text(&format!("helper: {}", ui.scan.helper), width)
            .into_iter()
            .map(dim_line),
    );
    if ui.scan.helper == "none" {
        lines.extend(
            wrap_text(
                "No helper found; pacman installs paru first, then paru installs packages.",
                width,
            )
            .into_iter()
            .map(dim_line),
        );
    }
    lines
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn choice_lines(focused: bool, label: &str, state: &str, width: u16) -> Vec<Line<'static>> {
    let style = if focused { Style::default() } else { dim() };
    let marker = if focused { ">" } else { " " };
    let prefix = format!("{marker}  ");
    let available = usize::from(width.max(1));
    let label_width = label.chars().count();
    let state_width = state.chars().count();
    if prefix.chars().count() + label_width + state_width + 3 <= available {
        return vec![Line::from(vec![
            Span::styled(marker, if focused { copper() } else { Style::default() }),
            Span::styled("  ", style),
            Span::styled(label.to_string(), style),
            Span::styled(" ".repeat(3), style),
            Span::styled(state.to_string(), style),
        ])];
    }
    let mut lines: Vec<Line<'static>> = wrap_text(label, width.saturating_sub(3).max(1))
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            Line::from(vec![
                Span::styled(
                    if index == 0 { marker } else { " " },
                    if index == 0 && focused {
                        copper()
                    } else {
                        Style::default()
                    },
                ),
                Span::styled("  ", style),
                Span::styled(text, style),
            ])
        })
        .collect();
    if let Some(last) = lines.last_mut() {
        let used = last.width();
        if used + state_width + 1 <= available {
            last.spans.push(Span::styled(" ", style));
            last.spans.push(Span::styled(state.to_string(), style));
            return lines;
        }
    }
    lines.push(Line::from(vec![
        Span::styled("   ", style),
        Span::styled(state.to_string(), style),
    ]));
    lines
}

fn dim_line(text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(text.into(), dim()))
}

fn running_lines(run: Option<&Run>, rows: u16, width: u16) -> Vec<Line<'static>> {
    let Some(run) = run else {
        return Vec::new();
    };
    let visual = visual_lines(&output_lines(run), width);
    let start = visual.len().saturating_sub(usize::from(rows));
    visual.into_iter().skip(start).map(Line::from).collect()
}

fn output_lines(run: &Run) -> Vec<String> {
    run.lines
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .tail(usize::MAX)
}

fn visual_lines(lines: &[String], width: u16) -> Vec<String> {
    lines
        .iter()
        .flat_map(|line| wrap_text(line, width))
        .collect()
}

fn body_natural_rows(ui: &Ui<'_>, width: u16) -> u16 {
    match ui.stage {
        Stage::Scan => scan_body_rows(ui.scan, width),
        Stage::Plan => u16::try_from(plan_lines(ui, width).len()).unwrap_or(u16::MAX),
        Stage::Running => {
            let lines = ui.run.map(output_lines).unwrap_or_default();
            u16::try_from(visual_lines(&lines, width).len()).unwrap_or(u16::MAX)
        }
    }
}

fn scan_body_rows(scan: &Scan, width: u16) -> u16 {
    let facts = fact_rows(scan, width);
    let refusal = refusal_rows(scan, width);
    if refusal == 0 {
        facts
    } else if facts == 0 {
        refusal
    } else {
        facts.saturating_add(1).saturating_add(refusal)
    }
}

fn fact_rows(scan: &Scan, width: u16) -> u16 {
    let pairs: Vec<(&str, &str)> = scan.lines.iter().map(|line| split_fact(line)).collect();
    let label_width = pairs
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    let label_w = u16::try_from(label_width).unwrap_or(u16::MAX).min(width);
    let value_w = width.saturating_sub(label_w.saturating_add(3));
    pairs.iter().fold(0u16, |rows, (_, value)| {
        rows.saturating_add(text_rows(value, value_w))
    })
}

fn refusal_rows(scan: &Scan, width: u16) -> u16 {
    if scan.refusal.is_empty() || width == 0 {
        return 0;
    }
    scan.refusal.iter().fold(0u16, |rows, line| {
        rows.saturating_add(wrap_len(line, width))
    })
}

fn wrap_len(text: &str, width: u16) -> u16 {
    u16::try_from(wrap_text(text, width).len())
        .unwrap_or(u16::MAX)
        .max(1)
}

/// Break on a space when one fits. The column is the measure, not the terminal.
fn wrap_text(text: &str, width: u16) -> Vec<String> {
    let width = usize::from(width);
    if width == 0 {
        return Vec::new();
    }
    if text.is_empty() {
        return vec![String::new()];
    }
    let chars: Vec<char> = text.chars().collect();
    let mut rows = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars.len() - index <= width {
            rows.push(chars[index..].iter().collect());
            break;
        }
        let end = index + width;
        let break_at = chars[index..end].iter().rposition(|ch| *ch == ' ');
        if let Some(rel) = break_at.filter(|rel| *rel > 0) {
            rows.push(chars[index..index + rel].iter().collect());
            index += rel + 1;
        } else {
            rows.push(chars[index..end].iter().collect());
            index = end;
        }
    }
    rows
}

fn key_line(ui: &Ui<'_>) -> Option<Line<'static>> {
    match ui.stage {
        Stage::Scan if ui.scan.refusal.is_empty() => {
            Some(keys(&[("enter: plan", true), ("q: quit", false)]))
        }
        Stage::Scan => Some(keys(&[("q: quit", true)])),
        Stage::Plan => Some(keys(&[
            ("space: toggle", false),
            ("enter: run the script", true),
            ("q: quit", false),
        ])),
        Stage::Running if ui.run.is_some_and(Run::finished) => Some(keys(&[("q: close", true)])),
        Stage::Running => None,
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
    let mut no_niri = false;
    let mut niri = false;
    let mut sddm = false;
    let mut greeter_niri = None;
    let mut optional = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--shell-only" => shell_only = true,
            "--no-niri" => no_niri = true,
            "--niri" => niri = true,
            "--sddm" => sddm = true,
            "--sddm-greeter" => {
                let Some(value) = args.next() else {
                    eprintln!("{USAGE}");
                    std::process::exit(2);
                };
                greeter_niri = Some(match value.as_str() {
                    "keep" => false,
                    "niri" => true,
                    _ => {
                        eprintln!("{USAGE}");
                        std::process::exit(2);
                    }
                });
            }
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
        no_niri,
        niri,
        sddm,
        greeter_niri,
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
        hybrid_nvidia: hybrid_nvidia(),
    }
}

fn hybrid_nvidia() -> bool {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return false;
    };
    let mut nvidia = false;
    let mut devices = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with("card") {
            continue;
        }
        let device = entry.path().join("device");
        let Ok(vendor) = std::fs::read_to_string(device.join("vendor")) else {
            continue;
        };
        if vendor.trim() == "0x10de" {
            nvidia = true;
        }
        let real = std::fs::canonicalize(&device).unwrap_or(device);
        if !devices.contains(&real) {
            devices.push(real);
        }
    }
    nvidia && devices.len() >= 2
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

struct Plan {
    shell_only: bool,
    niri: bool,
    sddm: bool,
    login_niri: bool,
    optional: bool,
}

fn start_script(script: &Path, plan: &Plan, cols: u16, rows: u16) -> io::Result<Run> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|err| io::Error::other(err.to_string()))?;
    let mut cmd = CommandBuilder::new("sh");
    cmd.arg(script);
    cmd.arg("--yes");
    if plan.shell_only {
        cmd.arg("--shell-only");
    }
    if plan.niri {
        cmd.arg("--niri");
    } else {
        cmd.arg("--no-niri");
    }
    if plan.sddm {
        cmd.arg("--sddm");
        cmd.arg("--sddm-greeter");
        cmd.arg(if plan.login_niri { "niri" } else { "keep" });
    }
    if plan.optional {
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
    use super::{LineBuf, choice_count, niri_on, toggle};

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

    #[test]
    fn choices_follow_the_rules() {
        let mut shell_only = false;
        let mut niri = true;
        let mut sddm = false;
        let mut login_niri = false;
        let mut optional = false;

        toggle(
            0,
            &mut shell_only,
            &mut niri,
            &mut sddm,
            &mut login_niri,
            &mut optional,
        );
        assert!(shell_only && !niri && !sddm);

        toggle(
            1,
            &mut shell_only,
            &mut niri,
            &mut sddm,
            &mut login_niri,
            &mut optional,
        );
        assert!(shell_only && niri && !sddm);

        toggle(
            2,
            &mut shell_only,
            &mut niri,
            &mut sddm,
            &mut login_niri,
            &mut optional,
        );
        assert!(shell_only && niri && sddm);
        assert_eq!(choice_count(sddm), 5);

        toggle(
            3,
            &mut shell_only,
            &mut niri,
            &mut sddm,
            &mut login_niri,
            &mut optional,
        );
        assert!(login_niri);
        toggle(
            4,
            &mut shell_only,
            &mut niri,
            &mut sddm,
            &mut login_niri,
            &mut optional,
        );
        assert!(optional);

        toggle(
            1,
            &mut shell_only,
            &mut niri,
            &mut sddm,
            &mut login_niri,
            &mut optional,
        );
        assert!(!niri && !sddm && optional);
        assert_eq!(choice_count(sddm), 4);

        assert!(niri_on(false, false, false, false));
        assert!(!niri_on(true, false, false, false));
        assert!(niri_on(true, true, true, false));
        assert!(niri_on(true, true, false, true));
        assert!(!niri_on(false, true, false, false));
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
