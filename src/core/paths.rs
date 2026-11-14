//! Config and state file locations (section 4.1).

use std::path::{Path, PathBuf};

/// Resolved locations of every file shhoba reads or writes.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub state_dir: PathBuf,
}

fn home() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

impl Paths {
    /// Resolve paths from the environment. `config_override` is the `--config` flag.
    pub fn resolve(config_override: Option<&Path>) -> Self {
        let config_dir = env_dir("SHHOBA_CONFIG_DIR")
            .or_else(|| env_dir("XDG_CONFIG_HOME").map(|d| d.join("shhoba")))
            .unwrap_or_else(|| home().join(".config").join("shhoba"));
        let config_file = config_override
            .map(Path::to_path_buf)
            .unwrap_or_else(|| config_dir.join("servers.toml"));
        let state_dir = env_dir("XDG_STATE_HOME")
            .map(|d| d.join("shhoba"))
            .unwrap_or_else(|| home().join(".local").join("state").join("shhoba"));
        Paths {
            config_dir,
            config_file,
            state_dir,
        }
    }

    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }
    pub fn log_file(&self) -> PathBuf {
        self.state_dir.join("shhoba.log")
    }
    pub fn ssh_config(&self) -> PathBuf {
        self.state_dir.join("ssh_config")
    }
    pub fn ctl_dir(&self) -> PathBuf {
        self.state_dir.join("ctl")
    }
    pub fn tunnels_dir(&self) -> PathBuf {
        self.state_dir.join("tunnels")
    }
    pub fn zellij_layouts_dir(&self) -> PathBuf {
        self.state_dir.join("zellij-layouts")
    }
}
