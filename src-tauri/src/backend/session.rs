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
    ActiveTurnBehavior, AttachmentReference, ErrorCode, PermissionOutcome, Persistence,
    PersistenceKind, PromptAttachment, ResumeResult, SessionResumeInfo, SessionTask,
    StoredAttachment, SubscriptionId, MAX_WRITE_BYTES,
};
use serde::Serialize;

use crate::client::{AttachmentSink, DaemonBridge};

use super::blocking::off_main_thread;
use super::error::CommandError;

#[cfg(test)]
use devboule_daemon::SafeText;

pub use devboule_protocol::{
    validate_session_id, Session, SessionEvent, SessionKind, SessionStateSnapshot,
};

/// The window must not wait on this call.
///
/// A non-`async` command is invoked on the main thread, so the wait for the
/// daemon's `session_create` — which runs the provider's whole handshake
/// inline, up to `SESSION_CREATE_RPC_TIMEOUT` — would freeze the window for
/// as long as the daemon takes. `off_main_thread` is where the wait goes.
///
/// `cols`/`rows` are the grid a laid-out terminal view last fitted, present
/// only when one has; both travel or neither does, and the daemon judges the
/// pair (falling back to its default for an absent or absurd ask). Absent,
/// not zero, before any fit: the daemon reads a missing field as "no size
/// asked".
#[tauri::command]
pub async fn session_create(
    bridge: State<'_, DaemonBridge>,
    workspace_id: Option<String>,
    kind: SessionKind,
    provider: Option<String>,
    mode: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<Session, CommandError> {
    require_terminal_kind(&kind)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || {
        client.session_create_with(workspace_id, kind, provider, mode, cols, rows, None)
    })
    .await
}

/// The window must not wait on this call: the daemon answers only after the
/// provider startup it runs inline has finished, and the recovery road can
/// carry a second startup — the measured freeze is the `Not Responding` of
/// `scout/user-pass/f08b.png`.
#[tauri::command]
pub async fn session_resume(
    bridge: State<'_, DaemonBridge>,
    session_id: String,
) -> Result<ResumeResult, CommandError> {
    require_session_id(&session_id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || {
        client.session_resume(
            Persistence {
                kind: PersistenceKind::Acp { handle: session_id },
            },
            None,
        )
    })
    .await
}

/// One frame on a session's channel: a daemon event, or the reset an attach's
/// reply named.
///
/// Untagged on the wire: every `SessionEvent` carries `type` and a reset does
/// not, so the frontend tells the two apart by that field's absence and no
/// event variant is spent on a message that is not one.
// A boxed event would be one heap allocation per transcript row, and this is
// built once per frame; the wide variant is the frame, the narrow one is the
// marker that arrives once per attach.
#[allow(clippy::large_enum_variant)]
#[derive(Serialize)]
#[serde(untagged)]
pub enum SessionAttachMessage {
    Event(SessionEvent),
    Reset(SessionResumeInfo),
}

/// IMPORTANT STARTUP ORDER: the client registers the Channel as the
/// session's event handler *before* it sends `session_attach`, so replay
/// frames that follow the attach reply cannot land on a missing subscriber. Live
/// reader output on the daemon waits until that attach is registered
/// under the stream mutex; there is no subscribe/snapshot race.
///
/// A reset lands on the same channel and by the same rule: the daemon client's
/// reader thread raises it at the frame where it matches the attach reply, so
/// the view has replaced its timeline before the first replayed row of that
/// attach reaches it. The bridge sends it from that hook, never from the
/// command body.
#[tauri::command]
pub async fn session_attach(
    bridge: State<'_, DaemonBridge>,
    id: String,
    from_cursor: Option<u64>,
    ch: Channel<SessionAttachMessage>,
) -> Result<SubscriptionId, CommandError> {
    require_session_id(&id)?;
    let events = ch.clone();
    let reset = ch.clone();
    let sink = AttachmentSink {
        events: Arc::new(move |event| {
            let _ = events.send(SessionAttachMessage::Event(event));
        }),
        reset: Arc::new(move |answer| {
            let _ = reset.send(SessionAttachMessage::Reset(answer));
        }),
    };
    let inner = bridge.shared();
    off_main_thread(move || inner.session_attach(&id, from_cursor, sink)).await
}

