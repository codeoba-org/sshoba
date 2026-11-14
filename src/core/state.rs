//! state.json: program-owned runtime data (section 4.5). Never fatal when corrupt.

use crate::core::error::Result;
use crate::core::fsutil::write_atomic;
use crate::core::paths::Paths;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub version: u32,
    pub servers: BTreeMap<String, ServerState>,
    pub ping: BTreeMap<String, PingRecord>,
    pub tunnels: Vec<TunnelRecord>,
}

impl Default for State {
    fn default() -> Self {
        State {
            version: STATE_VERSION,
            servers: BTreeMap::new(),
            ping: BTreeMap::new(),
            tunnels: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerState {
    pub last_connected: Option<DateTime<Utc>>,
    pub connect_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PingStatus {
    /// SSH banner received.
    Up,
    /// Port open but no SSH banner.
    OpenNoSsh,
    Down,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PingRecord {
    pub status: PingStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// Why it is down/unknown, e.g. "refused", "timeout", "dns", "auth".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// For jumped servers checked through their first hop: that hop's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TunnelRecord {
    pub id: String,
    pub server: String,
    pub forward: String,
    pub pid: i32,
    pub started_at: DateTime<Utc>,
    pub spec: String,
}

impl State {
    /// Load state.json; a missing or corrupt file yields empty state (with a logged warning).
    pub fn load(paths: &Paths) -> State {
        let path = paths.state_file();
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<State>(&text) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("ignoring corrupt {}: {e}", path.display());
                    State::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => {
                tracing::warn!("cannot read {}: {e}", path.display());
                State::default()
            }
        }
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let mut text = serde_json::to_string_pretty(self).expect("state serializes");
        text.push('\n');
        write_atomic(&paths.state_file(), text.as_bytes(), 0o600, false)
    }

    /// Load the latest state from disk, apply `f`, and save. Use this for every change so
    /// concurrent sshoba processes do not overwrite each other's updates.
    pub fn update<T>(paths: &Paths, f: impl FnOnce(&mut State) -> T) -> Result<(State, T)> {
        let mut s = State::load(paths);
        let out = f(&mut s);
        s.save(paths)?;
        Ok((s, out))
    }

    pub fn record_connect(&mut self, name: &str) {
        let e = self.servers.entry(name.to_string()).or_default();
        e.last_connected = Some(Utc::now());
        e.connect_count += 1;
    }

    /// Move every record keyed by `old` to `new`.
    pub fn rename_server(&mut self, old: &str, new: &str) {
        if let Some(v) = self.servers.remove(old) {
            self.servers.insert(new.to_string(), v);
        }
        if let Some(v) = self.ping.remove(old) {
            self.ping.insert(new.to_string(), v);
        }
        for t in &mut self.tunnels {
            if t.server == old {
                t.server = new.to_string();
                t.id = format!("{new}/{}", t.forward);
            }
        }
    }

    pub fn remove_server(&mut self, name: &str) {
        self.servers.remove(name);
        self.ping.remove(name);
    }

    pub fn last_connected(&self, name: &str) -> Option<DateTime<Utc>> {
        self.servers.get(name).and_then(|s| s.last_connected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(dir: &std::path::Path) -> Paths {
        Paths {
            config_dir: dir.into(),
            config_file: dir.join("servers.toml"),
            state_dir: dir.join("state"),
        }
    }

    #[test]
    fn missing_and_corrupt_are_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        assert_eq!(State::load(&p), State::default());
        std::fs::create_dir_all(&p.state_dir).unwrap();
        std::fs::write(p.state_file(), "{not json").unwrap();
        assert_eq!(State::load(&p), State::default());
    }

    #[test]
    fn update_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        State::update(&p, |s| s.record_connect("web")).unwrap();
        State::update(&p, |s| s.record_connect("web")).unwrap();
        let s = State::load(&p);
        assert_eq!(s.servers["web"].connect_count, 2);
        State::update(&p, |s| s.rename_server("web", "web2")).unwrap();
        assert_eq!(State::load(&p).servers["web2"].connect_count, 2);
    }
}
