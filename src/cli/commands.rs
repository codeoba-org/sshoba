//! One function per subcommand. Each loads what it needs and calls core.

use crate::cli::output::{self, paint, print_json, ServerJson, ShowJson, Style, Table};
use crate::cli::{Cli, Command, ConfigCommand, FwdCommand, ImportCommand, Shell, TunnelCommand};
use crate::core::config::{self, ConfigStore};
use crate::core::error::{suggest, Error, Result};
use crate::core::launcher::{self, LaunchMode, Target};
use crate::core::model::{parse_destination, Server};
use crate::core::paths::Paths;
use crate::core::resolve::{self, Lookup};
use crate::core::sshcmd::{self, Direction};
use crate::core::state::State;
use std::io::{IsTerminal, Write};

/// Shared context for every command.
pub struct Ctx {
    pub paths: Paths,
    pub quiet: bool,
}

impl Ctx {
    pub fn store(&self) -> Result<ConfigStore> {
        ConfigStore::load(&self.paths.config_file)
    }

    /// Print a non-essential message (suppressed by --quiet).
    fn note(&self, msg: impl AsRef<str>) {
        if !self.quiet {
            println!("{}", msg.as_ref());
        }
    }
}

/// Run a subcommand and return the process exit code.
pub fn run(cli: Cli) -> i32 {
    let ctx = Ctx {
        paths: Paths::resolve(cli.config.as_deref()),
        quiet: cli.quiet,
    };
    let Some(cmd) = cli.command else {
        return crate::tui::run(&ctx.paths);
    };
    match dispatch(&ctx, cmd) {
        Ok(code) => code,
        Err(e) => {
            tracing::warn!("command failed: {e}");
            eprintln!("{} {e}", paint("error:", Style::Red));
            e.exit_code()
        }
    }
}

fn dispatch(ctx: &Ctx, cmd: Command) -> Result<i32> {
    match cmd {
        Command::Connect {
            query,
            here,
            tab,
            with_forwards,
        } => {
            let mode = if here {
                LaunchMode::Here
            } else if tab {
                LaunchMode::Tab
            } else {
                LaunchMode::Auto
            };
            connect(ctx, &query, mode, with_forwards)
        }
        Command::List {
            tag,
            group,
            fav,
            json,
        } => list(ctx, tag, group, fav, json),
        Command::Add {
            name,
            destination,
            key,
            jump,
            tags,
            group,
            note,
        } => {
            let (user, host, port) = parse_destination(&destination).map_err(Error::Other)?;
            let server = Server {
                name,
                host,
                user,
                port,
                key,
                jump,
                group,
                tags,
                note,
                ..Default::default()
            };
            add(ctx, server)
        }
        Command::Edit { name } => edit(ctx, &name),
        Command::Set { name, field, value } => set(ctx, &name, &field, &value),
        Command::Rename { old, new } => rename(ctx, &old, &new),
        Command::Rm { name, force, yes } => rm(ctx, &name, force, yes),
        Command::Show { name, json } => show(ctx, &name, json),
        Command::Cmd { name } => print_cmd(ctx, &name),
        Command::Config {
            command: ConfigCommand::Path,
        } => {
            println!("config: {}", ctx.paths.config_file.display());
            println!("state:  {}", ctx.paths.state_dir.display());
            println!("log:    {}", ctx.paths.log_file().display());
            Ok(0)
        }
        Command::Completions { shell } => completions(shell),
        Command::Ping { queries, all, json } => ping(ctx, &queries, all, json),
        Command::Fwd { command } => fwd(ctx, command),
        Command::Send { server, paths } => transfer_cmd(ctx, &server, Direction::Send, paths),
        Command::Get { server, paths } => transfer_cmd(ctx, &server, Direction::Get, paths),
        Command::Tunnel { command } => tunnel(ctx, command),
        Command::Import {
            command: ImportCommand::SshConfig { file, dry_run },
        } => import_ssh_config(ctx, file, dry_run),
    }
}

fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Resolve a `<query>` per section 5.2, with an inline picker on a TTY.
pub fn resolve_query<'a>(store: &'a ConfigStore, query: &str) -> Result<&'a Server> {
    let cfg = &store.config;
    match resolve::lookup(cfg, query) {
        Lookup::One(s) => Ok(s),
        Lookup::None => Err(Error::not_found(
            format!("server \"{query}\""),
            suggest(query, cfg.names()),
        )),
        Lookup::Many(hits) => {
            let names: Vec<String> = hits.iter().map(|s| s.name.clone()).collect();
            if interactive() {
                let items: Vec<(String, String)> = hits
                    .iter()
                    .map(|s| (s.name.clone(), s.destination()))
                    .collect();
                match crate::tui::picker::pick(
                    &format!("Several servers match \"{query}\""),
                    &items,
                )? {
                    Some(i) => Ok(hits[i]),
                    None => Err(Error::other("cancelled")),
                }
            } else {
                Err(Error::Ambiguous {
                    query: query.to_string(),
                    candidates: names,
                })
            }
        }
    }
}

fn connect(ctx: &Ctx, query: &str, mode: LaunchMode, with_forwards: bool) -> Result<i32> {
    let store = ctx.store()?;
    let server = resolve_query(&store, query)?;
    let argv = launcher::connect_argv(&store.config, &ctx.paths, server, with_forwards)?;
    match launcher::target(store.config.settings.launch, mode)? {
        Target::Tab(env) => {
            launcher::open_tab(env, &server.name, &argv, &ctx.paths)?;
            State::update(&ctx.paths, |s| s.record_connect(&server.name))?;
            ctx.note(format!(
                "Opened {} in a new {} tab",
                server.name,
                env.label()
            ));
            Ok(0)
        }
        Target::Here => {
            if !std::io::stdin().is_terminal() {
                return Err(Error::other(
                    "connecting in the current terminal needs a TTY (try --tab inside tmux/Zellij)",
                ));
            }
            State::update(&ctx.paths, |s| s.record_connect(&server.name))?;
            tracing::info!("exec {}", sshcmd::shell_join(&argv));
            Err(launcher::exec(&argv))
        }
    }
}

fn list(
    ctx: &Ctx,
    tag: Option<String>,
    group: Option<String>,
    fav: bool,
    json: bool,
) -> Result<i32> {
    let store = ctx.store()?;
    let state = State::load(&ctx.paths);
    let cfg = &store.config;
    let servers: Vec<&Server> = cfg
        .server
        .iter()
        .filter(|s| tag.as_ref().is_none_or(|t| s.tags.contains(t)))
        .filter(|s| group.is_none() || s.group == group)
        .filter(|s| !fav || s.favorite)
        .collect();
    if json {
        let out: Vec<ServerJson> = servers.iter().map(|s| ServerJson::new(s, &state)).collect();
        print_json(&out);
        return Ok(0);
    }
    if servers.is_empty() {
        ctx.note("No servers. Add one with `shhoba add <name> [user@]host[:port]`.");
        return Ok(0);
    }
    let mut t = Table::new(&["NAME", "DESTINATION", "GROUP", "TAGS", "VIA"]);
    for s in servers {
        let name = if s.favorite {
            format!("{} {}", s.name, paint("★", Style::Yellow))
        } else {
            s.name.clone()
        };
        let via = resolve::chain(cfg, s).unwrap_or_default().join(" › ");
        t.row(vec![
            name,
            s.destination(),
            s.group.clone().unwrap_or_default(),
            s.tags.join(","),
            paint(&via, Style::Dim),
        ]);
    }
    print!("{}", t.render());
    Ok(0)
}

fn add(ctx: &Ctx, server: Server) -> Result<i32> {
    let mut store = ctx.store()?;
    let mut cfg = store.config.clone();
    let name = server.name.clone();
    if !config::valid_name(&name) {
        return Err(Error::other(format!(
            "invalid name \"{name}\": use 1-64 letters, digits, '.', '_' or '-'"
        )));
    }
    config::add_server(&mut cfg, server)?;
    store.save(cfg, None)?;
    ctx.note(format!("Added {name}"));
    Ok(0)
}

fn edit(ctx: &Ctx, name: &str) -> Result<i32> {
    let store = ctx.store()?;
    resolve::require(&store.config, name)?;
    let changed = config::edit_in_editor(&store.path, Some(name), &mut |e| {
        eprintln!("{} {e}", paint("invalid config:", Style::Red));
        confirm("Reopen the editor? [Y/n] ", true)
    })?;
    ctx.note(if changed { "Saved." } else { "No changes." });
    Ok(0)
}

fn set(ctx: &Ctx, name: &str, field: &str, value: &str) -> Result<i32> {
    let mut store = ctx.store()?;
    let mut cfg = store.config.clone();
    let renamed = config::set_field(&mut cfg, name, field, value)?;
    store.save(cfg, renamed.as_deref().map(|new| (name, new)))?;
    if let Some(new) = &renamed {
        State::update(&ctx.paths, |s| s.rename_server(name, new))?;
    }
    ctx.note(format!("Updated {name}.{field}"));
    Ok(0)
}

