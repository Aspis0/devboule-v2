//! The four queue edits the wire addresses — add, edit, remove, move — one
//! method per frame. Each resolves its session through `queue_target` (so a
//! terminal, a stranger and a missing id are refused there), applies through
//! `edit_queue` (so the mutation, its rows and its snapshot are one step),
//! and then wakes the drain, because every queue frame is what resumes it.

use super::{
    no_such_item, validate_attachment_references, validate_attachments, AttachmentReference,
    ConnHandle, ErrorCode, OwnerId, PromptAttachment, SessionRegistry, WireError, MAX_QUEUE_ITEMS,
    MAX_WRITE_BYTES,
};
use devboule_protocol::QueuedMessage;

impl SessionRegistry {
    /// Queue one message. Inline attachments are deposited to the session's
    /// store right here, so what the queue — and its snapshot, and the journal
    /// row behind it — ever holds is references: the bytes are stored once.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn queue_add(
        &self,
        session_id: &str,
        text: &str,
        attachments: &[PromptAttachment],
        attachment_references: &[AttachmentReference],
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        let runtime = self.queue_target(session_id, owner, conn)?;
        validate_attachments(attachments)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        validate_attachment_references(session_id, attachment_references)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        // A prompt is whatever it carries — text, inline attachments, or the
        // stored ones it refers to — the same three ways `SessionSend` forms
        // one (P2-9). The size cap on the text still holds when there is
        // text; an add with none of the three is not a prompt and is
        // refused.
        if text.len() > MAX_WRITE_BYTES {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Session input is too large.",
            ));
        }
        let text = text.trim().to_string();
        if text.is_empty() && attachments.is_empty() && attachment_references.is_empty() {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "A queued message needs text or an attachment.",
            ));
        }
        let mut references = attachment_references.to_vec();
        for attachment in attachments {
            references.push(self.deposit(session_id, owner, conn, attachment)?);
        }
        self.edit_queue(session_id, &runtime, |state| {
            if state.items.len() >= MAX_QUEUE_ITEMS {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("This session's queue already holds {MAX_QUEUE_ITEMS} messages."),
                ));
            }
            let item_id = state.mint_id();
            state.items.push(QueuedMessage {
                item_id,
                text: text.to_string(),
                attachment_references: references,
                error: None,
                unconfirmed: false,
            });
            Ok(())
        })?;
        self.wake_queue_drain(session_id);
        Ok(())
    }

    /// Replace one item's text, keep its place, and clear what the old text
    /// carried — its error and its unconfirmed mark: new text has not failed
    /// yet and has never been sent, which is also the one act that clears an
    /// in-flight item's fence (the user has rewritten it).
    pub(crate) fn queue_edit(
        &self,
        session_id: &str,
        item_id: &str,
        text: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        let runtime = self.queue_target(session_id, owner, conn)?;
        let text = checked_text(text)?;
        self.edit_queue(session_id, &runtime, |state| {
            let position = state
                .position(item_id)
                .ok_or_else(|| no_such_item(item_id))?;
            let item = &mut state.items[position];
            item.text = text;
            item.error = None;
            item.unconfirmed = false;
            Ok(())
        })?;
        self.wake_queue_drain(session_id);
        Ok(())
    }

    /// Take one item out. An item the drain has already claimed is not in the
    /// queue, and this says so rather than inventing a second removal.
    pub(crate) fn queue_remove(
        &self,
        session_id: &str,
        item_id: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        let runtime = self.queue_target(session_id, owner, conn)?;
        self.edit_queue(session_id, &runtime, |state| {
            let position = state
                .position(item_id)
                .ok_or_else(|| no_such_item(item_id))?;
            state.items.remove(position);
            Ok(())
        })?;
        self.wake_queue_drain(session_id);
        Ok(())
    }

    /// Move one item to `to_index`, judged **before** the removal: the index
    /// is where the item should sit once it is back in, so `len - 1` is the
    /// end and anything past it is refused by name.
    pub(crate) fn queue_move(
        &self,
        session_id: &str,
        item_id: &str,
        to_index: usize,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        let runtime = self.queue_target(session_id, owner, conn)?;
        self.edit_queue(session_id, &runtime, |state| {
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
        })?;
        self.wake_queue_drain(session_id);
        Ok(())
    }
}

/// One frame's text, as every queue edit may carry it: trimmed, non-empty,
/// at most the wire's own write cap — the same three rules the send path
/// applies to the text it is about to write.
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
