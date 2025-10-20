//! User profile representation and rendering.
use std::fmt;

use indexmap::IndexMap;
use thiserror::Error;

use crate::identity::Username;

#[derive(Debug, Clone)]
pub struct FingerProfile {
    username: Username,
    attributes: IndexMap<String, String>,
}

#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("profile data must be a table of key/value pairs")]
    NotATable,
    #[error("profile data must only contain string values (key: {key})")]
    NonStringValue { key: String },
    #[error("profile declares mismatched username")]
    UsernameMismatch,
    #[error("profile must declare a username field")]
    MissingUsername,
    #[error("profile data must be UTF-8 encoded")]
    InvalidEncoding(#[from] std::str::Utf8Error),
    #[error("invalid profile data: {0}")]
    Toml(#[from] toml::de::Error),
}

impl FingerProfile {
    pub fn parse(username: Username, payload: &[u8]) -> Result<Self, ProfileError> {
        let text = std::str::from_utf8(payload)?;
        let value: toml::Value = toml::from_str(text)?;
        let table = value.as_table().ok_or(ProfileError::NotATable)?;

        let mut attributes = IndexMap::with_capacity(table.len());
        let mut declared_username = None;

        for (key, value) in table {
            if key == "username" {
                let declared = value
                    .as_str()
                    .ok_or_else(|| ProfileError::NonStringValue { key: key.clone() })?;
                declared_username = Some(declared.to_string());
                continue;
            }

            let entry = value
                .as_str()
                .ok_or_else(|| ProfileError::NonStringValue { key: key.clone() })?;
            attributes.insert(key.clone(), entry.to_string());
        }

        let declared = declared_username.ok_or(ProfileError::MissingUsername)?;
        let parsed = Username::parse(&declared).map_err(|_| ProfileError::UsernameMismatch)?;
        if parsed != username {
            return Err(ProfileError::UsernameMismatch);
        }

        Ok(Self {
            username,
            attributes,
        })
    }

    pub fn render(&self, include_plan: bool, plan: Option<&str>) -> ResponseBody {
        let mut lines = Vec::with_capacity(self.attributes.len() + 4);
        lines.push(format!("User: {}", self.username.as_str()));

        for (key, value) in &self.attributes {
            lines.push(format!("{}: {}", normalise_key(key), sanitise_line(value)));
        }

        if include_plan {
            lines.push(String::new());
            lines.push("Plan:".to_string());
            if let Some(content) = plan {
                if content.trim().is_empty() {
                    lines.push("(empty plan)".to_string());
                } else {
                    for line in content.lines() {
                        lines.push(sanitise_line(line));
                    }
                }
            } else {
                lines.push("(no plan)".to_string());
            }
        }

        lines.push(String::new());
        lines.push("Powered by fingermouse".to_string());

        ResponseBody { lines }
    }
}

#[derive(Debug, Clone)]
pub struct ResponseBody {
    lines: Vec<String>,
}

impl ResponseBody {
    pub fn as_bytes(&self) -> Vec<u8> {
        let mut output = String::new();
        for line in &self.lines {
            output.push_str(line);
            output.push_str("\r\n");
        }
        output.into_bytes()
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

    #[rstest]
    fn parses_profile() {
        let username = match Username::parse("alice") {
            Ok(value) => value,
            Err(err) => panic!("username parse failed: {err}"),
        };
        let body = br#"
            username = "alice"
            full_name = "Alice Smith"
            email = "alice@example.com"
        "#;
        let profile = match FingerProfile::parse(username.clone(), body) {
            Ok(value) => value,
            Err(err) => panic!("profile parse failed: {err}"),
        };
        let response = profile.render(false, None);
        let text = match String::from_utf8(response.as_bytes()) {
            Ok(value) => value,
            Err(err) => panic!("utf8 conversion failed: {err}"),
        };
        assert!(text.contains("User: alice"));
        assert!(text.contains("Full name: Alice Smith"));
        assert!(text.contains("Email: alice@example.com"));
    }
}
