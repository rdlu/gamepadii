//! Drawing: the title line, the pad, the mode and profile lists, the last
//! result (or what to do on the pad), the status line, and a popup on top.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap};

use gamepadii_core::mode::InputMode;

use super::app::{App, ListPos, Overlay, Pane};
use super::keys::{Action, BINDINGS, CONFIRM_KEYS};
use crate::output;

const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The keys in the status line when nothing runs.
const HINTS: &str = "⏎ switch  S save  w web-config  b backup  r refresh  ? help  q quit";

/// Draws the whole screen. Takes `&mut App` only to keep each list's scroll
/// offset where the selection shows.
pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let pad = pad_lines(app);
    let result = result_lines(app);
    // Rows once wrapped inside the borders and padding.
    let inner = usize::from(area.width.saturating_sub(4)).max(1);
    let wrapped: usize = result
        .iter()
        .map(|l| l.width().max(1).div_ceil(inner))
        .sum();
    let result_height = (wrapped as u16 + 2).clamp(3, 10);
    let [title, pad_area, lists, result_area, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(pad.len() as u16 + 2),
        Constraint::Min(5),
        Constraint::Length(result_height),
        Constraint::Length(1),
    ])
    .areas(area);
    title_line(f, title, app);
    let pad_title = match &app.status {
        Some(s) if s.devices.len() > 1 => {
            format!("Pad (1 of {}: the one gamepadii acts on)", s.devices.len())
        }
        _ => "Pad".into(),
    };
    f.render_widget(
        Paragraph::new(pad).block(block(&pad_title, false).padding(Padding::horizontal(1))),
        pad_area,
    );
    let [modes, profiles] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(lists);
    modes_list(f, modes, app);
    profiles_list(f, profiles, app);
    let (result_title, result_style) = match (&app.busy, &app.outcome) {
        (Some(b), _) if !b.guide.is_empty() => ("Do this on the pad".to_string(), Color::Yellow),
        (Some(b), _) => (b.job.title(), Color::Yellow),
        (None, Some(o)) if o.error => (o.title.clone(), Color::Red),
        (None, Some(o)) => (o.title.clone(), Color::Green),
        (None, None) => ("Last result".into(), Color::DarkGray),
    };
    f.render_widget(
        Paragraph::new(result).wrap(Wrap { trim: false }).block(
            Block::bordered()
                .border_style(Style::new().fg(result_style))
                .padding(Padding::horizontal(1))
                .title(Span::raw(format!(" {result_title} ")).bold()),
        ),
        result_area,
    );
    status_line(f, status, app);
    overlay(f, area, app);
}

fn block(title: &str, focused: bool) -> Block<'static> {
    let (border, title) = if focused {
        (
            Style::new().fg(Color::Cyan),
            Span::raw(format!(" {title} ")).bold().cyan(),
        )
    } else {
        (
            Style::new().fg(Color::DarkGray),
            Span::raw(format!(" {title} ")).bold(),
        )
    };
    Block::bordered().border_style(border).title(title)
}

fn title_line(f: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![" gamepadii ".bold().cyan()];
    spans.push("GP2040-CE pads".dark_gray());
    let right = if app.busy.is_some() {
        "".into()
    } else {
        "watching USB ".dark_gray()
    };
    let [left, rest] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right.width() as u16)])
            .areas(area);
    f.render_widget(Paragraph::new(Line::from(spans)), left);
    f.render_widget(Paragraph::new(Line::from(right)), rest);
}

/// The pad panel: the mode line, then `status`'s rows.
fn pad_lines(app: &App) -> Vec<Line<'static>> {
    let Some(pad) = app.pad() else {
        return match &app.status_error {
            Some(e) => vec![
                Line::from(e.clone().yellow()),
                Line::from("watching USB: plug the pad in and it shows up here".dark_gray()),
            ],
            None => vec![Line::from("reading the pad…".dark_gray())],
        };
    };
    let d = &pad.device;
    let mut out = vec![Line::from(vec![
        output::mode_label(d).bold(),
        " ".into(),
        format!("({})", output::confidence(d.confidence)).dark_gray(),
    ])];
    let mut rows: Vec<(String, String)> = output::status_rows(pad)
        .into_iter()
        .filter(|(_, v)| v != "-")
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    if pad.control_channel.is_none() {
        if let Some(c) = &app.cache {
            let mut fw = c.firmware.version.clone().unwrap_or_else(|| "-".into());
            if let Some(l) = &c.firmware.board_label {
                fw.push_str(&format!(" ({l})"));
            }
            rows.push(("firmware".into(), format!("{fw}, cached {}", c.captured_at)));
            if let Some(b) = &c.firmware.board {
                rows.push(("board".into(), b.clone()));
            }
        }
        if pad.channel_note.is_none() && d.mode != Some(InputMode::Bootsel) {
            rows.push((
                "channel".into(),
                "none: stock GP2040-CE (no answer to HELLO)".into(),
            ));
        }
    }
    let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0) + 2;
    for (k, v) in rows {
        out.push(Line::from(vec![
            format!("{k:<width$}").dark_gray(),
            Span::raw(v),
        ]));
    }
    out
}

/// A list of `items` in `area`, its selection kept in view.
fn list(
    f: &mut Frame,
    area: Rect,
    title: &str,
    focused: bool,
    items: Vec<ListItem<'static>>,
    pos: &mut ListPos,
) {
    let highlight = if focused {
        Style::new().add_modifier(Modifier::REVERSED)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    };
    let widget = List::new(items)
        .block(block(title, focused))
        .highlight_style(highlight)
        .highlight_symbol(if focused { "▶ " } else { "  " });
    let mut state = ListState::default()
        .with_selected(Some(pos.sel))
        .with_offset(pos.offset);
    f.render_stateful_widget(widget, area, &mut state);
    pos.offset = state.offset();
}