/// Detach the current view without touching the process, reader, registry,
/// or scrollback. The daemon's idle-exit condition is clients==0 &&
/// sessions==0, so a detached-but-alive session keeps the daemon up.
#[tauri::command]
pub async fn session_detach(
    bridge: State<'_, DaemonBridge>,
    subscription_id: SubscriptionId,
) -> Result<(), CommandError> {
    let inner = bridge.shared();
    off_main_thread(move || inner.session_detach(subscription_id)).await
}

#[tauri::command]
pub async fn session_claim(
    bridge: State<'_, DaemonBridge>,
    subscription_id: SubscriptionId,
) -> Result<(), CommandError> {
    let inner = bridge.shared();
    off_main_thread(move || inner.session_claim(subscription_id)).await
}

#[tauri::command]
pub async fn session_presence(
    bridge: State<'_, DaemonBridge>,
    focused_session_id: Option<String>,
    app_visible: bool,
) -> Result<(), CommandError> {
    // Presence is best-effort UI state: preserve errors for observability, while a lost hint only leaves a transiently stale badge.
    let client = require_client(&bridge)?;
    off_main_thread(move || client.session_presence(focused_session_id.as_deref(), app_visible))
        .await
}

/// Send one prompt.
///
/// `attachments` is optional rather than a bare `Vec`: the terminal surface and
/// every other caller that predates attachments sends no such key, and a missing
/// key for a bare `Vec` is an `invalid args` rejection rather than an empty
/// vector.
///
/// `attachment_references` is the same kind of optional key, and it is the other
/// half of the composer's deposits: a page stored by `session_deposit` is named
/// here by the reference that call answered with, and the daemon resolves it
/// against the session's store. Optional for the same reason as the inline list
/// — a caller that predates deposits sends no key and gets the empty list.
///
/// `idempotency_key` is the send's retry identity. Absent for every send the
/// app does not intend to repeat; the app's message queue names the queued
/// item's own id, so a rung of its retry ladder that the daemon already took
/// is answered from the receipt rather than run again as a second prompt.
///
/// `active_turn_behavior` is the same kind of optional key for the slice-4
/// steering field: `"steer"` asks the daemon to deliver this text into a turn
/// that is already running, and an absent key asks nothing of a running turn —
/// the plain path starts its turn and interrupts nothing, so a caller that
/// means to replace a running turn sends `session_interrupt` first. The only
/// other spelling the protocol carries is refused here, so a misspelling costs
/// an `InvalidRequest` rather than a frame the daemon answers with an error.
///
/// The word is parsed to the protocol's own type on the way in
/// (`parse_active_turn_behavior` below), so a value the daemon would refuse is
/// refused as `InvalidRequest` before it leaves.
//
// The argument list is the wire's own: Tauri's macro reads these parameters to
// build the command's JS argument names, so the struct that would quiet
// `too_many_arguments` here would rename the keys `src/lib/tauri.ts` sends.
// The frozen-signature test in this file is what keeps the eight honest.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn session_send(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: SubscriptionId,
    text: String,
    attachments: Option<Vec<PromptAttachment>>,
    active_turn_behavior: Option<String>,
    attachment_references: Option<Vec<AttachmentReference>>,
    idempotency_key: Option<String>,
) -> Result<bool, CommandError> {
    require_session_id(&id)?;
    require_write_size(&text)?;
    let attachments = attachments.unwrap_or_default();
    require_attachment_limits(&attachments)?;
    let attachment_references = attachment_references.unwrap_or_default();
    require_attachment_reference_limits(&id, &attachment_references)?;
    require_idempotency_key(idempotency_key.as_deref())?;
    let active_turn_behavior = parse_active_turn_behavior(active_turn_behavior.as_deref())?;
    let client = require_client(&bridge)?;
    let inner = bridge.shared();
    off_main_thread(move || {
        inner.ensure_subscription_attached(subscription_id)?;
        client.session_send_with_subscription(
            &id,
            subscription_id,
            &text,
            &attachments,
            &attachment_references,
            active_turn_behavior,
            idempotency_key,
        )
    })
    .await
}

