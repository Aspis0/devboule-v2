//! Send, and the brakes on it: attachment deposits, the prompt roads into a
//! running turn, the message-slot boundary a delivery registers, and the
//! roster lookup a child creation resolves its workspace through.
//!
//! Split out of `session.rs` without a rewrite: every line below this header is
//! byte-identical to its text there, apart from the `pub(super)` markers a
//! caller in the parent module, its sibling modules or its tests reaches in for.

use super::*;
use crate::release_guard::ReleaseGuard;

impl super::SessionRegistry {
    /// Store one prompt attachment for a session and answer the reference the
    /// send that follows will name.
    ///
    /// The connection is threaded through for the same reason `set_mode`'s is:
    /// `SessionDeposit` is under `CAP_SEND` (`peer_policy.rs`), so a paired
    /// device *is* reachable here, and the requestor's identity is part of the
    /// authorization the ownership check makes (§8b A3/A4/A5, H5).
    ///
    /// The wire's own limits are enforced before the store sees the attachment
    /// (DEP-06). The store's `prepare` decodes the base64 and walks the image,
    /// which is the expensive half of a deposit, and a frame the protocol
    /// already refuses must not pay for it; the refusal is also the protocol's
    /// sentence rather than a store error, so an attachment that is too large
    /// reads the same here as it does on a send.
    ///
    /// The reference's digest and `stored_bytes` are the store's to state, not
    /// this function's: the digest names the bytes *as stored* (the strip makes
    /// them differ from what was sent) and the size is the file's own, read from
    /// the disk.
    ///
    /// The window between the ownership check and the store write is closed from
    /// the far side: the store writes with no registry lock held, so a `close`
    /// that lands in the middle of it is detected by the re-check below and the
    /// write is undone with it. A close that lands *after* that re-check is the
    /// same race every operation in this file has with close, and it is
    /// accepted — the folder goes away with the session, as it would for a send
    /// whose bytes were already in the provider's hands.
    pub(crate) fn deposit(
        &self,
        session_id: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
        attachment: &PromptAttachment,
    ) -> Result<AttachmentReference, WireError> {
        {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = map.get(session_id).ok_or_else(not_found)?;
            check_user_owner(entry, owner, &conn.conn_peer)?;
        }
        #[cfg(test)]
        self.fire_deposit_after_ownership_hook();
        validate_attachments(std::slice::from_ref(attachment))
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let deposited = self.attachments.deposit(session_id, attachment)?;
        // The store wrote outside the registry lock, so a `close` that landed in
        // the meantime has already removed this session's folder and the file
        // just written belongs to a session that no longer exists: nothing will
        // ever close it, and it stays charged to the store's budget until the
        // retention sweep. The entry's absence is the receipt that the close won,
        // so the write is undone under the lock that decides it — taken *after*
        // the store released its own, never across it.
        let gone = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            map.get(session_id).is_none()
        };
        if gone {
            self.attachments.remove_session(session_id);
            return Err(not_found());
        }
        Ok(AttachmentReference {
            session_id: session_id.to_string(),
            digest: deposited.digest,
            stored_bytes: deposited.stored_bytes,
        })
    }

    /// Read back the bytes of one deposited attachment, verified.
    ///
    /// The same door [`Self::deposit`] walks, in the same order: the
    /// registry's ownership check first — a reference resolves only inside
    /// a session the caller's own scope owns — then the wire's own
    /// reference rules, then the store. The digest names the file, but no
    /// check short of the hash binds the reply to the reference: the store
    /// resolves by name and stats by name, so anything rewritten under the
    /// same name comes back unless the bytes themselves are verified.
    ///
    /// The read is bounded before it is believed. The file is opened and at
    /// most [`MAX_AGENT_ARTIFACT_BYTES`] plus one byte is taken, so a file
    /// grown past the cap is never fully allocated and can never reach the
    /// frame. The bytes taken must then be exactly as long as the reference
    /// states, and their SHA-256 must equal its digest — the store hashed
    /// the same bytes on the write, so the legitimate path cannot fail
    /// this, and a store that disagrees with itself is refused. Each
    /// refusal echoes only what the reference stated: no sentence here
    /// reports a size or a digest the caller did not name.
    ///
    /// The MIME type is the store's own statement from its extension table.
    /// A reference whose session or file is gone answers with the store's
    /// own sentences, which cannot tell a swept folder from a digest
    /// deposited elsewhere.
    pub(crate) fn read_attachment(
        &self,
        reference: &AttachmentReference,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<StoredAttachment, WireError> {
        {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = map.get(&reference.session_id).ok_or_else(not_found)?;
            check_user_owner(entry, owner, &conn.conn_peer)?;
        }
        validate_attachment_references(&reference.session_id, std::slice::from_ref(reference))
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let (path, _) = self
            .attachments
            .resolve(&reference.session_id, &reference.digest, None)?;
        if reference.stored_bytes > MAX_AGENT_ARTIFACT_BYTES as u64 {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "The reference states {} bytes for '{}', over the {}-byte read cap.",
                    reference.stored_bytes, reference.digest, MAX_AGENT_ARTIFACT_BYTES
                ),
            ));
        }
        // Bounded before believed: an oversized file is never fully
        // allocated, so it can never reach the frame no matter what the
        // stat said a moment ago.
        let file = std::fs::File::open(&path).map_err(|error| {
            WireError::new(
                ErrorCode::Io,
                format!("Could not read a stored attachment: {error}"),
            )
        })?;
        let mut bytes = Vec::new();
        file.take(MAX_AGENT_ARTIFACT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                WireError::new(
                    ErrorCode::Io,
                    format!("Could not read a stored attachment: {error}"),
                )
            })?;
        if bytes.len() as u64 != reference.stored_bytes {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "The stored attachment '{}' is not {} bytes as the reference states.",
                    reference.digest, reference.stored_bytes
                ),
            ));
        }
        if crate::attachment_store::sha256_hex(&bytes) != reference.digest {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "The stored attachment '{}' does not match its digest.",
                    reference.digest
                ),
            ));
        }
        use base64::Engine as _;
        Ok(StoredAttachment {
            mime_type: path
                .extension()
                .and_then(|extension| extension.to_str())
                .and_then(crate::attachment_store::mime_type_for_extension)
                .unwrap_or("application/octet-stream")
                .to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        })
    }

    /// Deposit a finished child's whole last message in the **creator's**
    /// folder and answer the artifact the report names (`S5` decision 10).
    ///
    /// The same door every deposit uses ([`Self::deposit`]): ownership, the
    /// wire's own limits, the store's type table and its budget. The artifact is
    /// charged to the creator's folder exactly like any other attachment, which
    /// is the point of depositing it as one — a child's result is not a way
    /// around the meter.
    pub(super) fn deposit_child_message(
        &self,
        creator: &str,
        owner: &OwnerId,
        message: &AgentMessageSnapshot,
    ) -> Result<FinishArtifact, String> {
        use base64::Engine as _;
        let bytes = message.text.as_bytes();
        if bytes.len() > MAX_AGENT_ARTIFACT_BYTES {
            return Err(format!(
                "Its message is {} bytes and was not deposited; the artifact cap is {MAX_AGENT_ARTIFACT_BYTES}.",
                bytes.len()
            ));
        }
        let attachment = PromptAttachment {
            name: "agent-finished.md".to_string(),
            mime_type: "text/markdown".to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        };
        let internal_conn = ConnHandle::with_peer(0, None);
        let reference = self
            .deposit(creator, owner, &internal_conn, &attachment)
            .map_err(|error| format!("Its message was not stored: {}", error.message))?;
        let url = format!(
            "devboule-attachment:{}/{}",
            reference.session_id, reference.digest
        );
        Ok(FinishArtifact {
            artifact_id: url.clone(),
            parts: vec![FinishArtifactPart {
                url,
                mime_type: "text/markdown".to_string(),
                metadata: Some(FinishArtifactPartMetadata {
                    stored_bytes: reference.stored_bytes,
                }),
            }],
        })
    }

    #[cfg(test)]
    pub fn send(
        &self,
        session_id: &str,
        text: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        self.send_with_subscription(session_id, conn.id, text, &[], &[], owner, conn)
    }

    /// One prompt: the text, the inline attachments, and the references to
    /// attachments already deposited under this session.
    ///
    /// The two halves are separate arguments rather than one list because they
    /// travel differently — the inline bytes are in the frame the client built,
    /// a reference is a digest the daemon resolves against the store — and the
    /// send path keeps them apart from validation through to the prompt.
    ///
    /// The argument list is one past clippy's limit and stays a list: the
    /// struct that would collapse it exists (`SendRequest`), and the layer
    /// below already takes it — this is the thin entry point 20 call sites use,
    /// and giving them a struct to build would move the argument count into
    /// them rather than remove it. The crate makes this trade in nine other
    /// places.
    #[allow(clippy::too_many_arguments)]
    pub fn send_with_subscription(
        &self,
        session_id: &str,
        subscription_id: u64,
        text: &str,
        attachments: &[PromptAttachment],
        attachment_references: &[AttachmentReference],
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        self.send_with_subscription_behavior(
            session_id,
            subscription_id,
            text,
            attachments,
            attachment_references,
            owner,
            conn,
            None,
        )
        .map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn send_with_subscription_behavior(
        &self,
        session_id: &str,
        subscription_id: u64,
        text: &str,
        attachments: &[PromptAttachment],
        attachment_references: &[AttachmentReference],
        owner: &OwnerId,
        conn: &ConnHandle,
        active_turn_behavior: Option<ActiveTurnBehavior>,
    ) -> Result<(), WireError> {
        self.send_with_subscription_timeout(&SendRequest {
            session_id,
            subscription_id,
            text,
            attachments,
            attachment_references,
            owner,
            conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior,
            require_attachment: true,
            // The person at this machine, or a paired device: only the former
            // may have a refused steer fall back to an interrupt (S4-01).
            interrupt_on_steer_refusal: session_origin_for(&conn.conn_peer).is_local(),
            message_slot: None,
            // No preset preamble: a client's prompt is not a creation's.
            preset_preamble: None,
            spawn_prompt: None,
            author: UserMessageAuthor::Human,
            message_kind: UserMessageKind::Composer,
        })
        .map(|_| ())
    }

    pub(crate) fn agent_message_send(
        &self,
        from_session: &str,
        to_session: &str,
        text: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        self.agent_message_send_in_namespace(
            from_session,
            to_session,
            text,
            owner,
            conn,
            AgentMessageSourceNamespace::Local,
            Instant::now(),
        )
    }

    /// The wire frame carries a sender id from the authenticated peer's
    /// namespace. The connection proves the device; the frame supplies only
    /// that device-local session label.
    pub(crate) fn agent_message_send_from_peer(
        &self,
        from_session: &str,
        to_session: &str,
        text: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        self.agent_message_send_in_namespace(
            from_session,
            to_session,
            text,
            owner,
            conn,
            AgentMessageSourceNamespace::Far,
            Instant::now(),
        )
    }

    #[cfg(test)]
    pub(crate) fn agent_message_send_from_peer_at(
        &self,
        from_session: &str,
        to_session: &str,
        text: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
        now: Instant,
    ) -> Result<(), WireError> {
        self.agent_message_send_in_namespace(
            from_session,
            to_session,
            text,
            owner,
            conn,
            AgentMessageSourceNamespace::Far,
            now,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn agent_message_send_in_namespace(
        &self,
        from_session: &str,
        to_session: &str,
        text: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
        source_namespace: AgentMessageSourceNamespace,
        now: Instant,
    ) -> Result<(), WireError> {
        let caller_origin = session_origin_for(&conn.conn_peer);
        let far_sender = source_namespace == AgentMessageSourceNamespace::Far;
        if far_sender {
            validate_session_id(from_session)
                .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        }
        if !far_sender && from_session == to_session {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "An agent cannot send a message to itself.",
            ));
        }
        // A remote sender chooses the far session label, so it cannot choose
        // the brake key: one authenticated device gets one outstanding,
        // rate, and recipient budget however it spells that label. A local
        // caller stays keyed by its local session, which it owns and cannot
        // use to spend another session's budget.
        let brake_key = if far_sender {
            caller_origin.device_id.as_deref().unwrap_or_default()
        } else {
            from_session
        };
        // Target admission and the brake slot are one critical section (A2-05).
        // While this holds the session map, no close can take the target out from
        // under the check and no second send of the same sender can take the slot
        // this one is taking: "the target is there and this caller may reach it"
        // and "the sender has a slot for it" cannot answer differently, and the
        // sender's entry in the brake table cannot outlive the target it names.
        // The brake table's own lock is taken underneath this one — never the
        // other way round — and released with it.
        //
        // The turn this message joins is *not* snapshotted here (S4-03): the
        // reservation below asks the target's runtime for it, in the same critical
        // section `finish_turn` takes, and its answer is what decides steer versus
        // prompt. A turn that ends after that answer cannot make the decision
        // wrong, because the answer arrived with the boundary registration.
        let (from_runtime, target_owner, admission) = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let (from_runtime, source_origin) = if far_sender {
                // A wire sender has no transcript on this daemon. Its raw text
                // is echoed by the sender's own daemon, so the far namespace
                // deliberately skips local source resolution and echo.
                (None, None)
            } else {
                let source = peer_entry(&map, from_session, owner, &conn.conn_peer)?;
                let source = source.as_peer_visible().ok_or_else(process_gone)?;
                (
                    Some(Arc::clone(&source.runtime)),
                    Some(source.metadata.origin.clone()),
                )
            };
            let target = if far_sender {
                agent_message_target_entry(&map, to_session, owner, &conn.conn_peer)?
            } else {
                peer_entry(&map, to_session, owner, &conn.conn_peer)?
            };
            let target = target.as_peer_visible().ok_or_else(process_gone)?;
            // Refused here, inside the same section: a message that would cross
            // two peer hops never reaches the brake table, so the refusal cannot
            // leave a slot behind it.
            let target_origin = target.metadata.origin.clone();
            if source_origin
                .as_ref()
                .is_some_and(|origin| origin.kind == SessionOriginKind::Peer)
                && target_origin.kind == SessionOriginKind::Peer
            {
                return Err(WireError::new(
                    ErrorCode::CapabilityNotSupported,
                    "Forwarding agent messages beyond one peer hop is not supported; do not retry.",
                ));
            }
            let admission = reserve_message_brake(
                &self.message_brakes,
                brake_key,
                to_session,
                Some((&target.runtime, target.runtime.turn_counter())),
                now,
            )?;
            (from_runtime, target.owner.clone(), admission)
        };
        // The slot is taken, so its release is owed here whatever happens
        // between this line and the delivery's outcome: `finish_message_delivery`
        // is the only thing that gives the slot back, and a panic before it
        // leaves the slot counted against the sender's budget until the expiry
        // sweep — which only a later admission runs, up to `MESSAGE_SLOT_EXPIRY`
        // later.
        let release = ReleaseGuard::armed(|completed: bool| {
            finish_message_delivery(&self.message_brakes, brake_key, admission.slot, completed)
        });
        #[cfg(test)]
        self.fire_agent_message_after_admission_hook();
        // The entry point states whether this id is local or far. A far id is
        // namespaced with the authenticated device so a local-looking label
        // cannot masquerade as this daemon's sibling; a local source keeps the
        // existing local form (S4-05).
        let from_agent = if far_sender {
            format!(
                "peer:{}/{}",
                caller_origin.device_id.as_deref().unwrap_or_default(),
                from_session
            )
        } else {
            from_session.to_string()
        };
        let origin = match caller_origin.kind {
            SessionOriginKind::Peer => {
                format!(
                    "peer:{}",
                    caller_origin.device_id.as_deref().unwrap_or_default()
                )
            }
            SessionOriginKind::Local => "local".to_string(),
            // A caller whose peer record says neither fact is not the person at
            // this machine (§8 R2): the envelope names it as the journal spells
            // it, and claims no device.
            SessionOriginKind::Unknown => "unknown".to_string(),
        };
        let role = match caller_origin.role {
            Some(PeerRole::Daemon) => "daemon",
            Some(PeerRole::Client) | None => "client",
        };
        let envelope = agent_message_envelope(&origin, role, &from_agent, text);
        let internal_conn = ConnHandle::with_peer(0, None);
        // (S4-10) The slot this delivery holds, so the plain-prompt fallback can
        // re-key its boundary if the turn it was admitted into ends first.
        let slot_ref = MessageSlotRef {
            brakes: &self.message_brakes,
            brake_key,
            slot: admission.slot,
            admitted_turn_id: admission.expected_turn_id,
        };
        let result = self.send_with_subscription_timeout(&SendRequest {
            session_id: to_session,
            subscription_id: 0,
            text: &envelope,
            attachments: &[],
            // `AgentMessageSend` has no field for either half: an agent
            // message is the envelope's text, and nothing in this delivery
            // could have named a stored attachment.
            attachment_references: &[],
            owner: &target_owner,
            conn: &internal_conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            // (S4-03) Steer only if the runtime answered that the turn the
            // caller checked was still running when the boundary was registered:
            // that answer, not an earlier look, is what makes the delivery match
            // the decision.
            active_turn_behavior: admission
                .steered_into_turn
                .then_some(ActiveTurnBehavior::Steer),
            require_attachment: false,
            // The delivery itself is the daemon acting on the caller's behalf,
            // so a refused steer may only interrupt when the caller could have
            // asked for an interrupt itself (S4-01): a paired device's agent
            // message must not replace a running turn it may not stop.
            interrupt_on_steer_refusal: caller_origin.is_local(),
            message_slot: Some(&slot_ref),
            // No preset preamble: an agent message is not a creation's prompt.
            preset_preamble: None,
            spawn_prompt: None,
            author: UserMessageAuthor::Agent,
            message_kind: UserMessageKind::IncomingA2a,
        });
        if result.is_ok() {
            if let Some(from_runtime) = from_runtime {
                // A local source row belongs to this daemon, even when the
                // bearer connection is a reconstructed remote peer, so MCP
                // sends keep the sender-side transcript echo.
                if from_runtime
                    .publish_agent_user_message(
                        text.to_string(),
                        UserMessageAuthor::Agent,
                        UserMessageKind::OutgoingA2a,
                    )
                    .is_none()
                {
                    from_runtime.mark_journal_degraded();
                }
            }
            // The delivery returned: the slot now waits only for its boundary,
            // if this admission found one — the turn end it was admitted for.
            release.release(true);
        } else {
            // The message is in flight nowhere: give the slot back now instead
            // of holding the sender's budget until a boundary that will never see
            // this message arrives.
            release.release(false);
        }
        // The delivery's own id is not what this act answers with: the *sender*
        // is the caller here, and its echo (if any) is published above. The
        // receiver-side id is nobody's correlation key (S4-09).
        result.map(|_| ())
    }

    #[cfg(test)]
    pub(super) fn send_with_mcp_timeout(
        &self,
        session_id: &str,
        text: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
        timeout: Duration,
    ) -> Result<(), WireError> {
        self.send_with_subscription_timeout(&SendRequest {
            session_id,
            subscription_id: conn.id,
            text,
            attachments: &[],
            attachment_references: &[],
            owner,
            conn,
            mcp_timeout: timeout,
            active_turn_behavior: None,
            require_attachment: true,
            interrupt_on_steer_refusal: true,
            message_slot: None,
            preset_preamble: None,
            spawn_prompt: None,
            author: UserMessageAuthor::Human,
            message_kind: UserMessageKind::Composer,
        })
        .map(|_| ())
    }

    pub(super) fn send_with_subscription_timeout(
        &self,
        request: &SendRequest<'_>,
    ) -> Result<Option<String>, WireError> {
        let SendRequest {
            session_id,
            subscription_id,
            text,
            attachments,
            attachment_references,
            owner,
            conn,
            mcp_timeout,
            active_turn_behavior,
            require_attachment,
            interrupt_on_steer_refusal,
            message_slot,
            preset_preamble,
            spawn_prompt,
            author,
            message_kind,
        } = *request;
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        // The daemon does not trust the app's copy of these checks: the pipe
        // accepts frames from any client that can open it, so the limits are
        // enforced here too, on the payload as it arrived.
        if text.len() > MAX_WRITE_BYTES {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Session input is too large.",
            ));
        }
        validate_attachments(attachments)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        // The references are the half of an attachment send that does not
        // travel in the frame, and the wire's rules for them are enforced here
        // with the inline ones — before the store is asked anything, the same
        // order `deposit` keeps: ownership and the wire's limits first, the
        // disk after. `resolve_attachment_references` reads the store, so a
        // reference the protocol refuses costs no digest lookup and no file
        // read, and it is refused before the ownership check below rather than
        // after it for the same reason `validate_attachments` is.
        validate_attachment_references(session_id, attachment_references)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        // A prompt is whatever it carries: text, inline attachments, or the
        // stored ones it refers to. A send with none of the three is not a
        // prompt and skips the readiness gates below, which is the behaviour
        // it had before references existed.
        let has_prompt =
            !text.is_empty() || !attachments.is_empty() || !attachment_references.is_empty();
        let (
            writer,
            image_sink,
            static_image_sink,
            runtime,
            killer,
            mut steerer,
            is_agent,
            mcp_required,
        ) = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = peer_entry(&map, session_id, owner, &conn.conn_peer)?;
            let session = entry.as_peer_visible().ok_or_else(process_gone)?;
            (
                Arc::clone(&session.writer),
                session.image_sink.clone(),
                session.static_image_sink.clone(),
                Arc::clone(&session.runtime),
                session.killer.clone_killer(),
                session.steerer.clone_steerer(),
                session.metadata.kind.is_agent(),
                // S9: readiness waits only where the wait rule says so (never
                // pi/Codex — the S8 never-block default, twin-pinned). The wait
                // itself no-ops without `require_mcp`, so this flag is uniform
                // while the guarantee lives in the require gate.
                crate::mcp_broker::hosts_mcp(&session.metadata.kind),
            )
        };
        // A terminal's writer is a PTY, so an appended line is typed, not
        // read: nothing there can open a path. Writing the bytes would leave a
        // file behind for a session that can never consume it, and the pipe
        // accepts frames from any process that can open it, so the daemon does
        // not rely on the app never attaching to a terminal.
        //
        // A reference is refused by the same check for the same reason: what a
        // terminal would receive is the path line, and a path typed into a PTY
        // is input, not a file anything can open. The two halves are one
        // refusal here because neither reaches a terminal.
        if (!attachments.is_empty() || !attachment_references.is_empty()) && !is_agent {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "This session does not accept attachments.",
            ));
        }
        // A steer is text only, and that is refused before a single attachment
        // byte is planned, decoded or written anywhere (S4-10): the steer
        // branch below writes the text into a turn that is already running, and
        // there is no path from an attachment to a provider frame on it. The
        // refusal names the way to send one.
        //
        // A reference is text-only in the same sense and is refused by the
        // same check: the steer branch writes `text` and nothing else, so a
        // steer that named a stored attachment would drop it without a word —
        // the vanishing deck this whole path exists to prevent.
        if active_turn_behavior == Some(ActiveTurnBehavior::Steer)
            && (!attachments.is_empty() || !attachment_references.is_empty())
        {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "a steer carries text only; send attachments as a new message",
            ));
        }
        if require_attachment {
            check_attached(&runtime, conn, subscription_id)?;
        }
        let agent_runtime = is_agent.then(|| Arc::clone(&runtime));
        if let Some(runtime) = agent_runtime.as_ref() {
            if has_prompt && mcp_required {
                runtime.wait_for_mcp_ready(mcp_timeout)?;
            }
            if has_prompt && !runtime.can_publish_agent_user_message() {
                return Err(internal("Agent input could not be recorded."));
            }
        }
        if active_turn_behavior == Some(ActiveTurnBehavior::Steer) && is_agent {
            // (S4-14) The steer writes into whichever turn is running now, and the
            // admission registered this slot's boundary against the turn that was
            // running then. If that is not the same turn any more, the boundary is
            // re-keyed here — before the steer write, under the brakes lock — so the
            // slot ends with the turn the text actually enters.
            if let Some(slot) = message_slot {
                if message_slot_boundary_is_stale(slot, runtime.turn_counter()) {
                    rearm_message_slot_boundary(slot, &runtime);
                }
            }
            let expected_turn_id = runtime.turn_counter();
            // Compare-and-deliver (S4-02): the runtime hands the provider
            // adapter a token only while the daemon turn the caller checked is
            // still the running one, holding the same lock the `AgentFinished`
            // transition takes across the adapter's write. A turn therefore
            // cannot end — and the next one cannot start — between the check
            // and the write, so the text can only land in the turn it was
            // admitted for. `None` means the turn was over before admission:
            // there is nothing to steer and the text is delivered as the plain
            // send it would have been if the caller had not asked to join a
            // turn, with no interrupt, because nothing is running to replace.
            let steered = runtime.with_active_turn(expected_turn_id, |turn| {
                steerer.steer_active_turn(text, turn)
            });
            match steered {
                Some(Ok(true)) => {
                    // Cards are cancelled only now, once the provider has taken
                    // the input. Cancelling before this point would take a card
                    // away for a steer that never landed: `Ok(false)` (the
                    // provider cannot take a steer for this turn) and `Err` (the
                    // transport failed) both leave the cards exactly as they
                    // were, because the turn they belong to is still running.
                    // No provider needs them cleared *before* it can accept: on
                    // a refusal the local fallback's own `interrupt()` clears
                    // them, and each provider's killer does the same.
                    if let Some(permission_broker) = runtime.permission_broker() {
                        permission_broker.cancel_pending();
                    }
                    // The steered text is echoed into the session's own
                    // transcript as the `AgentUserMessage` every accepted input
                    // produces, so the sender's chat surface shows the steer
                    // inside the running turn; `Steered` stays the journal's
                    // audit row for the same text (it is not published to
                    // observers), carrying the echo's own `message_id` so the
                    // row and the transcript message name one message (A2-10).
                    // Both are best effort: the provider has already taken the
                    // text, so a recording failure is reported as a degraded
                    // session and never as an error — the caller must not be
                    // invited to retry a steer that already landed
                    // (S4-06/S4-09).
                    let echo_message_id =
                        runtime.publish_agent_user_message(text.to_string(), author, message_kind);
                    if echo_message_id.is_none() {
                        runtime.mark_journal_degraded();
                    }
                    // `journal_steered` takes the id by value (the audit row
                    // and the transcript message name one message): the
                    // correlation key this delivery answers with is kept
                    // beside it rather than moved into the journal.
                    let delivered_message_id = echo_message_id.clone();
                    if !runtime.journal_steered(echo_message_id, text.to_string()) {
                        runtime.mark_journal_degraded();
                    }
                    if runtime.clear_attention() {
                        self.notify_session_transition(owner, session_id);
                    }
                    // The steer's own echo id is the message the text became,
                    // so it is what this delivery answers with (audit S5-04):
                    // a caller that correlates to it names the message the
                    // creator's transcript actually shows.
                    return Ok(delivered_message_id);
                }
                Some(Ok(false)) => {
                    // The provider cannot take a steer for this turn. The
                    // person at this machine gets the pre-existing
                    // interrupt-and-replace; a paired device gets a refusal,
                    // because interrupting a running turn is the act
                    // `SessionInterrupt` decides and no capability opens it to
                    // a peer, so a steer must not reach it the long way round
                    // (S4-01).
                    if !interrupt_on_steer_refusal {
                        return Err(WireError::new(
                            ErrorCode::Unauthorized,
                            "this agent cannot take a steer and interrupting is not permitted for a paired device",
                        ));
                    }
                    let mut killer = killer;
                    killer.interrupt();
                }
                Some(Err(error)) => return Err(error),
                // (S4-10) The turn ended between the admission and this write: the
                // text goes as an ordinary prompt. `boundary_reached` is already
                // set by the fired hook, and the re-key below — which looks at
                // exactly that flag — moves the slot onto the turn this prompt
                // starts.
                None => {}
            }
        }
        // ---- the session's first prompt carries the standing instructions ----
        //
        // The human's standing instructions ride the first prompt of every session
        // the daemon starts, and this is the one place a prompt is composed: a
        // session a human opens, a child an agent creates (which passes its preset
        // preamble in `preset_preamble` and the resolved profile's spawn prompt in
        // `spawn_prompt`) and the Design host all reach this line,
        // and every provider's writer sits behind it (`session.rs:4899`-style
        // writes in `acp_client.rs`, `claude_client.rs`, `codex_client.rs`,
        // `pi_client.rs`). The order — standing instructions, then the spawn
        // prompt, then the preamble,
        // then the prompt — is fixed in `compose_first_prompt` and pinned by
        // `the_spawn_prompt_sits_between_the_standing_instructions_and_the_preamble`
        // and `standing_instructions_come_before_the_preset_preamble`.
        //
        // Three deliberate narrowings:
        //
        // - **Agent sessions only.** A terminal's writer is a PTY: prefixing a
        //   human's first shell line with their standing instructions would type
        //   prose into a shell.
        // - **The first prompt that has text.** A prompt made only of attachments
        //   has nothing to prefix, so the flag stays owed and the session's first
        //   *text* prompt carries them.
        // - **The flag is taken, once.** `take_first_prompt` swaps it, so a second
        //   prompt racing the first cannot compose a second copy, and a prompt
        //   that arrives after a failed write does not get one either.
        //
        // The store is read *now*, at the moment of the first prompt, and never
        // cached on the session: an edit to the standing instructions takes effect
        // on the next session the daemon starts, not at the next restart.
        //
        // The **recovered conversation** of a replacement session
        // (`session_recovery.rs`) rides the same prompt, in front of it and
        // behind the standing instructions: it is context for the agent and not
        // a message the human just wrote, which is exactly what the preamble
        // slot is for. Taken inside the closure, so the one prompt that gets it
        // is the one that consumes it.
        let first_prompt =
            (is_agent && !text.is_empty() && runtime.take_first_prompt()).then(|| {
                let recovered = runtime.take_recovered_context();
                let preamble = crate::session::session_recovery::preamble_with_recovered(
                    preset_preamble,
                    recovered.as_deref(),
                );
                compose_first_prompt(
                    &self.standing_instructions(),
                    spawn_prompt,
                    preamble.as_deref(),
                    text,
                )
            });
        let text = first_prompt.as_deref().unwrap_or(text);
        // (S4-10, S4-14) The last thing before the write: the slot's boundary must
        // be the turn this text actually enters. The admission registered it
        // against the turn that was running then, and that turn can have ended —
        // and another can have started — while the delivery was on its way here.
        // Both cases look the same from the slot's side (`boundary_reached` set by
        // the old turn's hook, or a turn id that is not the admitted one), and both
        // are fixed the same way: re-key the boundary onto whichever turn is
        // running now, or onto the turn the prompt is about to start. Steering into
        // the turn that is running is still the right delivery; only the slot's
        // bookkeeping has to follow it.
        if let Some(slot) = message_slot {
            if message_slot_boundary_is_stale(slot, runtime.turn_counter()) {
                rearm_message_slot_boundary(slot, &runtime);
            }
        }
        // The user's text was checked against MAX_WRITE_BYTES above, before a
        // single line of ours is added, so the cap can never refuse a prompt
        // that was legal on arrival. The appended block is bounded by a fixed
        // number of absolute paths the daemon composed itself: at most
        // MAX_ATTACHMENT_COUNT of them for the inline attachments and at most
        // MAX_ATTACHMENT_REFERENCES for the stored references, both enforced by
        // the wire validation above. Re-checking the extended prompt could only
        // refuse a prompt the daemon lengthened; the write is not re-checked
        // against the cap.
        //
        // The stored references are resolved before any of the prompt text is
        // built, which is the rule `with_attachment_paths` states for the
        // inline attachments: a request that fails on its third item must leave
        // nothing half-built. Every reference is either resolved here or the
        // call returns, so the string built below is never a prompt missing one
        // of the files it named. The store read is the first disk work this
        // request does, and the wire validation above is what keeps a malformed
        // reference from reaching it.
        let reference_paths =
            resolve_attachment_references(&self.attachments, session_id, attachment_references)?;
        // The structured route: the sibling is present (an ACP session) AND
        // the live negotiated capability says images are supported. The plan
        // decides both halves — the blocks that travel and the exact string
        // the journal records — so they cannot drift apart. Otherwise —
        // sibling absent (terminals, and the three providers that take the
        // static route below), or the handshake said no or nothing — fall
        // through to exactly today's path-line write, byte for byte
        // unchanged.
        let mut plan = match image_sink.as_ref() {
            Some(sink) if sink.delivery() == ImageDelivery::NegotiatedImageBlock => {
                plan_structured_prompt(&self.attachments, session_id, text, attachments)?
            }
            _ => None,
        };
        // The static route: the sibling is present only for the three
        // providers the daemon statically knows carry images (Claude, Codex,
        // Pi). Its plan is built here, before the writer is locked, for the
        // same reason the ACP plan is: the decode and the strip walk must not
        // run under that hold. `None` means the route did not run (no
        // attachments, or a provider not authorised for inline bytes) and
        // nothing was materialized for it.
        let mut static_plan = match static_image_sink.as_ref() {
            Some(sink) => sink.plan_prompt(&self.attachments, session_id, text, attachments)?,
            None => None,
        };
        // The references join the text of whichever route planned this prompt,
        // as path lines, before anything reads that text — the frame the
        // provider receives and the string the journal records are one value in
        // both plans, so appending to it here is appending to both. A
        // reference never becomes an image block, on any route: see
        // [`push_reference_path_lines`] for why that is a decision.
        if let Some(plan) = plan.as_mut() {
            push_reference_path_lines(&mut plan.fallback_text, &reference_paths);
        }
        if let Some(plan) = static_plan.as_mut() {
            plan.append_reference_path_lines(&reference_paths);
        }
        // A session carries one route or the other, never both: `image_sink`
        // is the ACP one and `static_image_sink` the three static providers'.
        // `plan` is `Some` only when at least one raster became a block, so
        // an SVG-only prompt on a capable session takes this arm too: the
        // legacy write, materialized once, never twice — and on the static
        // route the same holds for a prompt whose every block became a path
        // line, because the plan answers with its own text either way.
        let prompt = match plan.as_ref() {
            Some(plan) => plan.fallback_text.clone(),
            None => match static_plan.as_ref() {
                // The plan decoded, sniffed and stripped every attachment
                // already and built the text from the paths it holds, so
                // reaching for `with_attachment_paths` here would do all of
                // that a second time for each of them.
                Some(plan) => plan.text().to_string(),
                // The only route that composes its text here rather than in a
                // plan, so the references are appended here — with the same
                // function and the same separator the two plans use, since a
                // prompt's shape must not depend on which route wrote it.
                None => {
                    let mut prompt =
                        with_attachment_paths(&self.attachments, session_id, text, attachments)?;
                    push_reference_path_lines(&mut prompt, &reference_paths);
                    prompt
                }
            },
        };
        // Lock the writer FIRST, as today: the journaled transcript event is
        // published under this same hold further down, so the journal keeps
        // the order the process sees. The structured send runs under this
        // hold too, locking the transport's pending table and child stdin —
        // the same order the pre-existing `AcpWriter::flush` path already
        // used when it issued its request from under this hold — so no new
        // lock ordering is introduced.
        // Keep the complete write and its transcript event under this lock so
        // the journal preserves the same order the process receives.
        let mut writer = match writer.lock() {
            Ok(writer) => writer,
            Err(_) => {
                let error = internal("Session state is unavailable.");
                if let Some(runtime) = agent_runtime.as_ref() {
                    runtime.publish_agent_error(error.message.clone());
                }
                return Err(error);
            }
        };
        if let Err(error) = match plan {
            // Structured route: the text block (with any SVG path lines) plus
            // the image blocks go as one `session/prompt` content array on
            // the sibling. The plain-text `writer` is not touched. The plan
            // travels whole, so the text block the child receives IS the
            // string journaled below — one value, two destinations.
            Some(plan) => {
                let sink = image_sink.as_ref().expect("plan implies a capable sibling");
                sink.send_structured_prompt(plan)
            }
            // The static route's frame goes out here, under the same hold and
            // for the same reason: the text on the wire and the text journaled
            // below come from the one plan.
            None => match static_plan {
                Some(plan) => plan.send(),
                // Today's path, unchanged: the prompt (with path lines) is
                // typed into the plain-text writer.
                None => writer.write_all(prompt.as_bytes()).map_err(|error| {
                    WireError::new(
                        ErrorCode::Io,
                        format!("Could not send input to the terminal: {error}"),
                    )
                }),
            },
        } {
            drop(writer);
            if let Some(runtime) = agent_runtime.as_ref() {
                runtime.publish_agent_error(error.message.clone());
            }
            return Err(error);
        }
        if let Err(error) = writer.flush().map_err(|error| {
            WireError::new(
                ErrorCode::Io,
                format!("Could not flush input to the terminal: {error}"),
            )
        }) {
            drop(writer);
            if let Some(runtime) = agent_runtime.as_ref() {
                runtime.publish_agent_error(error.message.clone());
            }
            return Err(error);
        }
        // The transcript id this delivery produced, when it produced one: the
        // `AgentUserMessage` an agent session echoes for accepted input. A
        // terminal has no transcript record and answers `None`.
        let mut delivered_message_id: Option<String> = None;
        if has_prompt {
            if let Some(runtime) = agent_runtime.as_ref() {
                // The journal records `prompt`: on the fallback path that is
                // the same string the writer got (the user's text plus one
                // path per attachment); on the structured path it is the
                // text block (the user's text plus any SVG path lines). The
                // base64 never leaves `PromptAttachment` either way — a
                // turn's row must not grow by hundreds of KiB, and the user's
                // images must not be copied into the history database.
                match runtime.publish_agent_user_message(prompt.clone(), author, message_kind) {
                    Some(message_id) => delivered_message_id = Some(message_id),
                    None => return Err(internal("Agent input could not be recorded.")),
                }
                runtime.begin_turn();
                if runtime.clear_attention() {
                    self.notify_session_transition(owner, session_id);
                }
            }
        }
        drop(writer);
        Ok(delivered_message_id)
    }

    pub fn report_agent(
        &self,
        session_id: &str,
        report: crate::agent_report::AgentReport,
        peer: Option<&crate::agent_report::PeerIdentity>,
    ) -> Result<bool, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        crate::agent_report::validate_announcement(&report)?;
        #[cfg(not(windows))]
        {
            let _ = peer;
            return Err(crate::agent_report::peer_identity_unavailable_on_platform());
        }
        let runtime = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = map.get(session_id).ok_or_else(not_found)?;
            let live = entry
                .as_peer_visible()
                .ok_or_else(|| not_found_while_configuring(entry))?;
            #[cfg(windows)]
            {
                let daemon_sid = crate::security::current_user_sid().map_err(|error| {
                    crate::agent_report::unauthorized_peer(format!(
                        "Could not verify the announcing process identity: {error}"
                    ))
                })?;
                crate::agent_report::verify_announcement_peer(peer, &daemon_sid)?;
                crate::agent_report::verify_announcement_peer(peer, &live.owner.user)?;
            }
            Arc::clone(&live.runtime)
        };
        runtime.accept_agent_report(report)
    }

    #[cfg(test)]
    pub fn resize(
        &self,
        session_id: &str,
        cols: u16,
        rows: u16,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        self.resize_with_subscription(session_id, conn.id, cols, rows, owner, conn)
    }

    pub fn resize_with_subscription(
        &self,
        session_id: &str,
        subscription_id: u64,
        cols: u16,
        rows: u16,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let (runtime, master) = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = peer_entry(&map, session_id, owner, &conn.conn_peer)?;
            let session = entry.as_peer_visible().ok_or_else(process_gone)?;
            (Arc::clone(&session.runtime), session.master.clone())
        };
        check_resize_owner(&runtime, conn, subscription_id)?;
        // Resize is serialized with emulator parsing under the SAME state
        // lock as publish_output, in one defined order: emulator dimensions
        // first, then the PTY. A snapshot therefore sees the resize as wholly
        // before or wholly after itself, and no chunk is parsed into a grid
        // that is mid-resize.
        let mut stream = runtime
            .stream
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let Some(screen) = stream.screen.as_mut() else {
            // ACP sessions are structured streams and deliberately have no
            // terminal dimensions. Resize is already kind-agnostic at the
            // RPC seam; it is simply a no-op for this transport.
            return Ok(());
        };
        let (previous_cols, previous_rows) = screen.dimensions();
        screen.resize(cols.max(1), rows.max(1));
        let Some(master) = master else {
            return Ok(());
        };
        let master = master
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        if master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .is_err()
        {
            // Keep emulator and PTY in agreement: undo the grid change.
            screen.resize(previous_cols, previous_rows);
            return Err(WireError::new(
                ErrorCode::Io,
                "Could not resize the terminal.",
            ));
        }
        Ok(())
    }

    /// The directory a created child starts in (`S5` checklist).
    ///
    /// `requested` is a *relative* path inside the creator's own workspace, or
    /// `None` for the workspace root. The answer is canonicalised and checked to
    /// be inside that root: an absolute path, a `..`, a symlink pointing out, or
    /// a path that does not exist is refused rather than handed to a provider.
    /// A caller with no workspace cannot ask for a subdirectory of one.
    pub(crate) fn resolve_child_cwd(
        &self,
        workspace_id: Option<&str>,
        requested: Option<&str>,
    ) -> Result<Option<PathBuf>, WireError> {
        let Some(workspace_id) = workspace_id else {
            if requested.is_some() {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "cwd needs a workspace; this session has none.",
                ));
            }
            return Ok(None);
        };
        let root = self.workspace_cwd(workspace_id)?;
        let Some(requested) = requested.filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        let refused = || {
            WireError::new(
                ErrorCode::InvalidRequest,
                "cwd must be a directory inside the workspace.",
            )
        };
        let relative = Path::new(requested);
        if relative.is_absolute() {
            return Err(refused());
        }
        let canonical_root = root.canonicalize().map_err(|_| refused())?;
        let canonical = canonical_root
            .join(relative)
            .canonicalize()
            .map_err(|_| refused())?;
        if !canonical.starts_with(&canonical_root) || !canonical.is_dir() {
            return Err(refused());
        }
        // The confinement comparison ran on canonical spellings, and this
        // answer stays in them: the birth row records it, and the child's
        // plain cwd is taken once, at the creation hand-off
        // (`resolve_creation_inputs`) — converting here would write the
        // plain form into the journal.
        Ok(Some(canonical))
    }

    /// What a creation needs to know about the session that asked for it.
    ///
    /// Three facts, all read from the creator's own row and never from the
    /// request: its owner (the child's owner), its stored origin (the child's
    /// origin) and its workspace (the child's workspace). A session that is not
    /// this owner's is `session_not_found`, so a registration cannot be used to
    /// read a row it does not own.
    pub(crate) fn agent_creator(
        &self,
        session_id: &str,
        owner: &OwnerId,
    ) -> Result<AgentCreator, WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let entry = map.get(session_id).ok_or_else(not_found)?;
        if entry.owner().user != owner.user {
            return Err(not_found());
        }
        let live = entry
            .as_peer_visible()
            .ok_or_else(|| not_found_while_configuring(entry))?;
        Ok(AgentCreator {
            owner: entry.owner().clone(),
            origin: live.metadata.origin.clone(),
            workspace_id: live.metadata.workspace_id.clone(),
            display_name: live.metadata.display_name.clone(),
            title: live.metadata.title.clone(),
            // The context this creator belongs to, which is what its child
            // inherits (`create-from-profile`): one context for a creator and
            // everything it commissions, at any depth. Read from the creator's
            // own metadata, with the fallback the field states for a session
            // that is its own context — a live session created before v11 has
            // no context column to have read.
            context_id: live
                .metadata
                .context_id
                .clone()
                .unwrap_or_else(|| live.metadata.id.clone()),
        })
    }

    /// The workspace root the calling session's own row names, for a read that
    /// must be scoped to the caller (`S5`).
    ///
    /// The path comes from the session's `workspace_id` and a journal lookup,
    /// never from a request field, so a caller cannot name another project's
    /// folder. Fail-closed like [`Self::agent_creator`]: an unknown or unowned
    /// session is the same `session_not_found`, and a session whose row carries
    /// no workspace answers `None` so the caller refuses instead of reading
    /// whatever directory happens to be current.
    pub(crate) fn session_workspace_root(
        &self,
        session_id: &str,
        owner: &OwnerId,
    ) -> Result<Option<PathBuf>, WireError> {
        let creator = self.agent_creator(session_id, owner)?;
        let Some(workspace_id) = creator.workspace_id.as_deref() else {
            return Ok(None);
        };
        self.workspace_cwd(workspace_id).map(Some)
    }
}

