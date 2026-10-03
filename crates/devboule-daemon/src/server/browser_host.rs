//! The three browser-host frames on the connection's own thread: register,
//! unregister, and a host's answer to a command.
//!
//! Every arm is a lock and a non-blocking hand-off to the broker. The answer
//! frame in particular only resolves the pending call it names; the thread that
//! made the call waits for it elsewhere, so this connection's reader is never
//! parked behind anybody.

use super::*;

use crate::browser_broker::ResponseDisposition;
use devboule_protocol::ErrorDetails;

const MAX_HOST_COMMANDS: usize = 64;
const MAX_COMMAND_NAME_BYTES: usize = 64;

/// Serve one browser-host frame for `conn`. The caller has already checked
/// that the connection negotiated `browser.host`; the peer gate (the first
/// statement of `dispatch`) has already refused every peer connection.
pub(super) fn dispatch_browser_host(
    state: &ServerState,
    conn: &Arc<ConnHandle>,
    request: ClientMessage,
) -> DaemonMessage {
    match request {
        ClientMessage::BrowserHostRegister {
            id,
            supported_commands,
        } => {
            if !valid_commands(&supported_commands) {
                return invalid(id, "supportedCommands is not a list of command names");
            }
            let host_id =
                state
                    .browser
                    .register(conn.id, Arc::clone(&conn.outbound), supported_commands);
            DaemonMessage::BrowserHostRegistered { id, host_id }
        }
        ClientMessage::BrowserHostUnregister { id, host_id } => {
            if state.browser.unregister(conn.id, &host_id) {
                DaemonMessage::Ok { id }
            } else {
                invalid(id, "that browser host is not registered on this connection")
            }
        }
        ClientMessage::BrowserExecuteResponse {
            id,
            request_id,
            host_id,
            outcome,
        } => match state
            .browser
            .accept_response(conn.id, &request_id, &host_id, outcome)
        {
            // A late or foreign answer is acknowledged like a good one: the
            // reply must not tell a stranger which calls are pending.
            ResponseDisposition::Delivered | ResponseDisposition::Dropped => {
                DaemonMessage::Ok { id }
            }
            ResponseDisposition::Refused(error) => DaemonMessage::Error(
                WireError::new(ErrorCode::InvalidRequest, error.message)
                    .with_id(id)
                    .with_details(ErrorDetails::BrowserRefused { code: error.code }),
            ),
        },
        other => {
            let mut error = WireError::new(ErrorCode::InvalidRequest, "not a browser host frame");
            if let Some(id) = other.request_id() {
                error = error.with_id(id);
            }
            DaemonMessage::Error(error)
        }
    }
}

fn invalid(id: u64, message: &str) -> DaemonMessage {
    DaemonMessage::Error(WireError::new(ErrorCode::InvalidRequest, message).with_id(id))
}

fn valid_commands(commands: &[String]) -> bool {
    commands.len() <= MAX_HOST_COMMANDS
        && commands.iter().all(|name| {
            !name.is_empty()
                && name.len() <= MAX_COMMAND_NAME_BYTES
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
}