/// The shared follow-up queue, protocol 22's five frames behind the
/// `session.queue` capability.
///
/// All five carry `client_operation_id`, one per user intent: the daemon
/// answers a repeat of an operation it already answered instead of queueing a
/// second row or sending a row twice, and refuses the same id with different
/// bytes. The client half of that rule lives in `daemonQueue.ts`; what is
/// here is the same forwarder shape as every other command — validate, wait
/// off the window's thread, map the daemon's `Error` frame by the same `?`.
///
/// The queue's own list is not here: it arrives as a `queue_snapshot` event on
/// the session's channel, which is why nothing in this file sends queued text
/// to the agent. These commands only change it.
#[tauri::command]
pub async fn session_queue_add(
    bridge: State<'_, DaemonBridge>,
    id: String,
    client_operation_id: String,
    text: String,
    attachments: Option<Vec<PromptAttachment>>,
    attachment_references: Option<Vec<AttachmentReference>>,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    require_write_size(&text)?;
    let attachments = attachments.unwrap_or_default();
    require_attachment_limits(&attachments)?;
    let attachment_references = attachment_references.unwrap_or_default();
    require_attachment_reference_limits(&id, &attachment_references)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || {
        client.session_queue_add(
            &id,
            &client_operation_id,
            &text,
            &attachments,
            &attachment_references,
        )
    })
    .await
}

/// Replace one queued row's text, keeping its place: the daemon edits the row
/// in place rather than taking it out and putting it back.
#[tauri::command]
pub async fn session_queue_edit(
    bridge: State<'_, DaemonBridge>,
    id: String,
    client_operation_id: String,
    item_id: String,
    text: String,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    require_write_size(&text)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.session_queue_edit(&id, &client_operation_id, &item_id, &text))
        .await
}

/// Take one queued row out of the queue. A row a send has already claimed is
/// not in the queue, and the daemon says so.
#[tauri::command]
pub async fn session_queue_remove(
    bridge: State<'_, DaemonBridge>,
    id: String,
    client_operation_id: String,
    item_id: String,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.session_queue_remove(&id, &client_operation_id, &item_id)).await
}

/// Move one queued row, counting `to_index` in the queue the row has already
/// left. The daemon refuses an index the queue has no place for, so the client
/// clamps before it asks.
#[tauri::command]
pub async fn session_queue_move(
    bridge: State<'_, DaemonBridge>,
    id: String,
    client_operation_id: String,
    item_id: String,
    to_index: usize,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || {
        client.session_queue_move(&id, &client_operation_id, &item_id, to_index)
    })
    .await
}

/// Send one queued row now. This is a send — it interrupts the running turn
/// and opens a new one with that row's text — so it carries the subscription
/// and waits through the same attachment guard every other send does.
#[tauri::command]
pub async fn session_queue_send_now(
    bridge: State<'_, DaemonBridge>,
    id: String,
    client_operation_id: String,
    subscription_id: SubscriptionId,
    item_id: String,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    let client = require_client(&bridge)?;
    let inner = bridge.shared();
    off_main_thread(move || {
        inner.ensure_subscription_attached(subscription_id)?;
        client.session_queue_send_now(&id, &client_operation_id, subscription_id, &item_id)
    })
    .await
}

/// Store one attachment for a session and answer the reference a later
/// `session_send` names it by.
///
/// The forwarder is `session_send`'s, minus the subscription: `SessionDeposit`
/// carries no subscription id, so there is no registration to check and no
/// attach to ensure. Everything else is deliberately the same — the session id
/// is validated here, the wire's own attachment limits are enforced here before
/// the frame leaves (the same `validate_attachments` the daemon runs, so an
/// oversized page is refused as a round trip it never makes), and the daemon's
/// `Error` frame is mapped to a `CommandError` by the same `?`.
///
/// One attachment per call is the shape of the protocol, not a choice made
/// here: the caller deposits the pages of a deck one after the other, and the
/// reference this answers with is a value the caller cannot compute (the digest
/// is of the bytes as *stored*).
#[tauri::command]
pub async fn session_deposit(
    bridge: State<'_, DaemonBridge>,
    id: String,
    attachment: PromptAttachment,
) -> Result<AttachmentReference, CommandError> {
    require_session_id(&id)?;
    require_attachment_limits(std::slice::from_ref(&attachment))?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.session_deposit(&id, &attachment)).await
}

/// Open one chunked file upload, or adopt the one already in progress under
/// `upload_id`, and answer the offset it stands at.
///
/// The forwarder is `session_deposit`'s, minus the attachment: the session is
/// validated here under both names it travels by — the app's `id` and the
/// frame's `sessionId` — the upload id is validated here by the same
/// `validate_upload_id` the daemon runs, so a malformed token is refused
/// instead of becoming a round trip, and the daemon's `Error` frame is mapped
/// to a `CommandError` by the same `?`.
#[tauri::command]
pub async fn session_upload_begin(
    bridge: State<'_, DaemonBridge>,
    id: String,
    session_id: String,
    upload_id: String,
    name: String,
    total_bytes: u64,
) -> Result<u64, CommandError> {
    require_session_id(&id)?;
    require_session_id(&session_id)?;
    require_upload_id(&upload_id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || {
        client.session_upload_begin(&session_id, &upload_id, &name, total_bytes)
    })
    .await
}