/// What one admission answered: the slot it took, and whether the message went
/// into the target's running turn (S4-03).
///
/// `steered_into_turn` is the *runtime's* answer, taken under the same lock
/// `finish_turn` takes, not the caller's earlier look: it is what decides steer
/// versus plain prompt. The slot always has a boundary either way — the turn this
/// message joined, or the turn the plain prompt it became started; the expiry is
/// only the fallback for a turn that never ends.
#[derive(Debug)]
pub(crate) struct MessageAdmission {
    pub(crate) slot: u64,
    pub(crate) steered_into_turn: bool,
    /// The turn the admission registered its boundary against (S4-14): the target's
    /// counter at that moment. The delivery compares it with the turn that is
    /// running when it writes, so a message that ends up in a *different* turn is
    /// re-keyed onto that one instead of staying on the boundary of a turn that has
    /// already ended.
    pub(crate) expected_turn_id: u64,
}

/// The boundary callback of one slot, with the cell that tells it which hook it is
/// (S4-15, S5-01).
///
/// The callback compares its own hook id — read out of the cell *while it holds the
/// brakes lock* — with the id the slot currently holds, and acts only when they are
/// the same. A callback whose hook has been replaced by a re-arm therefore does
/// nothing: without that check it would take the *new* hook, unregister it, and mark
/// the slot `boundary_reached`, which is exactly how a re-armed slot loses its live
/// boundary when the old callback was already waiting for the brakes lock.
///
/// **Invariant:** registration stores the id into the cell *before* it releases
/// `brakes`, and the callback loads the cell *after* it takes `brakes`. Both halves
/// are required: the store under the lock serializes it against every callback that
/// acquires the lock, and the load under the lock is what makes the callback see a
/// value that is already stored. A callback that read the cell before taking the lock
/// could read the initial `0` in the window between `on_turn_end` returning and the
/// store, be rejected against the live id, and leave its slot without an effective
/// boundary until the expiry (S5-01). Both registrations — `reserve_message_brake`
/// and `rearm_message_slot_boundary` — keep the store inside their `brakes` hold.
///
/// A callback that fires before its slot's entry exists finds nothing to act on and
/// does nothing — never a panic and never a release — so the slot falls back to its
/// expiry, the safe direction. With the invariant in place that window is not
/// observable from a callback that runs after the registration completes: the lock
/// serializes it behind the store.
fn message_slot_boundary(
    brakes: &Arc<Mutex<MessageBrakeTable>>,
    brake_key: &str,
    slot: u64,
) -> (Arc<dyn Fn() + Send + Sync>, Arc<AtomicU64>) {
    let hook_id = Arc::new(AtomicU64::new(0));
    let callback: Arc<dyn Fn() + Send + Sync> = {
        let brakes = Arc::clone(brakes);
        let from = brake_key.to_string();
        let hook_id = Arc::clone(&hook_id);
        // (S5-01) The cell is handed to the callback, not a value read here: the
        // load happens inside `boundary_reached_message_slot`, under the brakes
        // lock, so it cannot observe the window before the registering side stored
        // the id.
        Arc::new(move || boundary_reached_message_slot(&brakes, &from, slot, &hook_id))
    };
    (callback, hook_id)
}

