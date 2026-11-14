//! Load, validate and save servers.toml (sections 4.3 and 4.4).
//!
//! Saves merge changes into the parsed `toml_edit` document so comments and ordering
//! written by the user survive edits made through shhoba.

use crate::core::error::{suggest, Error, Result};
use crate::core::fsutil::write_atomic;
use crate::core::model::{parse_host_port, Config, ForwardKind, Server};
use crate::core::resolve::{check_jumps, ChainError};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table};

pub const TEMPLATE: &str = r#"# shhoba server list. Edit by hand, with `shhoba add`, or in the TUI.
# Add servers as [[server]] tables below the settings.

[settings]
launch = "auto"          # auto | tmux | zellij | kitty | wezterm | current
stay_open = true         # keep the TUI open after opening a tab in a multiplexer
ping_interval = 30       # seconds; 0 disables auto ping
ping_timeout_ms = 1500
transfer_tool = "auto"   # auto | rsync | scp
control_master = true    # reuse connections via ControlMaster
ssh_bin = "ssh"          # override the ssh path if needed
"#;

/// Returns true if `name` is a valid server name.
pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Check every rule in section 4.3. Each problem names the file, server and field.
pub fn validate(cfg: &Config, file: &str) -> Result<()> {
    let mut errs = Vec::new();
    let mut seen = HashSet::new();
    for s in &cfg.server {
        let at = format!("{file}: server \"{}\"", s.name);
        if !valid_name(&s.name) {
            errs.push(format!(
                "{at}: name must be 1-64 characters of letters, digits, '.', '_' or '-'"
            ));
        }
        if !seen.insert(s.name.as_str()) {
            errs.push(format!("{at}: duplicate server name"));
        }
        if s.host.trim().is_empty() {
            errs.push(format!("{at}: host is empty"));
        }
        let mut jump_ok = true;
        for j in &s.jump {
            if *j == s.name {
                errs.push(format!("{at}: jump \"{j}\" is the server itself"));
                jump_ok = false;
            } else if cfg.get(j).is_none() {
                let hint = suggest(j, cfg.names().filter(|n| *n != s.name))
                    .map(|h| format!(" (did you mean \"{h}\"?)"))
                    .unwrap_or_default();
                errs.push(format!("{at}: jump \"{j}\" not found{hint}"));
                jump_ok = false;
            }
        }
        if jump_ok {
            match check_jumps(cfg, s) {
                Ok(()) | Err(ChainError::Missing(_)) => {}
                Err(e) => errs.push(format!("{at}: {e}")),
            }
        }
        let mut fwd_names = HashSet::new();
        for f in &s.forward {
            let fat = format!("{at}: forward \"{}\"", f.name);
            if f.name.is_empty() || !valid_name(&f.name) {
                errs.push(format!(
                    "{fat}: name must be 1-64 characters of letters, digits, '.', '_' or '-'"
                ));
            }
            if !fwd_names.insert(f.name.as_str()) {
                errs.push(format!("{fat}: duplicate forward name"));
            }
            if f.local_port == 0 {
                errs.push(format!("{fat}: local_port must be 1-65535"));
            }
            match (f.kind, &f.remote) {
                (ForwardKind::Socks, Some(_)) => {
                    errs.push(format!("{fat}: remote must be absent for socks forwards"))
                }
                (ForwardKind::Local | ForwardKind::Remote, None) => {
                    errs.push(format!("{fat}: remote (host:port) is required"))
                }
                (_, Some(r)) => {
                    if let Err(e) = parse_host_port(r) {
                        errs.push(format!("{fat}: remote {e}"));
                    }
                }
                _ => {}
            }
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(Error::InvalidConfig(errs))
    }
}

/// Parse and validate config text.
pub fn parse(text: &str, file: &str) -> Result<Config> {
    let cfg: Config = toml::from_str(text)
        .map_err(|e| Error::InvalidConfig(vec![format!("{file}: {}", e.to_string().trim())]))?;
    validate(&cfg, file)?;
    Ok(cfg)
}

/// servers.toml on disk together with its parsed, comment-preserving document.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    pub path: PathBuf,
    pub config: Config,
    doc: DocumentMut,
}

