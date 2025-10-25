//! User profile representation and rendering.
use std::fmt;

use indexmap::IndexMap;
use thiserror::Error;

use crate::framing::CrlfBuffer;
use crate::identity::{IdentityError, Username};

#[derive(Debug, Clone)]
/// Parsed user profile loaded from the object store.
pub struct FingerProfile {
    username: Username,
    attributes: IndexMap<String, String>,
}

#[derive(Debug, Error)]
/// Errors returned when parsing or rendering user profiles.
pub enum ProfileError {
    #[error("profile data must be a table of key/value pairs")]
    NotATable,
    #[error("profile data must only contain string values (key: {key})")]
    NonStringValue { key: String },
    #[error("profile declares mismatched username")]
    UsernameMismatch,
    #[error("profile declares invalid username: {0}")]
    UsernameInvalid(IdentityError),
    #[error("profile must declare a username field")]
    MissingUsername,
    #[error("profile data must be UTF-8 encoded")]
    InvalidEncoding(#[from] std::str::Utf8Error),
    #[error("invalid profile data: {0}")]
    Toml(#[from] toml::de::Error),
}

impl FingerProfile {
    /// Parse the provided payload into a [`FingerProfile`].
    pub fn parse(username: Username, payload: &[u8]) -> Result<Self, ProfileError> {
        let text = std::str::from_utf8(payload)?;
        let document: toml::Value = toml::from_str(text)?;
        let table = document.as_table().ok_or(ProfileError::NotATable)?;

        let mut attributes = IndexMap::with_capacity(table.len());
        let mut declared_username = None;

        for (key, field_value) in table {
            if key == "username" {
                let declared = field_value
                    .as_str()
                    .ok_or_else(|| ProfileError::NonStringValue { key: key.clone() })?;
                declared_username = Some(declared.to_owned());
                continue;
            }

            let entry = field_value
                .as_str()
                .ok_or_else(|| ProfileError::NonStringValue { key: key.clone() })?;
            attributes.insert(key.clone(), entry.to_owned());
        }

        let declared = declared_username.ok_or(ProfileError::MissingUsername)?;
        let parsed = Username::parse(&declared).map_err(ProfileError::UsernameInvalid)?;
        if parsed != username {
            return Err(ProfileError::UsernameMismatch);
        }

        Ok(Self {
            username,
            attributes,
        })
    }

    /// Render the profile and optional plan into a response body.
    pub fn render(&self, include_plan: bool, plan: Option<&str>) -> ResponseBody {
        let mut lines = Vec::with_capacity(self.attributes.len() + 4);
        lines.push(format!("User: {}", self.username.as_str()));

        for (key, value) in &self.attributes {
            lines.push(format!("{}: {}", normalise_key(key), sanitise_line(value)));
        }

        if include_plan {
            lines.push(String::new());
            lines.push("Plan:".to_owned());
            Self::append_plan_lines(&mut lines, plan);
        }

        lines.push(String::new());
        lines.push("Powered by fingermouse".to_owned());

        ResponseBody { lines }
    }

    // Keep `render` linear by extracting the plan formatting decisions.
    fn append_plan_lines(lines: &mut Vec<String>, plan: Option<&str>) {
        match plan {
            Some(content) if content.trim().is_empty() => {
                lines.push("(empty plan)".to_owned());
            }
            Some(content) => {
                lines.extend(content.lines().map(sanitise_line));
            }
            None => lines.push("(no plan)".to_owned()),
        }
    }
}

#[derive(Debug, Clone)]
/// Textual response sent back to a finger client.
pub struct ResponseBody {
    lines: Vec<String>,
}

impl ResponseBody {
    /// Serialise the response body into CRLF-terminated bytes.
    pub fn as_bytes(&self) -> Vec<u8> {
        let estimated = self.lines.iter().map(|line| line.len() + 2).sum::<usize>();
        let mut buffer = CrlfBuffer::with_capacity(estimated);
        for line in &self.lines {
            buffer.push_line(line);
        }
        buffer.into_bytes()
    }
}

impl fmt::Display for ResponseBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for line in &self.lines {
            writeln!(f, "{line}")?;
        }
        Ok(())
    }
}

