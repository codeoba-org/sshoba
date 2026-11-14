//! Small inline list picker for ambiguous CLI queries (not the full TUI).

use crate::core::error::{Error, Result};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::crossterm::terminal;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};
use ratatui::{Terminal, TerminalOptions, Viewport};

/// Let the user choose one of `items` (label, detail). Returns its index, or None if
/// cancelled. Draws on stderr so stdout stays clean.
pub fn pick(title: &str, items: &[(String, String)]) -> Result<Option<usize>> {
    let height = (items.len() as u16).min(10) + 2;
    let io = |e: std::io::Error| Error::io("picker", e);
    terminal::enable_raw_mode().map_err(io)?;
    let result = (|| -> std::io::Result<Option<usize>> {
        let mut term = Terminal::with_options(
            CrosstermBackend::new(std::io::stderr()),
            TerminalOptions {
                viewport: Viewport::Inline(height),
            },
        )?;
        let mut state = ListState::default().with_selected(Some(0));
        let chosen = loop {
            term.draw(|f| {
                let [head, body, foot] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Min(1),
                    Constraint::Length(1),
                ])
                .areas(f.area());
                f.render_widget(Line::from(title.bold()), head);
                let list = List::new(items.iter().map(|(name, detail)| {
                    ListItem::new(Line::from(vec![
                        Span::raw(format!("{name}  ")),
                        Span::styled(detail.clone(), Style::new().dim()),
                    ]))
                }))
                .highlight_symbol("❯ ")
                .highlight_style(Style::new().add_modifier(Modifier::BOLD));
                f.render_stateful_widget(list, body, &mut state);
                f.render_widget(Line::from("↑/↓ move  ⏎ select  esc cancel".dim()), foot);
            })?;
            if let Event::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                match k.code {
                    KeyCode::Down | KeyCode::Char('j') => state.select_next(),
                    KeyCode::Up | KeyCode::Char('k') => state.select_previous(),
                    KeyCode::Enter => break state.selected(),
                    KeyCode::Esc | KeyCode::Char('q') => break None,
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        break None
                    }
                    _ => {}
                }
                if let Some(i) = state.selected() {
                    state.select(Some(i.min(items.len() - 1)));
                }
            }
        };
        term.clear()?;
        Ok(chosen)
    })();
    let _ = terminal::disable_raw_mode();
    result.map_err(io)
}
