//! Helpers for assembling CRLF-framed responses.

/// Buffer that serialises individual lines using CRLF line endings.
#[derive(Debug, Default)]
pub struct CrlfBuffer {
    bytes: Vec<u8>,
}

impl CrlfBuffer {
    /// Create a buffer with the provided capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
        }
    }

    /// Append a line, ensuring it is terminated with CRLF without duplicating
    /// existing endings.
    pub fn push_line(&mut self, line: &str) {
        let trimmed = trim_line_endings(line);
        self.bytes.extend_from_slice(trimmed.as_bytes());
        self.bytes.extend_from_slice(b"\r\n");
    }

    /// Consume the buffer and return the accumulated bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Remove trailing CR and/or LF characters from a line.
pub fn trim_line_endings(line: &str) -> &str {
    line.trim_end_matches(&['\r', '\n'][..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("", b"\r\n")]
    #[case("status", b"status\r\n")]
    #[case("status\r\n", b"status\r\n")]
    #[case("status\r", b"status\r\n")]
    #[case("status\n", b"status\r\n")]
    fn push_line_normalises_endings(#[case] input: &str, #[case] expected: &[u8]) {
        let mut buf = CrlfBuffer::default();
        buf.push_line(input);
        assert_eq!(buf.into_bytes(), expected);
    }
}
