//! Parser for the finger protocol query line.
use std::fmt;

use thiserror::Error;

use crate::identity::{HostName, IdentityError, Username};

#[derive(Debug, Clone, PartialEq, Eq)]
/// Parsed representation of a finger query.
pub struct FingerQuery {
    pub username: Username,
    pub host: Option<HostName>,
    pub verbose: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
/// Errors that may be encountered when parsing a finger query.
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
    #[error("query contains invalid UTF-8")]
    InvalidEncoding,
}

fn is_embedded_control_char(b: u8) -> bool {
    matches!(b, 0x00..=0x1f) && b != b'\r' && b != b'\n'
}

/// Strip framing and validate the raw query input.
fn validate_input(line: &[u8]) -> Result<&str, QueryError> {
    let raw = std::str::from_utf8(line).map_err(|_| QueryError::InvalidEncoding)?;
    let raw = raw.strip_suffix("\r\n").ok_or(QueryError::MissingCrlf)?;

    if raw.bytes().any(is_embedded_control_char) {
        return Err(QueryError::ControlCharacter);
    }

    Ok(raw)
}

/// Parse the optional `/W` verbose flag from the query tokens.
fn parse_verbose_flag<'a>(
    tokens: &mut impl Iterator<Item = &'a str>,
    first: &'a str,
) -> Result<(bool, &'a str), QueryError> {
    let ensure_valid = |token: &'a str| -> Result<&'a str, QueryError> {
        if token.is_empty() {
            return Err(QueryError::MissingUsername);
        }
        if token.starts_with('/') {
            return Err(QueryError::UnknownOption);
        }
        Ok(token)
    };

    if first.eq_ignore_ascii_case("/W") {
        let next = tokens.next().ok_or(QueryError::MissingUsername)?;
        Ok((true, ensure_valid(next)?))
    } else {
        Ok((false, ensure_valid(first)?))
    }
}

/// Parse the username and optional host fragment from the provided token.
fn parse_user_host(user_token: &str) -> Result<(Username, Option<HostName>), QueryError> {
    let mut parts = user_token.splitn(2, '@');
    let user = parts.next().unwrap_or_default();
    if user.is_empty() {
        return Err(QueryError::MissingUsername);
    }

    let username =
        Username::parse(user).map_err(|err| QueryError::InvalidUsername(format_error(&err)))?;

    let host = parts
        .next()
        .map(|fragment| fragment.split('@').next().unwrap_or(""))
        .filter(|fragment| !fragment.is_empty())
        .map(|fragment| {
            HostName::parse(fragment).map_err(|err| QueryError::InvalidHostname(format_error(&err)))
        })
        .transpose()?;

    Ok((username, host))
}

/// Parse the supplied query line into a [`FingerQuery`].
pub fn parse(line: &[u8]) -> Result<FingerQuery, QueryError> {
    let raw = validate_input(line)?;
    let mut tokens = raw.split_whitespace();
    let first = tokens.next().ok_or(QueryError::MissingUsername)?;

    let (verbose, user_token) = parse_verbose_flag(&mut tokens, first)?;

    if tokens.next().is_some() {
        return Err(QueryError::TrailingInput);
    }

    let (username, host) = parse_user_host(user_token)?;

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
    #[case("/w alice", true, "alice", None)]
    #[case("/w alice@example.com", true, "alice", Some("example.com"))]
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
    #[case("/walice")]
    #[case("/Walice@example.com")]
    fn invalid_queries(#[case] input: &str) {
        let line = to_line(input);
        assert!(parse(&line).is_err());
    }
}
