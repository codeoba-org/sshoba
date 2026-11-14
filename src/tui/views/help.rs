//! Help overlay, generated from the keybinding table.

use crate::tui::keys;
use crate::tui::views::centered;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use ratatui::Frame;

pub fn draw(f: &mut Frame, area: Rect, table: &[keys::Binding]) {
    let rows: Vec<Line> = table
        .iter()
        .map(|b| {
            Line::from(vec![
                Span::styled(
                    format!("{:>12}  ", b.label),
                    Style::new().fg(Color::Cyan).bold(),
                ),
                Span::raw(b.help),
            ])
        })
        .collect();
    let area = centered(area, 60, rows.len() as u16 + 2);
    f.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Keys ".bold())
        .title_bottom(Line::from(" Esc close ").centered());
    f.render_widget(Paragraph::new(rows).block(block), area);
}
