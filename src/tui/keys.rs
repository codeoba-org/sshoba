//! Keybinding table. Key handling and the help overlay both read it, so they never drift.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Commands available on the main screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Down,
    Up,
    First,
    Last,
    Search,
    Connect,
    ConnectWithForwards,
    Add,
    Edit,
    EditInEditor,
    Delete,
    Rename,
    ToggleFavorite,
    Ping,
    PingAll,
    Tunnels,
    AllTunnels,
    Files,
    Transfer,
    CopyCommand,
    ToggleGroup,
    ToggleDetails,
    Import,
    Help,
    Quit,
    // Tunnels screen
    TunnelToggle,
    ForwardAdd,
    ForwardDelete,
    Back,
}

#[derive(Debug, Clone, Copy)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

const fn k(c: char) -> Key {
    Key {
        code: KeyCode::Char(c),
        mods: KeyModifiers::NONE,
    }
}

const fn code(code: KeyCode) -> Key {
    Key {
        code,
        mods: KeyModifiers::NONE,
    }
}

const fn ctrl(c: char) -> Key {
    Key {
        code: KeyCode::Char(c),
        mods: KeyModifiers::CONTROL,
    }
}

const fn shift(code: KeyCode) -> Key {
    Key {
        code,
        mods: KeyModifiers::SHIFT,
    }
}

pub struct Binding {
    pub keys: &'static [Key],
    /// How the keys are shown in help, e.g. `j / ↓`.
    pub label: &'static str,
    pub cmd: Cmd,
    pub help: &'static str,
    /// Short footer hint, if the binding appears in the footer.
    pub hint: Option<&'static str>,
}

pub const MAIN: &[Binding] = &[
    Binding {
        keys: &[k('j'), code(KeyCode::Down)],
        label: "j / ↓",
        cmd: Cmd::Down,
        help: "Move down",
        hint: None,
    },
    Binding {
        keys: &[k('k'), code(KeyCode::Up)],
        label: "k / ↑",
        cmd: Cmd::Up,
        help: "Move up",
        hint: None,
    },
    Binding {
        keys: &[k('g'), code(KeyCode::Home)],
        label: "g",
        cmd: Cmd::First,
        help: "First server",
        hint: None,
    },
    Binding {
        keys: &[k('G'), shift(KeyCode::Char('G')), code(KeyCode::End)],
        label: "G",
        cmd: Cmd::Last,
        help: "Last server",
        hint: None,
    },
    Binding {
        keys: &[k('/')],
        label: "/",
        cmd: Cmd::Search,
        help: "Search (Esc clears)",
        hint: Some("search"),
    },
    Binding {
        keys: &[code(KeyCode::Enter)],
        label: "⏎",
        cmd: Cmd::Connect,
        help: "Connect",
        hint: Some("connect"),
    },
    Binding {
        keys: &[shift(KeyCode::Enter), k('o')],
        label: "o / ⇧⏎",
        cmd: Cmd::ConnectWithForwards,
        help: "Connect with auto forwards",
        hint: None,
    },
    Binding {
        keys: &[k('a')],
        label: "a",
        cmd: Cmd::Add,
        help: "Add server",
        hint: Some("add"),
    },
    Binding {
        keys: &[k('e')],
        label: "e",
        cmd: Cmd::Edit,
        help: "Edit server",
        hint: Some("edit"),
    },
    Binding {
        keys: &[k('E'), shift(KeyCode::Char('E'))],
        label: "E",
        cmd: Cmd::EditInEditor,
        help: "Edit servers.toml in $EDITOR",
        hint: None,
    },
    Binding {
        keys: &[k('d')],
        label: "d",
        cmd: Cmd::Delete,
        help: "Delete server",
        hint: Some("del"),
    },
    Binding {
        keys: &[k('r')],
        label: "r",
        cmd: Cmd::Rename,
        help: "Rename server",
        hint: None,
    },
    Binding {
        keys: &[k('s')],
        label: "s",
        cmd: Cmd::ToggleFavorite,
        help: "Toggle favorite",
        hint: None,
    },
    Binding {
        keys: &[k('p')],
        label: "p",
        cmd: Cmd::Ping,
        help: "Ping selected (full check through jumps)",
        hint: Some("ping"),
    },
    Binding {
        keys: &[k('P'), shift(KeyCode::Char('P'))],
        label: "P",
        cmd: Cmd::PingAll,
        help: "Ping all",
        hint: None,
    },
    Binding {
        keys: &[k('t')],
        label: "t",
        cmd: Cmd::Tunnels,
        help: "Tunnels for selected server",
        hint: Some("tunnels"),
    },
    Binding {
        keys: &[k('T'), shift(KeyCode::Char('T'))],
        label: "T",
        cmd: Cmd::AllTunnels,
        help: "All tunnels",
        hint: None,
    },
    Binding {
        keys: &[k('f')],
        label: "f",
        cmd: Cmd::Files,
        help: "File browser for selected server",
        hint: Some("files"),
    },
    Binding {
        keys: &[k('x')],
        label: "x",
        cmd: Cmd::Transfer,
        help: "Quick send/get for selected server",
        hint: None,
    },
    Binding {
        keys: &[k('y')],
        label: "y",
        cmd: Cmd::CopyCommand,
        help: "Copy ssh command to clipboard",
        hint: None,
    },
    Binding {
        keys: &[k(' ')],
        label: "Space",
        cmd: Cmd::ToggleGroup,
        help: "Collapse / expand group",
        hint: None,
    },
    Binding {
        keys: &[k('i')],
        label: "i",
        cmd: Cmd::ToggleDetails,
        help: "Toggle details pane",
        hint: None,
    },
    Binding {
        keys: &[k('I'), shift(KeyCode::Char('I'))],
        label: "I",
        cmd: Cmd::Import,
        help: "Import from ~/.ssh/config",
        hint: None,
    },
    Binding {
        keys: &[k('?')],
        label: "?",
        cmd: Cmd::Help,
        help: "Help",
        hint: Some("help"),
    },
    Binding {
        keys: &[k('q'), ctrl('c')],
        label: "q / Ctrl+C",
        cmd: Cmd::Quit,
        help: "Quit",
        hint: None,
    },
];

