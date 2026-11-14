//! App state and the update function (Elm-style). Rendering lives in `views`.

use crate::core::config::{self, ConfigStore};
use crate::core::error::Error;
use crate::core::launcher::{self, Env, LaunchMode, Target};
use crate::core::model::{Config, Server};
use crate::core::paths::Paths;
use crate::core::resolve;
use crate::core::state::{PingRecord, State};
use crate::tui::event::{Event, Msg};
use crate::tui::input::TextInput;
use crate::tui::keys::{self, Cmd};
use crate::tui::views::server_form::{FormOutcome, ServerForm};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::mpsc::UnboundedSender;

/// Something the main loop must do outside `update` (it needs the terminal).
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Quit,
    /// Restore the terminal and replace the process with this argv.
    Exec(Vec<String>),
    /// Suspend the TUI and open servers.toml in $EDITOR at this server.
    Editor(Option<String>),
    /// Put this text on the clipboard (OSC 52).
    Copy(String),
}

/// A visible row of the server list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Group {
        name: String,
        count: usize,
        collapsed: bool,
    },
    Server(String),
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    Delete { name: String, force: bool },
    DeleteForward { server: String, name: String },
    Import(Vec<Server>),
}

#[derive(Debug, Clone)]
pub enum PromptAction {
    Rename(String),
    AddForward(String),
}

/// Which full screen is showing.
#[derive(Debug, Clone)]
pub enum Screen {
    Main,
    Files(Box<crate::tui::views::file_browser::FileBrowser>),
    /// Forwards of one server, or of all servers (None).
    Tunnels {
        server: Option<String>,
        selected: usize,
    },
}

/// One row of the tunnels screen.
#[derive(Debug, Clone, PartialEq)]
pub struct TunnelRow {
    pub server: String,
    pub forward: crate::core::model::Forward,
    pub id: String,
}

#[derive(Debug, Clone)]
pub enum Modal {
    Help,
    Confirm {
        message: String,
        action: ConfirmAction,
    },
    Prompt {
        title: String,
        input: TextInput,
        action: PromptAction,
        error: Option<String>,
    },
    Form(Box<ServerForm>),
    Error {
        title: String,
        text: String,
    },
    Transfer(Box<crate::tui::views::transfer::TransferView>),
}

#[derive(Debug, Clone)]
pub struct Status {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// Label of the group for servers without one.
pub const NO_GROUP: &str = "other";

pub struct App {
    pub paths: Paths,
    pub store: ConfigStore,
    pub state: State,
    pub env: Env,
    pub search: TextInput,
    pub searching: bool,
    pub collapsed: HashSet<String>,
    /// Server names in display order; recomputed only on explicit actions.
    pub order: Vec<String>,
    pub rows: Vec<Row>,
    pub selected: usize,
    /// Details pane override; None = automatic by width.
    pub details: Option<bool>,
    pub modal: Option<Modal>,
    pub status: Option<Status>,
    pub tx: UnboundedSender<Event>,
    config_mtime: Option<SystemTime>,
    last_mtime_check: Instant,
    /// Latest ping result per server (cached values from state.json until refreshed).
    pub ping: BTreeMap<String, PingRecord>,
    /// Servers whose ping result is from this session.
    pub fresh: HashSet<String>,
    /// Servers with a check in flight.
    pub checking: HashSet<String>,
    last_auto_ping: Option<Instant>,
    pub screen: Screen,
    /// Tunnels with a start/stop in flight.
    pub tunnel_busy: HashSet<String>,
    /// Last start failure per tunnel id.
    pub tunnel_failed: BTreeMap<String, String>,
    last_tunnel_refresh: Instant,
}

fn mtime(paths: &Paths) -> Option<SystemTime> {
    std::fs::metadata(&paths.config_file)
        .and_then(|m| m.modified())
        .ok()
}

impl App {
    pub fn new(paths: Paths, store: ConfigStore, tx: UnboundedSender<Event>) -> App {
        let state = crate::core::tunnel::refresh(&paths).unwrap_or_else(|_| State::load(&paths));
        let env = launcher::detect_env(store.config.settings.launch);
        let ping = state.ping.clone();
        let mut app = App {
            ping,
            fresh: HashSet::new(),
            checking: HashSet::new(),
            last_auto_ping: None,
            screen: Screen::Main,
            tunnel_busy: HashSet::new(),
            tunnel_failed: BTreeMap::new(),
            last_tunnel_refresh: Instant::now(),
            config_mtime: mtime(&paths),
            paths,
            store,
            state,
            env,
            search: TextInput::default(),
            searching: false,
            collapsed: HashSet::new(),
            order: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            details: None,
            modal: None,
            status: None,
            tx,
            last_mtime_check: Instant::now(),
        };
        app.resort();
        app.select_first_server();
        app
    }

    pub fn config(&self) -> &Config {
        &self.store.config
    }

    // ----- list model ------------------------------------------------------

    fn group_of(s: &Server) -> &str {
        s.group.as_deref().unwrap_or(NO_GROUP)
    }

    fn has_groups(&self) -> bool {
        self.config().server.iter().any(|s| s.group.is_some())
    }

