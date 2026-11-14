//! Add/edit server form (section 6.3.1).

use crate::core::config::valid_name;
use crate::core::model::{parse_destination, Config, Server};
use crate::tui::input::TextInput;
use crate::tui::views::{centered, input_line};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const FIELDS: [&str; 9] = [
    "name", "host", "user", "port", "key", "jump", "group", "tags", "note",
];
const NAME: usize = 0;
const HOST: usize = 1;
const USER: usize = 2;
const PORT: usize = 3;
const KEY: usize = 4;
const JUMP: usize = 5;
const GROUP: usize = 6;
const TAGS: usize = 7;
const NOTE: usize = 8;

const PLACEHOLDERS: [&str; 9] = [
    "unique name: letters, digits, . _ -",
    "hostname or IP (paste user@host:port to split)",
    "optional",
    "22",
    "~/.ssh/id_ed25519 (Tab completes)",
    "jump hosts in order, comma-separated",
    "optional",
    "comma-separated",
    "optional",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormOutcome {
    None,
    Cancel,
    Submit,
}

#[derive(Debug, Clone)]
pub struct ServerForm {
    /// The server being edited; None when adding.
    pub original: Option<Server>,
    pub fields: Vec<TextInput>,
    initial: Vec<String>,
    pub focus: usize,
    pub errors: BTreeMap<usize, String>,
    pub general_error: Option<String>,
    pub confirm_discard: bool,
}

fn values(s: Option<&Server>) -> Vec<String> {
    let Some(s) = s else {
        return vec![String::new(); FIELDS.len()];
    };
    vec![
        s.name.clone(),
        s.host.clone(),
        s.user.clone().unwrap_or_default(),
        s.port.map(|p| p.to_string()).unwrap_or_default(),
        s.key
            .as_ref()
            .map(|k| k.display().to_string())
            .unwrap_or_default(),
        s.jump.join(", "),
        s.group.clone().unwrap_or_default(),
        s.tags.join(", "),
        s.note.clone().unwrap_or_default(),
    ]
}

fn opt(s: String) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect()
}

impl ServerForm {
    pub fn new(original: Option<&Server>) -> Self {
        let initial = values(original);
        ServerForm {
            original: original.cloned(),
            fields: initial.iter().map(|v| TextInput::new(v)).collect(),
            initial,
            focus: 0,
            errors: BTreeMap::new(),
            general_error: None,
            confirm_discard: false,
        }
    }