/// Ask how many bytes of one upload the daemon holds.
#[tauri::command]
pub async fn session_upload_status(
    bridge: State<'_, DaemonBridge>,
    id: String,
    session_id: String,
    upload_id: String,
) -> Result<u64, CommandError> {
    require_session_id(&id)?;
    require_session_id(&session_id)?;
    require_upload_id(&upload_id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.session_upload_status(&session_id, &upload_id)).await
}

/// Append one chunk of an upload at exactly the offset the upload stands at.
#[tauri::command]
pub async fn session_upload_chunk(
    bridge: State<'_, DaemonBridge>,
    id: String,
    session_id: String,
    upload_id: String,
    offset: u64,
    data: String,
) -> Result<u64, CommandError> {
    require_session_id(&id)?;
    require_session_id(&session_id)?;
    require_upload_id(&upload_id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.session_upload_chunk(&session_id, &upload_id, offset, &data))
        .await
}

/// Close one fully received upload and answer the reference a send names.
#[tauri::command]
pub async fn session_upload_finish(
    bridge: State<'_, DaemonBridge>,
    id: String,
    session_id: String,
    upload_id: String,
) -> Result<AttachmentReference, CommandError> {
    require_session_id(&id)?;
    require_session_id(&session_id)?;
    require_upload_id(&upload_id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.session_upload_finish(&session_id, &upload_id)).await
}

/// Discard one upload and the bytes received so far. An id the daemon does not
/// hold is an `Ok`: there is nothing left to discard.
#[tauri::command]
pub async fn session_upload_abort(
    bridge: State<'_, DaemonBridge>,
    id: String,
    session_id: String,
    upload_id: String,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    require_session_id(&session_id)?;
    require_upload_id(&upload_id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.session_upload_abort(&session_id, &upload_id)).await
}

/// Delete one stored attachment and release the bytes it held.
///
/// The reference is the value a deposit or an upload finish answered with,
/// verbatim: the daemon re-stats the file and refuses a disagreement, so
/// re-deriving a digest or a size here would be naming a different file.
#[tauri::command]
pub async fn session_attachment_delete(
    bridge: State<'_, DaemonBridge>,
    reference: AttachmentReference,
) -> Result<(), CommandError> {
    require_session_id(&reference.session_id)?;
    require_attachment_reference_limits(&reference.session_id, std::slice::from_ref(&reference))?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.session_attachment_delete(&reference)).await
}

/// Read back the bytes of one deposited attachment, by reference.
///
/// The forwarder is `session_deposit`'s, minus the attachment: the reference
/// carries its session, so there is no separate id to validate — the wire's
/// own reference limits run here before the frame leaves (the same
/// `validate_attachment_references` the daemon runs, so a malformed digest
/// is refused as a round trip it never makes), and the daemon's `Error`
/// frame is mapped to a `CommandError` by the same `?`.
///
/// One reference per call is the shape of the protocol, not a choice made
/// here: the reply carries at most the artifact cap, well under the frame
/// ceiling, and a second reference would be a second answer with nowhere to
/// put it.
#[tauri::command]
pub async fn session_attachment_read(
    bridge: State<'_, DaemonBridge>,
    reference: AttachmentReference,
) -> Result<StoredAttachment, CommandError> {
    require_attachment_reference_limits(&reference.session_id, std::slice::from_ref(&reference))?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.session_attachment_read(&reference)).await
}

#[tauri::command]
pub async fn session_permission_respond(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: SubscriptionId,
    request_id: String,
    outcome: PermissionOutcome,
    option_id: Option<String>,
    answer: Option<String>,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    if request_id.is_empty() {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            "Permission request id is required.",
        ));
    }
    let client = require_client(&bridge)?;
    let inner = bridge.shared();
    off_main_thread(move || {
        inner.ensure_subscription_attached(subscription_id)?;
        client.session_permission_respond_with_subscription(
            &id,
            subscription_id,
            &request_id,
            outcome,
            option_id.as_deref(),
            answer.as_deref(),
        )
    })
    .await
}

#[tauri::command]
pub async fn session_resize(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: SubscriptionId,
    cols: u16,
    rows: u16,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    let client = require_client(&bridge)?;
    let inner = bridge.shared();
    off_main_thread(move || {
        inner.ensure_subscription_attached(subscription_id)?;
        client.session_resize_with_subscription(&id, subscription_id, cols, rows)
    })
    .await
}