fn modes_list(f: &mut Frame, area: Rect, app: &mut App) {
    let current = app.current_mode();
    let items: Vec<ListItem> = app
        .mode_items()
        .into_iter()
        .map(|m| {
            let mut spans = vec![Span::raw(format!("{:<20}", m.label()))];
            if current == Some(m) {
                spans.push(" current".green());
            } else if app.boots(m) == Some(false) {
                spans[0] = spans[0].clone().dark_gray();
                spans.push(" not in this build".dark_gray());
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let focused = app.focus == Pane::Modes && app.overlay.is_none();
    let mut pos = app.modes;
    list(f, area, "Input mode", focused, items, &mut pos);
    app.modes = pos;
}

fn profiles_list(f: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Pane::Profiles && app.overlay.is_none();
    if app.pad().is_none() || !app.can_switch_profiles() {
        let text = if app.pad().is_none() {
            vec![]
        } else {
            vec![
                Line::from("Profiles switch over the control channel of".dark_gray()),
                Line::from("the gamepadii firmware. On stock GP2040-CE,".dark_gray()),
                Line::from("use the pad's profile hotkeys.".dark_gray()),
            ]
        };
        f.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(block("Profile", focused).padding(Padding::horizontal(1))),
            area,
        );
        return;
    }
    let active = app.active_profile();
    let items: Vec<ListItem> = app
        .profile_items()
        .into_iter()
        .map(|p| {
            let mut spans = vec![Span::raw(format!("{:<20}", p.display()))];
            if active == Some(p.number) {
                spans.push(" running".green());
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let mut pos = app.profiles;
    list(f, area, "Profile", focused, items, &mut pos);
    app.profiles = pos;
}

/// What the result panel shows: the guide while the flow waits for the
/// user, else the last report.
fn result_lines(app: &App) -> Vec<Line<'static>> {
    if let Some(b) = &app.busy {
        if !b.guide.is_empty() {
            return b.guide.iter().map(|l| Line::raw(l.clone())).collect();
        }
        return vec![Line::from(
            b.line
                .clone()
                .unwrap_or_else(|| "starting…".into())
                .dark_gray(),
        )];
    }
    match &app.outcome {
        Some(o) => o.text.lines().map(|l| Line::raw(l.to_string())).collect(),
        None => vec![Line::from("nothing done yet".dark_gray())],
    }
}

fn status_line(f: &mut Frame, area: Rect, app: &App) {
    let mut left: Vec<Span> = Vec::new();
    let pending = app.pending_keys();
    if !pending.is_empty() {
        left.push(pending.cyan());
        left.push("  ".into());
    }
    if let Some(b) = &app.busy {
        let spinner = FRAMES[app.frame % FRAMES.len()];
        left.push(format!("{spinner} {}", b.job.title()).yellow().bold());
        if let Some(l) = &b.line {
            left.push(format!(": {l}").yellow());
        }
    } else if let Some(n) = &app.notice {
        left.push(n.clone().yellow());
    }
    let hints = if app.busy.is_some() {
        "q quit".dark_gray()
    } else {
        HINTS.dark_gray()
    };
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(hints.width() as u16)])
        .areas(area);
    f.render_widget(Paragraph::new(Line::from(left)), l);
    f.render_widget(Paragraph::new(Line::from(hints)).right_aligned(), r);
}

/// `j  ↓` from a binding's keys.
fn keys_text(keys: &[(&str, Action)]) -> String {
    keys.iter().map(|(k, _)| *k).collect::<Vec<_>>().join("  ")
}

const KEYS: usize = 18;

fn help_lines() -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut section = "";
    for b in BINDINGS {
        if b.section != section {
            if !section.is_empty() {
                out.push(Line::default());
            }
            section = b.section;
            out.push(Line::from(section.to_owned().bold().cyan()));
        }
        out.push(Line::from(vec![
            format!("  {:<KEYS$}", keys_text(b.keys)).bold(),
            Span::raw(b.help),
        ]));
    }
    out.push(Line::default());
    out.push(Line::from(
        "Every action is also a command: gamepadii mode, profile switch, webconfig, backup."
            .dark_gray(),
    ));
    out
}

fn confirm_lines(lines: &[String]) -> Vec<Line<'static>> {
    let mut out: Vec<Line> = lines.iter().map(|l| Line::raw(l.clone())).collect();
    out.push(Line::default());
    out.push(Line::from(
        CONFIRM_KEYS
            .iter()
            .flat_map(|(k, h)| [format!("{k} ").bold(), format!("{h}    ").into()])
            .collect::<Vec<_>>(),
    ));
    out
}

/// The help or the prompt, centred over `area`.
fn overlay(f: &mut Frame, area: Rect, app: &App) {
    let (title, lines, color) = match &app.overlay {
        Some(Overlay::Help) => ("Keys".to_string(), help_lines(), Color::Cyan),
        Some(Overlay::Confirm(c)) => (c.title.clone(), confirm_lines(&c.lines), Color::Yellow),
        None => return,
    };
    let content_width = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let width = (content_width + 4).clamp(40, 100).min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let [_, mid, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Fill(1),
    ])
    .areas(area);
    let [_, rect, _] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width),
        Constraint::Fill(1),
    ])
    .areas(mid);
    let block = Block::bordered()
        .border_style(Style::new().fg(color))
        .title(Span::raw(format!(" {title} ")).bold().fg(color));
    let inner = block.inner(rect).inner(Margin::new(1, 0));
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}