    pub fn title(&self) -> String {
        match &self.original {
            Some(s) => format!(" Edit {} ", s.name),
            None => " Add server ".into(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.fields
            .iter()
            .zip(&self.initial)
            .any(|(f, i)| f.value() != *i)
    }

    fn move_focus(&mut self, forward: bool) {
        if self.focus == HOST {
            self.split_host();
        }
        let n = FIELDS.len();
        self.focus = if forward {
            (self.focus + 1) % n
        } else {
            (self.focus + n - 1) % n
        };
    }

    /// `user@host:port` typed or pasted into host is split into the user and port fields.
    pub fn split_host(&mut self) {
        let v = self.fields[HOST].value();
        let v = v.trim();
        if !(v.contains('@') || v.matches(':').count() == 1 || v.starts_with('[')) {
            return;
        }
        if let Ok((user, host, port)) = parse_destination(v) {
            self.fields[HOST].set(&host);
            if let Some(u) = user {
                self.fields[USER].set(&u);
            }
            if let Some(p) = port {
                self.fields[PORT].set(&p.to_string());
            }
        }
    }

    /// Complete the key path in place. Returns true if anything changed.
    fn complete_key(&mut self) -> bool {
        let cur = self.fields[KEY].value();
        if cur.is_empty() {
            return false;
        }
        match complete_path(&cur) {
            Some(done) if done != cur => {
                self.fields[KEY].set(&done);
                true
            }
            _ => false,
        }
    }

    pub fn paste(&mut self, s: &str) {
        self.fields[self.focus].paste(s.trim());
        if self.focus == HOST {
            self.split_host();
        }
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> FormOutcome {
        if self.confirm_discard {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => return FormOutcome::Cancel,
                _ => {
                    self.confirm_discard = false;
                    return FormOutcome::None;
                }
            }
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('s') if ctrl => return FormOutcome::Submit,
            KeyCode::Esc => {
                if self.dirty() {
                    self.confirm_discard = true;
                } else {
                    return FormOutcome::Cancel;
                }
            }
            KeyCode::Tab if self.focus == KEY && self.complete_key() => {}
            KeyCode::Tab | KeyCode::Down => self.move_focus(true),
            KeyCode::BackTab | KeyCode::Up => self.move_focus(false),
            KeyCode::Enter => {
                if self.focus + 1 == FIELDS.len() {
                    return FormOutcome::Submit;
                }
                self.move_focus(true);
            }
            _ => {
                self.fields[self.focus].handle(key);
            }
        }
        FormOutcome::None
    }

    /// Validate fields and build the server. On failure, errors are set per field.
    pub fn build(&mut self, cfg: &Config) -> Option<Server> {
        self.split_host();
        self.errors.clear();
        self.general_error = None;
        let v: Vec<String> = self.fields.iter().map(TextInput::value).collect();
        let name = v[NAME].trim().to_string();
        let original_name = self.original.as_ref().map(|s| s.name.as_str());
        if !valid_name(&name) {
            self.errors
                .insert(NAME, "1-64 letters, digits, '.', '_' or '-'".into());
        } else if Some(name.as_str()) != original_name && cfg.get(&name).is_some() {
            self.errors
                .insert(NAME, format!("\"{name}\" already exists"));
        }
        if v[HOST].trim().is_empty() {
            self.errors.insert(HOST, "required".into());
        }
        let port = match opt(v[PORT].clone()) {
            None => None,
            Some(p) => match p.parse::<u16>() {
                Ok(n) if n > 0 => Some(n),
                _ => {
                    self.errors.insert(PORT, "must be 1-65535".into());
                    None
                }
            },
        };
        let jump = list(&v[JUMP]);
        for j in &jump {
            if Some(j.as_str()) == original_name || *j == name {
                self.errors
                    .insert(JUMP, "a server cannot jump through itself".into());
            } else if cfg.get(j).is_none() {
                self.errors.insert(JUMP, format!("no server named \"{j}\""));
            }
        }
        if !self.errors.is_empty() {
            self.focus = *self.errors.keys().next().unwrap_or(&0);
            return None;
        }
        let mut s = self.original.clone().unwrap_or_default();
        s.name = name;
        s.host = v[HOST].trim().to_string();
        s.user = opt(v[USER].clone());
        s.port = port;
        s.key = opt(v[KEY].clone()).map(PathBuf::from);
        s.jump = jump;
        s.group = opt(v[GROUP].clone());
        s.tags = list(&v[TAGS]);
        s.note = opt(v[NOTE].clone());
        Some(s)
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, cfg: &Config) {
        let height = (FIELDS.len() as u16) * 2 + 6;
        let area = centered(area, 70, height);
        f.render_widget(Clear, area);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(self.title().bold())
            .title_bottom(Line::from(" Tab next · ⇧Tab prev · Ctrl+S save · Esc cancel ").dim());
        let inner = block.inner(area);
        f.render_widget(block, area);
        let label_w = 7u16;
        let mut y = inner.y;
        for (i, name) in FIELDS.iter().enumerate() {
            if y + 1 >= inner.bottom() {
                break;
            }
            let focused = i == self.focus;
            let label_style = if focused {
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::new().dim()
            };
            f.render_widget(
                Paragraph::new(Span::styled(format!("{name:>6} "), label_style)),
                Rect::new(inner.x, y, label_w, 1),
            );
            let field_area = Rect::new(inner.x + label_w, y, inner.width - label_w, 1);
            input_line(f, field_area, &self.fields[i], focused, PLACEHOLDERS[i]);
            let below = if let Some(e) = self.errors.get(&i) {
                Span::styled(format!("⚠ {e}"), Style::new().fg(Color::Red))
            } else if i == JUMP && !self.fields[JUMP].is_empty() {
                let mut chain = list(&self.fields[JUMP].value());
                chain.push(self.fields[NAME].value());
                let known: Vec<_> = cfg.names().collect();
                let hint = if chain[..chain.len() - 1]
                    .iter()
                    .all(|j| known.contains(&j.as_str()))
                {
                    format!("route: {}", chain.join(" › "))
                } else {
                    format!("known: {}", known.join(", "))
                };
                Span::styled(hint, Style::new().dim())
            } else {
                Span::raw("")
            };
            f.render_widget(
                Paragraph::new(below),
                Rect::new(inner.x + label_w, y + 1, inner.width - label_w, 1),
            );
            y += 2;
        }
        let msg = if self.confirm_discard {
            Some(Line::from(
                "Discard changes? y / n".fg(Color::Yellow).bold(),
            ))
        } else {
            self.general_error
                .as_ref()
                .map(|e| Line::from(e.as_str().fg(Color::Red)))
        };
        if let Some(m) = msg {
            let r = Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 2);
            f.render_widget(Paragraph::new(m).wrap(Wrap { trim: true }), r);
        }
    }
}

/// Complete a filesystem path prefix: unique match → full name (dirs get `/`),
/// several → longest common prefix.
pub fn complete_path(input: &str) -> Option<String> {
    let expanded = shellexpand::tilde(input).into_owned();
    let (dir, prefix) = match expanded.rfind('/') {
        Some(i) => (expanded[..=i].to_string(), expanded[i + 1..].to_string()),
        None => ("./".to_string(), expanded.clone()),
    };
    let mut matches: Vec<(String, bool)> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (name.starts_with(&prefix) && (prefix.starts_with('.') || !name.starts_with('.')))
                .then(|| (name, e.path().is_dir()))
        })
        .collect();
    matches.sort();
    let completed = match matches.as_slice() {
        [] => return None,
        [(name, is_dir)] => format!("{name}{}", if *is_dir { "/" } else { "" }),
        many => {
            let first = &many[0].0;
            let mut len = first.len();
            for (m, _) in many {
                len = len.min(
                    first
                        .chars()
                        .zip(m.chars())
                        .take_while(|(a, b)| a == b)
                        .count(),
                );
            }
            first.chars().take(len).collect()
        }
    };
    // Keep the user's own spelling of the directory part (e.g. `~`).
    let head = match input.rfind('/') {
        Some(i) => &input[..=i],
        None => "",
    };
    Some(format!("{head}{completed}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(form: &mut ServerForm, code: KeyCode) -> FormOutcome {
        form.handle_key(&KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn type_str(form: &mut ServerForm, s: &str) {
        for c in s.chars() {
            press(form, KeyCode::Char(c));
        }
    }

    #[test]
    fn host_paste_splits() {
        let mut f = ServerForm::new(None);
        type_str(&mut f, "web");
        press(&mut f, KeyCode::Tab);
        f.paste("ata@1.2.3.4:2222");
        let s = f.build(&Config::default()).unwrap();
        assert_eq!(s.host, "1.2.3.4");
        assert_eq!(s.user.as_deref(), Some("ata"));
        assert_eq!(s.port, Some(2222));
    }

    #[test]
    fn validation_errors() {
        let mut f = ServerForm::new(None);
        type_str(&mut f, "bad name");
        f.fields[JUMP].set("nope");
        assert!(f.build(&Config::default()).is_none());
        assert!(f.errors.contains_key(&NAME));
        assert!(f.errors.contains_key(&HOST));
        assert!(f.errors.contains_key(&JUMP));
        assert_eq!(f.focus, NAME);
    }

    #[test]
    fn esc_confirms_when_dirty() {
        let mut f = ServerForm::new(None);
        assert_eq!(press(&mut f, KeyCode::Esc), FormOutcome::Cancel);
        type_str(&mut f, "x");
        assert_eq!(press(&mut f, KeyCode::Esc), FormOutcome::None);
        assert!(f.confirm_discard);
        assert_eq!(press(&mut f, KeyCode::Char('y')), FormOutcome::Cancel);
    }

    #[test]
    fn path_completion() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("id_ed25519"), "").unwrap();
        std::fs::write(dir.path().join("id_ed25519.pub"), "").unwrap();
        std::fs::create_dir(dir.path().join("keys")).unwrap();
        let base = dir.path().display().to_string();
        assert_eq!(
            complete_path(&format!("{base}/id")).unwrap(),
            format!("{base}/id_ed25519")
        );
        assert_eq!(
            complete_path(&format!("{base}/k")).unwrap(),
            format!("{base}/keys/")
        );
        assert_eq!(complete_path(&format!("{base}/zz")), None);
    }
}
