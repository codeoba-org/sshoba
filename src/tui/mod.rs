//! Terminal UI front end.

pub mod picker;

use crate::core::paths::Paths;

/// Run the TUI; returns the process exit code.
pub fn run(_paths: &Paths) -> i32 {
    eprintln!("TUI not implemented yet");
    1
}
