//! Name lookup, fuzzy matching and jump-chain resolution.

use crate::core::error::{suggest, Error, Result};
use crate::core::model::{Config, Server};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Matcher, Utf32Str};

/// Maximum number of hops in a resolved jump chain.
pub const MAX_JUMP_DEPTH: usize = 8;

/// The text fuzzy search runs against: name, host, tags and group.
fn haystack(s: &Server) -> String {
    let mut h = format!("{} {}", s.name, s.host);
    for t in &s.tags {
        h.push(' ');
        h.push_str(t);
    }
    if let Some(g) = &s.group {
        h.push(' ');
        h.push_str(g);
    }
    h
}

/// Servers matching `query`, best match first. An empty query matches everything in order.
pub fn fuzzy_filter<'a>(servers: &'a [Server], query: &str) -> Vec<&'a Server> {
    if query.trim().is_empty() {
        return servers.iter().collect();
    }
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, usize, &Server)> = servers
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let h = haystack(s);
            pattern
                .score(Utf32Str::new(&h, &mut buf), &mut matcher)
                .map(|score| (score, i, s))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, s)| s).collect()
}

/// Result of resolving a CLI `<query>` (section 5.2).
#[derive(Debug)]
pub enum Lookup<'a> {
    One(&'a Server),
    Many(Vec<&'a Server>),
    None,
}

pub fn lookup<'a>(cfg: &'a Config, query: &str) -> Lookup<'a> {
    if let Some(s) = cfg.get(query) {
        return Lookup::One(s);
    }
    let mut hits = fuzzy_filter(&cfg.server, query);
    match hits.len() {
        0 => Lookup::None,
        1 => Lookup::One(hits.remove(0)),
        _ => Lookup::Many(hits),
    }
}

/// Exact-name lookup with a "did you mean" error.
pub fn require<'a>(cfg: &'a Config, name: &str) -> Result<&'a Server> {
    cfg.get(name).ok_or_else(|| not_found_error(cfg, name))
}

pub fn not_found_error(cfg: &Config, name: &str) -> Error {
    Error::not_found(format!("server \"{name}\""), suggest(name, cfg.names()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainError {
    Missing(String),
    SelfJump,
    Cycle(Vec<String>),
    TooDeep,
}

impl std::fmt::Display for ChainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainError::Missing(n) => write!(f, "jump \"{n}\" not found"),
            ChainError::SelfJump => write!(f, "server lists itself as a jump"),
            ChainError::Cycle(path) => write!(f, "jump cycle: {}", path.join(" → ")),
            ChainError::TooDeep => write!(f, "jump chain longer than {MAX_JUMP_DEPTH} hops"),
        }
    }
}

/// Check every jump reachable from `server` for missing names, cycles and depth.
pub fn check_jumps(cfg: &Config, server: &Server) -> std::result::Result<(), ChainError> {
    if server.jump.contains(&server.name) {
        return Err(ChainError::SelfJump);
    }
    fn visit(
        cfg: &Config,
        s: &Server,
        stack: &mut Vec<String>,
    ) -> std::result::Result<(), ChainError> {
        if let Some(pos) = stack.iter().position(|n| *n == s.name) {
            let mut cycle = stack[pos..].to_vec();
            cycle.push(s.name.clone());
            return Err(ChainError::Cycle(cycle));
        }
        if stack.len() > MAX_JUMP_DEPTH {
            return Err(ChainError::TooDeep);
        }
        stack.push(s.name.clone());
        for j in &s.jump {
            let next = cfg.get(j).ok_or_else(|| ChainError::Missing(j.clone()))?;
            visit(cfg, next, stack)?;
        }
        stack.pop();
        Ok(())
    }
    visit(cfg, server, &mut Vec::new())?;
    if chain(cfg, server)?.len() > MAX_JUMP_DEPTH {
        return Err(ChainError::TooDeep);
    }
    Ok(())
}

