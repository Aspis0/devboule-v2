//! The browser-host surface: the typed failures, the caller context and the
//! outcome a browser command answers with.
//!
//! The desktop app registers as the one place that can run a browser command;
//! the daemon sends it a command on behalf of an agent and takes the answer
//! back. Everything here is data the two ends agree on. Who may register and
//! which connection may answer is the daemon's rule, not a field.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::MAX_FRAME_BYTES;

/// The most compact JSON a browser frame's `args` or `result` may carry
/// (900 KiB).
///
/// The rest of the frame is a request id, a host id, a command name and the
/// tags, a few hundred bytes; 900 KiB leaves well over 100 KiB of room under
/// [`MAX_FRAME_BYTES`], so a payload at the cap still fits one frame. A
/// larger one would otherwise fail on the pipe write and take the whole
/// connection down with it. Chunking a bigger result (a screenshot) is not
/// done here.
pub const MAX_BROWSER_PAYLOAD_BYTES: usize = 900 * 1024;

/// The longest error `message` a browser failure carries (2 KiB). The payload
/// cap covers only `result`; without this a host's error text could outgrow
/// [`MAX_FRAME_BYTES`] and cost the host its connection.
pub const MAX_BROWSER_ERROR_MESSAGE_BYTES: usize = 2 * 1024;

const _: () = assert!(MAX_BROWSER_PAYLOAD_BYTES + 16 * 1024 <= MAX_FRAME_BYTES);

/// Why a browser command did not produce a result. One list for the daemon's
/// own refusals and for the failures a host reports, so a caller matches one
/// vocabulary.
///
/// Mirrored by the `BrowserErrorCode` union in `src/types/ipc.ts`; alignment is
/// enforced by `browser_error_code_matches_frontend_union`.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum BrowserErrorCode {
    /// No browser host is registered, or the one serving the call went away.
    /// Retryable: the app may be starting.
    #[serde(rename = "browser_no_host")]
    NoHost,
    /// The host did not answer inside the call's deadline. Retryable.
    #[serde(rename = "browser_timeout")]
    Timeout,
    /// The pending-call budget (per host or daemon-wide) is spent. Retryable.
    #[serde(rename = "browser_busy")]
    Busy,
    /// The host's result would not fit one frame.
    #[serde(rename = "browser_result_too_large")]
    ResultTooLarge,
    /// The command's arguments would not fit one frame.
    #[serde(rename = "browser_args_too_large")]
    ArgsTooLarge,
    /// The host ran the command and reported a failure; its message rides along.
    #[serde(rename = "browser_host_error")]
    HostError,
    /// The host did not register this command.
    #[serde(rename = "browser_unsupported_command")]
    UnsupportedCommand,
    /// The host that owned the tab is gone. The call is not rerouted to
    /// another host, which would act on a different browser.
    #[serde(rename = "browser_owner_unavailable")]
    OwnerUnavailable,
    /// The tab is unknown, or belongs to another workspace than the caller's.
    /// Answered by the host.
    #[serde(rename = "browser_tab_not_found")]
    TabNotFound,
}

/// The name this code travels as, spelled once for the wire's own `serde`
/// rename and for the sentence an agent reads in a browser tool error, so the
/// two cannot drift apart.
impl BrowserErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoHost => "browser_no_host",
            Self::Timeout => "browser_timeout",
            Self::Busy => "browser_busy",
            Self::ResultTooLarge => "browser_result_too_large",
            Self::ArgsTooLarge => "browser_args_too_large",
            Self::HostError => "browser_host_error",
            Self::UnsupportedCommand => "browser_unsupported_command",
            Self::OwnerUnavailable => "browser_owner_unavailable",
            Self::TabNotFound => "browser_tab_not_found",
        }
    }
}

/// A failed browser command: the code, one sentence, and whether asking again
/// can help.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BrowserError {
    pub code: BrowserErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl BrowserError {
    /// The same error with `message` cut to [`MAX_BROWSER_ERROR_MESSAGE_BYTES`] at a
    /// character boundary, so the frame that carries it is always small.
    pub fn clamped(mut self) -> Self {
        if self.message.len() > MAX_BROWSER_ERROR_MESSAGE_BYTES {
            let mut end = MAX_BROWSER_ERROR_MESSAGE_BYTES;
            while !self.message.is_char_boundary(end) {
                end -= 1;
            }
            self.message.truncate(end);
        }
        self
    }

    /// A refusal the daemon makes itself. `retryable` follows the code: only
    /// the three conditions that clear on their own are retryable.
    pub fn daemon(code: BrowserErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: matches!(
                code,
                BrowserErrorCode::NoHost | BrowserErrorCode::Timeout | BrowserErrorCode::Busy
            ),
        }
    }
}

/// What a host answers one command with.
///
/// # Tab ownership
///
/// The daemon routes a later call for a tab back to the host that opened it,
/// and learns that owner from one place only: the successful `result` of a
/// tab-creating command (`new_tab`). **A tab-creating command returns
/// `{ "browserId": "<id>", ... }` at the top level of `result`.** The key is
/// the calling workspace plus that id, the first claim wins while its host is
/// connected, a conflicting claim is ignored, and a claim on a tab whose host
/// left replaces it. No other command's result is read for it. A successful
/// `close_tab` for a tab makes the daemon forget it, so its id may be reused.
/// `browserId` is at most 128 bytes.
///
/// A host's `message` is cut to [`MAX_BROWSER_ERROR_MESSAGE_BYTES`].
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BrowserOutcome {
    Ok { result: Value },
    Err(BrowserError),
}

/// Who asked for a command, as the daemon knows it: the agent session that made
/// the tool call and that session's workspace.
///
/// The daemon fills this from the caller's own registry row. It is never read
/// from the command's arguments, and a field of the same name in `args` is
/// dropped before the request leaves. The host scopes tabs by `workspace_id`:
/// a tab from another workspace is `browser_tab_not_found`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCaller {
    pub caller_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

/// One command, as the daemon pushes it to the host it chose.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserExecuteRequest {
    pub request_id: String,
    pub host_id: String,
    pub command: String,
    pub args: Value,
    pub caller: BrowserCaller,
}