/// Whether this slot's boundary is stale (S4-14): its admitted turn has already
/// ended, or the runtime has moved on to a different turn than the one the
/// admission checked.
///
/// Read under the brakes lock, but as its own step: the delivery uses it to decide
/// whether the slot has to be re-keyed onto the turn its text is about to enter,
/// and the re-arm itself is the only writer.
fn message_slot_boundary_is_stale(slot: &MessageSlotRef<'_>, entering_turn_id: u64) -> bool {
    let Ok(table) = slot.brakes.lock() else {
        return false;
    };
    let Some(brake) = table.get(slot.brake_key) else {
        return false;
    };
    let Some(entry) = brake
        .outstanding
        .iter()
        .find(|entry| entry.slot == slot.slot)
    else {
        return false;
    };
    entry.boundary_reached || entering_turn_id != slot.admitted_turn_id
}

/// Re-key one slot's boundary onto the turn its text actually enters (S4-10,
/// S4-14).
///
/// Called by the delivery before the write, under the brakes lock, once the slot's
/// boundary is known to be stale: the turn the message was admitted into ended —
/// its hook has already fired and set `boundary_reached` — and the text is about to
/// steer into a newer turn or become an ordinary prompt. Either way the turn it
/// enters is the turn that ends it, so: clear the flag, drop the old hook (a no-op
/// when it already fired, and harmless when it is still armed), and arm the same
/// boundary the admission arms, for the turn that is coming.
///
/// The expiry and a failed delivery still end the slot on their own; the point is
/// that a *successful* delivery never retires a slot whose turn is still running.
pub(super) fn rearm_message_slot_boundary(
    slot: &MessageSlotRef<'_>,
    runtime: &Arc<SessionRuntime>,
) {
    let Ok(mut table) = slot.brakes.lock() else {
        return;
    };
    let Some(brake) = table.get_mut(slot.brake_key) else {
        return;
    };
    let Some(entry) = brake
        .outstanding
        .iter_mut()
        .find(|entry| entry.slot == slot.slot)
    else {
        return;
    };
    entry.boundary_reached = false;
    if let Some((previous, hook)) = entry.release.take() {
        if let Some(previous) = previous.upgrade() {
            previous.off_turn_end(hook);
        }
    }
    // (S4-15) Registered first, then the id is written back into the cell: the
    // callback compares that id with the one this entry holds, so the hook that
    // was just replaced can no longer unregister its successor.
    let (boundary, hook_id) = message_slot_boundary(slot.brakes, slot.brake_key, slot.slot);
    let armed = runtime.on_turn_end(move || boundary());
    hook_id.store(armed, Ordering::Release);
    entry.release = Some((Arc::downgrade(runtime), armed));
}