    /// Recompute the display order: by group, favorites first, then most recently used.
    pub fn resort(&mut self) {
        let mut servers: Vec<&Server> = self.store.config.server.iter().collect();
        let state = &self.state;
        servers.sort_by(|a, b| {
            let ga = (a.group.is_none(), Self::group_of(a));
            let gb = (b.group.is_none(), Self::group_of(b));
            ga.cmp(&gb)
                .then(b.favorite.cmp(&a.favorite))
                .then(
                    state
                        .last_connected(&b.name)
                        .cmp(&state.last_connected(&a.name)),
                )
                .then(a.name.cmp(&b.name))
        });
        self.order = servers.into_iter().map(|s| s.name.clone()).collect();
        self.rebuild_rows();
    }

    fn selected_key(&self) -> Option<Row> {
        self.rows.get(self.selected).cloned()
    }

    /// Rebuild visible rows from `order`, the search and collapsed groups, keeping the
    /// selection on the same server or group.
    pub fn rebuild_rows(&mut self) {
        let keep = self.selected_key();
        let cfg = &self.store.config;
        let query = self.search.value();
        let mut rows = Vec::new();
        if !query.trim().is_empty() {
            let ranked = resolve::fuzzy_filter(&cfg.server, &query);
            rows.extend(ranked.into_iter().map(|s| Row::Server(s.name.clone())));
        } else if !self.has_groups() {
            rows.extend(self.order.iter().cloned().map(Row::Server));
        } else {
            let mut i = 0;
            while i < self.order.len() {
                let g = cfg
                    .get(&self.order[i])
                    .map(Self::group_of)
                    .unwrap_or(NO_GROUP)
                    .to_string();
                let members: Vec<String> = self.order[i..]
                    .iter()
                    .take_while(|n| cfg.get(n).map(Self::group_of) == Some(g.as_str()))
                    .cloned()
                    .collect();
                i += members.len().max(1);
                let collapsed = self.collapsed.contains(&g);
                rows.push(Row::Group {
                    name: g,
                    count: members.len(),
                    collapsed,
                });
                if !collapsed {
                    rows.extend(members.into_iter().map(Row::Server));
                }
            }
        }
        self.rows = rows;
        self.selected = keep
            .and_then(|k| {
                self.rows.iter().position(|r| match (r, &k) {
                    (Row::Server(a), Row::Server(b)) => a == b,
                    (Row::Group { name: a, .. }, Row::Group { name: b, .. }) => a == b,
                    _ => false,
                })
            })
            .unwrap_or_else(|| self.selected.min(self.rows.len().saturating_sub(1)));
    }

    fn select_first_server(&mut self) {
        if let Some(i) = self.rows.iter().position(|r| matches!(r, Row::Server(_))) {
            self.selected = i;
        }
    }