fn rename(ctx: &Ctx, old: &str, new: &str) -> Result<i32> {
    let mut store = ctx.store()?;
    let mut cfg = store.config.clone();
    config::rename_server(&mut cfg, old, new)?;
    store.save(cfg, Some((old, new)))?;
    State::update(&ctx.paths, |s| s.rename_server(old, new))?;
    ctx.note(format!("Renamed {old} → {new}"));
    Ok(0)
}

/// Ask a yes/no question on stderr. Returns `default` on empty input.
fn confirm(prompt: &str, default: bool) -> bool {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    match line.trim().to_lowercase().as_str() {
        "" => default,
        "y" | "yes" => true,
        _ => false,
    }
}

fn rm(ctx: &Ctx, name: &str, force: bool, yes: bool) -> Result<i32> {
    let mut store = ctx.store()?;
    let mut cfg = store.config.clone();
    config::remove_server(&mut cfg, name, force)?;
    if !yes {
        if !interactive() {
            return Err(Error::other(
                "refusing to delete without confirmation; pass -y",
            ));
        }
        if !confirm(&format!("Delete server \"{name}\"? [y/N] "), false) {
            ctx.note("Cancelled.");
            return Ok(1);
        }
    }
    store.save(cfg, None)?;
    State::update(&ctx.paths, |s| s.remove_server(name))?;
    ctx.note(format!("Deleted {name}"));
    Ok(0)
}

fn show(ctx: &Ctx, name: &str, json: bool) -> Result<i32> {
    let store = ctx.store()?;
    let cfg = &store.config;
    let s = resolve::require(cfg, name)?;
    let state = State::load(&ctx.paths);
    let sctx = sshcmd::ensure_ssh_config(cfg, &ctx.paths)?;
    let exact = sshcmd::shell_join(&sshcmd::ssh_connect(&sctx, s, &[]));
    let human = sshcmd::human_command(s, cfg);
    let chain = resolve::chain(cfg, s).unwrap_or_default();
    if json {
        print_json(&ShowJson {
            server: ServerJson::new(s, &state),
            chain,
            command: exact,
            human_command: human,
        });
        return Ok(0);
    }
    let label = |l: &str| paint(&format!("{l:<10}"), Style::Dim);
    let star = if s.favorite { " ★" } else { "" };
    println!("{}{star}", paint(&s.name, Style::Bold));
    println!("{}{}", label("host"), s.destination());
    if !chain.is_empty() {
        println!("{}{}", label("route"), output::chain_display(cfg, s));
    }
    if let Some(k) = &s.key {
        println!("{}{}", label("key"), k.display());
    }
    if let Some(g) = &s.group {
        println!("{}{g}", label("group"));
    }
    if !s.tags.is_empty() {
        println!("{}{}", label("tags"), s.tags.join(", "));
    }
    if let Some(n) = &s.note {
        println!("{}{n}", label("note"));
    }
    if !s.extra_args.is_empty() {
        println!("{}{}", label("args"), sshcmd::shell_join(&s.extra_args));
    }
    for f in &s.forward {
        let auto = if f.auto { " (auto)" } else { "" };
        println!("{}{}  {}{auto}", label("forward"), f.name, f.summary());
    }
    if let Some(st) = state.servers.get(&s.name) {
        if let Some(t) = st.last_connected {
            println!(
                "{}{} ({} connections)",
                label("last"),
                output::ago(t),
                st.connect_count
            );
        }
    }
    println!("{}{human}", label("command"));
    println!("{}{exact}", label("exact"));
    Ok(0)
}

fn print_cmd(ctx: &Ctx, name: &str) -> Result<i32> {
    let store = ctx.store()?;
    let s = resolve::require(&store.config, name)?;
    let sctx = sshcmd::ensure_ssh_config(&store.config, &ctx.paths)?;
    println!(
        "{}",
        sshcmd::shell_join(&sshcmd::ssh_connect(&sctx, s, &[]))
    );
    Ok(0)
}

fn completions(shell: Shell) -> Result<i32> {
    use clap_complete::env::{Bash, EnvCompleter, Fish, Zsh};
    let completer: &dyn EnvCompleter = match shell {
        Shell::Bash => &Bash,
        Shell::Zsh => &Zsh,
        Shell::Fish => &Fish,
    };
    let mut out = std::io::stdout();
    completer
        .write_registration("COMPLETE", "shhoba", "shhoba", "shhoba", &mut out)
        .map_err(|e| Error::io("write completions", e))?;
    Ok(0)
}

