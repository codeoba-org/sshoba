//! Detached background tunnels (section 7.4).

use crate::core::error::{Error, Result};
use crate::core::fsutil::private_dir;
use crate::core::launcher::last_lines;
use crate::core::model::{Config, Forward, ForwardKind, Server};
use crate::core::paths::Paths;
use crate::core::sshcmd::{self, alias};
use crate::core::state::{State, TunnelRecord};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a new tunnel must stay alive to count as started.
pub const STARTUP_WAIT: Duration = Duration::from_secs(2);

pub fn tunnel_id(server: &str, forward: &str) -> String {
    format!("{server}/{forward}")
}

pub fn log_path(paths: &Paths, server: &str, forward: &str) -> PathBuf {
    paths.tunnels_dir().join(format!("{server}__{forward}.log"))
}

/// True if nothing is listening on `bind:port` (we can bind it ourselves).
pub fn port_free(bind: &str, port: u16) -> bool {
    std::net::TcpListener::bind((bind, port)).is_ok()
}

/// The full command line of a running process, if it exists.
fn cmdline(pid: i32) -> Option<String> {
    if cfg!(target_os = "linux") {
        let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        Some(
            String::from_utf8_lossy(&raw)
                .split('\0')
                .collect::<Vec<_>>()
                .join(" "),
        )
    } else {
        let out = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "command="])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}

/// Is the recorded process still our tunnel? Guards against PID reuse by checking that
/// its command line names the server alias and the forward spec.
pub fn is_alive(rec: &TunnelRecord) -> bool {
    if rec.pid <= 0 || kill(Pid::from_raw(rec.pid), None).is_err() {
        return false;
    }
    let spec = rec
        .spec
        .split_once(' ')
        .map_or(rec.spec.as_str(), |(_, s)| s);
    cmdline(rec.pid).is_some_and(|c| c.contains(&alias(&rec.server)) && c.contains(spec))
}

/// Drop records of tunnels that are no longer running; returns the fresh state.
pub fn refresh(paths: &Paths) -> Result<State> {
    let state = State::load(paths);
    if state.tunnels.iter().all(is_alive) {
        return Ok(state);
    }
    Ok(State::update(paths, |s| s.tunnels.retain(is_alive))?.0)
}

/// Start a detached tunnel for `fwd`. Returns once it has survived [`STARTUP_WAIT`].
pub fn start(cfg: &Config, paths: &Paths, server: &Server, fwd: &Forward) -> Result<TunnelRecord> {
    let id = tunnel_id(&server.name, &fwd.name);
    let state = refresh(paths)?;
    if let Some(t) = state.tunnels.iter().find(|t| t.id == id) {
        return Err(Error::other(format!(
            "{id} is already running (pid {})",
            t.pid
        )));
    }
    if matches!(fwd.kind, ForwardKind::Local | ForwardKind::Socks) {
        let bind = fwd.bind.as_deref().unwrap_or("127.0.0.1");
        let bind = if bind == "*" || bind.is_empty() {
            "0.0.0.0"
        } else {
            bind
        };
        if !port_free(bind, fwd.local_port) {
            return Err(Error::other(format!(
                "local port {} is already in use on {bind}",
                fwd.local_port
            )));
        }
    }
    crate::core::launcher::require_bin(&cfg.settings.ssh_bin, "install OpenSSH")?;
    let ctx = sshcmd::ensure_ssh_config(cfg, paths)?;
    let argv = sshcmd::ssh_tunnel(&ctx, server, fwd);
    private_dir(&paths.tunnels_dir())?;
    let log = log_path(paths, &server.name, &fwd.name);
    let log_file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&log)
            .map_err(|e| Error::io(format!("open {}", log.display()), e))?
    };
    let log_start = log_file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file));
    {
        use std::os::unix::process::CommandExt;
        // A new session detaches the tunnel from our terminal and process group, so it
        // survives the terminal closing.
        unsafe {
            cmd.pre_exec(|| {
                nix::unistd::setsid()
                    .map(|_| ())
                    .map_err(std::io::Error::from)
            });
        }
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| Error::io(format!("start {}", argv[0]), e))?;
    let started = Instant::now();
    while started.elapsed() < STARTUP_WAIT {
        if let Ok(Some(status)) = child.try_wait() {
            let text = std::fs::read(&log)
                .map(|b| {
                    String::from_utf8_lossy(&b[(log_start as usize).min(b.len())..]).into_owned()
                })
                .unwrap_or_default();
            return Err(Error::Command {
                command: sshcmd::shell_join(&argv),
                stderr: format!("tunnel exited ({status})\n{}", last_lines(&text, 5)),
            });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let pid = child.id() as i32;
    // Reap the child if it exits while this (possibly long-lived) process is running.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    let [flag, spec] = fwd.args();
    let rec = TunnelRecord {
        id,
        server: server.name.clone(),
        forward: fwd.name.clone(),
        pid,
        started_at: chrono::Utc::now(),
        spec: format!("{flag} {spec}"),
    };
    let r = rec.clone();
    State::update(paths, move |s| {
        s.tunnels.retain(|t| t.id != r.id);
        s.tunnels.push(r);
    })?;
    tracing::info!("tunnel {} started (pid {pid})", rec.id);
    Ok(rec)
}

/// Stop a tunnel: SIGTERM, wait up to 2 s, then SIGKILL. Removes it from state.
pub fn stop(paths: &Paths, id: &str) -> Result<()> {
    let state = State::load(paths);
    let rec = state
        .tunnels
        .iter()
        .find(|t| t.id == id)
        .cloned()
        .ok_or_else(|| Error::not_found(format!("running tunnel \"{id}\""), None))?;
    if is_alive(&rec) {
        let pid = Pid::from_raw(rec.pid);
        let _ = kill(pid, Signal::SIGTERM);
        let start = Instant::now();
        while is_alive(&rec) && start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(50));
        }
        if is_alive(&rec) {
            let _ = kill(pid, Signal::SIGKILL);
        }
    }
    State::update(paths, |s| s.tunnels.retain(|t| t.id != id))?;
    tracing::info!("tunnel {id} stopped");
    Ok(())
}

