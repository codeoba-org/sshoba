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
}

#[derive(Debug, Clone)]
pub enum PromptAction {
    Rename(String),
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
}

fn mtime(paths: &Paths) -> Option<SystemTime> {
    std::fs::metadata(&paths.config_file)
        .and_then(|m| m.modified())
        .ok()
}

impl App {
    pub fn new(paths: Paths, store: ConfigStore, tx: UnboundedSender<Event>) -> App {
        let state = State::load(&paths);
        let env = launcher::detect_env(store.config.settings.launch);
        let ping = state.ping.clone();
        let mut app = App {
            ping,
            fresh: HashSet::new(),
            checking: HashSet::new(),
            last_auto_ping: None,
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
                None
            }
            Event::Msg(msg) => self.on_msg(msg),
        }
    }

    fn on_paste(&mut self, s: &str) {
        match &mut self.modal {
            Some(Modal::Form(form)) => form.paste(s),
            Some(Modal::Prompt { input, .. }) => input.paste(s),
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
            Cmd::Add => self.modal = Some(Modal::Form(Box::new(ServerForm::new(None)))),
            Cmd::Edit => {
                if let Some(s) = self.selected_server() {
                    self.modal = Some(Modal::Form(Box::new(ServerForm::new(Some(s)))));
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
            Cmd::Tunnels
            | Cmd::AllTunnels
            | Cmd::Files
            | Cmd::Transfer
            | Cmd::CopyCommand
            | Cmd::Import => self.warn("Not available yet"),
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