/// Run async core work from a synchronous command.
fn block_on<F: std::future::Future>(f: F) -> Result<F::Output> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::io("start async runtime", e))?;
    Ok(rt.block_on(f))
}

fn ping(ctx: &Ctx, queries: &[String], all: bool, json: bool) -> Result<i32> {
    use crate::core::ping;
    use crate::core::state::PingStatus;
    let store = ctx.store()?;
    let names: Vec<String> = if all || queries.is_empty() {
        store.config.server.iter().map(|s| s.name.clone()).collect()
    } else {
        let mut v = Vec::new();
        for q in queries {
            let n = resolve_query(&store, q)?.name.clone();
            if !v.contains(&n) {
                v.push(n);
            }
        }
        v
    };
    let sctx = sshcmd::ensure_ssh_config(&store.config, &ctx.paths)?;
    let results = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = results.clone();
    block_on(ping::check_many(
        std::sync::Arc::new(store.config.clone()),
        Some(sctx),
        names.clone(),
        true,
        move |name, outcome| sink.lock().expect("lock").push((name, outcome.record())),
    ))?;
    let mut results = std::mem::take(&mut *results.lock().expect("lock"));
    results.sort_by_key(|(n, _)| names.iter().position(|x| x == n));
    State::update(&ctx.paths, |s| {
        for (n, r) in &results {
            s.ping.insert(n.clone(), r.clone());
        }
    })?;
    let any_down = results.iter().any(|(_, r)| r.status == PingStatus::Down);
    if json {
        #[derive(serde::Serialize)]
        struct Row<'a> {
            name: &'a str,
            #[serde(flatten)]
            record: &'a crate::core::state::PingRecord,
        }
        let rows: Vec<Row> = results
            .iter()
            .map(|(n, r)| Row { name: n, record: r })
            .collect();
        print_json(&rows);
    } else {
        let mut t = Table::new(&["NAME", "STATUS", "LATENCY", "DETAIL"]);
        for (n, r) in &results {
            let status = match r.status {
                PingStatus::Up => paint("up", Style::Green),
                PingStatus::OpenNoSsh => paint("open (no ssh)", Style::Yellow),
                PingStatus::Down => paint("down", Style::Red),
                PingStatus::Unknown => paint("unknown", Style::Dim),
            };
            let latency = r.latency_ms.map(|l| format!("{l}ms")).unwrap_or_default();
            let mut detail = r.reason.clone().unwrap_or_default();
            if let Some(v) = &r.via {
                detail = format!("via {v} {detail}").trim().to_string();
            }
            t.row(vec![n.clone(), status, latency, detail]);
        }
        print!("{}", t.render());
    }
    Ok(if any_down { 5 } else { 0 })
}

fn import_ssh_config(ctx: &Ctx, file: Option<std::path::PathBuf>, dry_run: bool) -> Result<i32> {
    use crate::core::sshconfig;
    let mut store = ctx.store()?;
    // Relative Includes resolve against ~/.ssh, or the given file's directory.
    let (file, dir) = match file {
        Some(f) => {
            let dir = f.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            (f, dir)
        }
        None => (sshconfig::ssh_dir().join("config"), sshconfig::ssh_dir()),
    };
    let report = sshconfig::import_file(&file, &dir, &store.config)?;
    let verb = if dry_run { "Would add" } else { "Added" };
    for s in &report.added {
        let via = if s.jump.is_empty() {
            String::new()
        } else {
            format!("  via {}", s.jump.join(" › "))
        };
        println!(
            "{verb} {}  {}{via}",
            paint(&s.name, Style::Bold),
            s.destination()
        );
    }
    for n in &report.existing {
        println!(
            "{}",
            paint(&format!("Skipped {n}: already exists"), Style::Dim)
        );
    }
    for (n, why) in &report.skipped {
        println!("{}", paint(&format!("Skipped {n}: {why}"), Style::Yellow));
    }
    if report.added.is_empty() {
        ctx.note("Nothing to import.");
        return Ok(0);
    }
    if !dry_run {
        let mut cfg = store.config.clone();
        cfg.server.extend(report.added);
        store.save(cfg, None)?;
    }
    Ok(0)
}