/// "3m", "2h 5m", "4d 1h".
pub fn uptime(rec: &TunnelRecord) -> String {
    let secs = (chrono::Utc::now() - rec.started_at).num_seconds().max(0);
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h {}m", secs / 3600, (secs % 3600) / 60),
        _ => format!("{}d {}h", secs / 86_400, (secs % 86_400) / 3600),
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
    fn busy_port_is_detected() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(!port_free("127.0.0.1", port));
        drop(l);
        assert!(port_free("127.0.0.1", port));
    }

    #[test]
    fn dead_and_reused_pids_are_not_alive() {
        let mut rec = TunnelRecord {
            id: "s/f".into(),
            server: "s".into(),
            forward: "f".into(),
            pid: std::process::id() as i32,
            started_at: chrono::Utc::now(),
            spec: "-L 5432:localhost:5432".into(),
        };
        // Our own pid exists but is not an ssh tunnel for shhoba.s.
        assert!(!is_alive(&rec));
        rec.pid = 0;
        assert!(!is_alive(&rec));
    }

    #[test]
    fn start_fails_on_busy_port_without_spawning() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let cfg = crate::core::config::parse(
            &format!(
                "[[server]]\nname=\"s\"\nhost=\"h\"\n[[server.forward]]\nname=\"f\"\nkind=\"local\"\nlocal_port={port}\nremote=\"localhost:1\"\n"
            ),
            "t",
        )
        .unwrap();
        let s = cfg.get("s").unwrap();
        let e = start(&cfg, &p, s, s.forward("f").unwrap()).unwrap_err();
        assert!(e.to_string().contains(&port.to_string()));
        assert!(State::load(&p).tunnels.is_empty());
    }

    #[test]
    fn failing_tunnel_reports_log() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        // A fake "ssh" that writes an error and exits.
        let fake = dir.path().join("fake-ssh");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho 'channel_setup_fwd_listener: cannot listen' >&2\nexit 255\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cfg = crate::core::config::parse(
            &format!(
                "[settings]\nssh_bin=\"{}\"\n[[server]]\nname=\"s\"\nhost=\"h\"\n[[server.forward]]\nname=\"f\"\nkind=\"socks\"\nlocal_port=1\n",
                fake.display()
            ),
            "t",
        )
        .unwrap();
        let s = cfg.get("s").unwrap();
        // Port 1 needs root to bind, so use a remote forward to skip the port check.
        let mut fwd = s.forward("f").unwrap().clone();
        fwd.kind = ForwardKind::Remote;
        fwd.remote = Some("localhost:80".into());
        let started = Instant::now();
        let e = start(&cfg, &p, s, &fwd).unwrap_err();
        assert!(started.elapsed() < STARTUP_WAIT + Duration::from_secs(1));
        assert!(e.to_string().contains("cannot listen"), "{e}");
    }

    #[test]
    fn survives_and_stops() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        // A fake "ssh" that stays up like a working tunnel.
        let fake = dir.path().join("fake-ssh");
        // A shell loop keeps the original argv (`fake-ssh -F ... shhoba.s`) visible.
        std::fs::write(&fake, "#!/bin/sh\nwhile true; do sleep 1; done\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cfg = crate::core::config::parse(
            &format!(
                "[settings]\nssh_bin=\"{}\"\n[[server]]\nname=\"s\"\nhost=\"h\"\n[[server.forward]]\nname=\"f\"\nkind=\"remote\"\nlocal_port=9000\nremote=\"localhost:9000\"\n",
                fake.display()
            ),
            "t",
        )
        .unwrap();
        let s = cfg.get("s").unwrap();
        let rec = start(&cfg, &p, s, s.forward("f").unwrap()).unwrap();
        assert_eq!(State::load(&p).tunnels.len(), 1);
        assert!(is_alive(&rec));
        assert_eq!(refresh(&p).unwrap().tunnels.len(), 1);
        assert!(start(&cfg, &p, s, s.forward("f").unwrap())
            .unwrap_err()
            .to_string()
            .contains("already running"));
        stop(&p, &rec.id).unwrap();
        assert!(!is_alive(&rec));
        assert!(State::load(&p).tunnels.is_empty());
    }
}
