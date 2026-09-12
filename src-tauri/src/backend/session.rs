//! Tauri session commands. These are forwarders: they validate, translate
//! to a protocol request, send it over the daemon pipe, and translate the
//! reply. The app owns no PTY. Output arrives as `SessionEventEnvelope`
//! frames are delivered to the `Channel<SessionEvent>` the frontend already
//! consumes — that Channel contract is unchanged.

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::State;

use devboule_daemon::{DaemonClient, DiagnosticsReport, SessionStateHandler};
use devboule_protocol::{
    ActiveTurnBehavior, ErrorCode, PermissionOutcome, Persistence, PersistenceKind,
    PromptAttachment, ResumeResult, SubscriptionId, MAX_WRITE_BYTES,
};

use crate::client::DaemonBridge;

use super::error::CommandError;

#[cfg(test)]
use devboule_daemon::SafeText;

pub use devboule_protocol::{
    validate_session_id, Session, SessionEvent, SessionKind, SessionStateSnapshot,
};

#[tauri::command]
pub fn session_create(
    bridge: State<'_, DaemonBridge>,
    workspace_id: Option<String>,
    kind: SessionKind,
    provider: Option<String>,
    mode: Option<String>,
) -> Result<Session, CommandError> {
    require_terminal_kind(&kind)?;
    Ok(require_client(&bridge)?.session_create_with(workspace_id, kind, provider, mode, None)?)
}

#[tauri::command]
pub fn session_resume(
    bridge: State<'_, DaemonBridge>,
    session_id: String,
) -> Result<ResumeResult, CommandError> {
    require_session_id(&session_id)?;
    Ok(require_client(&bridge)?.session_resume(
        Persistence {
            kind: PersistenceKind::Acp { handle: session_id },
        },
        None,
    )?)
}

/// IMPORTANT STARTUP ORDER: the client registers the Channel as the
/// session's event handler *before* it sends `session_attach`, so replay
/// frames that follow the attach reply cannot land on a missing subscriber. Live
/// reader output on the daemon waits until that attach is registered
/// under the stream mutex; there is no subscribe/snapshot race.
#[tauri::command]
pub fn session_attach(
    bridge: State<'_, DaemonBridge>,
    id: String,
    from_cursor: Option<u64>,
    ch: Channel<SessionEvent>,
) -> Result<SubscriptionId, CommandError> {
    require_session_id(&id)?;
    let sink = Arc::new(move |event| {
        let _ = ch.send(event);
    });
    Ok(bridge.session_attach(&id, from_cursor, sink)?)
}

/// Detach the current view without touching the process, reader, registry,
/// or scrollback. The daemon's idle-exit condition is clients==0 &&
/// sessions==0, so a detached-but-alive session keeps the daemon up.
#[tauri::command]
pub fn session_detach(
    bridge: State<'_, DaemonBridge>,
    subscription_id: SubscriptionId,
) -> Result<(), CommandError> {
    Ok(bridge.session_detach(subscription_id)?)
}

#[tauri::command]
pub fn session_claim(
    bridge: State<'_, DaemonBridge>,
    subscription_id: SubscriptionId,
) -> Result<(), CommandError> {
    Ok(bridge.session_claim(subscription_id)?)
}

#[tauri::command]
pub fn session_presence(
    bridge: State<'_, DaemonBridge>,
    focused_session_id: Option<String>,
    app_visible: bool,
) -> Result<(), CommandError> {
    // Presence is best-effort UI state: preserve errors for observability, while a lost hint only leaves a transiently stale badge.
    Ok(require_client(&bridge)?.session_presence(focused_session_id.as_deref(), app_visible)?)
}

/// Send one prompt.
///
/// `attachments` is optional rather than a bare `Vec`: the terminal surface and
/// every other caller that predates attachments sends no such key, and a missing
/// key for a bare `Vec` is an `invalid args` rejection rather than an empty
/// vector.
///
/// `active_turn_behavior` is the same kind of optional key for the slice-4
/// steering field: `"steer"` asks the daemon to deliver this text into a turn
/// that is already running, `"queue"` asks it to hold the text for the next
/// turn, and an absent key keeps the old interrupt-and-replace default. The
/// value travels as the protocol's own string; the daemon owns what the two
/// words mean.
///
/// The word is parsed to the protocol's own type on the way in
/// (`parse_active_turn_behavior` below), so a value the daemon would refuse is
/// refused here as `InvalidRequest` instead of travelling as a frame the daemon
/// answers with an error.
#[tauri::command]
pub fn session_send(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: SubscriptionId,
    text: String,
    attachments: Option<Vec<PromptAttachment>>,
    active_turn_behavior: Option<String>,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    require_write_size(&text)?;
    let attachments = attachments.unwrap_or_default();
    require_attachment_limits(&attachments)?;
    let active_turn_behavior = parse_active_turn_behavior(active_turn_behavior.as_deref())?;
    bridge.ensure_subscription_attached(subscription_id)?;
    Ok(require_client(&bridge)?.session_send_with_subscription(
        &id,
        subscription_id,
        &text,
        &attachments,
        active_turn_behavior,
    )?)
}