impl ConfigStore {
    /// Load servers.toml, creating it from [`TEMPLATE`] if it does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            write_atomic(path, TEMPLATE.as_bytes(), 0o600, false)?;
        }
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::io(format!("read {}", path.display()), e))?;
        Self::from_text(path, &text)
    }

    pub fn from_text(path: &Path, text: &str) -> Result<Self> {
        let label = file_label(path);
        let config = parse(text, &label)?;
        let doc = text
            .parse::<DocumentMut>()
            .map_err(|e| Error::InvalidConfig(vec![format!("{label}: {e}")]))?;
        Ok(ConfigStore {
            path: path.to_path_buf(),
            config,
            doc,
        })
    }

    pub fn label(&self) -> String {
        file_label(&self.path)
    }

    /// Validate `new`, merge it into the document and write it atomically with a backup.
    /// `rename` maps an old server name to its new name so its table (and comments) carry over.
    pub fn save(&mut self, new: Config, rename: Option<(&str, &str)>) -> Result<()> {
        validate(&new, &self.label())?;
        merge_servers(&mut self.doc, &self.config, &new, rename)?;
        write_atomic(&self.path, self.doc.to_string().as_bytes(), 0o600, true)?;
        self.config = new;
        Ok(())
    }

    /// The rendered document (what `save` would write).
    pub fn render(&self) -> String {
        self.doc.to_string()
    }
}

pub fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// A server rendered as a standalone `[[server]]` table.
fn server_table(s: &Server) -> Result<Table> {
    let wrapper = Config {
        settings: Default::default(),
        server: vec![s.clone()],
    };
    #[derive(serde::Serialize)]
    struct OnlyServers<'a> {
        server: &'a [Server],
    }
    let text = toml::to_string(&OnlyServers {
        server: &wrapper.server,
    })
    .map_err(|e| Error::other(format!("serialize server: {e}")))?;
    let doc = text
        .parse::<DocumentMut>()
        .map_err(|e| Error::other(format!("serialize server: {e}")))?;
    doc.get("server")
        .and_then(Item::as_array_of_tables)
        .and_then(|a| a.get(0))
        .cloned()
        .map(|mut t| {
            clear_positions(&mut t);
            t
        })
        .ok_or_else(|| Error::other("serialize server: no table"))
}

fn clear_positions(t: &mut Table) {
    t.set_position(None);
    for (_, item) in t.iter_mut() {
        if let Item::ArrayOfTables(a) = item {
            a.iter_mut().for_each(clear_positions);
        }
    }
}

fn table_name(t: &Table) -> Option<&str> {
    t.get("name").and_then(Item::as_str)
}

fn merge_servers(
    doc: &mut DocumentMut,
    old: &Config,
    new: &Config,
    rename: Option<(&str, &str)>,
) -> Result<()> {
    let existing: Vec<Table> = doc
        .get("server")
        .and_then(Item::as_array_of_tables)
        .map(|a| a.iter().cloned().collect())
        .unwrap_or_default();
    let mut out = toml_edit::ArrayOfTables::new();
    for s in &new.server {
        let old_name = match rename {
            Some((from, to)) if to == s.name => from,
            _ => s.name.as_str(),
        };
        let prev = existing.iter().find(|t| table_name(t) == Some(old_name));
        let table = match prev {
            Some(t) if old.get(old_name) == Some(s) => t.clone(),
            Some(t) => {
                let mut t = t.clone();
                merge_table(&mut t, &server_table(s)?);
                t
            }
            None => server_table(s)?,
        };
        out.push(table);
    }
    if out.is_empty() {
        doc.remove("server");
    } else {
        doc.insert("server", Item::ArrayOfTables(out));
    }
    place_new_tables(doc);
    Ok(())
}