/// Admit one inter-agent message, answering the slot it took and the turn it
/// joined (S4-03).
///
/// The brakes are the sender's budget: at most [`MAX_MESSAGE_OUTSTANDING`]
/// messages in flight, at most [`MAX_MESSAGE_SENT_PER_WINDOW`] inside the
/// rate window, and at most [`MAX_MESSAGE_RECIPIENTS`] distinct recipients
/// inside the recipient window. "In flight" ends at a boundary, not at the
/// next probe: the slot is released by the turn it went into ending — the hook
/// registered on `target` here — by [`finish_message_delivery`] when the
/// delivery fails, or by expiry at [`MESSAGE_SLOT_EXPIRY`], whichever comes
/// first.
///
/// `target` is the runtime whose turn the message joins plus the turn id the
/// caller checked. The check and the registration are one atomic step on that
/// runtime, and its answer — not the caller's snapshot — is what decides between
/// steer and prompt (S4-03). The boundary is armed for either outcome: the turn
/// the message joined, or the turn the plain prompt it became started.
///
/// `now` is the caller's clock rather than `Instant::now()`, so the two windows
/// are testable by moving the clock instead of sleeping through it.
pub(super) fn reserve_message_brake(
    brakes: &Arc<Mutex<MessageBrakeTable>>,
    brake_key: &str,
    to_session: &str,
    target: Option<(&Arc<SessionRuntime>, u64)>,
    now: Instant,
) -> Result<MessageAdmission, WireError> {
    let mut table = brakes
        .lock()
        .map_err(|_| internal("Agent message state is unavailable."))?;
    // (S4-12, S4-16) The table is swept here, before this sender's own entry is
    // touched: a session that closed keeps its recipient window (that is the point
    // — a close-and-resume must not buy a fresh set of three), so something has to
    // age those entries out, and this is the path that sees the whole table with a
    // clock. Entries whose window has run out are pruned with the caller's clock,
    // their expired hooks join the list this function unregisters below, and an
    // entry with nothing left in it goes.
    //
    // The sweep costs one pass over every other sender while the single brakes lock
    // is held, so it runs at most once per [`MESSAGE_RATE_WINDOW`] (S4-16) — a
    // sender that never sweeps cannot make every other sender's admission pay for
    // it. The caller's own entry is still pruned on every reserve, which is what
    // its own braking needs.
    let mut expired: Vec<(Weak<SessionRuntime>, u64)> = Vec::new();
    if table.sweep_is_due(now) {
        let mut swept: Vec<String> = Vec::new();
        for (other, other_brake) in table.iter_mut() {
            if other == brake_key {
                continue;
            }
            expired.extend(other_brake.prune(now));
            if other_brake.is_idle() {
                swept.push(other.clone());
            }
        }
        for other in swept {
            table.remove(&other);
        }
        table.note_sweep(now);
    }
    let brake = table
        .entry(brake_key.to_string())
        .or_insert_with(MessageBrake::new);
    expired.extend(brake.prune(now));
    if now.saturating_duration_since(brake.window_started) >= MESSAGE_RATE_WINDOW {
        brake.window_started = now;
        brake.sent_in_window = 0;
    }
    // The refusals are *collected* rather than returned on the spot: the expired
    // hooks from `prune` are unregistered below, after this lock is released
    // (S4-02), and an early return here would leave them armed on their runtimes
    // forever.
    let refused = if brake.outstanding.len() >= MAX_MESSAGE_OUTSTANDING
        || brake.sent_in_window >= MAX_MESSAGE_SENT_PER_WINDOW
    {
        Some(WireError::new(
            ErrorCode::CapabilityNotSupported,
            "Agent message limit exceeded; do not retry.",
        ))
    } else if !brake.holds_recipient(to_session) && brake.recipients.len() >= MAX_MESSAGE_RECIPIENTS
    {
        Some(WireError::new(
            ErrorCode::CapabilityNotSupported,
            "Agent message recipient limit exceeded; do not retry.",
        ))
    } else {
        None
    };
    let admission = if refused.is_some() {
        None
    } else {
        let slot = brake.next_slot;
        brake.next_slot = brake.next_slot.saturating_add(1);
        // (S4-03) The turn this message joins and the boundary that releases its
        // slot are decided by one atomic step on the runtime: the turn cannot end
        // between the check and the registration without this answering `None`.
        //
        // A slot always has a boundary, whichever turn it turns out to be: the
        // turn this message joins when the answer is `Some` — and when it is
        // `None` the text goes as a plain prompt, so the boundary is the end of
        // the turn that prompt starts. The expiry is only the fallback for a
        // turn that never ends.
        let mut steered_into_turn = false;
        // (S4-15) The callback is built once and its id cell filled in as soon as
        // the runtime answers with the hook it registered; the second arm reads the
        // same cell, so whichever hook is live compares itself against the id this
        // entry ends up holding.
        let (boundary, hook_id) = message_slot_boundary(brakes, brake_key, slot);
        let release = target.map(|(runtime, expected_turn)| {
            let target = Arc::downgrade(runtime);
            let armed = {
                let first = Arc::clone(&boundary);
                match runtime.on_turn_end_if_active(expected_turn, move || first()) {
                    Some(hook) => {
                        steered_into_turn = true;
                        hook
                    }
                    None => {
                        let second = Arc::clone(&boundary);
                        runtime.on_turn_end(move || second())
                    }
                }
            };
            hook_id.store(armed, Ordering::Release);
            (target, armed)
        });
        brake.outstanding.push(OutstandingMessage {
            slot,
            sent_at: now,
            to_session: to_session.to_string(),
            delivered: false,
            boundary_reached: false,
            release,
        });
        brake.sent_in_window = brake.sent_in_window.saturating_add(1);
        if let Some(recipient) = brake
            .recipients
            .iter_mut()
            .find(|recipient| recipient.session_id == to_session)
        {
            // A recipient the sender keeps writing to stays in the window: the
            // window answers "who has this sender written to lately".
            recipient.sent_at = now;
        } else {
            brake.recipients.push(Recipient {
                session_id: to_session.to_string(),
                sent_at: now,
            });
        }
        Some(MessageAdmission {
            slot,
            steered_into_turn,
            expected_turn_id: target.map(|(_, turn)| turn).unwrap_or(0),
        })
    };
    // Released before any runtime lock is taken, the order
    // `boundary_reached_message_slot` uses.
    drop(table);
    // The brake lock is released before any runtime lock is taken: the expiry
    // hooks go back on their runtimes here, outside it (S4-02), the same order
    // `boundary_reached_message_slot` uses.
    for (runtime, hook) in expired {
        if let Some(runtime) = runtime.upgrade() {
            runtime.off_turn_end(hook);
        }
    }
    match (admission, refused) {
        (_, Some(error)) => Err(error),
        (Some(admission), None) => Ok(admission),
        (None, None) => Err(internal("Agent message state is unavailable.")),
    }
}