#[tauri::command]
pub fn session_permission_respond(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: SubscriptionId,
    request_id: String,
    outcome: PermissionOutcome,
    option_id: Option<String>,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    if request_id.is_empty() {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            "Permission request id is required.",
        ));
    }
    bridge.ensure_subscription_attached(subscription_id)?;
    Ok(
        require_client(&bridge)?.session_permission_respond_with_subscription(
            &id,
            subscription_id,
            &request_id,
            outcome,
            option_id.as_deref(),
        )?,
    )
}

#[tauri::command]
pub fn session_resize(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: SubscriptionId,
    cols: u16,
    rows: u16,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    bridge.ensure_subscription_attached(subscription_id)?;
    Ok(require_client(&bridge)?.session_resize_with_subscription(
        &id,
        subscription_id,
        cols,
        rows,
    )?)
}

#[tauri::command]
pub fn session_interrupt(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: SubscriptionId,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    bridge.ensure_subscription_attached(subscription_id)?;
    Ok(require_client(&bridge)?.session_interrupt_with_subscription(&id, subscription_id)?)
}

#[tauri::command]
pub fn session_set_model(
    bridge: State<'_, DaemonBridge>,
    id: String,
    model_id: Option<String>,
    effort: Option<String>,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    Ok(require_client(&bridge)?.session_set_model(&id, model_id.as_deref(), effort.as_deref())?)
}

/// Destroys the session. The subscription is optional: the wire `SessionClose`
/// frame carries only the session id, and the daemon authenticates the caller
/// as the session owner, so a session that never produced a subscription (a
/// startup that failed after `session_create`) can still be closed. Without
/// this, such a session stays alive in the daemon with nothing able to close
/// it, because every other teardown path is keyed on a subscription.
#[tauri::command]
pub fn session_set_mode(
    bridge: State<'_, DaemonBridge>,
    id: String,
    mode_id: String,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    Ok(require_client(&bridge)?.session_set_mode(&id, &mode_id)?)
}

#[tauri::command]
pub fn session_close(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: Option<SubscriptionId>,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    bridge.session_close(&id, subscription_id)?;
    bridge.forget_generation(&id);
    Ok(())
}

#[tauri::command]
pub fn sessions_list(bridge: State<'_, DaemonBridge>) -> Result<Vec<Session>, CommandError> {
    Ok(require_client(&bridge)?.sessions_list()?)
}

#[tauri::command]
pub fn daemon_diagnostics(
    bridge: State<'_, DaemonBridge>,
) -> Result<DiagnosticsReport, CommandError> {
    Ok(require_client(&bridge)?.daemon_diagnostics()?)
}

#[tauri::command]
pub fn sessions_watch(
    bridge: State<'_, DaemonBridge>,
    ch: Channel<Vec<SessionStateSnapshot>>,
) -> Result<(), CommandError> {
    let handler: SessionStateHandler = Arc::new(move |snapshots| {
        let _ = ch.send(snapshots);
    });
    Ok(bridge.sessions_watch(handler)?)
}

#[tauri::command]
pub fn sessions_unwatch(bridge: State<'_, DaemonBridge>) -> Result<(), CommandError> {
    Ok(bridge.sessions_unwatch()?)
}

fn require_client(bridge: &DaemonBridge) -> Result<Arc<DaemonClient>, CommandError> {
    bridge.client().map_err(disconnected)
}

fn disconnected(message: String) -> CommandError {
    CommandError::new(ErrorCode::Io, message)
}

fn require_session_id(id: &str) -> Result<(), CommandError> {
    validate_session_id(id).map_err(|message| CommandError::new(ErrorCode::InvalidRequest, message))
}

fn require_write_size(text: &str) -> Result<(), CommandError> {
    if text.len() > MAX_WRITE_BYTES {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            "Session input is too large.",
        ));
    }
    Ok(())
}

/// The same attachment limits the daemon enforces, refused here as well.
///
/// Both sides check, and the message comes from one place, for the reason the
/// [`MAX_WRITE_BYTES`] comment gives: an oversized or malformed request should
/// be answered before it becomes a pipe round-trip, and the daemon must not
/// depend on a client that may skip the check. `validate_attachments` is shared
/// rather than copied because five interdependent rules written twice are five
/// chances for the two sides to disagree about what the wire allows.
fn require_attachment_limits(attachments: &[PromptAttachment]) -> Result<(), CommandError> {
    devboule_protocol::validate_attachments(attachments)
        .map_err(|message| CommandError::new(ErrorCode::InvalidRequest, message))
}

