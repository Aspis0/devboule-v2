//! The four queue edits the wire addresses — add, edit, remove, move — one
//! method per frame. Each resolves its session through `queue_target` (so a
//! terminal, a stranger and a missing id are refused there), applies through
//! `mutate_queue` (so the change, the operation id's answer and the snapshot
//! every client reads are one step), and then wakes the drain, because every
//! accepted queue frame is what resumes it.

use devboule_protocol::{AttachmentReference, PromptAttachment, QueuedMessage};

use crate::attachment_store::attachment_digest;

use super::session_queue::{
    no_such_item, operation_conflict, QueueState, MAX_QUEUE_ITEMS, MAX_QUEUE_TEXT_BYTES,
};
use super::session_queue_lifecycle::QueueMutation;
use super::session_queue_operations::{
    checked_operation_id, Fingerprint, OperationSeen, PayloadFingerprint,
};
use super::{
    validate_attachment_references, validate_attachments, ConnHandle, ErrorCode, OwnerId,
    SessionRegistry, WireError, MAX_WRITE_BYTES,
};

impl SessionRegistry {
    /// Queue one message. Inline attachments are deposited into the session's
    /// store right here, so what the queue — and every snapshot of it — ever
    /// holds is references: the bytes are stored once.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn queue_add(
        &self,
        session_id: &str,
        client_operation_id: &str,
        text: &str,
        attachments: &[PromptAttachment],
        attachment_references: &[AttachmentReference],
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<QueueMutation, WireError> {
        let runtime = self.queue_target(session_id, owner, conn)?;
        validate_attachments(attachments)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        validate_attachment_references(session_id, attachment_references)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        // A prompt is whatever it carries — text, inline attachments, or the
        // stored ones it refers to — the same three ways `SessionSend` forms
        // one. The cap on the text holds when there is text; an add with none
        // of the three is not a prompt and is refused.
        if text.len() > MAX_WRITE_BYTES {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Session input is too large.",
            ));
        }
        let text = text.trim();
        if text.is_empty() && attachments.is_empty() && attachment_references.is_empty() {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "A queued message needs text or an attachment.",
            ));
        }
        let fingerprint = add_fingerprint(text, attachments, attachment_references);
        let added_bytes = text.len();
        // The queue is asked whether it will take this message before a single
        // attachment byte reaches the store: a refused add must leave nothing
        // behind, and a stored file nothing can send is exactly that. Two adds
        // can still race for the last row here, and the loser deposits bytes
        // the store keeps until it sweeps them — the store's own owner budget
        // is what bounds that, not this check.
        self.check_queue_accepts_add(session_id, client_operation_id, fingerprint, added_bytes)?;
        let mut references = attachment_references.to_vec();
        for attachment in attachments {
            references.push(self.deposit(session_id, owner, conn, attachment)?);
        }
        self.mutate_queue(
            session_id,
            &runtime,
            client_operation_id,
            fingerprint,
            |state| {
                check_add_fits(state, added_bytes)?;
                let item_id = state.mint_id();
                state.items.push(QueuedMessage {
                    item_id,
                    text: text.to_string(),
                    attachment_references: references,
                    error: None,
                });
                // The user just added to this queue, so a refused head starts
                // over rather than staying parked on the rung it had reached.
                state.paused = false;
                Ok(())
            },
        )
    }

    /// Replace one item's text, keeping its place, and clear what the old text
    /// carried: its recorded failure, because new text has not failed yet.
    pub(crate) fn queue_edit(
        &self,
        session_id: &str,
        client_operation_id: &str,
        item_id: &str,
        text: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<QueueMutation, WireError> {
        let runtime = self.queue_target(session_id, owner, conn)?;
        let text = checked_text(text)?;
        let added_bytes = text.len();
        let fingerprint = PayloadFingerprint::new("queue_edit")
            .field(item_id)
            .field(&text)
            .finish();
        self.mutate_queue(
            session_id,
            &runtime,
            client_operation_id,
            fingerprint,
            |state| {
                let position = state
                    .position(item_id)
                    .ok_or_else(|| no_such_item(item_id))?;
                let replaced_bytes = state.items[position].text.len();
                if state.text_bytes() - replaced_bytes + added_bytes > MAX_QUEUE_TEXT_BYTES {
                    return Err(queue_too_large());
                }
                let item = &mut state.items[position];
                item.text = text.clone();
                item.error = None;
                // A rewritten head is no longer a refusal the queue is parked
                // on: the row the user just fixed is the next thing out.
                if position == 0 {
                    state.paused = false;
                }
                Ok(())
            },
        )
    }

    /// Take one item out. An item a send has already claimed is not in the
    /// queue, and this says so rather than inventing a second removal.
    pub(crate) fn queue_remove(
        &self,
        session_id: &str,
        client_operation_id: &str,
        item_id: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<QueueMutation, WireError> {
        let runtime = self.queue_target(session_id, owner, conn)?;
        let fingerprint = PayloadFingerprint::new("queue_remove")
            .field(item_id)
            .finish();
        self.mutate_queue(
            session_id,
            &runtime,
            client_operation_id,
            fingerprint,
            |state| {
                let position = state
                    .position(item_id)
                    .ok_or_else(|| no_such_item(item_id))?;
                state.items.remove(position);
                Ok(())
            },
        )
    }

    /// Move one item to `to_index`, judged **before** the removal: the index is
    /// where the item should sit once it is back in, so `len - 1` is the end
    /// and anything past it is refused by name.
    pub(crate) fn queue_move(
        &self,
        session_id: &str,
        client_operation_id: &str,
        item_id: &str,
        to_index: usize,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<QueueMutation, WireError> {
        let runtime = self.queue_target(session_id, owner, conn)?;
        let fingerprint = PayloadFingerprint::new("queue_move")
            .field(item_id)
            .number(to_index as u64)
            .finish();
        self.mutate_queue(
            session_id,
            &runtime,
            client_operation_id,
            fingerprint,
            |state| {
                let position = state
                    .position(item_id)
                    .ok_or_else(|| no_such_item(item_id))?;
                if to_index >= state.items.len() {
                    return Err(WireError::new(
                        ErrorCode::InvalidRequest,
                        format!(
                            "Position {to_index} is past this queue's {} messages.",
                            state.items.len()
                        ),
                    ));
                }
                let item = state.items.remove(position);
                state.items.insert(to_index, item);
                Ok(())
            },
        )
    }

    /// Whether this queue will take a message of `added_bytes`, asked before an
    /// add deposits anything. It answers the same three questions the mutation
    /// door answers, in the same order, so a refused add never reaches the
    /// store.
    fn check_queue_accepts_add(
        &self,
        session_id: &str,
        client_operation_id: &str,
        fingerprint: Fingerprint,
        added_bytes: usize,
    ) -> Result<(), WireError> {
        checked_operation_id(client_operation_id)?;
        // A look, not a lookup that creates: a session this daemon forgot
        // between the add's resolve and here must not get a queue from a
        // question, and one nothing has queued in yet answers as an empty one.
        let queues = self.queues.lock();
        let empty;
        let state = match queues.get(session_id) {
            Some(state) => state,
            None => {
                empty = QueueState::new();
                &empty
            }
        };
        match state.seen_operation(client_operation_id, fingerprint) {
            // A replay is answered by the mutation door, which does nothing and
            // publishes nothing; this pass only has to let it through.
            OperationSeen::Answered(_) => return Ok(()),
            OperationSeen::Conflict => return Err(operation_conflict(client_operation_id)),
            OperationSeen::Fresh => {}
        }
        if state.fenced {
            return Err(super::session_queue::queue_fenced());
        }
        check_add_fits(state, added_bytes)
    }
}

/// The two walls an add runs into, named once because both the pre-check and the
/// mutation itself are the same rule.
fn check_add_fits(state: &QueueState, added_bytes: usize) -> Result<(), WireError> {
    if state.items.len() >= MAX_QUEUE_ITEMS {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("This session's queue already holds {MAX_QUEUE_ITEMS} messages."),
        ));
    }
    if state.text_bytes() + added_bytes > MAX_QUEUE_TEXT_BYTES {
        return Err(queue_too_large());
    }
    Ok(())
}

