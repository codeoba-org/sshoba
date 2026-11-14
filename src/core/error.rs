//! Typed errors for core. The CLI maps them to exit codes (section 5.4).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    /// A server or forward lookup failed.
    #[error("{what} not found{}", suggestion_suffix(.suggestion))]
    NotFound {
        what: String,
        suggestion: Option<String>,
    },
    /// A query matched more than one server.
    #[error("\"{query}\" matches several servers: {}", .candidates.join(", "))]
    Ambiguous {
        query: String,
        candidates: Vec<String>,
    },
    /// servers.toml failed to parse or validate. One message per problem.
    #[error("{}", .0.join("\n"))]
    InvalidConfig(Vec<String>),
    /// A ping target is down.
    #[error("{0}")]
    Unreachable(String),
    /// A required external program is not installed.
    #[error("{bin} not found in PATH — {hint}")]
    MissingBinary { bin: String, hint: String },
    /// An external command ran and failed.
    #[error("command failed: {command}\n{stderr}")]
    Command { command: String, stderr: String },
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Other(String),
}

fn suggestion_suffix(s: &Option<String>) -> String {
    match s {
        Some(s) => format!(" (did you mean \"{s}\"?)"),
        None => String::new(),
    }
}

impl Error {
    /// Process exit code for this error (section 5.4).
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::NotFound { .. } => 2,
            Error::Ambiguous { .. } => 3,
            Error::InvalidConfig(_) => 4,
            Error::Unreachable(_) => 5,
            Error::MissingBinary { .. } => 6,
            _ => 1,
        }
    }

    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Error::Io {
            context: context.into(),
            source,
        }
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Error::Other(msg.into())
    }

    pub fn not_found(what: impl Into<String>, suggestion: Option<String>) -> Self {
        Error::NotFound {
            what: what.into(),
            suggestion,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Levenshtein distance, used for "did you mean" suggestions.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The closest candidate to `target`, if it is close enough to be a plausible typo.
pub fn suggest<'a>(target: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let limit = (target.chars().count() / 3).max(2);
    candidates
        .into_iter()
        .map(|c| (edit_distance(target, c), c))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance() {
        assert_eq!(edit_distance("bastoin", "bastion"), 2);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("same", "same"), 0);
    }

    #[test]
    fn suggestions() {
        assert_eq!(
            suggest("bastoin", ["bastion", "db-prod"]),
            Some("bastion".into())
        );
        assert_eq!(suggest("zzzzzz", ["bastion"]), None);
    }
}
