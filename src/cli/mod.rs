//! Command-line front end.

use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "shhoba",
    version,
    about = "Manage, connect to and tunnel through SSH servers"
)]
pub struct Cli {
    /// Use this servers.toml instead of the default.
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Suppress non-essential output.
    #[arg(short, long, global = true)]
    pub quiet: bool,
}