fn normalise_key(key: &str) -> String {
    let mut chars = key.chars();
    let mut output = String::with_capacity(key.len());
    if let Some(first) = chars.next() {
        output.push(first.to_ascii_uppercase());
        for ch in chars {
            output.push(if ch == '_' { ' ' } else { ch });
        }
    }
    output
}

fn sanitise_line(input: &str) -> String {
    input.chars().filter(|c| matches!(c, ' '..='~')).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;
    use rstest::rstest;

    fn profile_bytes(body: &str) -> &[u8] {
        body.as_bytes()
    }

    fn parse_username(input: &str) -> Username {
        Username::parse(input)
            .unwrap_or_else(|err| panic!("failed to parse username '{input}': {err}"))
    }

    fn expect_profile_error(username: Username, body: &str) -> ProfileError {
        match FingerProfile::parse(username, profile_bytes(body)) {
            Ok(_) => panic!("expected profile parsing to fail"),
            Err(err) => err,
        }
    }

    #[test]
    fn parses_profile() {
        let username = parse_username("alice");
        let body = r#"
            username = "alice"
            full_name = "Alice Smith"
            email = "alice@example.com"
        "#;
        let profile = FingerProfile::parse(username.clone(), profile_bytes(body))
            .unwrap_or_else(|err| panic!("profile parse failed: {err}"));
        let response = profile.render(false, None);
        let text = String::from_utf8(response.as_bytes())
            .unwrap_or_else(|err| panic!("utf8 conversion failed: {err}"));
        assert!(text.contains("User: alice"));
        assert!(text.contains("Full name: Alice Smith"));
        assert!(text.contains("Email: alice@example.com"));
    }

    #[test]
    fn rejects_mismatched_username() {
        let username = parse_username("alice");
        let body = r#"
            username = "bob"
            full_name = "Alice Smith"
        "#;
        let err = expect_profile_error(username, body);
        assert!(matches!(err, ProfileError::UsernameMismatch));
    }

    #[test]
    fn rejects_invalid_username_value() {
        let username = parse_username("alice");
        let body = r#"
            username = "bad name"
            full_name = "Alice Smith"
        "#;
        let err = expect_profile_error(username, body);
        assert!(matches!(err, ProfileError::UsernameInvalid(_)));
    }

    #[test]
    fn rejects_missing_username_field() {
        let username = parse_username("alice");
        let body = r#"
            full_name = "Alice Smith"
        "#;
        let err = expect_profile_error(username, body);
        assert!(matches!(err, ProfileError::MissingUsername));
    }

    #[test]
    fn rejects_non_string_value() {
        let username = parse_username("alice");
        let body = r#"
            username = "alice"
            age = 42
        "#;
        let err = expect_profile_error(username, body);
        assert!(matches!(err, ProfileError::NonStringValue { ref key } if key == "age"));
    }

    #[test]
    fn rejects_invalid_toml() {
        let username = parse_username("alice");
        let body = "username = \"alice\"\nfull_name =";
        let err = expect_profile_error(username, body);
        assert!(matches!(err, ProfileError::Toml(_)));
    }

    #[rstest]
    #[case(Some(""), "(empty plan)")]
    #[case(None, "(no plan)")]
    fn renders_plan_variants(#[case] plan: Option<&str>, #[case] expected: &str) {
        let username = parse_username("alice");
        let body = r#"
            username = "alice"
        "#;
        let profile = FingerProfile::parse(username, profile_bytes(body))
            .unwrap_or_else(|err| panic!("profile parse failed: {err}"));
        let response = profile.render(true, plan);
        let text = String::from_utf8(response.as_bytes())
            .unwrap_or_else(|err| panic!("utf8 conversion failed: {err}"));
        assert!(text.contains(expected));
    }
}
