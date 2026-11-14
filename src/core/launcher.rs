//! Decide where a session opens and open it (section 7.2).

use crate::core::error::{Error, Result};
use crate::core::fsutil::{private_dir, write_atomic};
use crate::core::model::{Config, LaunchSetting, Server};
use crate::core::paths::Paths;
use crate::core::sshcmd::{self, shell_join};
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Env {
    Tmux,
    Zellij,
    Plain,
}

impl Env {
    pub fn label(self) -> &'static str {
        match self {
            Env::Tmux => "tmux",
            Env::Zellij => "zellij",
            Env::Plain => "terminal",
        }
    }
}

/// How the user asked to connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    /// Follow the `launch` setting.
    Auto,
    /// Force the current terminal (`--here`).
    Here,
    /// Force a new multiplexer tab (`--tab`).
    Tab,
}

/// Where the session will open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Tab(Env),
    Here,
}

fn env_set(var: &str) -> bool {
    std::env::var_os(var).is_some_and(|v| !v.is_empty())
}

/// The multiplexer we are running inside, if any. tmux wins when both are set.
pub fn detect_multiplexer() -> Option<Env> {
    if env_set("TMUX") {
        Some(Env::Tmux)
    } else if env_set("ZELLIJ") {
        Some(Env::Zellij)
    } else {
        None
    }
}

/// The environment the `launch` setting selects.
pub fn detect_env(launch: LaunchSetting) -> Env {
    match launch {
        LaunchSetting::Tmux => Env::Tmux,
        LaunchSetting::Zellij => Env::Zellij,
        LaunchSetting::Current => Env::Plain,
        LaunchSetting::Auto => detect_multiplexer().unwrap_or(Env::Plain),
    }
}

pub fn target(launch: LaunchSetting, mode: LaunchMode) -> Result<Target> {
    match mode {
        LaunchMode::Here => Ok(Target::Here),
        LaunchMode::Tab => match detect_env(launch) {
            Env::Plain => detect_multiplexer().map(Target::Tab).ok_or_else(|| {
                Error::other("--tab needs tmux or Zellij, but neither is running here")
            }),
            env => Ok(Target::Tab(env)),
        },
        LaunchMode::Auto => Ok(match detect_env(launch) {
            Env::Plain => Target::Here,
            env => Target::Tab(env),
        }),
    }
}

/// Find `bin` in PATH (or accept it as a path).
pub fn which(bin: &str) -> Option<PathBuf> {
    if bin.contains('/') {
        let p = PathBuf::from(bin);
        return p.is_file().then_some(p);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(bin))
            .find(|p| p.is_file())
    })
}

pub fn require_bin(bin: &str, hint: &str) -> Result<PathBuf> {
    which(bin).ok_or_else(|| Error::MissingBinary {
        bin: bin.to_string(),
        hint: hint.to_string(),
    })
}

/// The ssh argv for connecting to `server`, regenerating the ssh config first.
pub fn connect_argv(
    cfg: &Config,
    paths: &Paths,
    server: &Server,
    with_forwards: bool,
) -> Result<Vec<String>> {
    let ctx = sshcmd::ensure_ssh_config(cfg, paths)?;
    let fwds: Vec<_> = if with_forwards {
        server.forward.iter().filter(|f| f.auto).collect()
    } else {
        Vec::new()
    };
    Ok(sshcmd::ssh_connect(&ctx, server, &fwds))
}

