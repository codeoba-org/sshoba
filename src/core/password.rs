//! Saved passwords, kept in the OS keychain (macOS Keychain, Linux Secret Service).
//!
//! Passwords are never written to servers.toml or any sshoba file. They are keyed by
//! `user@host`, the same text ssh puts in its password prompt, so a password is only
//! ever given to the exact destination it was saved for. ssh gets it through
//! `SSH_ASKPASS`: sshoba writes a tiny script that calls `sshoba askpass <prompt>`.

use crate::core::error::{Error, Result};
use crate::core::fsutil::{private_dir, write_atomic};
use crate::core::model::{Config, Server};
use crate::core::paths::Paths;
use crate::core::resolve;
use crate::core::sshcmd::shell_quote;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

const SERVICE: &str = "sshoba";
/// Set for background ssh runs: the askpass helper must never open the terminal.
const NO_TTY_VAR: &str = "SSHOBA_ASKPASS_NOTTY";

/// The keychain account for a server: `user@host`. Without a configured user ssh logs in
/// as the local user, so `$USER` is used.
pub fn key_for(server: &Server) -> Option<String> {
    let user = server
        .user
        .clone()
        .or_else(|| std::env::var("USER").ok().filter(|u| !u.is_empty()))?;
    Some(format!("{user}@{}", server.host))
}

fn entry(key: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, key).map_err(keychain_err)
}

fn keychain_err(e: keyring::Error) -> Error {
    Error::other(format!("keychain: {e}"))
}

/// The saved password for `key`, if any. Keychain errors count as "none".
pub fn get(key: &str) -> Option<String> {
    match entry(key).ok()?.get_password() {
        Ok(p) => Some(p),
        Err(keyring::Error::NoEntry) => None,
        Err(e) => {
            tracing::debug!("keychain lookup for {key} failed: {e}");
            None
        }
    }
}

pub fn set(key: &str, password: &str) -> Result<()> {
    entry(key)?.set_password(password).map_err(keychain_err)
}

/// Delete the saved password. Returns false if there was none.
pub fn delete(key: &str) -> Result<bool> {
    match entry(key)?.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(keychain_err(e)),
    }
}

/// Whether `server` or any hop on its jump chain has a saved password.
pub fn chain_has_saved(cfg: &Config, server: &Server) -> bool {
    let hops = resolve::chain(cfg, server).unwrap_or_default();
    std::iter::once(server)
        .chain(hops.iter().filter_map(|h| cfg.get(h)))
        .filter_map(key_for)
        .any(|k| get(&k).is_some())
}

/// Environment that makes ssh ask sshoba for passwords. With `tty = false` the helper
/// never prompts on the terminal (for detached tunnels).
pub fn askpass_env(paths: &Paths, tty: bool) -> Result<Vec<(String, String)>> {
    let script = ensure_script(paths)?;
    let mut env = vec![
        ("SSH_ASKPASS".to_string(), script.display().to_string()),
        ("SSH_ASKPASS_REQUIRE".to_string(), "force".to_string()),
    ];
    if !tty {
        env.push((NO_TTY_VAR.to_string(), "1".to_string()));
    }
    Ok(env)
}

/// ssh runs `SSH_ASKPASS` with the prompt as its only argument, so it must be a single
/// program: a script that forwards to `sshoba askpass`.
fn ensure_script(paths: &Paths) -> Result<PathBuf> {
    let exe = std::env::current_exe().map_err(|e| Error::io("locate sshoba binary", e))?;
    private_dir(&paths.state_dir)?;
    let path = paths.state_dir.join("askpass");
    let text = format!(
        "#!/bin/sh\nexec {} askpass \"$1\"\n",
        shell_quote(&exe.display().to_string())
    );
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        write_atomic(&path, text.as_bytes(), 0o700, false)?;
    }
    Ok(path)
}

/// The `user@host` an ssh password prompt is for. Handles password authentication
/// (`user@host's password: `) and keyboard-interactive (`(user@host) Password: `).
pub fn parse_prompt(prompt: &str) -> Option<String> {
    let prompt = prompt.trim_end();
    let dest = if let Some(rest) = prompt.strip_prefix('(') {
        let (dest, text) = rest.split_once(") ")?;
        if !text.to_ascii_lowercase().contains("password") {
            return None;
        }
        dest
    } else {
        prompt.strip_suffix("'s password:")?
    };
    let (user, host) = dest.rsplit_once('@')?;
    (!user.is_empty() && !host.is_empty()).then(|| format!("{user}@{host}"))
}

