use std::fmt;
use std::io;

use devboule_daemon::DaemonError;
use devboule_protocol::{ErrorCode, WireError};

#[derive(Debug)]
pub enum PluginError {
    Handshake(WireError),
    Protocol(String),
    TimedOut(String),
    Io(io::Error),
    CapabilityNotSupported(String),
    ProcessExited,
}

impl PluginError {
    pub fn timed_out(what: impl Into<String>) -> Self {
        Self::TimedOut(what.into())
    }

    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Handshake(error) => error.code,
            Self::CapabilityNotSupported(_) => ErrorCode::CapabilityNotSupported,
            Self::TimedOut(_) | Self::Io(_) | Self::ProcessExited => ErrorCode::Io,
            Self::Protocol(_) => ErrorCode::Internal,
        }
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Handshake(error) => write!(formatter, "{}", error.message),
            Self::Protocol(message) => write!(formatter, "{message}"),
            Self::TimedOut(what) => write!(formatter, "timed out: {what}"),
            Self::Io(error) => write!(formatter, "{error}"),
            Self::CapabilityNotSupported(method) => {
                write!(
                    formatter,
                    "plugin method '{method}' was not in the granted capability set"
                )
            }
            Self::ProcessExited => write!(
                formatter,
                "the plugin backend process exited during the request"
            ),
        }
    }
}

impl std::error::Error for PluginError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for PluginError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<DaemonError> for PluginError {
    fn from(error: DaemonError) -> Self {
        match error {
            DaemonError::Handshake(wire) => Self::Handshake(wire),
            DaemonError::Io(error) => Self::Io(error),
            DaemonError::TimedOut(what) => Self::TimedOut(what),
            DaemonError::Protocol(message) => Self::Protocol(message),
            other => Self::Protocol(other.to_string()),
        }
    }
}

impl From<serde_json::Error> for PluginError {
    fn from(error: serde_json::Error) -> Self {
        Self::Protocol(error.to_string())
    }
}
