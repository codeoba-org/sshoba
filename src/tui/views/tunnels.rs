//! Tunnels screen: saved forwards with live tunnel status (section 6.3.3).

use crate::core::tunnel::uptime;
use crate::tui::app::{App, Screen};
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let Screen::Tunnels { server, selected } = &app.screen else {
        return;
    };
    let title = match server {
        Some(s) => format!(" Tunnels · {s} "),
        None => " All tunnels ".to_string(),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title.bold());
    let rows = app.tunnel_rows();
    if rows.is_empty() {
        f.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from("No saved forwards".bold()),
                Line::from("press a to add one, e.g. pg -L 5432:localhost:5432".dim()),
            ])
            .block(block)
            .centered(),
            area,
        );
        return;
    }
    let table_rows: Vec<Row> = rows
        .iter()
        .map(|r| {
            let running = app.running_tunnel(&r.id);
            let (status, color) = if app.tunnel_busy.contains(&r.id) {
                ("◌ …", Color::Yellow)
            } else if running.is_some() {
                ("● up", Color::Green)
            } else if app.tunnel_failed.contains_key(&r.id) {
                ("✗ failed", Color::Red)
            } else {
                ("○ down", Color::DarkGray)
            };
            let name = if server.is_some() {
                r.forward.name.clone()
            } else {
                r.id.clone()
            };
            let auto = if r.forward.auto { " (auto)" } else { "" };
            Row::new(vec![
                Cell::from(format!("{name}{auto}")),
                Cell::from(format!("{:?}", r.forward.kind).to_lowercase()),
                Cell::from(format!("{} {}", r.forward.kind.flag(), r.forward.spec())),
                Cell::from(status).style(Style::new().fg(color)),
                Cell::from(running.map(|t| t.pid.to_string()).unwrap_or_default()),
                Cell::from(running.map(uptime).unwrap_or_default()),
            ])
        })
        .collect();
    let table = Table::new(
        table_rows,
        [
            Constraint::Min(14),
            Constraint::Length(7),
            Constraint::Min(24),
            Constraint::Length(9),
            Constraint::Length(7),
            Constraint::Length(8),
        ],
    )
    .header(
        Row::new(["NAME", "KIND", "SPEC", "STATUS", "PID", "UPTIME"])
            .style(Style::new().bold().fg(Color::Cyan)),
    )
    .block(block)
    .row_highlight_style(Style::new().bg(Color::DarkGray))
    .highlight_symbol("❯ ");
    let mut state = TableState::default().with_selected(Some(*selected));
    f.render_stateful_widget(table, area, &mut state);
}