/// `sshoba askpass <prompt>`: print the saved password for the prompt's destination.
/// If there is none, or ssh asks again because it was rejected, ask on the terminal
/// like ssh would. Returns the exit code (non-zero tells ssh the prompt was cancelled).
pub fn run_askpass(prompt: &str, paths: &Paths) -> i32 {
    let tty = std::env::var_os(NO_TTY_VAR).is_none();
    let mut rejected = false;
    if let Some(key) = parse_prompt(prompt) {
        let marker = marker_path(paths);
        rejected = std::fs::read_to_string(&marker).is_ok_and(|k| k == key);
        if !rejected {
            if let Some(pw) = get(&key) {
                // Remember that this ssh process got the saved password, so a second
                // prompt (wrong password) goes to the user instead of looping.
                let _ = write_atomic(&marker, key.as_bytes(), 0o600, false);
                println!("{pw}");
                return 0;
            }
        }
    }
    if !tty {
        return 1;
    }
    let echo = prompt.contains("(yes/no");
    let note = if rejected {
        "sshoba: the saved password was rejected (update it with `sshoba password set`)\n"
    } else {
        ""
    };
    match read_tty(&format!("{note}{prompt}"), echo) {
        Ok(answer) => {
            println!("{answer}");
            0
        }
        Err(_) => 1,
    }
}

/// One marker per ssh process (our parent), cleaned up after a day.
fn marker_path(paths: &Paths) -> PathBuf {
    let dir = paths.state_dir.join("askpass.d");
    let _ = private_dir(&dir);
    prune_old(&dir);
    dir.join(std::os::unix::process::parent_id().to_string())
}

fn prune_old(dir: &Path) {
    let day = std::time::Duration::from_secs(24 * 3600);
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().unwrap_or_default() > day);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Ask on the controlling terminal, without echo unless `echo`.
pub fn read_tty(prompt: &str, echo: bool) -> std::io::Result<String> {
    use nix::sys::termios::{tcgetattr, tcsetattr, LocalFlags, SetArg};
    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")?;
    tty.write_all(prompt.as_bytes())?;
    tty.flush()?;
    let saved = if echo {
        None
    } else {
        let old = tcgetattr(&tty)?;
        let mut quiet = old.clone();
        quiet.local_flags.remove(LocalFlags::ECHO);
        tcsetattr(&tty, SetArg::TCSAFLUSH, &quiet)?;
        Some(old)
    };
    let mut line = String::new();
    let read = BufReader::new(&tty).read_line(&mut line);
    if let Some(old) = saved {
        let _ = tcsetattr(&tty, SetArg::TCSAFLUSH, &old);
        let _ = tty.write_all(b"\n");
    }
    if read? == 0 {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts() {
        assert_eq!(
            parse_prompt("deploy@db.example.com's password: ").as_deref(),
            Some("deploy@db.example.com")
        );
        assert_eq!(
            parse_prompt("(root@10.0.0.5) Password: ").as_deref(),
            Some("root@10.0.0.5")
        );
        assert_eq!(
            parse_prompt("u@fe80::1's password: ").as_deref(),
            Some("u@fe80::1")
        );
        assert_eq!(parse_prompt("(root@h) Verification code: "), None);
        assert_eq!(parse_prompt("Enter passphrase for key '/k': "), None);
        assert_eq!(
            parse_prompt("Are you sure you want to continue connecting (yes/no)? "),
            None
        );
    }

    #[test]
    fn key_uses_configured_user() {
        let s = Server {
            name: "db".into(),
            host: "db.example.com".into(),
            user: Some("deploy".into()),
            ..Default::default()
        };
        assert_eq!(key_for(&s).as_deref(), Some("deploy@db.example.com"));
    }

    #[test]
    fn script_calls_askpass() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: dir.path().into(),
            config_file: dir.path().join("servers.toml"),
            state_dir: dir.path().join("state"),
        };
        let env = askpass_env(&paths, false).unwrap();
        let script = std::fs::read_to_string(&env[0].1).unwrap();
        assert!(script.starts_with("#!/bin/sh\nexec "));
        assert!(script.ends_with(" askpass \"$1\"\n"));
        assert!(env
            .iter()
            .any(|(k, v)| k == "SSH_ASKPASS_REQUIRE" && v == "force"));
        assert!(env.iter().any(|(k, _)| k == NO_TTY_VAR));
    }
}
