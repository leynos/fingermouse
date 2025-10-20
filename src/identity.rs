//! Identity types for fingermouse queries.
use std::fmt;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("username must be between 1 and {max} characters")]
    UsernameLength { max: usize },
    #[error("username may only contain ASCII letters, digits, hyphen, or underscore")]
    UsernameCharacters,
    #[error("hostname must be between 1 and {max} characters")]
    HostLength { max: usize },
    #[error("hostname contains invalid characters")]
    HostCharacters,
    #[error("hostname labels must not begin or end with a hyphen")]
    HostLabelHyphen,
    #[error("hostname has empty label")]
    HostEmptyLabel,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Username(String);

impl Username {
    pub const MAX_LEN: usize = 32;

    pub fn parse(input: &str) -> Result<Self, IdentityError> {
        let trimmed = input.trim();
        if trimmed.is_empty() || trimmed.len() > Self::MAX_LEN {
            return Err(IdentityError::UsernameLength { max: Self::MAX_LEN });
        }

        if trimmed
            .chars()
            .all(|c| matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_'))
        {
            Ok(Self(trimmed.to_ascii_lowercase()))
        } else {
            Err(IdentityError::UsernameCharacters)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Username {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HostName(String);

impl HostName {
    pub const MAX_LEN: usize = 253;

    pub fn parse(input: &str) -> Result<Self, IdentityError> {
        let trimmed = input.trim();
        if trimmed.is_empty() || trimmed.len() > Self::MAX_LEN {
            return Err(IdentityError::HostLength { max: Self::MAX_LEN });
        }
        let mut output = String::with_capacity(trimmed.len());

        for label in trimmed.split('.') {
            Self::validate_label(label)?;

            if !output.is_empty() {
                output.push('.');
            }
            output.push_str(&label.to_ascii_lowercase());
        }

        Ok(Self(output))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl HostName {
    fn validate_label(label: &str) -> Result<(), IdentityError> {
        if label.is_empty() {
            return Err(IdentityError::HostEmptyLabel);
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(IdentityError::HostLabelHyphen);
        }
        if label
            .chars()
            .any(|c| !matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-'))
        {
            return Err(IdentityError::HostCharacters);
        }
        Ok(())
    }
}

impl fmt::Display for HostName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("alice", Some("alice"))]
    #[case("ALICE", Some("alice"))]
    #[case("bob-1", Some("bob-1"))]
    #[case("  charlie_2  ", Some("charlie_2"))]
    #[case("", None)]
    #[case("averylongusernamebeyondlimittoolong", None)]
    #[case("bad.name", None)]
    #[case("bad name", None)]
    fn username_validation(#[case] input: &str, #[case] expected: Option<&str>) {
        let outcome = Username::parse(input);
        match expected {
            Some(value) => match outcome {
                Ok(username) => assert_eq!(username.as_str(), value),
                Err(err) => panic!("expected success, got {err}"),
            },
            None => {
                assert!(outcome.is_err());
            }
        }
    }

    #[rstest]
    #[case("example.com", Some("example.com"))]
    #[case("Example.COM", Some("example.com"))]
    #[case("a-b.c-d", Some("a-b.c-d"))]
    #[case("-bad.com", None)]
    #[case("bad-.com", None)]
    #[case("bad..com", None)]
    #[case("bad@com", None)]
    fn hostname_validation(#[case] input: &str, #[case] expected: Option<&str>) {
        let outcome = HostName::parse(input);
        match expected {
            Some(value) => match outcome {
                Ok(hostname) => assert_eq!(hostname.as_str(), value),
                Err(err) => panic!("expected success, got {err}"),
            },
            None => {
                assert!(outcome.is_err());
            }
        }
    }
}