/// Give tables created by shhoba (which have no document position) a position right
/// after the table before them, so new servers and forwards render where they belong.
fn place_new_tables(doc: &mut DocumentMut) {
    const GAP: isize = 1000;
    fn scale(t: &mut Table, max: &mut isize) {
        for (_, item) in t.iter_mut() {
            match item {
                Item::Table(t) => {
                    if let Some(p) = t.position() {
                        t.set_position(Some(p * GAP));
                        *max = (*max).max(p * GAP);
                    }
                    scale(t, max);
                }
                Item::ArrayOfTables(a) => {
                    for t in a.iter_mut() {
                        if let Some(p) = t.position() {
                            t.set_position(Some(p * GAP));
                            *max = (*max).max(p * GAP);
                        }
                        scale(t, max);
                    }
                }
                _ => {}
            }
        }
    }
    fn place(t: &mut Table, last: &mut isize) {
        match t.position() {
            Some(p) => *last = (*last).max(p),
            None => {
                *last += 1;
                t.set_position(Some(*last));
                t.decor_mut().set_prefix("\n");
            }
        }
        for (_, item) in t.iter_mut() {
            if let Item::ArrayOfTables(a) = item {
                for sub in a.iter_mut() {
                    place(sub, last);
                }
            }
        }
    }
    let mut max = 0;
    scale(doc.as_table_mut(), &mut max);
    if let Some(Item::ArrayOfTables(servers)) = doc.get_mut("server") {
        // Start right after the first existing server (or after everything if none).
        let mut last = servers
            .iter()
            .find_map(Table::position)
            .map_or(max, |p| p - 1);
        for t in servers.iter_mut() {
            place(t, &mut last);
        }
    }
}

fn plain(v: &toml_edit::Value) -> String {
    let mut v = v.clone();
    v.decor_mut().clear();
    v.to_string()
}

/// Make `old` hold the same data as `new` while keeping `old`'s comments and key order.
fn merge_table(old: &mut Table, new: &Table) {
    let stale: Vec<String> = old
        .iter()
        .map(|(k, _)| k.to_string())
        .filter(|k| !new.contains_key(k))
        .collect();
    for k in stale {
        old.remove(&k);
    }
    for (k, new_item) in new.iter() {
        match (old.get_mut(k), new_item) {
            (Some(Item::Value(ov)), Item::Value(nv)) => {
                if plain(ov) != plain(nv) {
                    let decor = ov.decor().clone();
                    *ov = nv.clone();
                    *ov.decor_mut() = decor;
                }
            }
            (Some(Item::ArrayOfTables(oa)), Item::ArrayOfTables(na)) => {
                let olds: Vec<Table> = oa.iter().cloned().collect();
                let mut merged = toml_edit::ArrayOfTables::new();
                for nt in na.iter() {
                    match olds.iter().find(|t| table_name(t) == table_name(nt)) {
                        Some(ot) => {
                            let mut ot = ot.clone();
                            merge_table(&mut ot, nt);
                            merged.push(ot);
                        }
                        None => merged.push(nt.clone()),
                    }
                }
                *oa = merged;
            }
            (Some(slot), _) => *slot = new_item.clone(),
            (None, _) => {
                old.insert(k, new_item.clone());
            }
        }
    }
}

/// 1-based line of `name = "<server>"` in `text`, for opening an editor there.
pub fn server_line(text: &str, server: &str) -> Option<usize> {
    let needle = format!("\"{server}\"");
    text.lines()
        .position(|l| {
            let t = l.trim_start();
            t.starts_with("name") && t.contains(&needle) && t[4..].trim_start().starts_with('=')
        })
        .map(|i| i + 1)
}

/// The editor command from `$VISUAL` / `$EDITOR`, falling back to `vi`.
fn editor_command() -> Vec<String> {
    let raw = std::env::var("VISUAL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".into());
    raw.split_whitespace().map(String::from).collect()
}

/// Arguments that open `file` at `line` for common editors.
fn editor_args(editor: &str, file: &Path, line: Option<usize>) -> Vec<String> {
    let f = file.display().to_string();
    let base = Path::new(editor)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match line {
        Some(n) if matches!(base.as_str(), "code" | "codium" | "cursor") => {
            vec!["--wait".into(), "--goto".into(), format!("{f}:{n}")]
        }
        Some(n) if matches!(base.as_str(), "hx" | "helix" | "subl" | "zed") => {
            vec![format!("{f}:{n}")]
        }
        Some(n)
            if matches!(
                base.as_str(),
                "vi" | "vim" | "nvim" | "nano" | "emacs" | "micro" | "kak" | "mg" | "joe"
            ) =>
        {
            vec![format!("+{n}"), f]
        }
        _ => vec![f],
    }
}

