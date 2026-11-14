//! Reachability checks (section 7.3): TCP connect plus SSH banner.

use crate::core::model::{Config, Server};
use crate::core::resolve;
use crate::core::sshcmd::{self, SshCtx};
use crate::core::state::{PingRecord, PingStatus};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::sync::Semaphore;

/// Maximum checks running at once.
pub const MAX_CONCURRENT: usize = 16;
const BANNER_WAIT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub status: PingStatus,
    pub latency_ms: Option<u64>,
    pub reason: Option<String>,
    pub via: Option<String>,
}

impl Outcome {
    fn down(reason: &str) -> Self {
        Outcome {
            status: PingStatus::Down,
            latency_ms: None,
            reason: Some(reason.into()),
            via: None,
        }
    }

    fn unknown(reason: &str) -> Self {
        Outcome {
            status: PingStatus::Unknown,
            latency_ms: None,
            reason: Some(reason.into()),
            via: None,
        }
    }

    pub fn record(self) -> PingRecord {
        PingRecord {
            status: self.status,
            latency_ms: self.latency_ms,
            reason: self.reason,
            via: self.via,
            checked_at: chrono::Utc::now(),
        }
    }
}

/// Classify the first bytes the server sent.
pub fn classify_banner(bytes: &[u8]) -> PingStatus {
    if bytes.starts_with(b"SSH-") {
        PingStatus::Up
    } else {
        PingStatus::OpenNoSsh
    }
}

/// TCP connect to `host:port` within `timeout`, then wait briefly for an SSH banner.
pub async fn tcp_check(host: &str, port: u16, timeout: Duration) -> Outcome {
    let deadline = Instant::now() + timeout;
    let addrs: Vec<_> =
        match tokio::time::timeout(timeout, tokio::net::lookup_host((host, port))).await {
            Ok(Ok(a)) => a.collect(),
            Ok(Err(_)) => return Outcome::down("dns"),
            Err(_) => return Outcome::down("timeout"),
        };
    if addrs.is_empty() {
        return Outcome::down("dns");
    }
    let mut last = "refused";
    for addr in addrs {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Outcome::down("timeout");
        }
        let start = Instant::now();
        match tokio::time::timeout(left, TcpStream::connect(addr)).await {
            Ok(Ok(mut stream)) => {
                let latency = start.elapsed().as_millis() as u64;
                let mut buf = [0u8; 255];
                let status = match tokio::time::timeout(BANNER_WAIT, stream.read(&mut buf)).await {
                    Ok(Ok(n)) if n > 0 => classify_banner(&buf[..n]),
                    _ => PingStatus::OpenNoSsh,
                };
                return Outcome {
                    status,
                    latency_ms: Some(latency),
                    reason: None,
                    via: None,
                };
            }
            Ok(Err(e)) => {
                last = match e.kind() {
                    std::io::ErrorKind::ConnectionRefused => "refused",
                    _ => "unreachable",
                }
            }
            Err(_) => return Outcome::down("timeout"),
        }
    }
    Outcome::down(last)
}

/// Full check through the jump chain: `ssh -W host:port <last hop>`, reading the banner.
async fn jump_check(argv: Vec<String>, timeout: Duration) -> Outcome {
    let start = Instant::now();
    let mut child = match tokio::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Outcome::unknown("ssh missing"),
    };
    let mut stdout = child.stdout.take().expect("piped");
    let mut buf = [0u8; 255];
    let read = tokio::time::timeout(timeout, stdout.read(&mut buf)).await;
    let latency = start.elapsed().as_millis() as u64;
    let outcome = match read {
        Ok(Ok(n)) if n > 0 => Outcome {
            status: classify_banner(&buf[..n]),
            latency_ms: Some(latency),
            reason: None,
            via: None,
        },
        Ok(_) => {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ =
                    tokio::time::timeout(Duration::from_millis(500), e.read_to_string(&mut err))
                        .await;
            }
            classify_ssh_error(&err)
        }
        Err(_) => Outcome::down("timeout"),
    };
    let _ = child.kill().await;
    outcome
}

/// Map ssh's stderr from a failed `-W` check to an outcome.
pub fn classify_ssh_error(stderr: &str) -> Outcome {
    let s = stderr.to_lowercase();
    if s.contains("host key verification failed") || s.contains("no matching host key") {
        Outcome::unknown("host key not verified — connect once interactively")
    } else if s.contains("permission denied") || s.contains("too many authentication") {
        Outcome::unknown("auth")
    } else if s.contains("could not resolve") {
        Outcome::down("dns")
    } else if s.contains("timed out") {
        Outcome::down("timeout")
    } else if s.contains("refused") {
        Outcome::down("refused")
    } else {
        Outcome::down("unreachable")
    }
}