#[tauri::command]
pub async fn session_interrupt(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: SubscriptionId,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    let client = require_client(&bridge)?;
    let inner = bridge.shared();
    off_main_thread(move || {
        inner.ensure_subscription_attached(subscription_id)?;
        client.session_interrupt_with_subscription(&id, subscription_id)
    })
    .await
}

#[tauri::command]
pub async fn session_set_model(
    bridge: State<'_, DaemonBridge>,
    id: String,
    model_id: Option<String>,
    effort: Option<String>,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.session_set_model(&id, model_id.as_deref(), effort.as_deref()))
        .await
}

/// Destroys the session. The subscription is optional: the wire `SessionClose`
/// frame carries only the session id, and the daemon authenticates the caller
/// as the session owner, so a session that never produced a subscription (a
/// startup that failed after `session_create`) can still be closed. Without
/// this, such a session stays alive in the daemon with nothing able to close
/// it, because every other teardown path is keyed on a subscription.
#[tauri::command]
pub async fn session_set_mode(
    bridge: State<'_, DaemonBridge>,
    id: String,
    mode_id: String,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.session_set_mode(&id, &mode_id)).await
}

/// Rename a session. The daemon validates the name, stores it on the session
/// record and the journal row, and pushes the roster — the app's row picks
/// the carried name up with no further write (`workspaceSessions.ts`).
#[tauri::command]
pub async fn session_set_name(
    bridge: State<'_, DaemonBridge>,
    id: String,
    display_name: String,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.session_set_name(&id, &display_name)).await
}

#[tauri::command]
pub async fn session_set_feature(
    bridge: State<'_, DaemonBridge>,
    id: String,
    feature_id: String,
    enabled: bool,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.session_set_feature(&id, &feature_id, enabled)).await
}

#[tauri::command]
pub async fn session_close(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: Option<SubscriptionId>,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    let inner = bridge.shared();
    off_main_thread(move || {
        inner.session_close(&id, subscription_id)?;
        inner.forget_generation(&id);
        Ok::<(), CommandError>(())
    })
    .await
}

/// Stops a session's running process but keeps the session: id, scrollback,
/// metadata. Same argument shape as `session_close` — a live subscription
/// when the caller holds one, nothing when the tab was never attached — but
/// no `forget_generation`: the instance died, it was not replaced, so the
/// generation is unchanged and a reconnecting client must still recognize it.
#[tauri::command]
pub async fn session_stop(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: Option<SubscriptionId>,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    let inner = bridge.shared();
    off_main_thread(move || inner.session_stop(&id, subscription_id)).await
}

/// One session's background-task list as the daemon derives it now.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTaskList {
    tasks: Vec<SessionTask>,
    omitted: u32,
}

/// Read the list once, when a view attaches: the snapshot events carry it from
/// then on, so this only fills the gap until the first change.
#[tauri::command]
pub async fn session_tasks(
    bridge: State<'_, DaemonBridge>,
    id: String,
) -> Result<SessionTaskList, CommandError> {
    require_session_id(&id)?;
    let client = require_client(&bridge)?;
    let (tasks, omitted) = off_main_thread(move || client.session_tasks(&id)).await?;
    Ok(SessionTaskList { tasks, omitted })
}

#[tauri::command]
pub async fn sessions_list(bridge: State<'_, DaemonBridge>) -> Result<Vec<Session>, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || client.sessions_list()).await
}

#[tauri::command]
pub async fn daemon_diagnostics(
    bridge: State<'_, DaemonBridge>,
) -> Result<DiagnosticsReport, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || client.daemon_diagnostics()).await
}

#[tauri::command]
pub async fn sessions_watch(
    bridge: State<'_, DaemonBridge>,
    ch: Channel<Vec<SessionStateSnapshot>>,
) -> Result<(), CommandError> {
    let handler: SessionStateHandler = Arc::new(move |snapshots| {
        let _ = ch.send(snapshots);
    });
    let inner = bridge.shared();
    off_main_thread(move || inner.sessions_watch(handler)).await
}

#[tauri::command]
pub async fn sessions_unwatch(bridge: State<'_, DaemonBridge>) -> Result<(), CommandError> {
    let inner = bridge.shared();
    off_main_thread(move || inner.sessions_unwatch()).await
}

fn require_client(bridge: &DaemonBridge) -> Result<Arc<DaemonClient>, CommandError> {
    bridge.client().map_err(disconnected)
}

