//! Two-pane local/remote file browser (section 6.3.5).

use crate::core::remote_fs::{human_size, Entry, Kind};
use crate::core::transfer::Progress;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

#[derive(Debug, Clone, Default)]
pub struct Pane {
    /// Absolute path; empty for the remote pane until the first listing arrives.
    pub path: String,
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub marked: BTreeSet<String>,
    pub loading: bool,
    pub error: Option<String>,
}

impl Pane {
    /// Entries shown with the current hidden-file setting.
    pub fn visible(&self, show_hidden: bool) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|e| show_hidden || !e.hidden())
            .collect()
    }

    pub fn current(&self, show_hidden: bool) -> Option<&Entry> {
        self.visible(show_hidden).get(self.selected).copied()
    }

    pub fn set_entries(&mut self, path: String, entries: Vec<Entry>, keep: Option<&str>) {
        self.path = path;
        self.entries = entries;
        self.marked.clear();
        self.loading = false;
        self.error = None;
        self.selected = 0;
        if let Some(k) = keep {
            if let Some(i) = self.entries.iter().position(|e| e.name == k) {
                self.selected = i;
            }
        }
    }

    pub fn clamp(&mut self, show_hidden: bool) {
        let n = self.visible(show_hidden).len();
        self.selected = self.selected.min(n.saturating_sub(1));
    }

    /// Paths of the marked entries, or of the highlighted one if none are marked.
    pub fn selection(&self, show_hidden: bool) -> Vec<String> {
        let names: Vec<String> = if self.marked.is_empty() {
            self.current(show_hidden)
                .map(|e| vec![e.name.clone()])
                .unwrap_or_default()
        } else {
            self.marked.iter().cloned().collect()
        };
        names.into_iter().map(|n| join(&self.path, &n)).collect()
    }
}

pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

pub fn parent(dir: &str) -> String {
    match dir.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) | None => "/".into(),
        Some((p, _)) => p.to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct FileBrowser {
    pub server: String,
    pub local: Pane,
    pub remote: Pane,
    pub active: Side,
    pub show_hidden: bool,
    /// Copy in progress: (destination side, latest progress).
    pub copying: Option<(Side, Option<Progress>)>,
}

impl FileBrowser {
    pub fn new(server: &str, local_dir: String) -> Self {
        FileBrowser {
            server: server.to_string(),
            local: Pane {
                path: local_dir,
                ..Default::default()
            },
            remote: Pane {
                loading: true,
                ..Default::default()
            },
            active: Side::Local,
            show_hidden: false,
            copying: None,
        }
    }

    pub fn pane(&self, side: Side) -> &Pane {
        match side {
            Side::Local => &self.local,
            Side::Remote => &self.remote,
        }
    }

    pub fn pane_mut(&mut self, side: Side) -> &mut Pane {
        match side {
            Side::Local => &mut self.local,
            Side::Remote => &mut self.remote,
        }
    }

    pub fn other(side: Side) -> Side {
        match side {
            Side::Local => Side::Remote,
            Side::Remote => Side::Local,
        }
    }

    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let [body, status] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);
        let [l, r] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .areas(body);
        self.draw_pane(f, l, Side::Local, "local");
        self.draw_pane(f, r, Side::Remote, &self.server);
        let line = match &self.copying {
            Some((to, p)) => {
                let dir = if *to == Side::Remote { "→" } else { "←" };
                match p {
                    Some(p) => format!(" copying {dir} {}%  {}  ETA {}", p.percent, p.rate, p.eta),
                    None => format!(" copying {dir} …"),
                }
            }
            None => {
                let n = self.pane(self.active).marked.len();
                if n > 0 {
                    format!(" {n} marked · c copies to the other pane")
                } else {
                    String::new()
                }
            }
        };
        f.render_widget(Paragraph::new(line.fg(Color::Yellow)), status);
    }

    fn draw_pane(&self, f: &mut Frame, area: Rect, side: Side, label: &str) {
        let pane = self.pane(side);
        let active = side == self.active;
        let border = if active {
            Style::new().fg(Color::Cyan)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        let title = format!(
            " {label}: {} ",
            if pane.path.is_empty() {
                "…"
            } else {
                &pane.path
            }
        );
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border)
            .title(Span::styled(title, Style::new().bold()));
        if let Some(e) = &pane.error {
            f.render_widget(
                Paragraph::new(e.clone().fg(Color::Red))
                    .block(block)
                    .wrap(ratatui::widgets::Wrap { trim: true }),
                area,
            );
            return;
        }
        if pane.loading && pane.entries.is_empty() {
            f.render_widget(Paragraph::new("loading…".dim()).block(block), area);
            return;
        }
        let inner_w = area.width.saturating_sub(4) as usize;
        let items: Vec<ListItem> = pane
            .visible(self.show_hidden)
            .into_iter()
            .map(|e| {
                let marked = pane.marked.contains(&e.name);
                let (suffix, style) = match e.kind {
                    Kind::Dir => ("/", Style::new().fg(Color::Blue).bold()),
                    Kind::Link => ("@", Style::new().fg(Color::Cyan)),
                    _ => ("", Style::new()),
                };
                let size = if e.is_dir() {
                    String::new()
                } else {
                    human_size(e.size)
                };
                let time = e
                    .mtime
                    .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                    .map(|t| {
                        t.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                            .to_string()
                    })
                    .unwrap_or_default();
                let meta = format!("{size:>7}  {time:>16}");
                let name = format!("{}{}{suffix}", if marked { "✓ " } else { "  " }, e.name);
                let pad = inner_w.saturating_sub(name.chars().count() + meta.chars().count());
                let mut style = style;
                if marked {
                    style = style.fg(Color::Yellow).add_modifier(Modifier::BOLD);
                }
                ListItem::new(Line::from(vec![
                    Span::styled(name, style),
                    Span::raw(" ".repeat(pad.max(1))),
                    Span::styled(meta, Style::new().dim()),
                ]))
            })
            .collect();
        let list = List::new(items).block(block).highlight_style(if active {
            Style::new().bg(Color::DarkGray)
        } else {
            Style::new()
        });
        let mut state = ListState::default().with_selected(Some(pane.selected));
        f.render_stateful_widget(list, area, &mut state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(join("/a", "b"), "/a/b");
        assert_eq!(join("/", "b"), "/b");
        assert_eq!(parent("/a/b"), "/a");
        assert_eq!(parent("/a"), "/");
        assert_eq!(parent("/"), "/");
    }
}
