//! One function per subcommand. Each loads what it needs and calls core.

use crate::cli::output::{self, paint, print_json, ServerJson, ShowJson, Style, Table};
use crate::cli::{Cli, Command, ConfigCommand, Shell};
use crate::core::config::{self, ConfigStore};
use crate::core::error::{suggest, Error, Result};
use crate::core::launcher::{self, LaunchMode, Target};
use crate::core::model::{parse_destination, Server};
use crate::core::paths::Paths;
use crate::core::resolve::{self, Lookup};
use crate::core::sshcmd;
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