/// The hops ssh actually traverses to reach `server`, in order, excluding the target.
///
/// OpenSSH honours the first hop's own `ProxyJump` but overrides it for later hops, so
/// the chain is `chain(jump[0]) ++ jump`.
pub fn chain(cfg: &Config, server: &Server) -> std::result::Result<Vec<String>, ChainError> {
    let mut out = Vec::new();
    let mut cur = server;
    let mut seen = vec![server.name.clone()];
    let mut prefix: Vec<Vec<String>> = Vec::new();
    while let Some(first) = cur.jump.first() {
        prefix.push(cur.jump.clone());
        if seen.contains(first) {
            seen.push(first.clone());
            return Err(ChainError::Cycle(seen));
        }
        seen.push(first.clone());
        if prefix.len() > MAX_JUMP_DEPTH {
            return Err(ChainError::TooDeep);
        }
        cur = cfg
            .get(first)
            .ok_or_else(|| ChainError::Missing(first.clone()))?;
    }
    for hops in prefix.into_iter().rev() {
        out.extend(hops);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn srv(name: &str, jump: &[&str]) -> Server {
        Server {
            name: name.into(),
            host: format!("{name}.example.com"),
            jump: jump.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn cfg(servers: Vec<Server>) -> Config {
        Config {
            server: servers,
            ..Default::default()
        }
    }

    #[test]
    fn exact_beats_fuzzy() {
        let c = cfg(vec![srv("web", &[]), srv("web-2", &[])]);
        assert!(matches!(lookup(&c, "web"), Lookup::One(s) if s.name == "web"));
        assert!(matches!(lookup(&c, "we"), Lookup::Many(v) if v.len() == 2));
        assert!(matches!(lookup(&c, "zzz"), Lookup::None));
    }

    #[test]
    fn fuzzy_matches_tags_and_group() {
        let mut a = srv("alpha", &[]);
        a.tags = vec!["postgres".into()];
        let mut b = srv("beta", &[]);
        b.group = Some("personal".into());
        let c = cfg(vec![a, b]);
        assert!(matches!(lookup(&c, "postgres"), Lookup::One(s) if s.name == "alpha"));
        assert!(matches!(lookup(&c, "personal"), Lookup::One(s) if s.name == "beta"));
    }

    #[test]
    fn chain_follows_first_hop() {
        let c = cfg(vec![
            srv("x", &[]),
            srv("a", &["x"]),
            srv("b", &[]),
            srv("t", &["a", "b"]),
        ]);
        assert_eq!(chain(&c, c.get("t").unwrap()).unwrap(), ["x", "a", "b"]);
        assert_eq!(
            chain(&c, c.get("x").unwrap()).unwrap(),
            Vec::<String>::new()
        );
        assert!(check_jumps(&c, c.get("t").unwrap()).is_ok());
    }

    #[test]
    fn detects_cycles_missing_and_self() {
        let c = cfg(vec![srv("a", &["b"]), srv("b", &["c", "a"]), srv("c", &[])]);
        assert!(matches!(
            check_jumps(&c, c.get("a").unwrap()),
            Err(ChainError::Cycle(_))
        ));
        let c = cfg(vec![srv("a", &["nope"])]);
        assert_eq!(
            check_jumps(&c, c.get("a").unwrap()),
            Err(ChainError::Missing("nope".into()))
        );
        let c = cfg(vec![srv("a", &["a"])]);
        assert_eq!(
            check_jumps(&c, c.get("a").unwrap()),
            Err(ChainError::SelfJump)
        );
    }

    #[test]
    fn depth_limit() {
        let mut servers = vec![srv("h0", &[])];
        for i in 1..=10 {
            servers.push(srv(&format!("h{i}"), &[&format!("h{}", i - 1)]));
        }
        let c = cfg(servers);
        assert!(check_jumps(&c, c.get("h8").unwrap()).is_ok());
        assert_eq!(
            check_jumps(&c, c.get("h10").unwrap()),
            Err(ChainError::TooDeep)
        );
    }
}