fn fwd(ctx: &Ctx, command: FwdCommand) -> Result<i32> {
    use crate::core::model::{Forward, ForwardKind};
    use crate::core::tunnel;
    match command {
        FwdCommand::Add {
            server,
            name,
            local,
            remote,
            dynamic,
            auto,
        } => {
            let (kind, spec) = match (local, remote, dynamic) {
                (Some(s), _, _) => (ForwardKind::Local, s),
                (_, Some(s), _) => (ForwardKind::Remote, s),
                (_, _, Some(s)) => (ForwardKind::Socks, s),
                _ => return Err(Error::other("one of -L, -R or -D is required")),
            };
            let mut f = Forward::parse_spec(&name, kind, &spec).map_err(Error::Other)?;
            f.auto = auto;
            let summary = f.summary();
            let mut store = ctx.store()?;
            let mut cfg = store.config.clone();
            config::add_forward(&mut cfg, &server, f)?;
            store.save(cfg, None)?;
            ctx.note(format!("Added forward {server}/{name}: {summary}"));
            Ok(0)
        }
        FwdCommand::Ls { server, json } => {
            let store = ctx.store()?;
            let servers: Vec<&Server> = match &server {
                Some(n) => vec![resolve::require(&store.config, n)?],
                None => store.config.server.iter().collect(),
            };
            let state = tunnel::refresh(&ctx.paths)?;
            let running = |s: &str, f: &str| {
                state
                    .tunnels
                    .iter()
                    .any(|t| t.id == tunnel::tunnel_id(s, f))
            };
            if json {
                #[derive(serde::Serialize)]
                struct Row {
                    server: String,
                    #[serde(flatten)]
                    forward: output::ForwardJson,
                    running: bool,
                }
                let rows: Vec<Row> = servers
                    .iter()
                    .flat_map(|s| {
                        s.forward.iter().map(|f| Row {
                            server: s.name.clone(),
                            forward: f.into(),
                            running: running(&s.name, &f.name),
                        })
                    })
                    .collect();
                print_json(&rows);
                return Ok(0);
            }
            let mut t = Table::new(&["SERVER", "NAME", "KIND", "SPEC", "AUTO", "TUNNEL"]);
            let mut any = false;
            for s in &servers {
                for f in &s.forward {
                    any = true;
                    t.row(vec![
                        s.name.clone(),
                        f.name.clone(),
                        format!("{:?}", f.kind).to_lowercase(),
                        format!("{} {}", f.kind.flag(), f.spec()),
                        if f.auto { "yes".into() } else { String::new() },
                        if running(&s.name, &f.name) {
                            paint("up", Style::Green)
                        } else {
                            paint("down", Style::Dim)
                        },
                    ]);
                }
            }
            if any {
                print!("{}", t.render());
            } else {
                ctx.note("No forwards. Add one with `shhoba fwd add <server> <name> -L <spec>`.");
            }
            Ok(0)
        }
        FwdCommand::Rm { server, name } => {
            let mut store = ctx.store()?;
            let mut cfg = store.config.clone();
            config::remove_forward(&mut cfg, &server, &name)?;
            let id = tunnel::tunnel_id(&server, &name);
            if tunnel::refresh(&ctx.paths)?
                .tunnels
                .iter()
                .any(|t| t.id == id)
            {
                tunnel::stop(&ctx.paths, &id)?;
                ctx.note(format!("Stopped tunnel {id}"));
            }
            store.save(cfg, None)?;
            ctx.note(format!("Deleted forward {server}/{name}"));
            Ok(0)
        }
    }
}

/// Look up the named forwards of a server (all of them if `names` is empty).
fn pick_forwards<'a>(
    s: &'a Server,
    names: &[String],
) -> Result<Vec<&'a crate::core::model::Forward>> {
    if names.is_empty() {
        if s.forward.is_empty() {
            return Err(Error::not_found(
                format!(
                    "forwards for \"{}\" (add one with `shhoba fwd add`)",
                    s.name
                ),
                None,
            ));
        }
        return Ok(s.forward.iter().collect());
    }
    names
        .iter()
        .map(|n| {
            s.forward(n).ok_or_else(|| {
                Error::not_found(
                    format!("forward \"{}/{n}\"", s.name),
                    suggest(n, s.forward.iter().map(|f| f.name.as_str())),
                )
            })
        })
        .collect()
}

