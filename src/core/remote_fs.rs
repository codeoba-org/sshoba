//! Directory listings for the file browser (section 7.6).

use crate::core::error::{Error, Result};
use crate::core::launcher::last_lines;
use crate::core::model::{Config, Server};
use crate::core::paths::Paths;
use crate::core::sshcmd;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Dir,
    File,
    Link,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    pub size: u64,
    /// Modification time, Unix seconds.
    pub mtime: Option<i64>,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }

    pub fn hidden(&self) -> bool {
        self.name.starts_with('.')
    }
}

/// Directories first, then by name.
pub fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        (!a.is_dir())
            .cmp(&!b.is_dir())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// Parse `find -printf '%y\t%s\t%T@\t%f\n'` output.
pub fn parse_find(out: &str) -> Vec<Entry> {
    out.lines()
        .filter_map(|l| {
            let mut it = l.splitn(4, '\t');
            let (y, s, t, name) = (it.next()?, it.next()?, it.next()?, it.next()?);
            let kind = match y {
                "d" => Kind::Dir,
                "f" => Kind::File,
                "l" => Kind::Link,
                _ => Kind::Other,
            };
            Some(Entry {
                name: name.to_string(),
                kind,
                size: s.parse().unwrap_or(0),
                mtime: t.split('.').next().and_then(|x| x.parse().ok()),
            })
        })
        .collect()
}

/// Parse `LC_ALL=C ls -la` output (BSD and GNU formats).
pub fn parse_ls(out: &str) -> Vec<Entry> {
    use chrono::{Datelike, NaiveDate, NaiveDateTime, TimeZone, Utc};
    let now = Utc::now();
    out.lines()
        .filter_map(|l| {
            if l.starts_with("total ") {
                return None;
            }
            // Split off the first 8 whitespace-separated fields, keeping the name intact.
            let mut fields = Vec::with_capacity(8);
            let mut rest = l;
            for _ in 0..8 {
                rest = rest.trim_start();
                let end = rest.find(char::is_whitespace)?;
                fields.push(&rest[..end]);
                rest = &rest[end..];
            }
            let name = rest.strip_prefix(' ').unwrap_or(rest);
            let perms = fields[0];
            let kind = match perms.chars().next()? {
                'd' => Kind::Dir,
                '-' => Kind::File,
                'l' => Kind::Link,
                _ => Kind::Other,
            };
            let name = if kind == Kind::Link {
                name.split(" -> ").next().unwrap_or(name)
            } else {
                name
            };
            if name == "." || name == ".." || name.is_empty() {
                return None;
            }
            let month = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ]
            .iter()
            .position(|m| *m == fields[5])
            .map(|i| i as u32 + 1);
            let day = fields[6].parse::<u32>().ok();
            let mtime = month.zip(day).and_then(|(m, d)| {
                let (year, h, min) = match fields[7].split_once(':') {
                    Some((h, mi)) => {
                        let mut y = now.year();
                        if m > now.month() {
                            y -= 1;
                        }
                        (y, h.parse().ok()?, mi.parse().ok()?)
                    }
                    None => (fields[7].parse().ok()?, 0, 0),
                };
                let dt: NaiveDateTime =
                    NaiveDate::from_ymd_opt(year, m, d)?.and_hms_opt(h, min, 0)?;
                Some(Utc.from_utc_datetime(&dt).timestamp())
            });
            Some(Entry {
                name: name.to_string(),
                kind,
                size: fields[4].parse().unwrap_or(0),
                mtime,
            })
        })
        .collect()
}

const LS_MARKER: &str = "__SHHOBA_LS__";

/// The remote shell command: cd, print the absolute path, then list with GNU find or
/// fall back to `ls -la`.
pub fn list_command(path: &str) -> String {
    let cd = if path.is_empty() || path == "~" {
        "cd".to_string()
    } else {
        format!("cd -- {}", shell_escape::unix::escape(path.into()))
    };
    format!(
        "{cd} && pwd && {{ find . -mindepth 1 -maxdepth 1 -printf '%y\\t%s\\t%T@\\t%f\\n' 2>/dev/null || {{ echo {LS_MARKER}; LC_ALL=C ls -la; }}; }}"
    )
}

/// Parse the output of [`list_command`]: (absolute path, entries).
pub fn parse_listing(out: &str) -> Option<(String, Vec<Entry>)> {
    let (cwd, rest) = out.split_once('\n').unwrap_or((out, ""));
    let cwd = cwd.trim();
    if !cwd.starts_with('/') {
        return None;
    }
    let mut entries = match rest.split_once(&format!("{LS_MARKER}\n")) {
        Some((_, ls)) => parse_ls(ls),
        None => parse_find(rest),
    };
    entries.retain(|e| e.name != "." && e.name != "..");
    sort_entries(&mut entries);
    Some((cwd.to_string(), entries))
}

