//! Import from / export to ssh config format (section 7.7).

use crate::core::config::valid_name;
use crate::core::error::{Error, Result};
use crate::core::model::{Config, Forward, ForwardKind, Server};
use std::path::{Path, PathBuf};

/// One parsed `Host` block.
#[derive(Debug, Clone, Default, PartialEq)]
struct HostBlock {
    patterns: Vec<String>,
    /// (lowercased keyword, args), in file order.
    options: Vec<(String, Vec<String>)>,
}

impl HostBlock {
    fn first(&self, key: &str) -> Option<&[String]> {
        self.options
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_slice())
    }

    fn all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a [String]> + 'a {
        self.options
            .iter()
            .filter(move |(k, _)| k == key)
            .map(|(_, v)| v.as_slice())
    }
}

/// Split a config line into keyword and arguments (handles `Key=value` and quotes).
fn tokenize(line: &str) -> Option<(String, Vec<String>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (key, rest) = match line.find(|c: char| c.is_whitespace() || c == '=') {
        Some(i) => (
            &line[..i],
            line[i..].trim_start_matches(|c: char| c.is_whitespace() || c == '='),
        ),
        None => (line, ""),
    };
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut has = false;
    for c in rest.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                has = true;
            }
            '#' if !quoted && !has && cur.is_empty() => break,
            c if c.is_whitespace() && !quoted => {
                if has || !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        args.push(cur);
    }
    Some((key.to_lowercase(), args))
}