fn tunnel(ctx: &Ctx, command: TunnelCommand) -> Result<i32> {
    use crate::core::tunnel;
    match command {
        TunnelCommand::Up {
            server,
            forwards,
            all,
        } => {
            let store = ctx.store()?;
            let s = resolve::require(&store.config, &server)?;
            let names = if all { Vec::new() } else { forwards };
            let mut failed = None;
            for f in pick_forwards(s, &names)? {
                match tunnel::start(&store.config, &ctx.paths, s, f) {
                    Ok(rec) => ctx.note(format!(
                        "Tunnel {} up: {} (pid {})",
                        rec.id,
                        f.summary(),
                        rec.pid
                    )),
                    Err(e) => {
                        eprintln!("{} {}/{}: {e}", paint("error:", Style::Red), s.name, f.name);
                        failed = Some(e.exit_code());
                    }
                }
            }
            Ok(failed.unwrap_or(0))
        }
        TunnelCommand::Down {
            server,
            forwards,
            all,
        } => {
            let state = tunnel::refresh(&ctx.paths)?;
            let ids: Vec<String> = match &server {
                None => state.tunnels.iter().map(|t| t.id.clone()).collect(),
                Some(srv) if forwards.is_empty() => state
                    .tunnels
                    .iter()
                    .filter(|t| t.server == *srv)
                    .map(|t| t.id.clone())
                    .collect(),
                Some(srv) => forwards.iter().map(|f| tunnel::tunnel_id(srv, f)).collect(),
            };
            let _ = all;
            if ids.is_empty() {
                ctx.note("No running tunnels.");
                return Ok(0);
            }
            for id in ids {
                tunnel::stop(&ctx.paths, &id)?;
                ctx.note(format!("Stopped {id}"));
            }
            Ok(0)
        }
        TunnelCommand::Ls { json } => {
            let state = tunnel::refresh(&ctx.paths)?;
            if json {
                print_json(&state.tunnels);
                return Ok(0);
            }
            if state.tunnels.is_empty() {
                ctx.note("No running tunnels.");
                return Ok(0);
            }
            let mut t = Table::new(&["ID", "SPEC", "PID", "UPTIME", "STATUS"]);
            for r in &state.tunnels {
                t.row(vec![
                    r.id.clone(),
                    r.spec.clone(),
                    r.pid.to_string(),
                    tunnel::uptime(r),
                    paint("up", Style::Green),
                ]);
            }
            print!("{}", t.render());
            Ok(0)
        }
    }
}

/// Split CLI paths into sources and destination (see `send`/`get` help).
pub fn split_transfer_paths(dir: Direction, mut paths: Vec<String>) -> (Vec<String>, String) {
    let default = match dir {
        Direction::Send => "~",
        Direction::Get => ".",
    };
    if paths.len() >= 2 {
        let last = paths.last().expect("len >= 2");
        let is_dest = match dir {
            Direction::Send => std::fs::symlink_metadata(last).is_err(),
            Direction::Get => std::path::Path::new(last).is_dir(),
        };
        if is_dest {
            let dest = paths.pop().expect("len >= 2");
            return (paths, dest);
        }
    }
    (paths, default.to_string())
}

fn transfer_cmd(ctx: &Ctx, server: &str, dir: Direction, paths: Vec<String>) -> Result<i32> {
    use crate::core::transfer;
    let store = ctx.store()?;
    let s = resolve::require(&store.config, server)?;
    let (srcs, dest) = split_transfer_paths(dir, paths);
    let plan = transfer::plan(&store.config, &ctx.paths, s, dir, &srcs, &dest)?;
    tracing::info!("transfer: {}", sshcmd::shell_join(&plan.argv));
    let show = std::io::stderr().is_terminal() && !ctx.quiet;
    let (from, to) = match dir {
        Direction::Send => (srcs.join(" "), format!("{}:{dest}", s.name)),
        Direction::Get => (format!("{}:{}", s.name, srcs.join(" ")), dest.clone()),
    };
    if show {
        let tool = match plan.tool {
            transfer::Tool::Rsync(_) => "rsync",
            transfer::Tool::Scp => "scp",
        };
        eprintln!("{from} → {to} ({tool})");
    }
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    transfer::run(&plan, cancel, |p| {
        if show {
            let file = p
                .file
                .as_deref()
                .map(|f| format!("  {f}"))
                .unwrap_or_default();
            eprint!(
                "\r\x1b[2K{:>3}%  {:>12} B  {}  ETA {}{file}",
                p.percent, p.bytes, p.rate, p.eta
            );
            let _ = std::io::stderr().flush();
        }
    })?;
    if show {
        eprintln!("\r\x1b[2KDone.");
    }
    Ok(0)
}
