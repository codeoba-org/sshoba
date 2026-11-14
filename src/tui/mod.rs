//! Terminal UI front end: terminal setup/teardown and the main loop.

pub mod app;
pub mod clipboard;
pub mod event;
pub mod input;
pub mod keys;
pub mod picker;
pub mod views;

#[cfg(test)]
mod tests;

use crate::core::config::{self, ConfigStore};
use crate::core::error::{Error, Result};
use crate::core::launcher::{self, Env};
use crate::core::paths::Paths;
use app::{Action, App};
use event::{Event, Input};
use ratatui::crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use ratatui::crossterm::execute;
use ratatui::DefaultTerminal;
use std::io::{IsTerminal, Write};
use std::time::Duration;

enum Exit {
    Quit,
    Exec(Vec<String>),
}

fn setup() -> Result<DefaultTerminal> {
    let t = ratatui::try_init().map_err(|e| Error::io("initialise terminal", e))?;
    // ratatui's hook restores raw mode and the alternate screen; also turn off paste mode.
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = execute!(std::io::stdout(), DisableBracketedPaste);
            prev(info);
        }));
    });
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    Ok(t)
}

/// Leave the alternate screen, disable raw mode and show the cursor.
fn restore() {
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
}

/// Run the TUI; returns the process exit code.
pub fn run(paths: &Paths) -> i32 {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        eprintln!("error: the TUI needs a terminal; see `shhoba --help` for commands");
        return 1;
    }
    let store = match ConfigStore::load(&paths.config_file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return e.exit_code();
        }
    };
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: cannot start async runtime: {e}");
            return 1;
        }
    };
    let result = rt.block_on(main_loop(paths.clone(), store));
    rt.shutdown_timeout(Duration::from_millis(200));
    match result {
        Ok(Exit::Quit) => 0,
        Ok(Exit::Exec(argv)) => {
            tracing::info!("exec {}", crate::core::sshcmd::shell_join(&argv));
            let e = launcher::exec(&argv);
            eprintln!("error: {e}");
            e.exit_code()
        }
        Err(e) => {
            eprintln!("error: {e}");
            e.exit_code()
        }
    }
}

async fn main_loop(paths: Paths, store: ConfigStore) -> Result<Exit> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let input = Input::spawn(tx.clone());
    let tick_tx = tx.clone();
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_millis(250));
        loop {
            iv.tick().await;
            if tick_tx.send(Event::Tick).is_err() {
                return;
            }
        }
    });
    let mut terminal = setup()?;
    let mut app = App::new(paths, store, tx.clone());
    if app.env == Env::Zellij {
        launcher::log_zellij_version();
    }
    let result = loop {
        if let Err(e) = terminal.draw(|f| views::draw(f, &app)) {
            break Err(Error::io("draw", e));
        }
        let Some(ev) = rx.recv().await else {
            break Ok(Exit::Quit);
        };
        if let Event::Key(k) = &ev {
            if App::is_force_quit(k) {
                break Ok(Exit::Quit);
            }
        }
        match app.update(ev) {
            None => {}
            Some(Action::Quit) => break Ok(Exit::Quit),
            Some(Action::Exec(argv)) => break Ok(Exit::Exec(argv)),
            Some(Action::Copy(text)) => {
                let seq = clipboard::osc52(&text, std::env::var_os("TMUX").is_some());
                let mut out = std::io::stdout();
                let _ = out.write_all(seq.as_bytes()).and_then(|_| out.flush());
            }
            Some(Action::Editor(name)) => {
                input.pause();
                restore();
                let res =
                    config::edit_in_editor(&app.paths.config_file, name.as_deref(), &mut |e| {
                        eprintln!("\ninvalid config:\n{e}\n");
                        eprint!("Reopen the editor? [Y/n] ");
                        let _ = std::io::stderr().flush();
                        let mut line = String::new();
                        let _ = std::io::stdin().read_line(&mut line);
                        !matches!(line.trim(), "n" | "N" | "no")
                    });
                terminal = setup()?;
                let _ = terminal.clear();
                input.resume();
                app.reload_config();
                match res {
                    Ok(true) => app.info("Saved servers.toml"),
                    Ok(false) => app.info("No changes"),
                    Err(e) => app.popup("Edit abandoned", &e),
                }
            }
        }
    };
    restore();
    result
}