/// The boundary arrived for one slot: the target's turn ended, or the slot's own
/// expiry ran out.
///
/// A slot whose delivery has already returned is over and goes here, with its
/// hook unregistered and its recipient entry dropped once no other slot names
/// that target (A2-06). One whose delivery is still in flight keeps its place: the message it counts is still being
/// written, and releasing it now would let the next send past the cap this
/// count exists to keep (A2-05).
///
/// `hook_id` is the cell holding the id of the hook this callback was armed as
/// (S4-15, S5-01). It is loaded *inside* the locked section below — never before —
/// so that the registering side's store, which it performs while it holds the same
/// lock, is always visible here. A callback that ran after the slot was re-keyed
/// holds the *old* id, while the entry holds the new one: it is a no-op, because
/// its turn is not the turn the slot is waiting on any more and taking the live
/// hook here would leave the slot without a boundary.
pub(super) fn boundary_reached_message_slot(
    brakes: &Arc<Mutex<MessageBrakeTable>>,
    brake_key: &str,
    slot: u64,
    hook_id: &AtomicU64,
) {
    let mut hooks: Vec<(Weak<SessionRuntime>, u64)> = Vec::new();
    {
        let Ok(mut table) = brakes.lock() else {
            return;
        };
        // (S5-01) Under the lock: the id the registering side stored before it
        // released this same lock, so a turn end dispatched in the window between
        // `on_turn_end` returning and the store cannot be rejected with the initial
        // zero.
        let hook_id = hook_id.load(Ordering::Acquire);
        let mut to_session = None;
        let mut drop_sender = false;
        if let Some(brake) = table.get_mut(brake_key) {
            let mut remove_slot = false;
            if let Some(entry) = brake
                .outstanding
                .iter_mut()
                .find(|entry| entry.slot == slot)
            {
                if entry.release.as_ref().map(|(_, armed)| *armed) != Some(hook_id) {
                    return;
                }
                entry.boundary_reached = true;
                if let Some(hook) = entry.release.take() {
                    hooks.push(hook);
                }
                if entry.delivered {
                    remove_slot = true;
                    to_session = Some(entry.to_session.clone());
                }
            }
            if remove_slot {
                let _ = brake.take_slot(slot);
                if let Some(to_session) = &to_session {
                    brake.drop_recipient_if_idle(to_session, Instant::now());
                }
            }
            drop_sender = brake.is_idle();
        }
        if drop_sender {
            table.remove(brake_key);
        }
    }
    for (runtime, hook) in hooks {
        if let Some(runtime) = runtime.upgrade() {
            runtime.off_turn_end(hook);
        }
    }
}