fn disconnected(message: String) -> CommandError {
    CommandError::new(ErrorCode::Io, message)
}

pub(super) fn require_session_id(id: &str) -> Result<(), CommandError> {
    validate_session_id(id).map_err(|message| CommandError::new(ErrorCode::InvalidRequest, message))
}

/// The upload's client-chosen token, checked on this side of the pipe as well.
///
/// Same reason as [`require_attachment_limits`]: the daemon runs
/// `validate_upload_id` on every upload frame, and an id this side invented
/// wrong — empty, longer than 64 characters, or carrying a character outside
/// the token alphabet — should cost the caller a rejection, not a round trip
/// the daemon answers after the frame has already been queued.
fn require_upload_id(upload_id: &str) -> Result<(), CommandError> {
    devboule_protocol::validate_upload_id(upload_id)
        .map_err(|message| CommandError::new(ErrorCode::InvalidRequest, message))
}

/// The send's retry identity, checked on this side of the pipe as well.
///
/// Same reason as [`require_attachment_limits`]: the daemon runs
/// `validate_idempotency_key` on every keyed request, and a key this side
/// invented wrong — an empty one, a character the wire's id alphabet does not
/// carry — should cost the caller a rejection, not a round trip that the daemon
/// answers after the frame has already been queued.
pub(super) fn require_idempotency_key(key: Option<&str>) -> Result<(), CommandError> {
    let Some(key) = key else {
        return Ok(());
    };
    devboule_protocol::validate_idempotency_key(key)
        .map_err(|message| CommandError::new(ErrorCode::InvalidRequest, message))
}