fn require_terminal_kind(kind: &SessionKind) -> Result<(), CommandError> {
    match kind {
        SessionKind::Terminal
        | SessionKind::Acp
        | SessionKind::Claude
        | SessionKind::Pi
        | SessionKind::Codex => Ok(()),
    }
}

/// The app's `active_turn_behavior` word, as the protocol's own type.
///
/// The word travels as the protocol's (`"steer"`; absent is the daemon's
/// interrupt-and-replace default), so the parse is the protocol's too: serde is
/// what says which words exist, and a word the daemon would refuse is refused
/// here as `InvalidRequest` instead of travelling as a frame the daemon answers
/// with an error.
fn parse_active_turn_behavior(
    word: Option<&str>,
) -> Result<Option<ActiveTurnBehavior>, CommandError> {
    let Some(word) = word else {
        return Ok(None);
    };
    serde_json::from_value::<ActiveTurnBehavior>(serde_json::Value::String(word.to_string()))
        .map(Some)
        .map_err(|_| {
            CommandError::new(
                ErrorCode::InvalidRequest,
                format!("Unknown active turn behavior: {word}"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use devboule_protocol::{
        MAX_ATTACHMENTS_TOTAL_BYTES, MAX_ATTACHMENT_COUNT, MAX_ATTACHMENT_DATA_BYTES,
    };

    #[test]
    fn invalid_session_id_is_invalid_request() {
        let error = require_session_id("../other").expect_err("rejected");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "Invalid session id.");
    }

    #[test]
    fn oversized_write_is_invalid_request() {
        require_write_size(&"x".repeat(MAX_WRITE_BYTES)).expect("at cap");
        let error = require_write_size(&"x".repeat(MAX_WRITE_BYTES + 1)).expect_err("rejected");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "Session input is too large.");
    }

    #[test]
    fn only_a_word_the_protocol_has_is_a_send_behavior() {
        assert_eq!(
            parse_active_turn_behavior(None).expect("absent"),
            None,
            "an absent key is the daemon's interrupt-and-replace default"
        );
        assert_eq!(
            parse_active_turn_behavior(Some("steer")).expect("steer"),
            Some(ActiveTurnBehavior::Steer)
        );
        // `"queue"` is the word the app's TS union deliberately does not have:
        // no daemon branch implements it, so it is refused here rather than
        // travelling as a frame the daemon answers with an error.
        let error = parse_active_turn_behavior(Some("queue")).expect_err("refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "Unknown active turn behavior: queue");
    }

    fn attachment(mime_type: &str, data: String) -> PromptAttachment {
        PromptAttachment {
            name: "a.png".to_string(),
            mime_type: mime_type.to_string(),
            data,
        }
    }

    #[test]
    fn no_attachments_is_not_a_limit_violation() {
        require_attachment_limits(&[]).expect("an empty list is the common case");
    }

    #[test]
    fn attachments_at_the_count_limit_are_accepted() {
        let four = vec![attachment("image/png", "AA==".to_string()); MAX_ATTACHMENT_COUNT];
        require_attachment_limits(&four).expect("at the count cap");
    }

    #[test]
    fn an_attachment_limit_violation_is_invalid_request_and_names_the_limit() {
        let five = vec![attachment("image/png", "AA==".to_string()); MAX_ATTACHMENT_COUNT + 1];
        let error = require_attachment_limits(&five).expect_err("rejected");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains(&MAX_ATTACHMENT_COUNT.to_string()),
            "{}",
            error.message
        );

        let error = require_attachment_limits(&[attachment("image/gif", "AA==".to_string())])
            .expect_err("rejected");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("image/gif"), "{}", error.message);

        let error = require_attachment_limits(&[attachment(
            "image/png",
            "A".repeat(MAX_ATTACHMENT_DATA_BYTES + 4),
        )])
        .expect_err("rejected");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error
                .message
                .contains(&MAX_ATTACHMENT_DATA_BYTES.to_string()),
            "{}",
            error.message
        );

        let error =
            require_attachment_limits(&[attachment("image/png", "not base64!".to_string())])
                .expect_err("rejected");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            format!(
                "Attachment 1 ('a.png'): {}",
                devboule_protocol::invalid_base64_message()
            )
        );
    }

    /// The daemon module is `server`-gated, so the app cannot call into it. The
    /// limit set is one function in the protocol crate for exactly that reason;
    /// this asserts the two sides are looking at the same numbers.
    #[test]
    fn the_app_and_the_protocol_agree_on_the_attachment_limits() {
        assert_eq!(
            MAX_ATTACHMENT_COUNT,
            devboule_protocol::MAX_ATTACHMENT_COUNT
        );
        assert_eq!(
            MAX_ATTACHMENT_DATA_BYTES,
            devboule_protocol::MAX_ATTACHMENT_DATA_BYTES
        );
        assert_eq!(
            MAX_ATTACHMENTS_TOTAL_BYTES,
            devboule_protocol::MAX_ATTACHMENTS_TOTAL_BYTES
        );
    }

    #[test]
    fn supported_session_kinds_are_accepted() {
        require_terminal_kind(&SessionKind::Terminal).expect("terminal");
        require_terminal_kind(&SessionKind::Acp).expect("acp");
        require_terminal_kind(&SessionKind::Claude).expect("claude");
        require_terminal_kind(&SessionKind::Pi).expect("pi");
        require_terminal_kind(&SessionKind::Codex).expect("codex");
    }

    #[test]
    fn session_presence_forwarder_has_the_frozen_tauri_signature() {
        let _: fn(State<'_, DaemonBridge>, Option<String>, bool) -> Result<(), CommandError> =
            session_presence;
    }

    #[test]
    fn lost_daemon_connection_is_io() {
        let error = disconnected("The daemon connection was lost.".to_string());
        assert_eq!(error.code, ErrorCode::Io);
        assert_eq!(error.message, "The daemon connection was lost.");
    }

    #[test]
    fn safe_text_agrees_with_oracle_and_extends_it() {
        struct OracleCase {
            name: &'static str,
            input: &'static str,
            removed_literal: &'static str,
        }

        let oracle_cases = [
            OracleCase {
                name: "github token",
                input: "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
                removed_literal: "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            },
            OracleCase {
                name: "slack token",
                input: "xoxb-1234567890-1234567890-1234567890",
                removed_literal: "xoxb-1234567890-1234567890-1234567890",
            },
            OracleCase {
                name: "aws access key",
                input: "AKIA1234567890ABCDEF",
                removed_literal: "AKIA1234567890ABCDEF",
            },
            OracleCase {
                name: "bearer token",
                input: "Bearer abcdefghijklmnopqrstuvwxyz0123456789",
                removed_literal: "abcdefghijklmnopqrstuvwxyz0123456789",
            },
            OracleCase {
                name: "jwt",
                input: "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c",
                removed_literal: "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c",
            },
            OracleCase {
                name: "api key assignment",
                input: "api_key=super_secret_value_123",
                removed_literal: "super_secret_value_123",
            },
            OracleCase {
                name: "password assignment",
                input: "password = \"hunter2\"",
                removed_literal: "hunter2",
            },
            OracleCase {
                name: "high entropy base64",
                input: "Aa0Bb1Cc2Dd3Ee4Ff5Gg6Hh7Ii8Jj9Kk0Ll1Mm2Nn3Oo4Pp5",
                removed_literal: "Aa0Bb1Cc2Dd3Ee4Ff5Gg6Hh7Ii8Jj9Kk0Ll1Mm2Nn3Oo4Pp5",
            },
            OracleCase {
                name: "long hex",
                input: "0123456789abcdef0123456789abcdef01234567",
                removed_literal: "0123456789abcdef0123456789abcdef01234567",
            },
        ];

        for case in oracle_cases {
            let oracle = oracle_core::redact_secret_tokens(case.input);
            assert!(
                !oracle.contains(case.removed_literal),
                "corpus case no longer exercises oracle-core: {} -> {oracle:?}",
                case.name
            );
            let safe = SafeText::new(case.input);
            assert!(
                !safe.as_str().contains(case.removed_literal),
                "diagnostics redactor drift on {}: {:?}",
                case.name,
                safe.as_str()
            );
        }

        for (name, input, removed_literal) in [
            (
                "Windows home path",
                r"C:\Users\alice\secret-project",
                r"C:\Users\alice",
            ),
            (
                "Windows SID",
                "S-1-5-21-111-222-333-1001",
                "S-1-5-21-111-222-333-1001",
            ),
        ] {
            let oracle = oracle_core::redact_secret_tokens(input);
            assert!(
                oracle.contains(removed_literal),
                "diagnostics-only case unexpectedly belongs to oracle-core: {name}"
            );
            let safe = SafeText::new(input);
            assert!(
                !safe.as_str().contains(removed_literal),
                "diagnostics-only identifier survived: {name} -> {:?}",
                safe.as_str()
            );
        }
    }
}
