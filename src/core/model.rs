//! Data model for servers.toml (section 4.2).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub server: Vec<Server>,
}

impl Config {
    pub fn get(&self, name: &str) -> Option<&Server> {
        self.server.iter().find(|s| s.name == name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Server> {
        self.server.iter_mut().find(|s| s.name == name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.server.iter().map(|s| s.name.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LaunchSetting {
    #[default]
    Auto,
    Tmux,
    Zellij,
    Current,
    Kitty,
    Wezterm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferTool {
    #[default]
    Auto,
    Rsync,
    Scp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub launch: LaunchSetting,
    pub stay_open: bool,
    pub ping_interval: u64,
    pub ping_timeout_ms: u64,
    pub transfer_tool: TransferTool,
    pub control_master: bool,
    pub ssh_bin: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            launch: LaunchSetting::Auto,
            stay_open: true,
            ping_interval: 30,
            ping_timeout_ms: 1500,
            transfer_tool: TransferTool::Auto,
            control_master: true,
            ssh_bin: "ssh".into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub name: String,
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jump: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub favorite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forward: Vec<Forward>,
}

impl Server {
    /// `user@host:port`, omitting parts that are not set.
    pub fn destination(&self) -> String {
        let mut s = String::new();
        if let Some(u) = &self.user {
            s.push_str(u);
            s.push('@');
        }
        if self.host.contains(':') {
            s.push_str(&format!("[{}]", self.host));
        } else {
            s.push_str(&self.host);
        }
        if let Some(p) = self.port {
            s.push_str(&format!(":{p}"));
        }
        s
    }

    /// The SSH port, falling back to 22.
    pub fn port_or_default(&self) -> u16 {
        self.port.unwrap_or(22)
    }

    pub fn forward(&self, name: &str) -> Option<&Forward> {
        self.forward.iter().find(|f| f.name == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForwardKind {
    Local,
    Remote,
    Socks,
}

impl ForwardKind {
    pub fn flag(self) -> &'static str {
        match self {
            ForwardKind::Local => "-L",
            ForwardKind::Remote => "-R",
            ForwardKind::Socks => "-D",
        }
    }

    pub fn letter(self) -> &'static str {
        match self {
            ForwardKind::Local => "L",
            ForwardKind::Remote => "R",
            ForwardKind::Socks => "D",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forward {
    pub name: String,
    pub kind: ForwardKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
    pub local_port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto: bool,
}

impl Forward {
    /// The ssh forward argument without the flag, e.g. `127.0.0.1:5432:localhost:5432`.
    pub fn spec(&self) -> String {
        let mut s = String::new();
        if let Some(b) = &self.bind {
            s.push_str(b);
            s.push(':');
        }
        s.push_str(&self.local_port.to_string());
        if let Some(r) = &self.remote {
            s.push(':');
            s.push_str(r);
        }
        s
    }

    /// `["-L", "<spec>"]`.
    pub fn args(&self) -> [String; 2] {
        [self.kind.flag().to_string(), self.spec()]
    }

    /// One-line human summary, e.g. `L 5432→localhost:5432`.
    pub fn summary(&self) -> String {
        match &self.remote {
            Some(r) => format!("{} {}→{}", self.kind.letter(), self.local_port, r),
            None => format!("{} {}", self.kind.letter(), self.local_port),
        }
    }

    /// Parse an ssh-style spec for `kind` (`5432:localhost:5432`,
    /// `127.0.0.1:8080:web:80`, `1080`, `127.0.0.1:1080`).
    pub fn parse_spec(name: &str, kind: ForwardKind, spec: &str) -> Result<Forward, String> {
        let parts = split_spec(spec);
        let bad = || format!("invalid {} forward spec \"{spec}\"", kind.flag());
        let port = |p: &str| -> Result<u16, String> {
            p.parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| format!("invalid port \"{p}\" in \"{spec}\""))
        };
        let (bind, local_port, remote) = match (kind, parts.as_slice()) {
            (ForwardKind::Socks, [p]) => (None, port(p)?, None),
            (ForwardKind::Socks, [b, p]) => (Some(b.clone()), port(p)?, None),
            (ForwardKind::Socks, _) => return Err(bad()),
            (_, [p, h, rp]) => (None, port(p)?, Some(join_host_port(h, rp))),
            (_, [b, p, h, rp]) => (Some(b.clone()), port(p)?, Some(join_host_port(h, rp))),
            _ => return Err(bad()),
        };
        if let Some(r) = &remote {
            parse_host_port(r)?;
        }
        Ok(Forward {
            name: name.to_string(),
            kind,
            bind,
            local_port,
            remote,
            auto: false,
        })
    }
}

/// Split on `:` while keeping `[ipv6]` groups intact (brackets removed).
fn split_spec(spec: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_bracket = false;
    for c in spec.chars() {
        match c {
            '[' => in_bracket = true,
            ']' => in_bracket = false,
            ':' if !in_bracket => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn join_host_port(host: &str, port: &str) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Parse `host:port` (or `[v6]:port`) into its parts.
pub fn parse_host_port(s: &str) -> Result<(String, u16), String> {
    let err = || format!("\"{s}\" is not host:port");
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (h, p) = rest.split_once("]:").ok_or_else(err)?;
        (h, p)
    } else {
        s.rsplit_once(':').ok_or_else(err)?
    };
    if host.is_empty() || host.contains(':') && !s.starts_with('[') {
        return Err(err());
    }
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(err)?;
    Ok((host.to_string(), port))
}

/// Parse `[user@]host[:port]` as given to `shhoba add`.
pub fn parse_destination(s: &str) -> Result<(Option<String>, String, Option<u16>), String> {
    let (user, rest) = match s.rsplit_once('@') {
        Some((u, r)) if !u.is_empty() => (Some(u.to_string()), r),
        Some(_) => return Err(format!("\"{s}\": empty user")),
        None => (None, s),
    };
    let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
        match inner.split_once(']') {
            Some((h, "")) => (h.to_string(), None),
            Some((h, p)) => {
                let p = p
                    .strip_prefix(':')
                    .ok_or_else(|| format!("\"{s}\": bad port"))?;
                (h.to_string(), Some(p))
            }
            None => return Err(format!("\"{s}\": missing ]")),
        }
    } else if rest.matches(':').count() == 1 {
        let (h, p) = rest.split_once(':').unwrap_or((rest, ""));
        (h.to_string(), Some(p))
    } else {
        (rest.to_string(), None)
    };
    if host.is_empty() {
        return Err(format!("\"{s}\": empty host"));
    }
    let port = match port {
        Some(p) => Some(
            p.parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| format!("\"{s}\": invalid port \"{p}\""))?,
        ),
        None => None,
    };
    Ok((user, host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations() {
        assert_eq!(
            parse_destination("ata@1.2.3.4:2222").unwrap(),
            (Some("ata".into()), "1.2.3.4".into(), Some(2222))
        );
        assert_eq!(
            parse_destination("example.com").unwrap(),
            (None, "example.com".into(), None)
        );
        assert_eq!(
            parse_destination("root@[::1]:22").unwrap(),
            (Some("root".into()), "::1".into(), Some(22))
        );
        assert_eq!(
            parse_destination("fe80::1").unwrap(),
            (None, "fe80::1".into(), None)
        );
        assert!(parse_destination("a@").is_err());
        assert!(parse_destination("h:99999").is_err());
    }

    #[test]
    fn host_port() {
        assert_eq!(
            parse_host_port("localhost:5432").unwrap(),
            ("localhost".into(), 5432)
        );
        assert_eq!(parse_host_port("[::1]:80").unwrap(), ("::1".into(), 80));
        assert!(parse_host_port("localhost").is_err());
        assert!(parse_host_port(":80").is_err());
    }

    #[test]
    fn forward_specs() {
        let f = Forward::parse_spec("pg", ForwardKind::Local, "5432:localhost:5432").unwrap();
        assert_eq!(f.bind, None);
        assert_eq!(f.local_port, 5432);
        assert_eq!(f.remote.as_deref(), Some("localhost:5432"));
        assert_eq!(f.spec(), "5432:localhost:5432");

        let f = Forward::parse_spec("w", ForwardKind::Remote, "127.0.0.1:8080:web:80").unwrap();
        assert_eq!(f.bind.as_deref(), Some("127.0.0.1"));
        assert_eq!(f.args(), ["-R".to_string(), "127.0.0.1:8080:web:80".into()]);

        let f = Forward::parse_spec("s", ForwardKind::Socks, "1080").unwrap();
        assert_eq!(f.remote, None);
        assert_eq!(f.summary(), "D 1080");

        assert!(Forward::parse_spec("x", ForwardKind::Socks, "1:2:3").is_err());
        assert!(Forward::parse_spec("x", ForwardKind::Local, "5432").is_err());
        assert!(Forward::parse_spec("x", ForwardKind::Local, "0:h:1").is_err());
    }
}
