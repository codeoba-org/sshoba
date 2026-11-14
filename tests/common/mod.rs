//! Shared helpers: every test gets its own config and state dirs.
#![allow(dead_code)]

use assert_cmd::Command;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub struct Env {
    pub dir: TempDir,
}

impl Env {
    pub fn new() -> Self {
        Env {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn config_file(&self) -> PathBuf {
        self.path().join("cfg").join("servers.toml")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.path().join("state").join("shhoba")
    }

    pub fn write_config(&self, text: &str) {
        std::fs::create_dir_all(self.path().join("cfg")).unwrap();
        std::fs::write(self.config_file(), text).unwrap();
    }

    pub fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("shhoba").unwrap();
        c.env("SHHOBA_CONFIG_DIR", self.path().join("cfg"))
            .env("XDG_STATE_HOME", self.path().join("state"))
            .env("HOME", self.path().join("home"))
            .env("NO_COLOR", "1")
            .env_remove("TMUX")
            .env_remove("ZELLIJ")
            .env_remove("COMPLETE");
        c
    }
}