pub const TUNNELS: &[Binding] = &[
    Binding {
        keys: &[k('j'), code(KeyCode::Down)],
        label: "j / ↓",
        cmd: Cmd::Down,
        help: "Move down",
        hint: None,
    },
    Binding {
        keys: &[k('k'), code(KeyCode::Up)],
        label: "k / ↑",
        cmd: Cmd::Up,
        help: "Move up",
        hint: None,
    },
    Binding {
        keys: &[code(KeyCode::Enter)],
        label: "⏎",
        cmd: Cmd::TunnelToggle,
        help: "Start / stop tunnel",
        hint: Some("up/down"),
    },
    Binding {
        keys: &[k('a')],
        label: "a",
        cmd: Cmd::ForwardAdd,
        help: "Add forward (e.g. pg -L 5432:localhost:5432)",
        hint: Some("add"),
    },
    Binding {
        keys: &[k('d')],
        label: "d",
        cmd: Cmd::ForwardDelete,
        help: "Delete forward",
        hint: Some("del"),
    },
    Binding {
        keys: &[code(KeyCode::Esc), k('q')],
        label: "Esc",
        cmd: Cmd::Back,
        help: "Back to servers",
        hint: Some("back"),
    },
    Binding {
        keys: &[k('?')],
        label: "?",
        cmd: Cmd::Help,
        help: "Help",
        hint: Some("help"),
    },
    Binding {
        keys: &[ctrl('c')],
        label: "Ctrl+C",
        cmd: Cmd::Quit,
        help: "Quit",
        hint: None,
    },
];

/// Find the command bound to `ev` in `table`.
pub fn lookup(table: &[Binding], ev: &KeyEvent) -> Option<Cmd> {
    // Terminals report Shift+letter as an uppercase char, with or without SHIFT.
    let mods = match ev.code {
        KeyCode::Char(_) => ev.modifiers - KeyModifiers::SHIFT,
        _ => ev.modifiers,
    };
    table.iter().find_map(|b| {
        b.keys
            .iter()
            .any(|key| {
                key.code == ev.code
                    && match ev.code {
                        KeyCode::Char(_) => (key.mods - KeyModifiers::SHIFT) == mods,
                        _ => key.mods == mods,
                    }
            })
            .then_some(b.cmd)
    })
}

/// Footer hints: `⏎ connect  a add ...`.
pub fn hints(table: &[Binding]) -> Vec<(&'static str, &'static str)> {
    table
        .iter()
        .filter_map(|b| {
            b.hint
                .map(|h| (b.label.split(" / ").next().unwrap_or(b.label), h))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn lookups() {
        assert_eq!(
            lookup(MAIN, &ev(KeyCode::Char('j'), KeyModifiers::NONE)),
            Some(Cmd::Down)
        );
        assert_eq!(
            lookup(MAIN, &ev(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            Some(Cmd::Last)
        );
        assert_eq!(
            lookup(MAIN, &ev(KeyCode::Char('G'), KeyModifiers::NONE)),
            Some(Cmd::Last)
        );
        assert_eq!(
            lookup(MAIN, &ev(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Cmd::Quit)
        );
        assert_eq!(
            lookup(MAIN, &ev(KeyCode::Enter, KeyModifiers::SHIFT)),
            Some(Cmd::ConnectWithForwards)
        );
        assert_eq!(
            lookup(MAIN, &ev(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Cmd::Connect)
        );
        assert_eq!(
            lookup(MAIN, &ev(KeyCode::Char('z'), KeyModifiers::NONE)),
            None
        );
    }
}
