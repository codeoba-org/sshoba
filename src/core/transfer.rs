//! Send/get files with rsync or scp (section 7.5).

use crate::core::error::{Error, Result};
use crate::core::launcher::{last_lines, which};
use crate::core::model::{Config, Server, TransferTool};
use crate::core::paths::Paths;
use crate::core::sshcmd::{self, Direction, RsyncCaps};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Rsync(RsyncCaps),
    Scp,
}

/// Parse `rsync --version` output. Returns (major, minor) of the rsync version, treating
/// openrsync as the version it claims compatibility with.
pub fn parse_rsync_version(out: &str) -> Option<(u32, u32)> {
    for line in out.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        for w in words.windows(3) {
            if w[0].ends_with("rsync") && w[1] == "version" {
                let mut it = w[2].split('.').map(|n| {
                    n.chars()
                        .take_while(char::is_ascii_digit)
                        .collect::<String>()
                        .parse::<u32>()
                        .ok()
                });
                if let (Some(Some(a)), Some(Some(b))) = (it.next(), it.next()) {
                    return Some((a, b));
                }
            }
        }
    }
    None
}

pub fn caps_for(version: Option<(u32, u32)>) -> RsyncCaps {
    let v = version.unwrap_or((0, 0));
    RsyncCaps {
        progress2: v >= (3, 1),
        protect_args: v >= (3, 0),
    }
}

/// Detect rsync capabilities by running `rsync --version`.
pub fn detect_rsync() -> Option<RsyncCaps> {
    which("rsync")?;
    let out = Command::new("rsync").arg("--version").output().ok()?;
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    Some(caps_for(parse_rsync_version(&text)))
}

/// Pick the transfer tool from the `transfer_tool` setting.
pub fn choose_tool(setting: TransferTool) -> Result<Tool> {
    match setting {
        TransferTool::Scp => Ok(Tool::Scp),
        TransferTool::Rsync => detect_rsync().map(Tool::Rsync).ok_or(Error::MissingBinary {
            bin: "rsync".into(),
            hint: "install rsync or set transfer_tool = \"scp\"".into(),
        }),
        TransferTool::Auto => Ok(detect_rsync().map(Tool::Rsync).unwrap_or(Tool::Scp)),
    }
}

/// One progress update.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Progress {
    pub bytes: u64,
    pub percent: u8,
    pub rate: String,
    pub eta: String,
    /// The file being copied (per-file progress mode only).
    pub file: Option<String>,
}

/// Parse an rsync progress line such as
/// `  1,234,567  45%   12.34MB/s    0:00:05 (xfr#3, to-chk=0/10)`.
pub fn parse_progress(line: &str) -> Option<Progress> {
    let mut it = line.split_whitespace();
    let bytes = it.next()?.replace(',', "").parse::<u64>().ok()?;
    let percent = it.next()?.strip_suffix('%')?.parse::<u8>().ok()?;
    let rate = it.next()?.to_string();
    let eta = it.next()?.to_string();
    if !rate.ends_with("/s") {
        return None;
    }
    Some(Progress {
        bytes,
        percent,
        rate,
        eta,
        file: None,
    })
}

/// A ready-to-run transfer.
#[derive(Debug, Clone)]
pub struct Plan {
    pub argv: Vec<String>,
    pub tool: Tool,
    /// Per-file progress (old rsync): track the current file name.
    pub per_file: bool,
}

/// Build the transfer command. Fails before spawning if a local source is missing.
pub fn plan(
    cfg: &Config,
    paths: &Paths,
    server: &Server,
    dir: Direction,
    srcs: &[String],
    dest: &str,
) -> Result<Plan> {
    if srcs.is_empty() {
        return Err(Error::other("nothing to transfer"));
    }
    if dir == Direction::Send {
        for s in srcs {
            if std::fs::symlink_metadata(s).is_err() {
                return Err(Error::not_found(format!("local file \"{s}\""), None));
            }
        }
    }
    let tool = choose_tool(cfg.settings.transfer_tool)?;
    let ctx = sshcmd::ensure_ssh_config(cfg, paths)?;
    let argv = match tool {
        Tool::Rsync(caps) => sshcmd::rsync_args(&ctx, server, dir, srcs, dest, caps),
        Tool::Scp => sshcmd::scp_args(&ctx, server, dir, srcs, dest),
    };
    Ok(Plan {
        argv,
        tool,
        per_file: matches!(tool, Tool::Rsync(c) if !c.progress2),
    })
}

/// Run a transfer, reporting progress. Setting `cancel` kills the child (rsync's
/// `--partial` keeps what was copied so a rerun resumes).
pub fn run(
    plan: &Plan,
    cancel: Arc<AtomicBool>,
    mut on_progress: impl FnMut(Progress),
) -> Result<()> {
    let bin = &plan.argv[0];
    let mut child = Command::new(bin)
        .args(&plan.argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::MissingBinary {
                    bin: bin.clone(),
                    hint: "install it or change transfer_tool".into(),
                }
            } else {
                Error::io(format!("run {bin}"), e)
            }
        })?;
    let mut stderr = child.stderr.take().expect("piped");
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let mut stdout = child.stdout.take().expect("piped");
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = stdout.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut pending = String::new();
    let mut current_file: Option<String> = None;
    let status = loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::other("transfer cancelled"));
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => {
                pending.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(i) = pending.find(['\r', '\n']) {
                    let line: String = pending.drain(..=i).collect();
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    match parse_progress(line) {
                        Some(mut p) => {
                            p.file = current_file.clone();
                            on_progress(p);
                        }
                        None if plan.per_file => current_file = Some(line.to_string()),
                        None => {}
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                break child
                    .wait()
                    .map_err(|e| Error::io("wait for transfer", e))?;
            }
        }
    };
    let err = err_thread.join().unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(Error::Command {
            command: sshcmd::shell_join(&plan.argv),
            stderr: last_lines(&err, 5),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rsync_versions() {
        assert_eq!(
            parse_rsync_version("rsync  version 3.2.7  protocol version 31\nCopyright"),
            Some((3, 2))
        );
        assert_eq!(
            parse_rsync_version("openrsync: protocol version 29\nrsync version 2.6.9 compatible"),
            Some((2, 6))
        );
        assert_eq!(
            parse_rsync_version("rsync  version 2.6.9  protocol version 29"),
            Some((2, 6))
        );
        assert_eq!(parse_rsync_version("nonsense"), None);
        assert!(caps_for(Some((3, 1))).progress2);
        assert!(!caps_for(Some((2, 6))).protect_args);
        assert!(caps_for(Some((3, 0))).protect_args && !caps_for(Some((3, 0))).progress2);
    }

    #[test]
    fn progress_lines() {
        assert_eq!(
            parse_progress("    123,456,789  45%   12.34MB/s    0:00:05 (xfr#3, to-chk=0/10)"),
            Some(Progress {
                bytes: 123_456_789,
                percent: 45,
                rate: "12.34MB/s".into(),
                eta: "0:00:05".into(),
                file: None
            })
        );
        assert!(parse_progress("sending incremental file list").is_none());
        assert!(parse_progress("my file.txt").is_none());
    }
}
