//! Right pane: details of the selected server.

use crate::cli::output::{ago, chain_display};
use crate::core::sshcmd::human_command;
use crate::tui::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};
use ratatui::Frame;

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().border_type(BorderType::Rounded);
    let Some(s) = app.selected_server() else {
        f.render_widget(block, area);
        return;
    };
    let cfg = app.config();
    let dim = Style::new().dim();
    let mut lines = vec![Line::from(vec![
        Span::styled(s.name.clone(), Style::new().bold()),
        if s.favorite {
            Span::styled("  ★", Style::new().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
    ])];
    lines.push(Line::from(s.destination()));
    if !s.jump.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("via ", dim),
            Span::raw(chain_display(cfg, s)),
        ]));
    }
    if let Some(k) = &s.key {
        lines.push(Line::from(vec![
            Span::styled("key: ", dim),
            Span::raw(k.display().to_string()),
        ]));
    }
    if let Some(g) = &s.group {
        lines.push(Line::from(vec![
            Span::styled("group: ", dim),
            Span::raw(g.clone()),
        ]));
    }
    if !s.tags.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("tags: ", dim),
            Span::styled(s.tags.join(", "), Style::new().fg(Color::Cyan)),
        ]));
    }
    if let Some(n) = &s.note {
        lines.push(Line::from(vec![
            Span::styled("note: ", dim),
            Span::raw(n.clone()),
        ]));
    }
    lines.extend(extra_lines(app, s));
    if let Some(t) = app.state.last_connected(&s.name) {
        lines.push(Line::from(vec![
            Span::styled("last: ", dim),
            Span::raw(ago(t)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("$ ", Style::new().fg(Color::Green)),
        Span::raw(human_command(s, cfg)),
    ]));
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Additional detail lines (forwards, ping) contributed by later features.
fn extra_lines(app: &App, s: &crate::core::model::Server) -> Vec<Line<'static>> {
    use crate::core::state::PingStatus;
    let mut out = Vec::new();
    if let Some(r) = app.ping.get(&s.name) {
        let (word, color) = match r.status {
            PingStatus::Up => ("up", Color::Green),
            PingStatus::OpenNoSsh => ("open, no ssh banner", Color::Yellow),
            PingStatus::Down => ("down", Color::Red),
            PingStatus::Unknown => ("unknown", Color::DarkGray),
        };
        let mut text = word.to_string();
        if let Some(l) = r.latency_ms {
            text.push_str(&format!(" {l}ms"));
        }
        if let Some(reason) = &r.reason {
            text.push_str(&format!(" ({reason})"));
        }
        if let Some(v) = &r.via {
            text.push_str(&format!(" via {v}"));
        }
        let mut line = vec![
            Span::styled("ping: ", Style::new().dim()),
            Span::styled(text, Style::new().fg(color)),
        ];
        if !app.fresh.contains(&s.name) {
            line.push(Span::styled(
                format!(" · {}", ago(r.checked_at)),
                Style::new().dim(),
            ));
        }
        out.push(Line::from(line));
    }
    if !s.forward.is_empty() {
        out.push(Line::from("forwards:".dim()));
        for fw in &s.forward {
            let id = crate::core::tunnel::tunnel_id(&s.name, &fw.name);
            let dot = if app.running_tunnel(&id).is_some() {
                Span::styled(" ●", Style::new().fg(Color::Green))
            } else {
                Span::raw("")
            };
            out.push(Line::from(vec![
                Span::raw(format!("  {}  {}", fw.name, fw.summary())),
                dot,
            ]));
        }
    }
    out
}
