//! Quick send/get dialog with progress (section 6.3.4).

use crate::core::sshcmd::Direction;
use crate::core::transfer::Progress;
use crate::tui::input::TextInput;
use crate::tui::views::server_form::complete_path;
use crate::tui::views::{centered, input_line};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Gauge, Paragraph, Wrap};
use ratatui::Frame;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Direction,
    Source,
    Dest,
    Running,
    Done(Result<(), String>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    Close,
    Start,
}

#[derive(Debug, Clone)]
pub struct TransferView {
    pub server: String,
    pub dir: Direction,
    pub step: Step,
    pub source: TextInput,
    pub dest: TextInput,
    pub progress: Option<Progress>,
    pub cancel: Arc<AtomicBool>,
    pub started: Instant,
}

impl TransferView {
    pub fn new(server: &str) -> Self {
        TransferView {
            server: server.to_string(),
            dir: Direction::Send,
            step: Step::Direction,
            source: TextInput::default(),
            dest: TextInput::default(),
            progress: None,
            cancel: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
        }
    }

    pub fn running(&self) -> bool {
        self.step == Step::Running
    }

    fn complete(input: &mut TextInput) {
        if let Some(done) = complete_path(&input.value()) {
            input.set(&done);
        }
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> Outcome {
        match self.step.clone() {
            Step::Direction => match key.code {
                KeyCode::Char('s') => {
                    self.dir = Direction::Send;
                    self.step = Step::Source;
                }
                KeyCode::Char('g') => {
                    self.dir = Direction::Get;
                    self.step = Step::Source;
                }
                KeyCode::Esc | KeyCode::Char('q') => return Outcome::Close,
                _ => {}
            },
            Step::Source => match key.code {
                KeyCode::Esc => self.step = Step::Direction,
                KeyCode::Tab if self.dir == Direction::Send => Self::complete(&mut self.source),
                KeyCode::Enter if !self.source.value().trim().is_empty() => {
                    if self.dest.is_empty() {
                        self.dest.set(match self.dir {
                            Direction::Send => "~",
                            Direction::Get => ".",
                        });
                    }
                    self.step = Step::Dest;
                }
                _ => {
                    self.source.handle(key);
                }
            },
            Step::Dest => match key.code {
                KeyCode::Esc => self.step = Step::Source,
                KeyCode::Tab if self.dir == Direction::Get => Self::complete(&mut self.dest),
                KeyCode::Enter => return Outcome::Start,
                _ => {
                    self.dest.handle(key);
                }
            },
            Step::Running => {
                if key.code == KeyCode::Esc {
                    self.cancel.store(true, Ordering::SeqCst);
                }
            }
            Step::Done(_) => return Outcome::Close,
        }
        Outcome::None
    }

    pub fn paste(&mut self, s: &str) {
        match self.step {
            Step::Source => self.source.paste(s.trim()),
            Step::Dest => self.dest.paste(s.trim()),
            _ => {}
        }
    }

    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let area = centered(area, 72, 11);
        f.render_widget(Clear, area);
        let verb = match self.dir {
            Direction::Send => "Send to",
            Direction::Get => "Get from",
        };
        let title = match self.step {
            Step::Direction => format!(" Transfer · {} ", self.server),
            _ => format!(" {verb} {} ", self.server),
        };
        let hint = match self.step {
            Step::Direction => " s send · g get · Esc close ",
            Step::Source | Step::Dest => " Tab complete · ⏎ next · Esc back ",
            Step::Running => " Esc cancel ",
            Step::Done(_) => " any key to close ",
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(title.bold())
            .title_bottom(Line::from(hint).centered());
        let inner = block.inner(area);
        f.render_widget(block, area);
        let row = |i: u16| Rect::new(inner.x + 1, inner.y + i, inner.width.saturating_sub(2), 1);
        let (src_label, dst_label) = match self.dir {
            Direction::Send => ("local path", "remote dir"),
            Direction::Get => ("remote path", "local dir"),
        };
        match &self.step {
            Step::Direction => {
                f.render_widget(Paragraph::new("s  Send local files to the server"), row(1));
                f.render_widget(Paragraph::new("g  Get files from the server"), row(2));
            }
            _ => {
                f.render_widget(Paragraph::new(src_label.dim()), row(0));
                input_line(f, row(1), &self.source, self.step == Step::Source, "");
                if self.step != Step::Source {
                    f.render_widget(Paragraph::new(dst_label.dim()), row(2));
                    input_line(f, row(3), &self.dest, self.step == Step::Dest, "");
                }
            }
        }
        match &self.step {
            Step::Running => match &self.progress {
                Some(p) => {
                    let mut label = format!("{}%  {}  ETA {}", p.percent, p.rate, p.eta);
                    if let Some(file) = &p.file {
                        label = format!("{label}  {file}");
                    }
                    f.render_widget(
                        Gauge::default()
                            .gauge_style(Style::new().fg(Color::Green))
                            .percent(p.percent.min(100) as u16)
                            .label(label),
                        row(5),
                    );
                }
                None => {
                    const SPIN: [&str; 4] = ["◐", "◓", "◑", "◒"];
                    let i = (self.started.elapsed().as_millis() / 250) as usize % 4;
                    let secs = self.started.elapsed().as_secs();
                    f.render_widget(
                        Paragraph::new(format!("{} copying… {secs}s", SPIN[i]).fg(Color::Yellow)),
                        row(5),
                    );
                }
            },
            Step::Done(Ok(())) => {
                f.render_widget(Paragraph::new("✓ Done".fg(Color::Green).bold()), row(5));
            }
            Step::Done(Err(e)) => {
                let r = Rect::new(row(5).x, row(5).y, row(5).width, 3);
                f.render_widget(
                    Paragraph::new(e.clone().fg(Color::Red)).wrap(Wrap { trim: true }),
                    r,
                );
            }
            _ => {}
        }
    }
}