fn glob_match(pattern: &str, s: &str) -> bool {
    fn rec(p: &[char], s: &[char]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some('*'), _) => rec(&p[1..], s) || (!s.is_empty() && rec(p, &s[1..])),
            (Some('?'), Some(_)) => rec(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a == b => rec(&p[1..], &s[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = s.chars().collect();
    rec(&p, &s)
}

/// Files matching an `Include` argument (globs allowed in the file name).
fn expand_include(arg: &str, ssh_dir: &Path) -> Vec<PathBuf> {
    let expanded = shellexpand::tilde(arg).into_owned();
    let path = if Path::new(&expanded).is_absolute() {
        PathBuf::from(expanded)
    } else {
        ssh_dir.join(expanded)
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !name.contains(['*', '?']) {
        return if path.is_file() { vec![path] } else { vec![] };
    }
    let dir = path.parent().unwrap_or(Path::new("/"));
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.file_name()
                            .is_some_and(|n| glob_match(&name, &n.to_string_lossy()))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Parse Host blocks from `text`, following `Include` one level deep.
fn parse_blocks(text: &str, ssh_dir: &Path, follow_includes: bool) -> Vec<HostBlock> {
    let mut blocks = Vec::new();
    let mut cur: Option<HostBlock> = None;
    for line in text.lines() {
        let Some((key, args)) = tokenize(line) else {
            continue;
        };
        match key.as_str() {
            "host" => {
                blocks.extend(cur.take());
                cur = Some(HostBlock {
                    patterns: args,
                    options: Vec::new(),
                });
            }
            "match" => {
                blocks.extend(cur.take());
                // Options under Match are skipped until the next Host.
                cur = Some(HostBlock::default());
            }
            "include" if follow_includes => {
                for arg in &args {
                    for file in expand_include(arg, ssh_dir) {
                        if let Ok(t) = std::fs::read_to_string(&file) {
                            blocks.extend(cur.take());
                            blocks.extend(parse_blocks(&t, ssh_dir, false));
                        }
                    }
                }
            }
            _ => {
                if let Some(b) = cur.as_mut() {
                    b.options.push((key, args));
                }
            }
        }
    }
    blocks.extend(cur);
    blocks.retain(|b| !b.patterns.is_empty());
    blocks
}

#[derive(Debug, Default, PartialEq)]
pub struct ImportReport {
    pub added: Vec<Server>,
    pub existing: Vec<String>,
    pub skipped: Vec<(String, String)>,
}

fn split_bind_port(s: &str) -> Option<(Option<String>, u16)> {
    let (bind, port) = match s.rsplit_once(':') {
        Some((b, p)) => (Some(b.trim_matches(['[', ']']).to_string()), p),
        None => (None, s),
    };
    port.parse::<u16>()
        .ok()
        .filter(|p| *p > 0)
        .map(|p| (bind, p))
}

fn parse_forward(kind: ForwardKind, args: &[String], name: String) -> Option<Forward> {
    let (bind, local_port) = split_bind_port(args.first()?)?;
    let remote = match kind {
        ForwardKind::Socks => None,
        _ => {
            let r = args.get(1)?.clone();
            crate::core::model::parse_host_port(&r).ok()?;
            Some(r)
        }
    };
    Some(Forward {
        name,
        kind,
        bind,
        local_port,
        remote,
        auto: false,
    })
}

/// Build servers from ssh config text. Names already in `existing` are reported, not added.
pub fn import_text(text: &str, ssh_dir: &Path, existing: &Config) -> ImportReport {
    let blocks = parse_blocks(text, ssh_dir, true);
    let mut report = ImportReport::default();
    let mut aliases: Vec<String> = Vec::new();
    for b in &blocks {
        for p in &b.patterns {
            if !p.contains(['*', '?', '!']) && !aliases.contains(p) {
                aliases.push(p.clone());
            }
        }
    }
    let known = |n: &str| aliases.iter().any(|a| a == n) || existing.get(n).is_some();
    for b in &blocks {
        for alias in &b.patterns {
            if alias.contains(['*', '?', '!']) {
                continue;
            }
            if existing.get(alias).is_some() {
                if !report.existing.contains(alias) {
                    report.existing.push(alias.clone());
                }
                continue;
            }
            if report.added.iter().any(|s| s.name == *alias) {
                continue; // first block wins, like ssh
            }
            if !valid_name(alias) {
                report
                    .skipped
                    .push((alias.clone(), "name has unsupported characters".into()));
                continue;
            }
            let one = |k: &str| b.first(k).and_then(|a| a.first()).cloned();
            let mut s = Server {
                name: alias.clone(),
                host: one("hostname").unwrap_or_else(|| alias.clone()),
                user: one("user"),
                port: one("port").and_then(|p| p.parse().ok()),
                key: one("identityfile").map(PathBuf::from),
                ..Default::default()
            };
            if let Some(pj) = one("proxyjump").filter(|p| !p.eq_ignore_ascii_case("none")) {
                let hops: Vec<String> = pj.split(',').map(|h| h.trim().to_string()).collect();
                if hops.iter().all(|h| known(h) && h != alias) {
                    s.jump = hops;
                } else {
                    s.extra_args = vec!["-o".into(), format!("ProxyJump={pj}")];
                }
            }
            let mut n = 0;
            for (key, kind) in [
                ("localforward", ForwardKind::Local),
                ("remoteforward", ForwardKind::Remote),
                ("dynamicforward", ForwardKind::Socks),
            ] {
                for args in b.all(key) {
                    n += 1;
                    match parse_forward(kind, args, format!("fwd{n}")) {
                        Some(f) => s.forward.push(f),
                        None => report.skipped.push((
                            format!("{alias}/fwd{n}"),
                            format!("cannot parse {key} {}", args.join(" ")),
                        )),
                    }
                }
            }
            report.added.push(s);
        }
    }
    report
}

/// Import from a file into `cfg`. Jumps to hosts that ended up skipped become
/// `-o ProxyJump=` args so the result always validates.
pub fn import_file(file: &Path, ssh_dir: &Path, cfg: &Config) -> Result<ImportReport> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| Error::io(format!("read {}", file.display()), e))?;
    let mut report = import_text(&text, ssh_dir, cfg);
    let names: Vec<String> = report
        .added
        .iter()
        .map(|s| s.name.clone())
        .chain(cfg.server.iter().map(|s| s.name.clone()))
        .collect();
    for s in &mut report.added {
        if s.jump.iter().any(|j| !names.contains(j)) {
            let pj = s.jump.join(",");
            s.jump.clear();
            s.extra_args = vec!["-o".into(), format!("ProxyJump={pj}")];
        }
    }
    Ok(report)
}

/// Result of `export ssh-config`.
#[derive(Debug, PartialEq)]
pub struct ExportReport {
    pub file: PathBuf,
    /// Server names that also appear as `Host` aliases in the user's ssh config.
    pub clashes: Vec<String>,
}

/// Default export target: `~/.ssh/config.d/shhoba`.
pub fn default_export_file() -> PathBuf {
    ssh_dir().join("config.d").join("shhoba")
}

/// `Host` aliases defined in an ssh config (following Includes), skipping `exclude`.
fn user_aliases(config: &Path, ssh_dir: &Path, exclude: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(config) else {
        return Vec::new();
    };
    // Drop Include lines that point at our own export so we don't clash with ourselves.
    let filtered: String = text
        .lines()
        .filter(|l| {
            let t = l.trim_start().to_lowercase();
            !(t.starts_with("include")
                && l.split_whitespace()
                    .skip(1)
                    .flat_map(|a| expand_include(a, ssh_dir))
                    .any(|p| p == exclude))
        })
        .collect::<Vec<_>>()
        .join("\n");
    parse_blocks(&filtered, ssh_dir, true)
        .into_iter()
        .flat_map(|b| b.patterns)
        .collect()
}

/// Write an ssh config include file with plain aliases (`Host db-prod`). Never touches
/// `~/.ssh/config` itself.
pub fn export(
    cfg: &Config,
    file: &Path,
    user_config: &Path,
    ssh_dir: &Path,
) -> Result<ExportReport> {
    let text = crate::core::sshcmd::render_ssh_config(
        cfg,
        &crate::core::sshcmd::RenderOpts {
            prefix: "",
            control_dir: None,
            include_defaults: false,
        },
    );
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::io(format!("create {}", dir.display()), e))?;
    }
    crate::core::fsutil::write_atomic(file, text.as_bytes(), 0o600, false)?;
    let aliases = user_aliases(user_config, ssh_dir, file);
    let clashes = cfg
        .server
        .iter()
        .filter(|s| aliases.contains(&s.name))
        .map(|s| s.name.clone())
        .collect();
    Ok(ExportReport {
        file: file.to_path_buf(),
        clashes,
    })
}

/// Default `~/.ssh` directory.
pub fn ssh_dir() -> PathBuf {
    PathBuf::from(shellexpand::tilde("~/.ssh").into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer() {
        assert_eq!(
            tokenize("  HostName=example.com"),
            Some(("hostname".into(), vec!["example.com".into()]))
        );
        assert_eq!(
            tokenize("IdentityFile \"~/my keys/id\" # c"),
            Some(("identityfile".into(), vec!["~/my keys/id".into()]))
        );
        assert_eq!(tokenize("# comment"), None);
    }

    #[test]
    fn export_writes_file_and_reports_clashes() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path();
        let out = ssh.join("config.d").join("shhoba");
        std::fs::write(
            ssh.join("config"),
            "Include config.d/shhoba\nHost web\n  HostName other\n",
        )
        .unwrap();
        let cfg = crate::core::config::parse(
            "[[server]]\nname=\"web\"\nhost=\"1.2.3.4\"\n[[server]]\nname=\"db\"\nhost=\"10.0.0.5\"\njump=[\"web\"]\n",
            "t",
        )
        .unwrap();
        // First export, then again: our own file must not count as a clash.
        export(&cfg, &out, &ssh.join("config"), ssh).unwrap();
        let r = export(&cfg, &out, &ssh.join("config"), ssh).unwrap();
        assert_eq!(r.clashes, ["web"]);
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("Host db\n") && text.contains("ProxyJump web"));
        assert!(!text.contains("Host shhoba."));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&out).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn globbing() {
        assert!(glob_match("*.conf", "work.conf"));
        assert!(!glob_match("*.conf", "work.txt"));
        assert!(glob_match("h?st", "host"));
    }
}
