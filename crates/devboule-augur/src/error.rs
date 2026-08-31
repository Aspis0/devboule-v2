//! Failures a caller can show. Detectors that cannot run say so; they do not
//! invent an empty clean scan.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    Tool(String),
    Rules(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Sqlite(error) => write!(formatter, "{error}"),
            Self::Tool(message) | Self::Rules(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    pub fn tool(message: impl AsRef<str>) -> Self {
        Self::Tool(crate::finding::outbound_text(message.as_ref()))
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
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
        assert!(
            !shown.contains(&secret),
            "Error::Tool leaked a secret: {shown}"
        );
    }
}