pub(super) fn require_write_size(text: &str) -> Result<(), CommandError> {
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
pub(super) fn require_attachment_limits(
    attachments: &[PromptAttachment],
) -> Result<(), CommandError> {
    devboule_protocol::validate_attachments(attachments)
        .map_err(|message| CommandError::new(ErrorCode::InvalidRequest, message))
}

/// The reference half of a send's attachment limits, refused here as well.
///
/// Same argument as [`require_attachment_limits`], and the same shared function
/// the daemon runs: `validate_attachment_references` is what decides whether a
/// reference names this session, whether it is a digest as the store writes
/// one, and whether the stored bytes it points at would take the owner over the
/// store's budget. A reference that fails any of those is refused on this side
/// of the pipe instead of as a frame round trip.
pub(super) fn require_attachment_reference_limits(
    session_id: &str,
    references: &[AttachmentReference],
) -> Result<(), CommandError> {
    devboule_protocol::validate_attachment_references(session_id, references)
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
/// The word travels as the protocol's (`"steer"`; absent asks nothing of a
/// running turn), so the parse is the protocol's too: serde is what says which
/// words exist, and a word the daemon would refuse is refused here as
/// `InvalidRequest` instead of travelling as a frame the daemon answers with an
/// error.
pub(super) fn parse_active_turn_behavior(
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
    fn a_send_retry_identity_is_checked_before_the_pipe() {
        // The shape the app's message queue puts on a queued or steered send.
        require_idempotency_key(Some("s.app-4242.00000001.queued-3")).expect("queue key");
        require_idempotency_key(None).expect("a plain send names no identity");
        let empty = require_idempotency_key(Some("")).expect_err("rejected");
        assert_eq!(empty.code, ErrorCode::InvalidRequest);
        // The wire's own alphabet and cap, so a key the daemon could never
        // store is refused as a round trip it does not make.
        let colon = require_idempotency_key(Some("queued:3")).expect_err("rejected");
        assert_eq!(colon.code, ErrorCode::InvalidRequest);
        let long = require_idempotency_key(Some(&"k".repeat(129))).expect_err("rejected");
        assert_eq!(long.code, ErrorCode::InvalidRequest);
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
            "an absent key is the plain send that interrupts nothing"
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

        let error = require_attachment_limits(&[attachment("image/bmp", "AA==".to_string())])
            .expect_err("rejected");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("image/bmp"), "{}", error.message);

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

    /// The frontend tells a frame from a reset by the absence of `type`, so
    /// the reset has to reach the channel as the daemon's bare object and an
    /// event has to keep its discriminator. Nothing else on this seam
    /// serializes: the registry tests hand `SessionResumeInfo` to the sink
    /// directly, past the wire.
    #[test]
    fn the_untagged_channel_message_serializes_both_of_its_arms() {
        use devboule_protocol::{
            Cursor, NoticeSeverity, SessionResumeOutcome, SessionResumeReason, SessionResumeTail,
        };

        let frame =
            serde_json::to_value(SessionAttachMessage::Event(SessionEvent::SessionNotice {
                text: "a frame".to_string(),
                severity: NoticeSeverity::Info,
            }))
            .expect("a notice serializes");
        assert_eq!(
            frame.get("type").and_then(|value| value.as_str()),
            Some("session_notice")
        );

        let marker = serde_json::to_value(SessionAttachMessage::Reset(SessionResumeInfo {
            resume: SessionResumeOutcome::Reset {
                reason: SessionResumeReason::EpochChanged,
                tail: SessionResumeTail {
                    cursor: Cursor {
                        generation: 1,
                        seq: 9,
                    },
                    events: Vec::new(),
                    tail_complete: true,
                },
            },
            oldest_seq: 0,
            head: 9,
        }))
        .expect("a reset serializes");
        assert!(
            marker.get("type").is_none(),
            "the frontend reads a present `type` as an event: {marker}"
        );
        assert_eq!(
            marker.get("outcome").and_then(|value| value.as_str()),
            Some("reset")
        );
    }

    /// The Tauri boundary `src/lib/tauri.ts` is written against: `{
    /// workspaceId, kind, provider, mode, cols?, rows? }` in, the session out.
    /// The size pair is optional in both spellings — absent keys from a
    /// caller that measured nothing, and Tauri derives the JS-side key names
    /// from these parameters, so a rename here silently changes the command's
    /// argument shape.
    #[allow(clippy::type_complexity)]
    #[test]
    fn session_create_forwarder_has_the_frozen_tauri_signature() {
        fn frozen<Fut: std::future::Future<Output = Result<Session, CommandError>>>(
            _: fn(
                State<'static, DaemonBridge>,
                Option<String>,
                SessionKind,
                Option<String>,
                Option<String>,
                Option<u16>,
                Option<u16>,
            ) -> Fut,
        ) {
        }
        frozen(session_create);
    }

    #[test]
    fn session_presence_forwarder_has_the_frozen_tauri_signature() {
        fn frozen<Fut: std::future::Future<Output = Result<(), CommandError>>>(
            _: fn(State<'static, DaemonBridge>, Option<String>, bool) -> Fut,
        ) {
        }
        frozen(session_presence);
    }

    /// The Tauri boundary `src/lib/tauri.ts` is written against: `{ id,
    /// attachment }` in, the stored reference out. Tauri derives the JS-side key
    /// names from these parameters, so a rename here silently changes the
    /// command's argument shape.
    #[test]
    fn session_deposit_forwarder_has_the_frozen_tauri_signature() {
        fn frozen<Fut: std::future::Future<Output = Result<AttachmentReference, CommandError>>>(
            _: fn(State<'static, DaemonBridge>, String, PromptAttachment) -> Fut,
        ) {
        }
        frozen(session_deposit);
    }

    /// The Tauri boundary `src/lib/tauri.ts` is written against: `{ reference }`
    /// in, the stored bytes and MIME type out. Tauri derives the JS-side key
    /// names from these parameters, so a rename here silently changes the
    /// command's argument shape.
    #[test]
    fn session_attachment_read_forwarder_has_the_frozen_tauri_signature() {
        fn frozen<Fut: std::future::Future<Output = Result<StoredAttachment, CommandError>>>(
            _: fn(State<'static, DaemonBridge>, AttachmentReference) -> Fut,
        ) {
        }
        frozen(session_attachment_read);
    }

    /// The Tauri boundary `src/lib/tauri.ts` is written against: `{ id,
    /// subscriptionId, text, attachments?, activeTurnBehavior?,
    /// attachmentReferences?, idempotencyKey? }` in, nothing out. The last name
    /// is the queue's retry identity, and this is the one place all eight are
    /// written down together — the manifest in `src/lib/tauri.ts` is checked
    /// against the TS type by its own guard, and the type against this command
    /// here.
    // The eight-type list is the test: it is the command's argument shape, and
    // factoring it into named parts would hide the thing being pinned.
    // The reply says whether a turn is running: the app settles its
    // optimistic turn on `false` instead of waiting for a finish.
    #[allow(clippy::type_complexity)]
    #[test]
    fn session_send_forwarder_has_the_frozen_tauri_signature() {
        fn frozen<Fut: std::future::Future<Output = Result<bool, CommandError>>>(
            _: fn(
                State<'static, DaemonBridge>,
                String,
                SubscriptionId,
                String,
                Option<Vec<PromptAttachment>>,
                Option<String>,
                Option<Vec<AttachmentReference>>,
                Option<String>,
            ) -> Fut,
        ) {
        }
        frozen(session_send);
    }

    /// The sibling of `session_close`'s shape: `{ id, subscription_id? }` in,
    /// nothing out. The optionality is the point — a swiped background tab was
    /// never attached, so the bridge resolves the subscription itself.
    #[test]
    fn session_stop_forwarder_has_the_frozen_tauri_signature() {
        fn frozen<Fut: std::future::Future<Output = Result<(), CommandError>>>(
            _: fn(State<'static, DaemonBridge>, String, Option<SubscriptionId>) -> Fut,
        ) {
        }
        frozen(session_stop);
    }

    /// The Tauri boundary `src/lib/tauri.ts` is written against: `{ id,
    /// clientOperationId, text, attachments?, attachmentReferences? }` in,
    /// nothing out. The add is the only queue frame that carries a prompt, so
    /// it is the only one with the two attachment lists.
    // The six-type list is the test: it is this command's argument shape, and
    // naming the parts would hide the thing being pinned.
    #[allow(clippy::type_complexity)]
    #[test]
    fn session_queue_add_forwarder_has_the_frozen_tauri_signature() {
        fn frozen<Fut: std::future::Future<Output = Result<(), CommandError>>>(
            _: fn(
                State<'static, DaemonBridge>,
                String,
                String,
                String,
                Option<Vec<PromptAttachment>>,
                Option<Vec<AttachmentReference>>,
            ) -> Fut,
        ) {
        }
        frozen(session_queue_add);
    }

    /// The Tauri boundary `src/lib/tauri.ts` is written against: `{ id,
    /// clientOperationId, itemId, text }` in, nothing out.
    #[test]
    fn session_queue_edit_forwarder_has_the_frozen_tauri_signature() {
        fn frozen<Fut: std::future::Future<Output = Result<(), CommandError>>>(
            _: fn(State<'static, DaemonBridge>, String, String, String, String) -> Fut,
        ) {
        }
        frozen(session_queue_edit);
    }

    /// The Tauri boundary `src/lib/tauri.ts` is written against: `{ id,
    /// clientOperationId, itemId }` in, nothing out.
    #[test]
    fn session_queue_remove_forwarder_has_the_frozen_tauri_signature() {
        fn frozen<Fut: std::future::Future<Output = Result<(), CommandError>>>(
            _: fn(State<'static, DaemonBridge>, String, String, String) -> Fut,
        ) {
        }
        frozen(session_queue_remove);
    }

    /// The Tauri boundary `src/lib/tauri.ts` is written against: `{ id,
    /// clientOperationId, itemId, toIndex }` in, nothing out.
    #[test]
    fn session_queue_move_forwarder_has_the_frozen_tauri_signature() {
        fn frozen<Fut: std::future::Future<Output = Result<(), CommandError>>>(
            _: fn(State<'static, DaemonBridge>, String, String, String, usize) -> Fut,
        ) {
        }
        frozen(session_queue_move);
    }

    /// The Tauri boundary `src/lib/tauri.ts` is written against: `{ id,
    /// clientOperationId, subscriptionId, itemId }` in, nothing out. The
    /// subscription is this one — a send-now is a send, and the daemon checks
    /// the attach against it.
    #[test]
    fn session_queue_send_now_forwarder_has_the_frozen_tauri_signature() {
        fn frozen<Fut: std::future::Future<Output = Result<(), CommandError>>>(
            _: fn(State<'static, DaemonBridge>, String, String, SubscriptionId, String) -> Fut,
        ) {
        }
        frozen(session_queue_send_now);
    }

    fn reference(session_id: &str, digest: &str) -> AttachmentReference {
        AttachmentReference {
            session_id: session_id.to_string(),
            digest: digest.to_string(),
            stored_bytes: 1024,
            name: String::new(),
        }
    }

    #[test]
    fn no_references_is_not_a_limit_violation() {
        require_attachment_reference_limits("s.owner.1", &[]).expect("the common case");
    }

    /// A reference carries the session it was deposited to, so one that names
    /// another session is refused here — before the frame, with the protocol's
    /// own sentence, rather than by the daemon as a round trip.
    #[test]
    fn a_reference_to_another_session_is_invalid_request() {
        let digest = "a".repeat(64);
        let error =
            require_attachment_reference_limits("s.owner.1", &[reference("s.owner.2", &digest)])
                .expect_err("rejected");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("s.owner.2"), "{}", error.message);
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