/// List a remote directory over the shared ControlMaster connection. An empty path
/// means the home directory. Returns the absolute path and its entries.
pub fn list_remote(
    cfg: &Config,
    paths: &Paths,
    server: &Server,
    path: &str,
) -> Result<(String, Vec<Entry>)> {
    let ctx = sshcmd::ensure_ssh_config(cfg, paths)?;
    let argv = sshcmd::ssh_exec(&ctx, server, &list_command(path));
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| Error::io(format!("run {}", argv[0]), e))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    match parse_listing(&stdout) {
        Some(r) if out.status.success() || !r.1.is_empty() => Ok(r),
        _ => Err(Error::Command {
            command: sshcmd::shell_join(&argv),
            stderr: last_lines(&String::from_utf8_lossy(&out.stderr), 5),
        }),
    }
}

/// List a local directory.
pub fn list_local(path: &std::path::Path) -> Result<Vec<Entry>> {
    let rd =
        std::fs::read_dir(path).map_err(|e| Error::io(format!("list {}", path.display()), e))?;
    let mut entries: Vec<Entry> = rd
        .flatten()
        .map(|e| {
            let meta = e.metadata().ok();
            let link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
            let kind = if e.path().is_dir() {
                Kind::Dir
            } else if link {
                Kind::Link
            } else if meta.as_ref().is_some_and(|m| m.is_file()) {
                Kind::File
            } else {
                Kind::Other
            };
            Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                kind,
                size: meta.as_ref().map_or(0, |m| m.len()),
                mtime: meta
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64),
            }
        })
        .collect();
    sort_entries(&mut entries);
    Ok(entries)
}

/// "1.2 MB"-style size.
pub fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n}B")
    } else {
        format!("{v:.1}{}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_output() {
        let out = "d\t4096\t1700000000.5\tsrc\nf\t12\t1700000001.0000000000\tmy file.txt\nl\t7\t1700000002.0\tlink\n";
        let e = parse_find(out);
        assert_eq!(e.len(), 3);
        assert_eq!(e[1].name, "my file.txt");
        assert_eq!(e[1].size, 12);
        assert_eq!(e[0].mtime, Some(1_700_000_000));
        assert_eq!(e[2].kind, Kind::Link);
    }

    #[test]
    fn ls_output() {
        let out = "total 16\n\
drwxr-xr-x   5 ata  staff   160 Nov 14 12:00 .\n\
drwxr-xr-x  10 ata  staff   320 Nov 14 12:00 ..\n\
-rw-r--r--   1 ata  staff  1234 Jan  2  2024 a file.txt\n\
lrwxr-xr-x   1 ata  staff     4 Mar  3 09:15 l -> /tmp\n\
drwxr-xr-x   2 ata  staff    64 Feb 10 08:00 dir\n";
        let e = parse_ls(out);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].name, "a file.txt");
        assert_eq!(e[0].size, 1234);
        assert!(e[0].mtime.is_some());
        assert_eq!(e[1].name, "l");
        assert_eq!(e[2].kind, Kind::Dir);
    }

    #[test]
    fn listing_with_fallback_marker() {
        let out = "/home/u\n__SHHOBA_LS__\ntotal 0\n-rw-r--r-- 1 u u 3 Nov 14 12:00 b\ndrwxr-xr-x 2 u u 64 Nov 14 12:00 a\n";
        let (cwd, e) = parse_listing(out).unwrap();
        assert_eq!(cwd, "/home/u");
        assert_eq!(e[0].name, "a"); // dirs first
        assert!(parse_listing("not a path\n").is_none());
    }

    #[test]
    fn command_quotes_path() {
        assert!(list_command("/var/my logs").starts_with("cd -- '/var/my logs' && pwd"));
        assert!(list_command("").starts_with("cd && pwd"));
    }

    /// Run the real command in a local shell (this machine's find/ls).
    #[test]
    fn command_runs_locally() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x y.txt"), "hi").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let out = Command::new("sh")
            .arg("-c")
            .arg(list_command(&dir.path().display().to_string()))
            .output()
            .unwrap();
        let (cwd, e) = parse_listing(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert!(cwd.ends_with(dir.path().file_name().unwrap().to_str().unwrap()));
        let names: Vec<&str> = e.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["sub", "x y.txt"]);
        assert_eq!(e[1].size, 2);
    }

    #[test]
    fn thousand_entries_list_fast() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..1000 {
            std::fs::write(dir.path().join(format!("file {i:04}.log")), "x").unwrap();
        }
        let start = std::time::Instant::now();
        let out = Command::new("sh")
            .arg("-c")
            .arg(list_command(&dir.path().display().to_string()))
            .output()
            .unwrap();
        let (_, e) = parse_listing(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert_eq!(e.len(), 1000);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(12), "12B");
        assert_eq!(human_size(2048), "2.0K");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0M");
    }
}
