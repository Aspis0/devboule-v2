//! Mapping of Oracle failures onto the shared [`CommandError`] vocabulary, so
//! every command reports errors the panel can act on.

use std::fmt::Display;

use devboule_protocol::ErrorCode;

use crate::backend::error::CommandError;

pub(super) fn invalid_configuration(message: impl Into<String>) -> CommandError {
    CommandError::new(ErrorCode::InvalidRequest, message)
}

pub(super) fn unimplemented_command(message: impl Into<String>) -> CommandError {
    CommandError::new(ErrorCode::Unimplemented, message)
}

pub(super) fn core_error(context: &str, error: impl Display) -> CommandError {
    CommandError::new(ErrorCode::Internal, format!("{context}: {error}"))
}