    pub fn select_server(&mut self, name: &str) {
        if let Some(i) = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::Server(n) if n == name))
        {
            self.selected = i;
        }
    }

    pub fn selected_server(&self) -> Option<&Server> {
        match self.rows.get(self.selected) {
            Some(Row::Server(n)) => self.store.config.get(n),
            _ => None,
        }
    }

    // ----- status and errors ----------------------------------------------

    pub fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: false,
            at: Instant::now(),
        });
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: true,
            at: Instant::now(),
        });
    }

    pub fn popup(&mut self, title: &str, err: &Error) {
        tracing::warn!("{title}: {err}");
        self.modal = Some(Modal::Error {
            title: title.to_string(),
            text: err.to_string(),
        });
    }

    // ----- persistence -----------------------------------------------------

    fn save(&mut self, new: Config, rename: Option<(&str, &str)>) -> Result<(), Error> {
        self.store.save(new, rename)?;
        self.config_mtime = mtime(&self.paths);
        self.resort();
        Ok(())
    }

    /// Reload servers.toml from disk, keeping the selection by name.
    pub fn reload_config(&mut self) {
        self.config_mtime = mtime(&self.paths);
        match ConfigStore::load(&self.paths.config_file) {
            Ok(store) => {
                self.store = store;
                self.env = launcher::detect_env(self.store.config.settings.launch);
                self.state = State::load(&self.paths);
                self.resort();
            }
            Err(e) => self.warn(format!("servers.toml not reloaded: {e}")),
        }
    }

    fn check_config_changed(&mut self) {
        if self.last_mtime_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_mtime_check = Instant::now();
        if mtime(&self.paths) != self.config_mtime {
            self.reload_config();
            self.info("servers.toml changed on disk; reloaded");
        }
    }

    // ----- update ----------------------------------------------------------

    pub fn update(&mut self, event: Event) -> Option<Action> {
        match event {
            Event::Key(key) => self.on_key(key),
            Event::Paste(s) => {
                self.on_paste(&s);
                None
            }
            Event::Resize => None,
            Event::Tick => {
                if self
                    .status
                    .as_ref()
                    .is_some_and(|s| s.at.elapsed() > Duration::from_secs(3))
                {
                    self.status = None;
                }
                self.check_config_changed();
                self.maybe_auto_ping();
                if matches!(self.screen, Screen::Tunnels { .. })
                    && self.last_tunnel_refresh.elapsed() >= Duration::from_secs(5)
                {
                    self.refresh_tunnels();
                }
                None
            }
            Event::Msg(msg) => self.on_msg(msg),
        }
    }

    fn on_paste(&mut self, s: &str) {
        match &mut self.modal {
            Some(Modal::Form(form)) => form.paste(s),
            Some(Modal::Prompt { input, .. }) => input.paste(s),
            Some(Modal::Transfer(t)) => t.paste(s),
            None if self.searching => {
                self.search.paste(s);
                self.rebuild_rows();
            }
            _ => {}
        }
    }

    fn on_msg(&mut self, msg: Msg) -> Option<Action> {
        match msg {
            Msg::Ping { name, outcome } => {
                self.checking.remove(&name);
                self.fresh.insert(name.clone());
                let record = outcome.record();
                self.ping.insert(name.clone(), record.clone());
                if let Err(e) = State::update(&self.paths, |s| {
                    s.ping.insert(name, record);
                }) {
                    tracing::warn!("ping not cached: {e}");
                }
            }
            Msg::Tunnel { id, up, result } => {
                self.tunnel_busy.remove(&id);
                self.refresh_tunnels();
                match result {
                    Ok(()) => {
                        self.tunnel_failed.remove(&id);
                        self.info(format!("Tunnel {id} {}", if up { "up" } else { "stopped" }));
                    }
                    Err(e) => {
                        if up {
                            self.tunnel_failed.insert(id.clone(), e.clone());
                        }
                        self.modal = Some(Modal::Error {
                            title: format!("Tunnel {id}"),
                            text: e,
                        });
                    }
                }
            }
            Msg::RemoteListing {
                server,
                keep,
                result,
            } => {
                if let Screen::Files(b) = &mut self.screen {
                    if b.server == server {
                        match result {
                            Ok((path, entries)) => {
                                b.remote.set_entries(path, entries, keep.as_deref())
                            }
                            Err(e) => {
                                b.remote.loading = false;
                                b.remote.error = Some(e);
                            }
                        }
                    }
                }
            }
            Msg::TransferProgress(p) => {
                if let Some(Modal::Transfer(t)) = &mut self.modal {
                    t.progress = Some(p);
                } else if let Screen::Files(b) = &mut self.screen {
                    if let Some((_, prog)) = &mut b.copying {
                        *prog = Some(p);
                    }
                }
            }
            Msg::TransferDone(result) => {
                use crate::tui::views::transfer::Step;
                if let (None, Screen::Files(b)) = (&self.modal, &mut self.screen) {
                    if let Some((to, _)) = b.copying.take() {
                        b.pane_mut(crate::tui::views::file_browser::FileBrowser::other(to))
                            .marked
                            .clear();
                        match result {
                            Ok(()) => self.info("Copy finished"),
                            Err(e) => {
                                self.modal = Some(Modal::Error {
                                    title: "Copy failed".into(),
                                    text: e,
                                })
                            }
                        }
                        self.refresh_pane(to, None);
                        return None;
                    }
                }
                match &mut self.modal {
                    Some(Modal::Transfer(t)) => t.step = Step::Done(result),
                    _ => match result {
                        Ok(()) => self.info("Transfer finished"),
                        Err(e) => self.warn(format!("Transfer failed: {e}")),
                    },
                }
            }
            Msg::TabOpened { name, result } => match result {
                Ok(()) => {
                    if let Ok((state, ())) = State::update(&self.paths, |s| s.record_connect(&name))
                    {
                        self.state = state;
                    }
                    if !self.config().settings.stay_open {
                        return Some(Action::Quit);
                    }
                    self.info(format!("Opened {name} in new {} tab", self.env.label()));
                }
                Err(e) => {
                    self.modal = Some(Modal::Error {
                        title: format!("Could not open {name}"),
                        text: e,
                    })
                }
            },
        }
        None
    }

    fn on_key(&mut self, key: KeyEvent) -> Option<Action> {
        if self.modal.is_some() {
            return self.on_modal_key(key);
        }
        if self.searching {
            self.on_search_key(key);
            return None;
        }
        if key.code == KeyCode::Esc && !self.search.is_empty() {
            self.search = TextInput::default();
            self.rebuild_rows();
            return None;
        }
        if let Screen::Tunnels { .. } = self.screen {
            let cmd = keys::lookup(keys::TUNNELS, &key)?;
            return self.run_tunnels(cmd);
        }
        if let Screen::Files(_) = self.screen {
            let cmd = keys::lookup(keys::FILES, &key)?;
            return self.run_files(cmd);
        }
        let cmd = keys::lookup(keys::MAIN, &key)?;
        self.run(cmd)
    }

    fn on_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.searching = false;
                self.search = TextInput::default();
                self.rebuild_rows();
                self.select_first_server();
            }
            KeyCode::Enter => {
                self.searching = false;
                self.select_first_server();
            }
            KeyCode::Down => self.move_selection(1),
            KeyCode::Up => self.move_selection(-1),
            _ => {
                if self.search.handle(&key) {
                    self.rebuild_rows();
                    self.select_first_server();
                }
            }
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let max = self.rows.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, max) as usize;
    }

    fn on_modal_key(&mut self, key: KeyEvent) -> Option<Action> {
        let modal = self.modal.take()?;
        match modal {
            Modal::Help | Modal::Error { .. } => {
                if !matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('?')
                ) {
                    self.modal = Some(modal);
                }
            }
            Modal::Confirm { message, action } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.confirmed(action),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {}
                _ => self.modal = Some(Modal::Confirm { message, action }),
            },
            Modal::Prompt {
                title,
                mut input,
                action,
                ..
            } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if let Err(e) = self.prompt_submitted(&action, &input.value()) {
                        self.modal = Some(Modal::Prompt {
                            title,
                            input,
                            action,
                            error: Some(e.to_string()),
                        });
                    }
                }
                _ => {
                    input.handle(&key);
                    self.modal = Some(Modal::Prompt {
                        title,
                        input,
                        action,
                        error: None,
                    });
                }
            },
            Modal::Transfer(mut t) => {
                use crate::tui::views::transfer::Outcome;
                match t.handle_key(&key) {
                    Outcome::None => self.modal = Some(Modal::Transfer(t)),
                    Outcome::Close => {}
                    Outcome::Start => self.start_transfer(t),
                }
            }
            Modal::Form(mut form) => match form.handle_key(&key) {
                FormOutcome::None => self.modal = Some(Modal::Form(form)),
                FormOutcome::Cancel => {}
                FormOutcome::Submit => self.submit_form(form),
            },
        }
        None
    }

    fn confirmed(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::Import(servers) => {
                let n = servers.len();
                let mut cfg = self.config().clone();
                cfg.server.extend(servers);
                match self.save(cfg, None) {
                    Ok(()) => self.info(format!("Imported {n} server(s)")),
                    Err(e) => self.popup("Import failed", &e),
                }
            }
            ConfirmAction::DeleteForward { server, name } => {
                let id = crate::core::tunnel::tunnel_id(&server, &name);
                if self.state.tunnels.iter().any(|t| t.id == id) {
                    if let Err(e) = crate::core::tunnel::stop(&self.paths, &id) {
                        return self.popup("Stop tunnel", &e);
                    }
                }
                let mut cfg = self.config().clone();
                match config::remove_forward(&mut cfg, &server, &name)
                    .and_then(|_| self.save(cfg, None))
                {
                    Ok(()) => {
                        self.refresh_tunnels();
                        self.info(format!("Deleted forward {id}"));
                    }
                    Err(e) => self.popup("Delete forward", &e),
                }
            }
            ConfirmAction::Delete { name, force } => {
                let mut cfg = self.config().clone();
                let res = config::remove_server(&mut cfg, &name, force)
                    .and_then(|_| self.save(cfg, None));
                match res {
                    Ok(()) => {
                        let _ = State::update(&self.paths, |s| s.remove_server(&name));
                        self.info(format!("Deleted {name}"));
                    }
                    Err(e) => self.popup("Delete failed", &e),
                }
            }
        }
    }

    fn prompt_submitted(&mut self, action: &PromptAction, value: &str) -> Result<(), Error> {
        match action {
            PromptAction::AddForward(server) => {
                let fwd = parse_forward_input(value)?;
                let name = fwd.name.clone();
                let mut cfg = self.config().clone();
                config::add_forward(&mut cfg, server, fwd)?;
                self.save(cfg, None)?;
                self.info(format!("Added forward {server}/{name}"));
            }
            PromptAction::Rename(old) => {
                let new = value.trim();
                let mut cfg = self.config().clone();
                config::rename_server(&mut cfg, old, new)?;
                self.save(cfg, Some((old, new)))?;
                if let Ok((state, ())) = State::update(&self.paths, |s| s.rename_server(old, new)) {
                    self.state = state;
                }
                self.select_server(new);
                self.info(format!("Renamed {old} → {new}"));
            }
        }
        Ok(())
    }

    fn submit_form(&mut self, mut form: Box<ServerForm>) {
        let Some(server) = form.build(self.config()) else {
            self.modal = Some(Modal::Form(form));
            return;
        };
        let mut cfg = self.config().clone();
        let name = server.name.clone();
        let old_name = form.original.as_ref().map(|s| s.name.clone());
        let result = match &old_name {
            None => config::add_server(&mut cfg, server),
            Some(old) => config::rename_server(&mut cfg, old, &name).map(|_| {
                if let Some(slot) = cfg.get_mut(&name) {
                    // Jump references to this server were already renamed in `cfg`.
                    *slot = server;
                }
            }),
        }
        .and_then(|_| {
            let rename = old_name.as_deref().map(|o| (o, name.as_str()));
            self.save(cfg, rename)
        });
        match result {
            Ok(()) => {
                if let Some(old) = old_name.as_deref().filter(|o| *o != name) {
                    if let Ok((state, ())) =
                        State::update(&self.paths, |s| s.rename_server(old, &name))
                    {
                        self.state = state;
                    }
                }
                self.select_server(&name);
                self.info(format!("Saved {name}"));
            }
            Err(e) => {
                form.general_error = Some(e.to_string());
                self.modal = Some(Modal::Form(form));
            }
        }
    }

    // ----- commands --------------------------------------------------------

    fn run(&mut self, cmd: Cmd) -> Option<Action> {
        match cmd {
            Cmd::Quit => return Some(Action::Quit),
            Cmd::Down => self.move_selection(1),
            Cmd::Up => self.move_selection(-1),
            Cmd::First => self.selected = 0,
            Cmd::Last => self.selected = self.rows.len().saturating_sub(1),
            Cmd::Search => self.searching = true,
            Cmd::Help => self.modal = Some(Modal::Help),
            Cmd::ToggleDetails => {
                let shown = self.details.unwrap_or(true);
                self.details = Some(!shown);
            }
            Cmd::ToggleGroup => self.toggle_group(),
            Cmd::Connect => return self.connect(false),
            Cmd::ConnectWithForwards => return self.connect(true),
            Cmd::Add => {
                let known = self.order.clone();
                self.modal = Some(Modal::Form(Box::new(ServerForm::new(None, known))));
            }
            Cmd::Edit => {
                if let Some(s) = self.selected_server() {
                    let form = ServerForm::new(Some(s), self.order.clone());
                    self.modal = Some(Modal::Form(Box::new(form)));
                }
            }
            Cmd::EditInEditor => {
                return Some(Action::Editor(
                    self.selected_server().map(|s| s.name.clone()),
                ))
            }
            Cmd::Delete => self.ask_delete(),
            Cmd::Rename => {
                if let Some(s) = self.selected_server() {
                    self.modal = Some(Modal::Prompt {
                        title: format!("Rename {}", s.name),
                        input: TextInput::new(&s.name),
                        action: PromptAction::Rename(s.name.clone()),
                        error: None,
                    });
                }
            }
            Cmd::ToggleFavorite => self.toggle_favorite(),
            Cmd::Ping => {
                if let Some(n) = self.selected_server().map(|s| s.name.clone()) {
                    self.start_ping(vec![n], true);
                }
            }
            Cmd::PingAll => {
                let all = self.order.clone();
                self.start_ping(all, false);
            }
            Cmd::Import => self.ask_import(),
            Cmd::Tunnels => {
                if let Some(n) = self.selected_server().map(|s| s.name.clone()) {
                    self.open_tunnels(Some(n));
                }
            }
            Cmd::AllTunnels => self.open_tunnels(None),
            Cmd::TunnelToggle | Cmd::ForwardAdd | Cmd::ForwardDelete | Cmd::Back => {}
            Cmd::Transfer => {
                if let Some(s) = self.selected_server() {
                    let view = crate::tui::views::transfer::TransferView::new(&s.name);
                    self.modal = Some(Modal::Transfer(Box::new(view)));
                }
            }
            Cmd::Files => self.open_files(),
            Cmd::CopyCommand => {
                if let Some(s) = self.selected_server() {
                    let cmd = crate::core::sshcmd::human_command(s, self.config());
                    self.info(format!("Copied: {cmd}"));
                    return Some(Action::Copy(cmd));
                }
            }
            Cmd::SwitchPane
            | Cmd::Open
            | Cmd::ParentDir
            | Cmd::Mark
            | Cmd::CopyMarked
            | Cmd::Refresh
            | Cmd::ToggleHidden => {}
        }
        None
    }

    fn toggle_group(&mut self) {
        let group = match self.rows.get(self.selected) {
            Some(Row::Group { name, .. }) => name.clone(),
            Some(Row::Server(n)) if self.has_groups() && self.search.is_empty() => self
                .config()
                .get(n)
                .map(|s| Self::group_of(s).to_string())
                .unwrap_or_default(),
            _ => return,
        };
        if !self.collapsed.remove(&group) {
            self.collapsed.insert(group.clone());
        }
        self.rebuild_rows();
        if let Some(i) = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::Group { name, .. } if *name == group))
        {
            if self.collapsed.contains(&group) {
                self.selected = i;
            }
        }
    }

    fn toggle_favorite(&mut self) {
        let Some(name) = self.selected_server().map(|s| s.name.clone()) else {
            return;
        };
        let mut cfg = self.config().clone();
        let fav = match cfg.get_mut(&name) {
            Some(s) => {
                s.favorite = !s.favorite;
                s.favorite
            }
            None => return,
        };
        match self.save(cfg, None) {
            Ok(()) => {
                self.select_server(&name);
                self.info(if fav {
                    format!("★ {name}")
                } else {
                    format!("{name} is no longer a favorite")
                });
            }
            Err(e) => self.popup("Save failed", &e),
        }
    }

    fn ask_import(&mut self) {
        use crate::core::sshconfig;
        let dir = sshconfig::ssh_dir();
        let report = match sshconfig::import_file(&dir.join("config"), &dir, self.config()) {
            Ok(r) => r,
            Err(e) => return self.popup("Import from ~/.ssh/config", &e),
        };
        if report.added.is_empty() {
            let skipped = report.existing.len() + report.skipped.len();
            return self.info(format!("Nothing new to import ({skipped} skipped)"));
        }
        let names: Vec<&str> = report.added.iter().map(|s| s.name.as_str()).collect();
        let mut message = format!(
            "Import {} server(s) from ~/.ssh/config?\n{}",
            names.len(),
            names.join(", ")
        );
        if !report.existing.is_empty() {
            message.push_str(&format!(
                "\nAlready present: {}",
                report.existing.join(", ")
            ));
        }
        self.modal = Some(Modal::Confirm {
            message,
            action: ConfirmAction::Import(report.added),
        });
    }

    fn ask_delete(&mut self) {
        let Some(name) = self.selected_server().map(|s| s.name.clone()) else {
            return;
        };
        let deps = config::dependents(self.config(), &name);
        let (message, force) = if deps.is_empty() {
            (format!("Delete server \"{name}\"?"), false)
        } else {
            (
                format!(
                    "\"{name}\" is a jump host for {}.\nDelete it and remove those references?",
                    deps.join(", ")
                ),
                true,
            )
        };
        self.modal = Some(Modal::Confirm {
            message,
            action: ConfirmAction::Delete { name, force },
        });
    }

    fn connect(&mut self, with_forwards: bool) -> Option<Action> {
        let server = self.selected_server()?.clone();
        let argv = match launcher::connect_argv(self.config(), &self.paths, &server, with_forwards)
        {
            Ok(a) => a,
            Err(e) => {
                self.popup("Cannot connect", &e);
                return None;
            }
        };
        match launcher::target(self.config().settings.launch, LaunchMode::Auto) {
            Ok(Target::Here) => {
                if let Err(e) = State::update(&self.paths, |s| s.record_connect(&server.name)) {
                    tracing::warn!("state not saved: {e}");
                }
                Some(Action::Exec(argv))
            }
            Ok(Target::Tab(env)) => {
                let tx = self.tx.clone();
                let paths = self.paths.clone();
                let name = server.name.clone();
                tokio::task::spawn_blocking(move || {
                    let result =
                        launcher::open_tab(env, &name, &argv, &paths).map_err(|e| e.to_string());
                    let _ = tx.send(Event::Msg(Msg::TabOpened { name, result }));
                });
                None
            }
            Err(e) => {
                self.popup("Cannot connect", &e);
                None
            }
        }
    }

    // ----- transfers -------------------------------------------------------

    fn start_transfer(&mut self, mut t: Box<crate::tui::views::transfer::TransferView>) {
        use crate::tui::views::transfer::Step;
        let srcs = vec![t.source.value().trim().to_string()];
        let dest = t.dest.value().trim().to_string();
        let plan = self
            .config()
            .get(&t.server)
            .ok_or_else(|| resolve::not_found_error(self.config(), &t.server))
            .and_then(|s| {
                crate::core::transfer::plan(self.config(), &self.paths, s, t.dir, &srcs, &dest)
            });
        match plan {
            Err(e) => t.step = Step::Done(Err(e.to_string())),
            Ok(plan) => {
                t.step = Step::Running;
                t.started = Instant::now();
                let cancel = t.cancel.clone();
                let tx = self.tx.clone();
                let job = move || {
                    let ptx = tx.clone();
                    let result = crate::core::transfer::run(&plan, cancel, |p| {
                        let _ = ptx.send(Event::Msg(Msg::TransferProgress(p)));
                    });
                    let _ = tx.send(Event::Msg(Msg::TransferDone(
                        result.map_err(|e| e.to_string()),
                    )));
                };
                match tokio::runtime::Handle::try_current() {
                    Ok(h) => {
                        h.spawn_blocking(job);
                    }
                    Err(_) => job(),
                }
            }
        }
        self.modal = Some(Modal::Transfer(t));
    }

    // ----- file browser ----------------------------------------------------

    fn open_files(&mut self) {
        use crate::tui::views::file_browser::FileBrowser;
        let Some(name) = self.selected_server().map(|s| s.name.clone()) else {
            return;
        };
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "/".into());
        self.screen = Screen::Files(Box::new(FileBrowser::new(&name, cwd)));
        self.refresh_pane(crate::tui::views::file_browser::Side::Local, None);
        self.list_remote(String::new(), None);
    }

    /// Request a remote listing of `path` (empty = home) in the background.
    fn list_remote(&mut self, path: String, keep: Option<String>) {
        let Screen::Files(b) = &mut self.screen else {
            return;
        };
        b.remote.loading = true;
        b.remote.error = None;
        let server_name = b.server.clone();
        let Some(server) = self.store.config.get(&server_name).cloned() else {
            return;
        };
        let (tx, cfg, paths) = (self.tx.clone(), self.config().clone(), self.paths.clone());
        let job = move || {
            let result = crate::core::remote_fs::list_remote(&cfg, &paths, &server, &path)
                .map_err(|e| e.to_string());
            let _ = tx.send(Event::Msg(Msg::RemoteListing {
                server: server_name,
                keep,
                result,
            }));
        };
        match tokio::runtime::Handle::try_current() {
            Ok(h) => {
                h.spawn_blocking(job);
            }
            Err(_) => job(),
        }
    }

    /// Re-list one pane, keeping the highlighted entry (or `keep`).
    fn refresh_pane(&mut self, side: crate::tui::views::file_browser::Side, keep: Option<String>) {
        use crate::tui::views::file_browser::Side;
        let Screen::Files(b) = &mut self.screen else {
            return;
        };
        let keep = keep.or_else(|| b.pane(side).current(b.show_hidden).map(|e| e.name.clone()));
        match side {
            Side::Local => {
                let path = b.local.path.clone();
                match crate::core::remote_fs::list_local(std::path::Path::new(&path)) {
                    Ok(entries) => b.local.set_entries(path, entries, keep.as_deref()),
                    Err(e) => b.local.error = Some(e.to_string()),
                }
            }
            Side::Remote => {
                let path = b.remote.path.clone();
                self.list_remote(path, keep);
            }
        }
    }

    fn run_files(&mut self, cmd: Cmd) -> Option<Action> {
        use crate::tui::views::file_browser::{join, parent, FileBrowser, Side};
        let Screen::Files(b) = &mut self.screen else {
            return None;
        };
        let side = b.active;
        let hidden = b.show_hidden;
        match cmd {
            Cmd::Back => self.screen = Screen::Main,
            Cmd::Help => self.modal = Some(Modal::Help),
            Cmd::Quit => return Some(Action::Quit),
            Cmd::Down => {
                let p = b.pane_mut(side);
                p.selected += 1;
                p.clamp(hidden);
            }
            Cmd::Up => {
                let p = b.pane_mut(side);
                p.selected = p.selected.saturating_sub(1);
            }
            Cmd::SwitchPane => b.active = FileBrowser::other(side),
            Cmd::ToggleHidden => {
                b.show_hidden = !b.show_hidden;
                let show = b.show_hidden;
                b.local.clamp(show);
                b.remote.clamp(show);
            }
            Cmd::Mark => {
                let p = b.pane_mut(side);
                if let Some(name) = p.current(hidden).map(|e| e.name.clone()) {
                    if !p.marked.remove(&name) {
                        p.marked.insert(name);
                    }
                    p.selected += 1;
                    p.clamp(hidden);
                }
            }
            Cmd::Open | Cmd::ParentDir => {
                let p = b.pane(side);
                if p.loading {
                    return None;
                }
                let (target, keep) = if cmd == Cmd::Open {
                    match p.current(hidden) {
                        Some(e) if e.is_dir() || e.kind == crate::core::remote_fs::Kind::Link => {
                            (join(&p.path, &e.name), None)
                        }
                        _ => return None,
                    }
                } else {
                    let name = p
                        .path
                        .trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .map(String::from);
                    (parent(&p.path), name)
                };
                match side {
                    Side::Local => {
                        if std::path::Path::new(&target).is_dir() {
                            b.local.path = target;
                            self.refresh_pane(Side::Local, keep);
                        }
                    }
                    Side::Remote => self.list_remote(target, keep),
                }
            }
            Cmd::Refresh => self.refresh_pane(side, None),
            Cmd::CopyMarked => self.copy_marked(),
            _ => {}
        }
        None
    }

    fn copy_marked(&mut self) {
        use crate::core::sshcmd::Direction;
        use crate::tui::views::file_browser::{FileBrowser, Side};
        let Screen::Files(b) = &mut self.screen else {
            return;
        };
        if b.copying.is_some() {
            return self.warn("A copy is already running");
        }
        let from = b.active;
        let to = FileBrowser::other(from);
        let srcs = b.pane(from).selection(b.show_hidden);
        let dest = b.pane(to).path.clone();
        if srcs.is_empty() || dest.is_empty() {
            return;
        }
        let dir = if from == Side::Local {
            Direction::Send
        } else {
            Direction::Get
        };
        let server_name = b.server.clone();
        let Some(server) = self.store.config.get(&server_name).cloned() else {
            return;
        };
        let plan = match crate::core::transfer::plan(
            self.config(),
            &self.paths,
            &server,
            dir,
            &srcs,
            &dest,
        ) {
            Ok(p) => p,
            Err(e) => return self.popup("Copy failed", &e),
        };
        if let Screen::Files(b) = &mut self.screen {
            b.copying = Some((to, None));
        }
        let tx = self.tx.clone();
        let job = move || {
            let ptx = tx.clone();
            let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let result = crate::core::transfer::run(&plan, cancel, |p| {
                let _ = ptx.send(Event::Msg(Msg::TransferProgress(p)));
            });
            let _ = tx.send(Event::Msg(Msg::TransferDone(
                result.map_err(|e| e.to_string()),
            )));
        };
        match tokio::runtime::Handle::try_current() {
            Ok(h) => {
                h.spawn_blocking(job);
            }
            Err(_) => job(),
        }
    }

    // ----- tunnels ---------------------------------------------------------

    fn open_tunnels(&mut self, server: Option<String>) {
        self.refresh_tunnels();
        self.screen = Screen::Tunnels {
            server,
            selected: 0,
        };
    }

    pub fn refresh_tunnels(&mut self) {
        self.last_tunnel_refresh = Instant::now();
        match crate::core::tunnel::refresh(&self.paths) {
            Ok(s) => self.state = s,
            Err(e) => tracing::warn!("tunnel refresh: {e}"),
        }
    }

    /// Rows for the tunnels screen.
    pub fn tunnel_rows(&self) -> Vec<TunnelRow> {
        let Screen::Tunnels { server, .. } = &self.screen else {
            return Vec::new();
        };
        self.order
            .iter()
            .filter(|n| server.as_ref().is_none_or(|s| s == *n))
            .filter_map(|n| self.config().get(n))
            .flat_map(|s| {
                s.forward.iter().map(|f| TunnelRow {
                    server: s.name.clone(),
                    forward: f.clone(),
                    id: crate::core::tunnel::tunnel_id(&s.name, &f.name),
                })
            })
            .collect()
    }

    pub fn running_tunnel(&self, id: &str) -> Option<&crate::core::state::TunnelRecord> {
        self.state.tunnels.iter().find(|t| t.id == id)
    }

    fn run_tunnels(&mut self, cmd: Cmd) -> Option<Action> {
        let rows = self.tunnel_rows();
        let Screen::Tunnels { server, selected } = &mut self.screen else {
            return None;
        };
        let current = rows.get(*selected).cloned();
        let server_for_add = server
            .clone()
            .or(current.as_ref().map(|r| r.server.clone()));
        match cmd {
            Cmd::Down => *selected = (*selected + 1).min(rows.len().saturating_sub(1)),
            Cmd::Up => *selected = selected.saturating_sub(1),
            Cmd::Back => self.screen = Screen::Main,
            Cmd::Help => self.modal = Some(Modal::Help),
            Cmd::Quit => return Some(Action::Quit),
            Cmd::ForwardAdd => match server_for_add {
                Some(s) => {
                    self.modal = Some(Modal::Prompt {
                        title: format!("Add forward to {s}: name -L|-R|-D spec [auto]"),
                        input: TextInput::default(),
                        action: PromptAction::AddForward(s),
                        error: None,
                    })
                }
                None => self.warn("Select a server first"),
            },
            Cmd::ForwardDelete => {
                if let Some(r) = current {
                    self.modal = Some(Modal::Confirm {
                        message: format!("Delete forward {}?", r.id),
                        action: ConfirmAction::DeleteForward {
                            server: r.server,
                            name: r.forward.name,
                        },
                    });
                }
            }
            Cmd::TunnelToggle => {
                if let Some(r) = current {
                    self.toggle_tunnel(r);
                }
            }
            _ => {}
        }
        None
    }

    fn toggle_tunnel(&mut self, row: TunnelRow) {
        if self.tunnel_busy.contains(&row.id) {
            return;
        }
        let up = self.running_tunnel(&row.id).is_none();
        let Some(server) = self.config().get(&row.server).cloned() else {
            return;
        };
        self.tunnel_busy.insert(row.id.clone());
        let (tx, paths, cfg) = (self.tx.clone(), self.paths.clone(), self.config().clone());
        let id = row.id.clone();
        let job = move || {
            let result = if up {
                crate::core::tunnel::start(&cfg, &paths, &server, &row.forward).map(|_| ())
            } else {
                crate::core::tunnel::stop(&paths, &row.id)
            };
            let _ = tx.send(Event::Msg(Msg::Tunnel {
                id,
                up,
                result: result.map_err(|e| e.to_string()),
            }));
        };
        match tokio::runtime::Handle::try_current() {
            Ok(h) => {
                h.spawn_blocking(job);
            }
            Err(_) => job(),
        }
    }

    /// Run reachability checks in the background; results arrive as `Msg::Ping`.
    /// `full` checks jumped servers through their chain instead of just the first hop.
    pub fn start_ping(&mut self, names: Vec<String>, full: bool) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let names: Vec<String> = names
            .into_iter()
            .filter(|n| !self.checking.contains(n))
            .collect();
        if names.is_empty() {
            return;
        }
        let ctx = if full {
            match crate::core::sshcmd::ensure_ssh_config(self.config(), &self.paths) {
                Ok(c) => Some(c),
                Err(e) => {
                    self.warn(format!("ping: {e}"));
                    None
                }
            }
        } else {
            None
        };
        self.checking.extend(names.iter().cloned());
        let tx = self.tx.clone();
        let cfg = std::sync::Arc::new(self.config().clone());
        handle.spawn(crate::core::ping::check_many(
            cfg,
            ctx,
            names,
            full,
            move |name, outcome| {
                let _ = tx.send(Event::Msg(Msg::Ping { name, outcome }));
            },
        ));
    }

    fn maybe_auto_ping(&mut self) {
        let interval = self.config().settings.ping_interval;
        if interval == 0 {
            return;
        }
        let due = self
            .last_auto_ping
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(interval));
        if due && tokio::runtime::Handle::try_current().is_ok() {
            self.last_auto_ping = Some(Instant::now());
            let all = self.order.clone();
            self.start_ping(all, false);
        }
    }

    /// Ctrl+C anywhere quits, even inside dialogs.
    pub fn is_force_quit(key: &KeyEvent) -> bool {
        key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
    }
}

/// Parse `name -L|-R|-D spec [auto]` from the add-forward prompt.
pub fn parse_forward_input(value: &str) -> Result<crate::core::model::Forward, Error> {
    use crate::core::model::{Forward, ForwardKind};
    let parts: Vec<&str> = value.split_whitespace().collect();
    let usage = || Error::other("expected: name -L|-R|-D spec [auto]");
    let (name, flag, spec) = match parts.as_slice() {
        [n, f, s] | [n, f, s, _] => (*n, *f, *s),
        _ => return Err(usage()),
    };
    let kind = match flag {
        "-L" | "L" => ForwardKind::Local,
        "-R" | "R" => ForwardKind::Remote,
        "-D" | "D" => ForwardKind::Socks,
        _ => return Err(usage()),
    };
    let mut f = Forward::parse_spec(name, kind, spec).map_err(Error::Other)?;
    f.auto = matches!(parts.get(3), Some(&"auto") | Some(&"--auto"));
    Ok(f)
}
