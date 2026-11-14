//! Small dialogs: confirm, text prompt and error popup.

use crate::tui::input::TextInput;
use crate::tui::views::{centered, input_line};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use ratatui::Frame;

pub fn draw_confirm(f: &mut Frame, area: Rect, message: &str) {
    let lines = message.lines().count() as u16;
    let area = centered(area, 60, lines + 4);
    f.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Yellow))
        .title(" Confirm ".bold())
        .title_bottom(Line::from(" y yes · n no ").centered());
    let text: Vec<Line> = message.lines().map(Line::from).collect();
    f.render_widget(
        Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: true })
            .centered(),
        area,
    );
}

pub fn draw_prompt(f: &mut Frame, area: Rect, title: &str, input: &TextInput, error: Option<&str>) {
    let area = centered(area, 50, 5);
    f.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(format!(" {title} ").bold())
        .title_bottom(Line::from(" ⏎ ok · Esc cancel ").centered());
    let inner = block.inner(area);
    f.render_widget(block, area);
    input_line(
        f,
        Rect::new(inner.x + 1, inner.y, inner.width - 2, 1),
        input,
        true,
        "",
    );
    if let Some(e) = error {
        f.render_widget(
            Paragraph::new(e.to_string().fg(Color::Red)),
            Rect::new(inner.x + 1, inner.y + 2, inner.width - 2, 1),
        );
    }
}

pub fn draw_error(f: &mut Frame, area: Rect, title: &str, text: &str) {
    let width = 72u16;
    let lines: u16 = text
        .lines()
        .map(|l| (l.chars().count() as u16 / (width - 4)) + 1)
        .sum();
    let area = centered(area, width, lines + 4);
    f.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Red))
        .title(format!(" {title} ").bold())
        .title_bottom(Line::from(" Esc close ").centered());
    f.render_widget(
        Paragraph::new(text.to_string())
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}
