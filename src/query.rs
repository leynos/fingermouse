//! Parser for the finger protocol query line.
use std::fmt;

use thiserror::Error;

use crate::identity::{HostName, IdentityError, Username};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerQuery {
    pub username: Username,
    pub host: Option<HostName>,
    pub verbose: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum QueryError {
    #[error("request must terminate with CRLF")]
    MissingCrlf,
    #[error("query may not contain embedded control characters")]
    ControlCharacter,
    #[error("query must specify a username")]
    MissingUsername,
    #[error("unrecognised option")]
    UnknownOption,
    #[error("unexpected trailing input")]
    TrailingInput,
    #[error("invalid username: {0}")]
    InvalidUsername(String),
    #[error("invalid hostname: {0}")]
    InvalidHostname(String),
}

pub fn parse(line: &[u8]) -> Result<FingerQuery, QueryError> {
    if line.is_empty() || !line.ends_with(b"\r\n") {
        return Err(QueryError::MissingCrlf);
    }
    if line
        .iter()
        .any(|&b| matches!(b, 0x00..=0x1f) && b != b'\r' && b != b'\n')
    {
        return Err(QueryError::ControlCharacter);
    }

    let Some(raw_bytes) = line.get(..line.len().saturating_sub(2)) else {
        return Err(QueryError::MissingCrlf);
    };
    let raw = std::str::from_utf8(raw_bytes).map_err(|_| QueryError::ControlCharacter)?;

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(QueryError::MissingUsername);
    }

    let (verbose, remainder) = trimmed.strip_prefix("/W").map_or((false, trimmed), |rest| {
        let stripped = rest.trim_start_matches(' ');
        (true, stripped)
    });

    if remainder.is_empty() {
        return Err(QueryError::MissingUsername);
    }

    if remainder.starts_with('/') {
        return Err(QueryError::UnknownOption);
    }

    let mut parts = remainder.split_whitespace();
    let primary = parts
        .next()
        .ok_or(QueryError::MissingUsername)?
        .trim_end_matches('@');
    if parts.next().is_some() {
        return Err(QueryError::TrailingInput);
    }

    let mut host_split = primary.splitn(2, '@');
    let user_part = host_split.next().unwrap_or_default();
    if user_part.is_empty() {
        return Err(QueryError::MissingUsername);
    }

    let username = Username::parse(user_part)
        .map_err(|err| QueryError::InvalidUsername(format_error(&err)))?;

    let host = host_split
        .next()
        .map(|fragment| fragment.split('@').next().unwrap_or(""))
        .filter(|fragment| !fragment.is_empty())
        .map(|fragment| {
            HostName::parse(fragment).map_err(|err| QueryError::InvalidHostname(format_error(&err)))
        })
        .transpose()?;

    Ok(FingerQuery {
        username,
        host,
        verbose,
    })
}

fn format_error(err: &IdentityError) -> String {
    err.to_string()
}

impl fmt::Display for FingerQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.username, self.host_display())
    }
}

impl FingerQuery {
    fn host_display(&self) -> &str {
        self.host.as_ref().map_or("<default>", HostName::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const CRLF: &str = "\r\n";

    fn to_line(query: &str) -> Vec<u8> {
        format!("{query}{CRLF}").into_bytes()
    }

    #[rstest]
    #[case("alice", false, "alice", None)]
    #[case("alice@remote", false, "alice", Some("remote"))]
    #[case("/W alice", true, "alice", None)]
    #[case("/W    alice@example.com", true, "alice", Some("example.com"))]
    #[case("/W alice@example.com@other", true, "alice", Some("example.com"))]
    fn successful_parse(
        #[case] input: &str,
        #[case] verbose: bool,
        #[case] expected_user: &str,
        #[case] expected_host: Option<&str>,
    ) {
        let line = to_line(input);
        let query = match parse(&line) {
            Ok(value) => value,
            Err(err) => panic!("expected success, got {err}"),
        };
        assert_eq!(query.verbose, verbose);
        assert_eq!(query.username.as_str(), expected_user);
        let host = query.host.as_ref().map(HostName::as_str);
        assert_eq!(host, expected_host);
    }

    #[rstest]
    #[case("/W alice more")]
    #[case("/x alice")]
    #[case("")]
    #[case("AL ice")]
    fn invalid_queries(#[case] input: &str) {
        let line = to_line(input);
        assert!(parse(&line).is_err());
    }
}
