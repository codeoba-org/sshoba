//! Human tables and stable `--json` shapes.

use crate::core::model::{Config, Forward, Server};
use crate::core::resolve;
use crate::core::state::State;
use serde::Serialize;
use std::io::IsTerminal;

/// Colors only when stdout is a TTY and NO_COLOR is unset.
pub fn color_enabled() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

#[derive(Clone, Copy)]
pub enum Style {
    Bold,
    Dim,
    Green,
    Red,
    Yellow,
    Cyan,
}

pub fn paint(s: &str, style: Style) -> String {
    if !color_enabled() {
        return s.to_string();
    }
    let code = match style {
        Style::Bold => "1",
        Style::Dim => "2",
        Style::Green => "32",
        Style::Red => "31",
        Style::Yellow => "33",
        Style::Cyan => "36",
    };
    format!("\x1b[{code}m{s}\x1b[0m")
}

/// An aligned text table. Cells may contain ANSI color codes.
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

fn visible_len(s: &str) -> usize {
    let mut n = 0;
    let mut in_esc = false;
    for c in s.chars() {
        match (in_esc, c) {
            (false, '\x1b') => in_esc = true,
            (true, 'm') => in_esc = false,
            (true, _) => {}
            (false, _) => n += 1,
        }
    }
    n
}

impl Table {
    pub fn new(headers: &[&str]) -> Self {
        Table {
            headers: headers.iter().map(|h| h.to_string()).collect(),
            rows: Vec::new(),
        }
    }

    pub fn row(&mut self, cells: Vec<String>) {
        self.rows.push(cells);
    }

    pub fn render(&self) -> String {
        let cols = self.headers.len();
        let mut widths: Vec<usize> = self.headers.iter().map(|h| h.len()).collect();
        for r in &self.rows {
            for (i, c) in r.iter().enumerate().take(cols) {
                widths[i] = widths[i].max(visible_len(c));
            }
        }
        let line = |cells: Vec<String>| -> String {
            let mut out = String::new();
            for (i, c) in cells.iter().enumerate() {
                out.push_str(c);
                if i + 1 < cells.len() {
                    out.push_str(&" ".repeat(widths[i] - visible_len(c) + 2));
                }
            }
            out.trim_end().to_string()
        };
        let mut out = line(self.headers.iter().map(|h| paint(h, Style::Bold)).collect());
        out.push('\n');
        for r in &self.rows {
            out.push_str(&line(r.clone()));
            out.push('\n');
        }
        out
    }
}

/// Stable JSON shape for a forward.
#[derive(Debug, Serialize)]
pub struct ForwardJson {
    pub name: String,
    pub kind: crate::core::model::ForwardKind,
    pub bind: Option<String>,
    pub local_port: u16,
    pub remote: Option<String>,
    pub auto: bool,
    pub spec: String,
}

impl From<&Forward> for ForwardJson {
    fn from(f: &Forward) -> Self {
        ForwardJson {
            name: f.name.clone(),
            kind: f.kind,
            bind: f.bind.clone(),
            local_port: f.local_port,
            remote: f.remote.clone(),
            auto: f.auto,
            spec: f.spec(),
        }
    }
}

/// Stable JSON shape for a server: every field is always present.
#[derive(Debug, Serialize)]
pub struct ServerJson {
    pub name: String,
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub key: Option<String>,
    pub jump: Vec<String>,
    pub group: Option<String>,
    pub tags: Vec<String>,
    pub favorite: bool,
    pub note: Option<String>,
    pub extra_args: Vec<String>,
    pub forwards: Vec<ForwardJson>,
    pub last_connected: Option<String>,
    pub connect_count: u64,
}

impl ServerJson {
    pub fn new(s: &Server, state: &State) -> Self {
        let st = state.servers.get(&s.name);
        ServerJson {
            name: s.name.clone(),
            host: s.host.clone(),
            user: s.user.clone(),
            port: s.port,
            key: s.key.as_ref().map(|k| k.display().to_string()),
            jump: s.jump.clone(),
            group: s.group.clone(),
            tags: s.tags.clone(),
            favorite: s.favorite,
            note: s.note.clone(),
            extra_args: s.extra_args.clone(),
            forwards: s.forward.iter().map(ForwardJson::from).collect(),
            last_connected: st.and_then(|x| x.last_connected).map(|t| t.to_rfc3339()),
            connect_count: st.map_or(0, |x| x.connect_count),
        }
    }
}

/// `show --json`: the server plus resolved chain and commands.
#[derive(Debug, Serialize)]
pub struct ShowJson {
    #[serde(flatten)]
    pub server: ServerJson,
    pub chain: Vec<String>,
    pub command: String,
    pub human_command: String,
}

pub fn print_json<T: Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("json serializes")
    );
}

/// "via a › b" display of the hops to a server.
pub fn chain_display(cfg: &Config, s: &Server) -> String {
    let hops = resolve::chain(cfg, s).unwrap_or_else(|_| s.jump.clone());
    let mut all = hops;
    all.push(s.name.clone());
    all.join(" › ")
}

/// "2h ago"-style relative time.
pub fn ago(t: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (chrono::Utc::now() - t).num_seconds().max(0);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}