/// Edit servers.toml in the user's editor. Edits go to a temporary copy; on close it is
/// validated and atomically saved. On a validation error `reopen` decides whether to
/// open the editor again (true) or abandon the edit (false, original file untouched).
/// Returns whether the file changed.
pub fn edit_in_editor(
    path: &Path,
    server: Option<&str>,
    reopen: &mut dyn FnMut(&Error) -> bool,
) -> Result<bool> {
    let original = std::fs::read_to_string(path)
        .map_err(|e| Error::io(format!("read {}", path.display()), e))?;
    let tmp = path.with_file_name(format!(".{}.edit.toml", file_label(path)));
    write_atomic(&tmp, original.as_bytes(), 0o600, false)?;
    let line = server.and_then(|s| server_line(&original, s));
    let cmd = editor_command();
    let result = loop {
        let status = std::process::Command::new(&cmd[0])
            .args(&cmd[1..])
            .args(editor_args(&cmd[0], &tmp, line))
            .status();
        match status {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                break Err(Error::MissingBinary {
                    bin: cmd[0].clone(),
                    hint: "set $EDITOR to your editor".into(),
                })
            }
            Err(e) => break Err(Error::io(format!("run {}", cmd[0]), e)),
            Ok(_) => {}
        }
        let text = match std::fs::read_to_string(&tmp) {
            Ok(t) => t,
            Err(e) => break Err(Error::io(format!("read {}", tmp.display()), e)),
        };
        if text == original {
            break Ok(false);
        }
        match ConfigStore::from_text(path, &text) {
            Ok(_) => break write_atomic(path, text.as_bytes(), 0o600, true).map(|_| true),
            Err(e) => {
                if !reopen(&e) {
                    break Err(e);
                }
            }
        }
    };
    let _ = std::fs::remove_file(&tmp);
    result
}

// ---------------------------------------------------------------------------
// Pure edit operations on a Config. Callers save the result with ConfigStore::save.

pub fn add_server(cfg: &mut Config, server: Server) -> Result<()> {
    if cfg.get(&server.name).is_some() {
        return Err(Error::other(format!(
            "server \"{}\" already exists",
            server.name
        )));
    }
    cfg.server.push(server);
    Ok(())
}

/// Servers that list `name` as a jump.
pub fn dependents(cfg: &Config, name: &str) -> Vec<String> {
    cfg.server
        .iter()
        .filter(|s| s.jump.iter().any(|j| j == name))
        .map(|s| s.name.clone())
        .collect()
}

/// Rename a server and update every jump reference to it.
pub fn rename_server(cfg: &mut Config, old: &str, new: &str) -> Result<()> {
    if old == new {
        return Ok(());
    }
    if cfg.get(old).is_none() {
        return Err(crate::core::resolve::not_found_error(cfg, old));
    }
    if !valid_name(new) {
        return Err(Error::other(format!(
            "invalid name \"{new}\": use 1-64 letters, digits, '.', '_' or '-'"
        )));
    }
    if cfg.get(new).is_some() {
        return Err(Error::other(format!("server \"{new}\" already exists")));
    }
    for s in &mut cfg.server {
        if s.name == old {
            s.name = new.to_string();
        }
        for j in &mut s.jump {
            if j == old {
                *j = new.to_string();
            }
        }
    }
    Ok(())
}

/// Delete a server. Refused if others jump through it, unless `force`, which also
/// removes those references.
pub fn remove_server(cfg: &mut Config, name: &str, force: bool) -> Result<()> {
    if cfg.get(name).is_none() {
        return Err(crate::core::resolve::not_found_error(cfg, name));
    }
    let deps = dependents(cfg, name);
    if !deps.is_empty() && !force {
        return Err(Error::other(format!(
            "\"{name}\" is a jump host for: {} (use --force to delete it and remove those references)",
            deps.join(", ")
        )));
    }
    cfg.server.retain(|s| s.name != name);
    for s in &mut cfg.server {
        s.jump.retain(|j| j != name);
    }
    Ok(())
}

/// Add a forward to a server.
pub fn add_forward(cfg: &mut Config, server: &str, fwd: crate::core::model::Forward) -> Result<()> {
    let not_found = crate::core::resolve::not_found_error(cfg, server);
    let s = cfg.get_mut(server).ok_or(not_found)?;
    if !valid_name(&fwd.name) {
        return Err(Error::other(format!(
            "invalid forward name \"{}\": use letters, digits, '.', '_' or '-'",
            fwd.name
        )));
    }
    if s.forward(&fwd.name).is_some() {
        return Err(Error::other(format!(
            "server \"{server}\" already has a forward named \"{}\"",
            fwd.name
        )));
    }
    s.forward.push(fwd);
    Ok(())
}