/// An add's fingerprint: its text and every semantic field of every attachment
/// it names. An inline attachment hashes its name, its type and the SHA-256 of
/// its decoded bytes — all three are what the message says, and two images share
/// nothing but a name — and a reference hashes the digest that stands for the
/// stored bytes, because that digest is the payload.
///
/// The bytes are decoded here, ahead of the store's own decode in `deposit`,
/// because the fingerprint has to exist before anything is deposited: a refused
/// or replayed add must leave nothing in the store.
fn add_fingerprint(
    text: &str,
    attachments: &[PromptAttachment],
    attachment_references: &[AttachmentReference],
) -> Fingerprint {
    let mut fingerprint = PayloadFingerprint::new("queue_add").field(text);
    for attachment in attachments {
        fingerprint = fingerprint
            .field(&attachment.name)
            .field(&attachment.mime_type)
            .field(&attachment_digest(attachment));
    }
    for reference in attachment_references {
        fingerprint = fingerprint.field(&reference.digest);
    }
    fingerprint.finish()
}

fn queue_too_large() -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        "This session's queue already holds as much text as one message may; \
         remove a queued message or queue a shorter one.",
    )
}

/// One frame's text, as every queue edit may carry it: trimmed, non-empty, at
/// most the wire's own write cap — the same three rules the send path applies
/// to the text it is about to write.
fn checked_text(text: &str) -> Result<String, WireError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "A queued message needs text.",
        ));
    }
    if text.len() > MAX_WRITE_BYTES {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "Session input is too large.",
        ));
    }
    Ok(text.to_string())
}