/// Check one server. Direct servers get a TCP check. Jumped servers get a TCP check of
/// the first hop (`full = false`, shown "via <hop>") or a full `-W` check (`full = true`).
pub async fn check(
    cfg: &Config,
    ctx: Option<&SshCtx>,
    server: &Server,
    full: bool,
    timeout: Duration,
) -> Outcome {
    if server.jump.is_empty() {
        return tcp_check(&server.host, server.port_or_default(), timeout).await;
    }
    if full {
        if let Some(argv) = ctx.and_then(|c| sshcmd::ssh_check(c, server)) {
            // ConnectTimeout=5 per hop, plus a margin.
            let hops = resolve::chain(cfg, server).map(|c| c.len()).unwrap_or(1) as u64;
            return jump_check(argv, Duration::from_secs(5 * hops + 2)).await;
        }
    }
    let first = resolve::chain(cfg, server)
        .ok()
        .and_then(|c| c.first().cloned())
        .and_then(|n| cfg.get(&n).cloned());
    match first {
        Some(hop) => {
            let mut o = tcp_check(&hop.host, hop.port_or_default(), timeout).await;
            o.via = Some(hop.name.clone());
            o
        }
        None => Outcome::unknown("bad jump"),
    }
}

/// Check many servers with at most [`MAX_CONCURRENT`] in flight, reporting each result
/// through `on_result` as it arrives.
pub async fn check_many<F>(
    cfg: Arc<Config>,
    ctx: Option<SshCtx>,
    names: Vec<String>,
    full: bool,
    on_result: F,
) where
    F: Fn(String, Outcome) + Send + Sync + 'static,
{
    let sem = Arc::new(Semaphore::new(MAX_CONCURRENT));
    let on_result = Arc::new(on_result);
    let ctx = Arc::new(ctx);
    let timeout = Duration::from_millis(cfg.settings.ping_timeout_ms.max(1));
    let mut tasks = tokio::task::JoinSet::new();
    for name in names {
        let (sem, cfg, ctx, on_result) = (sem.clone(), cfg.clone(), ctx.clone(), on_result.clone());
        tasks.spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore open");
            let Some(server) = cfg.get(&name) else { return };
            let outcome = check(&cfg, ctx.as_ref().as_ref(), server, full, timeout).await;
            on_result(name, outcome);
        });
    }
    while tasks.join_next().await.is_some() {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    #[test]
    fn banners() {
        assert_eq!(classify_banner(b"SSH-2.0-OpenSSH_9.9\r\n"), PingStatus::Up);
        assert_eq!(classify_banner(b"HTTP/1.1 400"), PingStatus::OpenNoSsh);
    }

    #[test]
    fn ssh_errors() {
        assert_eq!(
            classify_ssh_error("ata@x: Permission denied (publickey).")
                .reason
                .as_deref(),
            Some("auth")
        );
        assert_eq!(
            classify_ssh_error("Host key verification failed.").status,
            PingStatus::Unknown
        );
        assert_eq!(
            classify_ssh_error("ssh: connect to host x port 22: Connection refused")
                .reason
                .as_deref(),
            Some("refused")
        );
    }

    #[tokio::test]
    async fn up_with_banner() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            s.write_all(b"SSH-2.0-test\r\n").await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let o = tcp_check("127.0.0.1", port, Duration::from_secs(1)).await;
        assert_eq!(o.status, PingStatus::Up);
        assert!(o.latency_ms.is_some());
    }

    #[tokio::test]
    async fn open_without_banner() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (_s, _) = l.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(2)).await;
        });
        let o = tcp_check("127.0.0.1", port, Duration::from_secs(1)).await;
        assert_eq!(o.status, PingStatus::OpenNoSsh);
    }

    #[tokio::test]
    async fn refused_is_down() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let o = tcp_check("127.0.0.1", port, Duration::from_secs(1)).await;
        assert_eq!(o.status, PingStatus::Down);
        assert_eq!(o.reason.as_deref(), Some("refused"));
    }

    #[tokio::test]
    async fn dns_failure() {
        let o = tcp_check("nonexistent.invalid", 22, Duration::from_secs(2)).await;
        assert_eq!(o.status, PingStatus::Down);
    }

    #[tokio::test]
    async fn jumped_auto_checks_first_hop() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            s.write_all(b"SSH-2.0-test\r\n").await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let cfg = crate::core::config::parse(
            &format!(
                "[[server]]\nname=\"hop\"\nhost=\"127.0.0.1\"\nport={port}\n[[server]]\nname=\"t\"\nhost=\"10.0.0.1\"\njump=[\"hop\"]\n"
            ),
            "t",
        )
        .unwrap();
        let o = check(
            &cfg,
            None,
            cfg.get("t").unwrap(),
            false,
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(o.status, PingStatus::Up);
        assert_eq!(o.via.as_deref(), Some("hop"));
    }
}
