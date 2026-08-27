use std::fmt;
use std::io;

use devboule_protocol::WireError;

#[derive(Debug)]
pub enum DaemonError {
    AlreadyRunning,
    UnsupportedPlatform,
    Handshake(WireError),
    Protocol(String),
    TimedOut(String),
    Io(io::Error),
}

impl DaemonError {
    pub fn timed_out(what: impl Into<String>) -> Self {
        Self::TimedOut(what.into())
    }
}

impl fmt::Display for DaemonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => {
                write!(formatter, "another Devboule daemon is already running")
            }
            Self::UnsupportedPlatform => {
                write!(formatter, "devboule-daemon M3a targets Windows only")
            }
            Self::Handshake(error) => write!(formatter, "{}", error.message),
            Self::Protocol(message) => write!(formatter, "{message}"),
            Self::TimedOut(what) => write!(formatter, "timed out: {what}"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for DaemonError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for DaemonError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for DaemonError {
    fn from(error: serde_json::Error) -> Self {
        Self::Protocol(error.to_string())
    }
}
