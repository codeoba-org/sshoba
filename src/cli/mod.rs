//! Command-line front end: clap definitions. Handlers live in [`commands`].

pub mod commands;
pub mod output;

use clap::{Parser, Subcommand, ValueEnum};
use clap_complete::engine::{ArgValueCandidates, CompletionCandidate};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "shhoba",
    version,
    about = "Manage, connect to and tunnel through SSH servers",
    long_about = "Manage, connect to and tunnel through SSH servers.\n\nRun without a subcommand to open the TUI."
)]
pub struct Cli {
    /// Use this servers.toml instead of the default.
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Suppress non-essential output.
    #[arg(short, long, global = true)]
    pub quiet: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

fn servers() -> ArgValueCandidates {
    ArgValueCandidates::new(complete::server_names)
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Connect to a server (new tab in tmux/Zellij, else this terminal).
    #[command(visible_alias = "c")]
    Connect {
        /// Server name or fuzzy query.
        #[arg(add = servers())]
        query: String,
        /// Connect in the current terminal.
        #[arg(long, conflicts_with = "tab")]
        here: bool,
        /// Open a new multiplexer tab.
        #[arg(long)]
        tab: bool,
        /// Add the server's `auto` forwards to the session.
        #[arg(long)]
        with_forwards: bool,
    },
    /// List servers.
    #[command(visible_alias = "ls")]
    List {
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        group: Option<String>,
        /// Only favorites.
        #[arg(long)]
        fav: bool,
        #[arg(long)]
        json: bool,
    },
    /// Add a server.
    Add {
        name: String,
        /// [user@]host[:port]
        destination: String,
        /// Private key file.
        #[arg(short = 'i', value_name = "KEY")]
        key: Option<PathBuf>,
        /// Jump hosts, comma-separated, in hop order.
        #[arg(short = 'J', value_delimiter = ',', value_name = "A,B", add = servers())]
        jump: Vec<String>,
        #[arg(long = "tag", value_name = "T")]
        tags: Vec<String>,
        #[arg(long, value_name = "G")]
        group: Option<String>,
        #[arg(long, value_name = "N")]
        note: Option<String>,
    },
    /// Edit servers.toml in $EDITOR at the given server.
    Edit {
        #[arg(add = servers())]
        name: String,
    },
    /// Change one field, e.g. `shhoba set db1 port 2222`. An empty value clears it.
    Set {
        #[arg(add = servers())]
        name: String,
        #[arg(value_parser = clap::builder::PossibleValuesParser::new(crate::core::config::SETTABLE_FIELDS))]
        field: String,
        value: String,
    },
    /// Rename a server and update jump references to it.
    Rename {
        #[arg(add = servers())]
        old: String,
        new: String,
    },
    /// Delete a server.
    Rm {
        #[arg(add = servers())]
        name: String,
        /// Also delete if other servers jump through it (removes those references).
        #[arg(long)]
        force: bool,
        /// Do not ask for confirmation.
        #[arg(short = 'y', long = "yes")]
        yes: bool,
    },
    /// Show one server with its resolved jump chain and ssh command.
    Show {
        #[arg(add = servers())]
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Print the exact ssh command for a server.
    Cmd {
        #[arg(add = servers())]
        name: String,
    },
    /// Check reachability (TCP connect + SSH banner). Jumped servers are checked through
    /// their jump chain.
    Ping {
        /// Servers to check (names or queries). Default: all.
        #[arg(add = servers())]
        queries: Vec<String>,
        /// Check every server.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Configuration helpers.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Print a shell completion script.
    Completions { shell: Shell },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print config and state paths.
    Path,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

/// Dynamic completion candidates.
pub mod complete {
    use super::CompletionCandidate;
    use crate::core::config::ConfigStore;
    use crate::core::paths::Paths;

    fn load() -> Option<ConfigStore> {
        ConfigStore::load(&Paths::resolve(None).config_file).ok()
    }

    pub fn server_names() -> Vec<CompletionCandidate> {
        load()
            .map(|s| {
                s.config
                    .server
                    .iter()
                    .map(|srv| {
                        CompletionCandidate::new(&srv.name).help(Some(srv.destination().into()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Forward names of the server named earlier on the command line (or of all servers).
    pub fn forward_names() -> Vec<CompletionCandidate> {
        let Some(store) = load() else {
            return Vec::new();
        };
        let words: Vec<String> = std::env::args().skip_while(|a| a != "--").collect();
        let server = words.iter().rev().find_map(|w| store.config.get(w));
        let servers: Vec<_> = match server {
            Some(s) => vec![s],
            None => store.config.server.iter().collect(),
        };
        let mut out: Vec<CompletionCandidate> = Vec::new();
        for s in servers {
            for f in &s.forward {
                out.push(CompletionCandidate::new(&f.name).help(Some(f.summary().into())));
            }
        }
        out
    }
}