/// Report one delivery back to the bookkeeping (A2-05).
///
/// `delivered` false is a delivery that reached nothing: the slot goes back at
/// once, because holding a sender's budget for a turn that will never see the
/// message is exactly what the release-on-failure rule is for. `delivered` true
/// keeps the slot until its boundary — one already reached releases it here, one
/// still ahead releases it when it arrives.
pub(super) fn finish_message_delivery(
    brakes: &Arc<Mutex<MessageBrakeTable>>,
    brake_key: &str,
    slot: u64,
    delivered: bool,
) {
    let mut hooks: Vec<(Weak<SessionRuntime>, u64)> = Vec::new();
    {
        let Ok(mut table) = brakes.lock() else {
            return;
        };
        let mut to_session = None;
        let mut drop_sender = false;
        if let Some(brake) = table.get_mut(brake_key) {
            let mut remove_slot = false;
            if let Some(entry) = brake
                .outstanding
                .iter_mut()
                .find(|entry| entry.slot == slot)
            {
                entry.delivered = true;
                if !delivered || entry.boundary_reached {
                    remove_slot = true;
                    to_session = Some(entry.to_session.clone());
                }
            }
            if remove_slot {
                if let Some(hook) = brake.take_slot(slot) {
                    hooks.push(hook);
                }
                if let Some(to_session) = &to_session {
                    brake.drop_recipient_if_idle(to_session, Instant::now());
                }
            }
            drop_sender = brake.is_idle();
        }
        if drop_sender {
            table.remove(brake_key);
        }
    }
    for (runtime, hook) in hooks {
        if let Some(runtime) = runtime.upgrade() {
            runtime.off_turn_end(hook);
        }
    }
}