/// Remove a forward from a server.
pub fn remove_forward(cfg: &mut Config, server: &str, name: &str) -> Result<()> {
    let not_found = crate::core::resolve::not_found_error(cfg, server);
    let s = cfg.get_mut(server).ok_or(not_found)?;
    let before = s.forward.len();
    s.forward.retain(|f| f.name != name);
    if s.forward.len() == before {
        let names: Vec<&str> = s.forward.iter().map(|f| f.name.as_str()).collect();
        return Err(Error::not_found(
            format!("forward \"{server}/{name}\""),
            suggest(name, names),
        ));
    }
    Ok(())
}

/// Fields accepted by `shhoba set`.
pub const SETTABLE_FIELDS: &[&str] = &[
    "name",
    "host",
    "user",
    "port",
    "key",
    "jump",
    "group",
    "tags",
    "favorite",
    "note",
    "extra_args",
];

fn opt(value: &str) -> Option<String> {
    let v = value.trim();
    (!v.is_empty()).then(|| v.to_string())
}

fn list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// Change one field. An empty value clears optional fields. Returns the server's new
/// name if `field` was `name`.
pub fn set_field(cfg: &mut Config, name: &str, field: &str, value: &str) -> Result<Option<String>> {
    if field == "name" {
        rename_server(cfg, name, value.trim())?;
        return Ok(Some(value.trim().to_string()));
    }
    let not_found = crate::core::resolve::not_found_error(cfg, name);
    let s = cfg.get_mut(name).ok_or(not_found)?;
    match field {
        "host" => {
            let v = opt(value).ok_or_else(|| Error::other("host cannot be empty"))?;
            s.host = v;
        }
        "user" => s.user = opt(value),
        "port" => {
            s.port = match opt(value) {
                None => None,
                Some(v) => Some(
                    v.parse::<u16>()
                        .ok()
                        .filter(|p| *p > 0)
                        .ok_or_else(|| Error::other(format!("invalid port \"{v}\"")))?,
                ),
            }
        }
        "key" => s.key = opt(value).map(PathBuf::from),
        "jump" => s.jump = list(value),
        "group" => s.group = opt(value),
        "tags" => s.tags = list(value),
        "note" => s.note = opt(value),
        "favorite" => {
            s.favorite = match value.trim() {
                "true" | "yes" | "1" | "on" => true,
                "false" | "no" | "0" | "off" | "" => false,
                v => {
                    return Err(Error::other(format!(
                        "favorite must be true or false, not \"{v}\""
                    )))
                }
            }
        }
        "extra_args" => s.extra_args = value.split_whitespace().map(String::from).collect(),
        _ => {
            return Err(Error::other(format!(
                "unknown field \"{field}\"; expected one of: {}",
                SETTABLE_FIELDS.join(", ")
            )))
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# my servers
[settings]
launch = "auto" # keep auto

# the bastion
[[server]]
name = "bastion"
host = "203.0.113.10" # public ip
user = "ata"

[[server]]
name = "db-prod"
host = "10.0.0.5"
jump = ["bastion"]

  [[server.forward]]
  name = "pg"
  kind = "local"
  local_port = 5432
  remote = "localhost:5432"
"#;

    fn store(dir: &Path, text: &str) -> ConfigStore {
        let p = dir.join("servers.toml");
        std::fs::write(&p, text).unwrap();
        ConfigStore::load(&p).unwrap()
    }

    #[test]
    fn missing_file_gets_template() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("servers.toml");
        let s = ConfigStore::load(&p).unwrap();
        assert!(s.config.server.is_empty());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), TEMPLATE);
    }

    #[test]
    fn edits_preserve_comments() {
        let dir = tempfile::tempdir().unwrap();
        let mut st = store(dir.path(), SAMPLE);
        let mut cfg = st.config.clone();
        cfg.get_mut("bastion").unwrap().port = Some(2222);
        add_server(
            &mut cfg,
            Server {
                name: "web".into(),
                host: "1.2.3.4".into(),
                ..Default::default()
            },
        )
        .unwrap();
        st.save(cfg, None).unwrap();
        let text = std::fs::read_to_string(&st.path).unwrap();
        assert!(text.contains("# my servers"));
        assert!(text.contains("# the bastion"));
        assert!(text.contains("host = \"203.0.113.10\" # public ip"));
        assert!(text.contains("launch = \"auto\" # keep auto"));
        assert!(text.contains("port = 2222"));
        assert!(text.contains("name = \"web\""));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("servers.toml.bak")).unwrap(),
            SAMPLE
        );
        // Round-trips.
        let reloaded = ConfigStore::load(&st.path).unwrap();
        assert_eq!(reloaded.config, st.config);
        insta::assert_snapshot!(text);
    }

    #[test]
    fn rename_updates_jumps_and_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let mut st = store(dir.path(), SAMPLE);
        let mut cfg = st.config.clone();
        rename_server(&mut cfg, "bastion", "jump1").unwrap();
        assert_eq!(cfg.get("db-prod").unwrap().jump, ["jump1"]);
        st.save(cfg, Some(("bastion", "jump1"))).unwrap();
        let text = st.render();
        assert!(text.contains("# the bastion\n[[server]]\nname = \"jump1\""));
        assert!(text.contains("jump = [\"jump1\"]"));
    }

    #[test]
    fn remove_refuses_used_jump() {
        let dir = tempfile::tempdir().unwrap();
        let st = store(dir.path(), SAMPLE);
        let mut cfg = st.config.clone();
        let e = remove_server(&mut cfg, "bastion", false).unwrap_err();
        assert!(e.to_string().contains("db-prod"));
        remove_server(&mut cfg, "bastion", true).unwrap();
        assert!(cfg.get("db-prod").unwrap().jump.is_empty());
    }

    #[test]
    fn validation_messages() {
        let bad = r#"
[[server]]
name = "a"
host = "h"
jump = ["bastoin"]

[[server]]
name = "bastion"
host = "h"

[[server]]
name = "bastion"
host = "h"

[[server]]
name = "c1"
host = "h"
jump = ["c2"]

[[server]]
name = "c2"
host = "h"
jump = ["c1"]

[[server]]
name = "bad name"
host = "h"
  [[server.forward]]
  name = "s"
  kind = "socks"
  local_port = 1080
  remote = "x:1"
"#;
        let e = parse(bad, "servers.toml").unwrap_err();
        assert_eq!(e.exit_code(), 4);
        let msg = e.to_string();
        assert!(msg.contains(
            r#"servers.toml: server "a": jump "bastoin" not found (did you mean "bastion"?)"#
        ));
        assert!(msg.contains(r#"server "bastion": duplicate server name"#));
        assert!(msg.contains(r#"server "c1": jump cycle: c1 → c2 → c1"#));
        assert!(msg.contains(r#"server "bad name": name must be"#));
        assert!(msg.contains(r#"forward "s": remote must be absent"#));
    }

    #[test]
    fn unknown_field_is_an_error() {
        let e = parse(
            "[[server]]\nname = \"a\"\nhostname = \"h\"\n",
            "servers.toml",
        )
        .unwrap_err();
        assert_eq!(e.exit_code(), 4);
        assert!(e.to_string().contains("hostname"));
    }

    #[test]
    fn finds_server_line() {
        assert_eq!(server_line(SAMPLE, "db-prod"), Some(12));
        assert_eq!(server_line(SAMPLE, "nope"), None);
        assert_eq!(
            editor_args("nvim", Path::new("/c.toml"), Some(3)),
            ["+3", "/c.toml"]
        );
        assert_eq!(
            editor_args("/usr/bin/code", Path::new("/c.toml"), Some(3)),
            ["--wait", "--goto", "/c.toml:3"]
        );
    }

    #[test]
    fn set_fields() {
        let mut cfg = parse(SAMPLE, "servers.toml").unwrap();
        set_field(&mut cfg, "bastion", "port", "2222").unwrap();
        set_field(&mut cfg, "bastion", "tags", "a, b").unwrap();
        set_field(&mut cfg, "bastion", "user", "").unwrap();
        set_field(&mut cfg, "bastion", "favorite", "yes").unwrap();
        let b = cfg.get("bastion").unwrap();
        assert_eq!(b.port, Some(2222));
        assert_eq!(b.tags, ["a", "b"]);
        assert_eq!(b.user, None);
        assert!(b.favorite);
        assert!(set_field(&mut cfg, "bastion", "port", "x").is_err());
        assert!(set_field(&mut cfg, "bastion", "nope", "x").is_err());
        assert_eq!(
            set_field(&mut cfg, "bastion", "name", "b2").unwrap(),
            Some("b2".into())
        );
        assert_eq!(cfg.get("db-prod").unwrap().jump, ["b2"]);
    }
}
