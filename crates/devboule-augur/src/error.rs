//! Failures a caller can show. Detectors that cannot run say so; they do not
//! invent an empty clean scan.
//!
//! Variants are not public tuples: every message is redacted at construction.
//! There is no `Error::Tool(raw_secret)` from outside this module.

use std::fmt;

use crate::finding::Outbound;

pub struct Error {
    kind: Kind,
}

enum Kind {
    Io(Outbound),
    Sqlite(Outbound),
    Tool(Outbound),
    Rules(Outbound),
}

impl Error {
    pub fn tool(message: impl AsRef<str>) -> Self {
        Self {
            kind: Kind::Tool(Outbound::new(message.as_ref())),
        }
    }

    pub fn rules(message: impl AsRef<str>) -> Self {
        Self {
            kind: Kind::Rules(Outbound::new(message.as_ref())),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            Kind::Io(text) | Kind::Sqlite(text) | Kind::Tool(text) | Kind::Rules(text) => {
                formatter.write_str(text.as_str())
            }
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Error")
            .field("message", &self.to_string())
            .finish()
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self {
            kind: Kind::Io(Outbound::new(&error.to_string())),
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self {
            kind: Kind::Sqlite(Outbound::new(&error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_error_does_not_echo_a_secret() {
        let secret = crate::tokens::aws_access_token();
        let error = Error::tool(format!("clippy failed: {secret}"));
        let shown = error.to_string();
        assert!(!shown.contains(&secret), "Error leaked a secret: {shown}");
        assert!(
            !format!("{error:?}").contains(&secret),
            "Error Debug leaked a secret: {error:?}"
        );
    }

    #[test]
    fn io_and_rules_errors_are_redacted_too() {
        let secret = crate::tokens::aws_access_token();
        let rules = Error::rules(format!("bad toml near {secret}"));
        assert!(!rules.to_string().contains(&secret));
        let io = Error::from(std::io::Error::other(format!("read {secret}")));
        assert!(!io.to_string().contains(&secret));
        assert!(!format!("{io:?}").contains(&secret));
    }
}