/// Forget one session as a *target* (A2-06): every slot pointing at it, and its
/// recipient entries once they age out of the window (S4-01).
///
/// Called where the target closes, inside the same session-map critical section
/// that removes it from the registry, so a send that found the target cannot
/// reserve a slot for it afterwards (A2-05). A closed target's turn can never
/// end, so its slots would otherwise sit out their whole expiry holding their
/// senders' budgets for a session that is gone — those go at once.
///
/// A recipient entry is *deliberately* kept while it is still inside the window:
/// the window is the fan-out brake, and letting a close erase it early would
/// hand the sender a free slot to reach a fresh agent, which is the rotation the
/// window exists to stop.
///
/// The closing session's own entry is kept for the same reason (S4-12): closing
/// and resuming the same session id must not buy a fresh set of three recipients
/// inside the window. Its *slots* go, to this target or to any other — a closed
/// session will not send again, so those messages have no turn left to be
/// answered by — and every hook of theirs is collected here and unregistered
/// below, outside the lock, exactly as expiry does (S4-11).
///
/// The table stays bounded because the expiry sweep inside
/// [`reserve_message_brake`] drops an entry once its window has aged out.
pub(super) fn forget_message_brake_target(
    brakes: &Arc<Mutex<MessageBrakeTable>>,
    target_session: &str,
) {
    let now = Instant::now();
    let mut hooks: Vec<(Weak<SessionRuntime>, u64)> = Vec::new();
    {
        let Ok(mut table) = brakes.lock() else {
            return;
        };
        let mut idle: Vec<String> = Vec::new();
        for (from_session, brake) in table.iter_mut() {
            let closing_sender = from_session == target_session;
            let mut kept: Vec<OutstandingMessage> = Vec::with_capacity(brake.outstanding.len());
            for mut entry in brake.outstanding.drain(..) {
                if entry.to_session == target_session || closing_sender {
                    if let Some(hook) = entry.release.take() {
                        hooks.push(hook);
                    }
                } else {
                    kept.push(entry);
                }
            }
            brake.outstanding = kept;
            // Written out rather than called as a method so the closure borrows
            // only `recipients`.
            brake.recipients.retain(|recipient| {
                recipient.session_id != target_session
                    || now.saturating_duration_since(recipient.sent_at) < MESSAGE_SLOT_EXPIRY
            });
            if brake.is_idle() {
                idle.push(from_session.clone());
            }
        }
        for from_session in idle {
            table.remove(&from_session);
        }
    }
    for (runtime, hook) in hooks {
        if let Some(runtime) = runtime.upgrade() {
            runtime.off_turn_end(hook);
        }
    }
}
