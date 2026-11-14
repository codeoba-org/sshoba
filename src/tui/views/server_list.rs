//! Left pane: servers grouped by group.

use crate::tui::app::{App, Row};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().border_type(BorderType::Rounded);
    if app.config().server.is_empty() {
        let text = vec![
            Line::from(""),
            Line::from("No servers yet".bold()),
            Line::from(""),
            Line::from("press a to add one, or I to import ~/.ssh/config".dim()),
        ];
        f.render_widget(Paragraph::new(text).block(block).centered(), area);
        return;
    }
    if app.rows.is_empty() {
        f.render_widget(
            Paragraph::new("no matches".dim()).block(block).centered(),
            area,
        );
        return;
    }
    let name_w = app
        .config()
        .server
        .iter()
        .map(|s| s.name.chars().count())
        .max()
        .unwrap_or(4)
        .min(28);
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| match row {
            Row::Group {
                name,
                count,
                collapsed,
            } => {
                let arrow = if *collapsed { "▸" } else { "▾" };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{arrow} {name}"),
                        Style::new().bold().fg(Color::Magenta),
                    ),
                    Span::styled(format!("  {count}"), Style::new().dim()),
                ]))
            }
            Row::Server(n) => {
                let Some(s) = app.config().get(n) else {
                    return ListItem::new("");
                };
                let mut spans = vec![Span::raw("  ")];
                spans.extend(crate::tui::views::server_list::status_spans(app, &s.name));
                let name: String = s.name.chars().take(name_w).collect();
                spans.push(Span::styled(
                    format!("{name:<name_w$}"),
                    Style::new().add_modifier(Modifier::BOLD),
                ));
                spans.push(if s.favorite {
                    Span::styled(" ★ ", Style::new().fg(Color::Yellow))
                } else {
                    Span::raw("   ")
                });
                spans.push(Span::styled(s.tags.join(" "), Style::new().fg(Color::Cyan)));
                if let Some(hop) = s.jump.last() {
                    spans.push(Span::styled(format!("  via {hop}"), Style::new().dim()));
                }
                ListItem::new(Line::from(spans))
            }
        })
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_symbol("❯ ")
        .highlight_style(Style::new().bg(Color::DarkGray));
    let mut state = ListState::default().with_selected(Some(app.selected));
    f.render_stateful_widget(list, area, &mut state);
}

/// Reachability dot and latency for a server: green ● up, red ○ down, grey · unknown,
/// yellow ◌ checking. Cached results from a previous run are dimmed.
pub fn status_spans(app: &App, name: &str) -> Vec<Span<'static>> {
    use crate::core::state::PingStatus;
    let (dot, color, text) = if app.checking.contains(name) && !app.ping.contains_key(name) {
        ("◌", Color::Yellow, String::new())
    } else {
        match app.ping.get(name) {
            None => ("·", Color::DarkGray, String::new()),
            Some(r) => match r.status {
                PingStatus::Up => (
                    "●",
                    Color::Green,
                    r.latency_ms.map(|l| format!("{l}ms")).unwrap_or_default(),
                ),
                PingStatus::OpenNoSsh => ("●", Color::Yellow, "open".into()),
                PingStatus::Down => ("○", Color::Red, "down".into()),
                PingStatus::Unknown => ("·", Color::DarkGray, "?".into()),
            },
        }
    };
    let (dot, color) = if app.checking.contains(name) {
        ("◌", Color::Yellow)
    } else {
        (dot, color)
    };
    let mut style = Style::new().fg(color);
    let mut text_style = Style::new();
    if !app.fresh.contains(name) {
        style = style.add_modifier(Modifier::DIM);
        text_style = text_style.add_modifier(Modifier::DIM);
    }
    vec![
        Span::styled(format!("{dot} "), style),
        Span::styled(format!("{text:>5} "), text_style),
    ]
}
