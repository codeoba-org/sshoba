//! Rendering. `draw` is a pure function of `App`.

pub mod confirm;
pub mod details;
pub mod file_browser;
pub mod help;
pub mod server_form;
pub mod server_list;
pub mod transfer;
pub mod tunnels;

use crate::tui::app::{App, Modal, Screen};
use crate::tui::input::TextInput;
use crate::tui::keys;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Width below which the details pane hides unless toggled on.
pub const DETAILS_MIN_WIDTH: u16 = 90;

pub fn draw(f: &mut Frame, app: &App) {
    let [header, search, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(f.area());
    draw_header(f, header, app);
    draw_search(f, search, app);
    let show_details = app.details.unwrap_or(true)
        && (app.details == Some(true) || body.width >= DETAILS_MIN_WIDTH);
    if let Screen::Files(b) = &app.screen {
        b.draw(f, body);
    } else if let Screen::Tunnels { .. } = app.screen {
        tunnels::draw(f, body, app);
    } else if show_details {
        let [list, details] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(body);
        server_list::draw(f, list, app);
        details::draw(f, details, app);
    } else {
        server_list::draw(f, body, app);
    }
    draw_footer(f, footer, app);
    match &app.modal {
        None => {}
        Some(Modal::Help) => help::draw(f, f.area(), key_table(app)),
        Some(Modal::Form(form)) => form.draw(f, f.area(), app.config()),
        Some(Modal::Confirm { message, .. }) => confirm::draw_confirm(f, f.area(), message),
        Some(Modal::Prompt {
            title,
            input,
            error,
            ..
        }) => confirm::draw_prompt(f, f.area(), title, input, error.as_deref()),
        Some(Modal::Error { title, text }) => confirm::draw_error(f, f.area(), title, text),
        Some(Modal::Transfer(t)) => t.draw(f, f.area()),
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let n = app.config().server.len();
    let left = format!(
        " shhoba ─ {n} server{} ─ {} ",
        if n == 1 { "" } else { "s" },
        app.env.label()
    );
    let right = header_right(app);
    let pad = (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
    let line = Line::from(vec![
        Span::styled(left, Style::new().add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(pad)),
        Span::raw(right),
    ]);
    f.render_widget(
        Paragraph::new(line).style(Style::new().bg(Color::Blue).fg(Color::White)),
        area,
    );
}

fn header_right(app: &App) -> String {
    match app.state.tunnels.len() {
        0 => String::new(),
        1 => "1 tunnel up ".into(),
        n => format!("{n} tunnels up "),
    }
}

/// The keybinding table for the current screen.
pub fn key_table(app: &App) -> &'static [keys::Binding] {
    match app.screen {
        Screen::Main => keys::MAIN,
        Screen::Tunnels { .. } => keys::TUNNELS,
        Screen::Files(_) => keys::FILES,
    }
}

fn draw_search(f: &mut Frame, area: Rect, app: &App) {
    if !matches!(app.screen, Screen::Main) {
        return;
    }
    if app.searching || !app.search.is_empty() {
        let label = Span::styled(" / search: ", Style::new().fg(Color::Cyan));
        let [l, r] = Layout::horizontal([Constraint::Length(11), Constraint::Min(1)]).areas(area);
        f.render_widget(Paragraph::new(label), l);
        input_line(f, r, &app.search, app.searching, "");
    } else {
        f.render_widget(Paragraph::new(" / to search".dim()), area);
    }
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let line = match &app.status {
        Some(s) if s.error => Line::from(format!(" {}", s.text).fg(Color::Red)),
        Some(s) => Line::from(format!(" {}", s.text).fg(Color::Green)),
        None => {
            let mut spans = vec![Span::raw(" ")];
            for (k, h) in keys::hints(key_table(app)) {
                spans.push(Span::styled(k, Style::new().fg(Color::Cyan).bold()));
                spans.push(Span::raw(format!(" {h}  ")));
            }
            Line::from(spans)
        }
    };
    f.render_widget(Paragraph::new(line), area);
}

/// A rectangle of at most `w`×`h` centered in `area`.
pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// Render a text input on one line, scrolling to keep the cursor visible.
pub fn input_line(f: &mut Frame, area: Rect, input: &TextInput, focused: bool, placeholder: &str) {
    let value = input.value();
    if value.is_empty() && !focused {
        f.render_widget(Paragraph::new(placeholder.to_string().dim()), area);
        return;
    }
    let width = area.width.max(1) as usize;
    let cursor = input.cursor();
    let start = cursor.saturating_sub(width.saturating_sub(1));
    let visible: String = value.chars().skip(start).take(width).collect();
    let style = if focused {
        Style::new().add_modifier(Modifier::UNDERLINED)
    } else {
        Style::new()
    };
    if value.is_empty() {
        f.render_widget(Paragraph::new(placeholder.to_string().dim()), area);
    } else {
        f.render_widget(Paragraph::new(Span::styled(visible, style)), area);
    }
    if focused {
        f.set_cursor_position((area.x + (cursor - start) as u16, area.y));
    }
}