pub fn tmux_args(name: &str, argv: &[String]) -> Vec<String> {
    let mut v: Vec<String> = ["new-window", "-n", name, "--"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    v.extend(argv.iter().cloned());
    v
}

fn kdl_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn zellij_layout(argv: &[String]) -> String {
    let (cmd, args) = argv.split_first().expect("argv is never empty");
    let args: Vec<String> = args.iter().map(|a| kdl_string(a)).collect();
    format!(
        "layout {{\n    pane size=1 borderless=true {{ plugin location=\"zellij:tab-bar\"; }}\n    pane command={} close_on_exit=true {{\n        args {}\n    }}\n    pane size=2 borderless=true {{ plugin location=\"zellij:status-bar\"; }}\n}}\n",
        kdl_string(cmd),
        args.join(" ")
    )
}

fn run_checked(bin: &str, args: &[String], hint: &str) -> Result<()> {
    require_bin(bin, hint)?;
    let out = Command::new(bin)
        .args(args)
        .output()
        .map_err(|e| Error::io(format!("run {bin}"), e))?;
    if out.status.success() {
        Ok(())
    } else {
        let mut full = vec![bin.to_string()];
        full.extend(args.iter().cloned());
        Err(Error::Command {
            command: shell_join(&full),
            stderr: last_lines(&String::from_utf8_lossy(&out.stderr), 5),
        })
    }
}

pub fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Open `argv` in a new tab/window of `env`, named `name`.
pub fn open_tab(env: Env, name: &str, argv: &[String], paths: &Paths) -> Result<()> {
    match env {
        Env::Tmux => run_checked(
            "tmux",
            &tmux_args(name, argv),
            "install tmux or set launch = \"current\"",
        ),
        Env::Zellij => {
            let dir = paths.zellij_layouts_dir();
            private_dir(&dir)?;
            let file = dir.join(format!("{name}.kdl"));
            write_atomic(&file, zellij_layout(argv).as_bytes(), 0o600, false)?;
            let args: Vec<String> = vec![
                "action".into(),
                "new-tab".into(),
                "--name".into(),
                name.into(),
                "--layout".into(),
                file.display().to_string(),
            ];
            run_checked(
                "zellij",
                &args,
                "install Zellij or set launch = \"current\"",
            )
        }
        Env::Plain => Err(Error::other("open_tab called without a multiplexer")),
    }
}

/// Replace this process with `argv`. Only returns on failure. The caller must restore
/// the terminal first.
pub fn exec(argv: &[String]) -> Error {
    use std::os::unix::process::CommandExt;
    let err = Command::new(&argv[0]).args(&argv[1..]).exec();
    if err.kind() == std::io::ErrorKind::NotFound {
        Error::MissingBinary {
            bin: argv[0].clone(),
            hint: "install OpenSSH or set ssh_bin in servers.toml".into(),
        }
    } else {
        Error::io(format!("exec {}", argv[0]), err)
    }
}

/// Log the Zellij version once (we target 0.40+).
pub fn log_zellij_version() {
    if let Ok(out) = Command::new("zellij").arg("--version").output() {
        tracing::info!(
            "zellij version: {}",
            String::from_utf8_lossy(&out.stdout).trim()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_argv() {
        let argv = vec![
            "ssh".into(),
            "-F".into(),
            "/x y/c".into(),
            "shhoba.db".into(),
        ];
        assert_eq!(
            tmux_args("db", &argv),
            [
                "new-window",
                "-n",
                "db",
                "--",
                "ssh",
                "-F",
                "/x y/c",
                "shhoba.db"
            ]
        );
    }

    #[test]
    fn zellij_kdl() {
        let argv = vec![
            "ssh".into(),
            "-F".into(),
            "/home/u/.local/state/shhoba/ssh_config".into(),
            "we\"ird\\arg".into(),
        ];
        insta::assert_snapshot!(zellij_layout(&argv));
    }

    #[test]
    fn targets() {
        assert_eq!(
            target(LaunchSetting::Tmux, LaunchMode::Auto).unwrap(),
            Target::Tab(Env::Tmux)
        );
        assert_eq!(
            target(LaunchSetting::Zellij, LaunchMode::Here).unwrap(),
            Target::Here
        );
        assert_eq!(
            target(LaunchSetting::Current, LaunchMode::Auto).unwrap(),
            Target::Here
        );
        assert_eq!(
            target(LaunchSetting::Zellij, LaunchMode::Tab).unwrap(),
            Target::Tab(Env::Zellij)
        );
    }
}
