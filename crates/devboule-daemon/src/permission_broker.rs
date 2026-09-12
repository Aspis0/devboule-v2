//! Shared permission broker for ACP and Claude stream-json sessions.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex};

use devboule_protocol::{
    PermissionOption, PermissionOutcome, SessionEvent, SessionOrigin, SessionOriginKind,
};

use super::SessionRuntime;

const MAX_PENDING_ACP_PERMISSIONS: usize = 32;
/// How many undecided permission cards one paired device may hold at once
/// (§8b A14).
///
/// A peer's session can raise one card per tool call, and every one of them
/// lands in the same queue the person at this machine reads. Three is the
/// design's number: enough for an agent's immediate steps, small enough that a
/// device cannot turn the desktop into its own approval prompt. The fourth is
/// refused, not stacked.
const MAX_PENDING_FOR_PEER: usize = 3;
pub(super) const MAX_ACP_PERMISSION_FIELD_BYTES: usize = 8 * 1024;
pub(super) const MAX_ACP_PERMISSION_OPTIONS: usize = 32;
const MAX_ACP_PERMISSION_ARGS: usize = 256;
const MAX_ACP_PERMISSION_ENV: usize = 64;

pub(super) type PermissionSender = dyn Fn(u64, serde_json::Value) -> io::Result<()> + Send + Sync;

/// How a pending permission is completed. Agent-initiated prompts write a
/// JSON-RPC result to the agent's stdin. Host-initiated prompts (the
/// `terminal/create` gate) wake the host RPC thread that is blocked on the
/// decision instead — they must not invent an ACP permission response.
enum PermissionResponder {
    Agent { acp_id: u64 },
    Host,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HostDecision {
    Allow,
    Deny,
    Timeout,
    Cancelled,
}

struct PermissionCompletion {
    done: bool,
    decision: Option<HostDecision>,
}

pub(super) struct PendingPermission {
    responder: PermissionResponder,
    tool_call_id: String,
    session_id: String,
    request: SessionEvent,
    runtime: std::sync::Weak<SessionRuntime>,
    done: Arc<(Mutex<PermissionCompletion>, std::sync::Condvar)>,
}

/// An allow option chosen for an unattended-mode request, with the pending
/// entry it decides.
struct AutoAnswer {
    pending: Arc<PendingPermission>,
    option: PermissionOption,
}

struct PermissionTable {
    entries: HashMap<String, Arc<PendingPermission>>,
    closed: bool,
}

pub(crate) struct PermissionBroker {
    sender: Arc<PermissionSender>,
    pending: Mutex<PermissionTable>,
    require_journal: bool,
    #[cfg(test)]
    after_take_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

#[derive(Debug)]
pub(super) enum PermissionResponseError {
    NotFound,
    InvalidRequest(String),
    Io(io::Error),
}

impl fmt::Display for PermissionResponseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("permission request is no longer pending"),
            Self::InvalidRequest(message) => formatter.write_str(message),
            Self::Io(error) => write!(
                formatter,
                "could not answer ACP permission request: {error}"
            ),
        }
    }
}

impl PermissionBroker {
    #[cfg(test)]
    pub(super) fn for_test(sender: Arc<PermissionSender>) -> Arc<Self> {
        Arc::new(Self {
            sender,
            pending: Mutex::new(PermissionTable {
                entries: HashMap::new(),
                closed: false,
            }),
            require_journal: false,
            #[cfg(test)]
            after_take_hook: Mutex::new(None),
        })
    }

    pub(super) fn send(&self, id: u64, result: serde_json::Value) -> io::Result<()> {
        (self.sender)(id, result)
    }

    pub(super) fn with_sender(sender: Arc<PermissionSender>) -> Arc<Self> {
        Arc::new(Self {
            sender,
            pending: Mutex::new(PermissionTable {
                entries: HashMap::new(),
                closed: false,
            }),
            require_journal: true,
            #[cfg(test)]
            after_take_hook: Mutex::new(None),
        })
    }

    pub(super) fn register(
        &self,
        acp_id: u64,
        request: SessionEvent,
        runtime: &Arc<SessionRuntime>,
    ) -> Result<Arc<PendingPermission>, PermissionResponseError> {
        self.register_with(PermissionResponder::Agent { acp_id }, request, runtime)
    }

    fn register_host(
        &self,
        request: SessionEvent,
        runtime: &Arc<SessionRuntime>,
    ) -> Result<Arc<PendingPermission>, PermissionResponseError> {
        self.register_with(PermissionResponder::Host, request, runtime)
    }

    fn register_with(
        &self,
        responder: PermissionResponder,
        request: SessionEvent,
        runtime: &Arc<SessionRuntime>,
    ) -> Result<Arc<PendingPermission>, PermissionResponseError> {
        // The origin is stamped here, at the one point a request becomes
        // pending: the card renders a `peer` origin as its own first line, and
        // the request's own text must never be able to imitate it (§8b A14).
        // `publish_agent_event_with_seq` writes the same value again on the way
        // out, which is what covers publishers that never come through here.
        let request = stamp_origin(request, runtime.origin());
        let tool_call_id = match &request {
            SessionEvent::PermissionRequest { tool_call_id, .. } => tool_call_id.clone(),
            _ => {
                return Err(PermissionResponseError::InvalidRequest(
                    "not a permission request".to_string(),
                ));
            }
        };
        validate_permission_request(&tool_call_id, &request)?;
        let pending = Arc::new(PendingPermission {
            responder,
            tool_call_id: tool_call_id.clone(),
            session_id: runtime.session_id.clone(),
            request,
            runtime: Arc::downgrade(runtime),
            done: Arc::new((
                Mutex::new(PermissionCompletion {
                    done: false,
                    decision: None,
                }),
                std::sync::Condvar::new(),
            )),
        });
        let mut table = self
            .pending
            .lock()
            .map_err(|_| io_error("permission broker lock poisoned"))?;
        if table.closed {
            return Err(PermissionResponseError::InvalidRequest(
                "permission broker is closed".to_string(),
            ));
        }
        if table.entries.contains_key(&tool_call_id) {
            return Err(PermissionResponseError::InvalidRequest(format!(
                "permission request {tool_call_id} is already pending"
            )));
        }
        // One paired device may hold at most `MAX_PENDING_FOR_PEER` undecided
        // cards: the queue a peer fills is the queue the person at this
        // machine has to read. Counted per origin device, so two devices'
        // sessions never share the allowance.
        if let Some(device_id) = peer_origin_device(&pending.request) {
            let peer_pending = table
                .entries
                .values()
                .filter(|entry| {
                    peer_origin_device(&entry.request).as_deref() == Some(device_id.as_str())
                })
                .count();
            if peer_pending >= MAX_PENDING_FOR_PEER {
                return Err(PermissionResponseError::InvalidRequest(format!(
                    "that device already has {MAX_PENDING_FOR_PEER} permission requests waiting"
                )));
            }
        }
        let session_pending = table
            .entries
            .values()
            .filter(|pending| pending.session_id == runtime.session_id)
            .count();
        if session_pending >= MAX_PENDING_ACP_PERMISSIONS {
            return Err(PermissionResponseError::InvalidRequest(format!(
                "session has reached the maximum of {MAX_PENDING_ACP_PERMISSIONS} pending permission requests"
            )));
        }
        table.entries.insert(tool_call_id, Arc::clone(&pending));
        Ok(pending)
    }

    pub(super) fn respond(
        &self,
        tool_call_id: &str,
        outcome: PermissionOutcome,
    ) -> Result<(), PermissionResponseError> {
        self.respond_with_option(tool_call_id, outcome, None)
    }

    pub(super) fn respond_with_option(
        &self,
        tool_call_id: &str,
        outcome: PermissionOutcome,
        option_id: Option<String>,
    ) -> Result<(), PermissionResponseError> {
        let options = {
            let table = self
                .pending
                .lock()
                .map_err(|_| io_error("permission broker lock poisoned"))?;
            let pending = table
                .entries
                .get(tool_call_id)
                .ok_or(PermissionResponseError::NotFound)?;
            match &pending.request {
                SessionEvent::PermissionRequest { options, .. } => options.clone(),
                _ => Vec::new(),
            }
        };
        let option = select_option(&options, outcome, option_id.as_deref())
            .map_err(PermissionResponseError::InvalidRequest)?;
        let pending = self.take(tool_call_id, None)?;
        #[cfg(test)]
        self.run_after_take_hook();
        let Some(option) = option else {
            let reason = unsupported_outcome_reason(&options, outcome);
            return match self.complete(
                &pending,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
                None,
                "cancelled",
            ) {
                Ok(()) => Err(PermissionResponseError::InvalidRequest(reason)),
                Err(error) => Err(error),
            };
        };
        // Only the exact one-shot kind is resolved implicitly; a durable
        // option stays pending until the client names it.
        let result = serde_json::json!({
            "outcome": { "outcome": "selected", "optionId": option.option_id }
        });
        self.complete(
            &pending,
            result,
            Some(&option),
            match outcome {
                PermissionOutcome::AllowOnce => "allow_once",
                PermissionOutcome::Deny => "deny",
            },
        )
    }

    /// Auto-answer unattended modes only when the agent offers one allow
    /// choice; chooser requests stay with the client. Paseo's chooser rule:
    /// the same allow kind twice (two `allow_once` with different names) is a
    /// question, the standard `allow_once`/`allow_always`/`reject_once` batch
    /// is not. Prefer allow_once, then allow_always; a request with no allow
    /// option stays pending for the user. The journal records the kind that
    /// was really granted, never a one-shot constant.
    pub(super) fn auto_answer(
        &self,
        tool_call_id: &str,
        runtime: &Arc<SessionRuntime>,
    ) -> Result<bool, PermissionResponseError> {
        let Some(mode_id) = runtime.current_mode_id() else {
            return Ok(false);
        };
        if !matches!(
            mode_id.as_str(),
            "bypass" | "auto_accept" | "bypassPermissions"
        ) {
            return Ok(false);
        }
        let Some(AutoAnswer { pending, option }) = self.take_auto_answerable(tool_call_id)? else {
            return Ok(false);
        };
        let result = serde_json::json!({
            "outcome": { "outcome": "selected", "optionId": option.option_id }
        });
        self.complete(&pending, result, Some(&option), &option.kind)?;
        Ok(true)
    }

    #[cfg(test)]
    pub(super) fn expire(&self, tool_call_id: &str, expected: &Arc<PendingPermission>) -> bool {
        self.cancel(tool_call_id, expected, "timeout")
    }

    pub(super) fn cancel(
        &self,
        tool_call_id: &str,
        expected: &Arc<PendingPermission>,
        journal_outcome: &str,
    ) -> bool {
        let Ok(pending) = self.take(tool_call_id, Some(expected)) else {
            return false;
        };
        self.complete(
            &pending,
            serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
            None,
            journal_outcome,
        )
        .is_ok()
    }

    /// Soft interrupt: complete every pending request as cancelled but leave
    /// the broker open, so later turns can register new permissions.
    pub(super) fn cancel_pending(&self) {
        let pending = self
            .pending
            .lock()
            .map(|mut table| table.entries.drain().map(|(_, pending)| pending).collect())
            .unwrap_or_else(|_| Vec::new());
        self.complete_cancelled(pending);
    }

    /// Tear the session down: no later request may register.
    pub(super) fn close(&self) {
        let pending = self
            .pending
            .lock()
            .map(|mut table| {
                table.closed = true;
                table.entries.drain().map(|(_, pending)| pending).collect()
            })
            .unwrap_or_else(|_| Vec::new());
        self.complete_cancelled(pending);
    }

    fn complete_cancelled(&self, pending: Vec<Arc<PendingPermission>>) {
        for pending in pending {
            let _ = self.complete(
                &pending,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
                None,
                "cancelled",
            );
        }
    }

    fn take(
        &self,
        tool_call_id: &str,
        expected: Option<&Arc<PendingPermission>>,
    ) -> Result<Arc<PendingPermission>, PermissionResponseError> {
        let mut table = self
            .pending
            .lock()
            .map_err(|_| io_error("permission broker lock poisoned"))?;
        let Some(current) = table.entries.get(tool_call_id) else {
            return Err(PermissionResponseError::NotFound);
        };
        if let Some(expected) = expected {
            if !Arc::ptr_eq(current, expected) {
                return Err(PermissionResponseError::NotFound);
            }
        }
        table
            .entries
            .remove(tool_call_id)
            .ok_or(PermissionResponseError::NotFound)
    }

    /// Decide the auto-answer and remove the entry in the same lock. The
    /// entry is removed only once an allow option has been selected, so a
    /// chooser or an allow-less request stays pending for the client.
    fn take_auto_answerable(
        &self,
        tool_call_id: &str,
    ) -> Result<Option<AutoAnswer>, PermissionResponseError> {
        let mut table = self
            .pending
            .lock()
            .map_err(|_| io_error("permission broker lock poisoned"))?;
        let Some(current) = table.entries.get(tool_call_id) else {
            return Err(PermissionResponseError::NotFound);
        };
        let options = match &current.request {
            SessionEvent::PermissionRequest { options, .. } => options.clone(),
            _ => return Ok(None),
        };
        if is_allow_chooser(&options) {
            return Ok(None);
        }
        let Some(option) = select_allow_option(&options).cloned() else {
            return Ok(None);
        };
        table
            .entries
            .remove(tool_call_id)
            .map(|pending| Some(AutoAnswer { pending, option }))
            .ok_or(PermissionResponseError::NotFound)
    }

    fn complete(
        &self,
        pending: &Arc<PendingPermission>,
        result: serde_json::Value,
        selected_option: Option<&PermissionOption>,
        journal_outcome: &str,
    ) -> Result<(), PermissionResponseError> {
        let runtime = pending.runtime.upgrade();
        let recorded = runtime
            .as_ref()
            .map(|runtime| {
                runtime.record_permission_decision(
                    &pending.tool_call_id,
                    journal_outcome,
                    &pending.request,
                ) || !self.require_journal
            })
            .unwrap_or(!self.require_journal);
        let decision = if recorded {
            decision_from_outcome(journal_outcome)
        } else {
            HostDecision::Cancelled
        };
        if !recorded {
            let send_result = self.dispatch_with_fallback(
                pending,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
            );
            if let Some(runtime) = runtime {
                runtime.remove_permission_request(&pending.tool_call_id);
                let _ = runtime.publish_agent_event(permission_resolved_event(pending, None), None);
            }
            self.mark_done(pending, decision);
            return match send_result {
                Ok(()) => Err(PermissionResponseError::Io(io::Error::other(
                    "permission decision was not journaled; ACP request was cancelled",
                ))),
                Err(error) => Err(PermissionResponseError::Io(error)),
            };
        }
        let send_result = self.dispatch_with_fallback(pending, result);
        if let Some(runtime) = runtime {
            runtime.remove_permission_request(&pending.tool_call_id);
            let _ = runtime
                .publish_agent_event(permission_resolved_event(pending, selected_option), None);
        }
        self.mark_done(pending, decision);
        send_result.map_err(PermissionResponseError::Io)
    }

    fn dispatch_responder(
        &self,
        pending: &PendingPermission,
        result: serde_json::Value,
    ) -> io::Result<()> {
        match pending.responder {
            PermissionResponder::Agent { acp_id } => (self.sender)(acp_id, result),
            PermissionResponder::Host => Ok(()),
        }
    }

    fn dispatch_with_fallback(
        &self,
        pending: &PendingPermission,
        result: serde_json::Value,
    ) -> io::Result<()> {
        match self.dispatch_responder(pending, result) {
            Ok(()) => Ok(()),
            Err(error) => match self.dispatch_responder(
                pending,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
            ) {
                Ok(()) => Ok(()),
                Err(_) => Err(error),
            },
        }
    }

    fn mark_done(&self, pending: &Arc<PendingPermission>, decision: HostDecision) {
        let (done, wake) = &*pending.done;
        if let Ok(mut completed) = done.lock() {
            if completed.done {
                return;
            }
            completed.done = true;
            completed.decision = Some(decision);
            wake.notify_all();
        }
    }

    pub(super) fn pending_len(&self) -> usize {
        self.pending
            .lock()
            .map(|table| table.entries.len())
            .unwrap_or(0)
    }

    /// Register a host-initiated permission, publish it, and block until the
    /// user or cancellation decides. The ACP agent is not written to.
    pub(super) fn request_host_permission(
        self: &Arc<Self>,
        request: SessionEvent,
        runtime: &Arc<SessionRuntime>,
    ) -> HostDecision {
        let pending = match self.register_host(request, runtime) {
            Ok(pending) => pending,
            Err(_) => return HostDecision::Cancelled,
        };
        if runtime.permission_delivery_enabled() == Some(false) {
            let _ = self.cancel(&pending.tool_call_id, &pending, "capability_not_supported");
            return HostDecision::Cancelled;
        }
        // The card the client is shown is the registered one, origin included:
        // the caller's own event never carries a stamp.
        let _ = runtime.publish_agent_event(pending.request.clone(), None);
        self.wait_for_decision(&pending)
    }

    fn wait_for_decision(&self, pending: &PendingPermission) -> HostDecision {
        let (done, wake) = &*pending.done;
        let Ok(mut completed) = done.lock() else {
            return HostDecision::Cancelled;
        };
        while !completed.done {
            let Ok(next) = wake.wait(completed) else {
                return HostDecision::Cancelled;
            };
            completed = next;
        }
        completed.decision.unwrap_or(HostDecision::Cancelled)
    }

    #[cfg(test)]
    pub(super) fn pending_ids(&self) -> Vec<String> {
        self.pending
            .lock()
            .map(|table| table.entries.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[cfg(test)]
    fn set_after_take_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut stored) = self.after_take_hook.lock() {
            *stored = Some(hook);
        }
    }

    #[cfg(test)]
    fn run_after_take_hook(&self) {
        let hook = self
            .after_take_hook
            .lock()
            .ok()
            .and_then(|mut stored| stored.take());
        if let Some(hook) = hook {
            hook();
        }
    }
}

fn permission_resolved_event(
    pending: &PendingPermission,
    selected_option: Option<&PermissionOption>,
) -> SessionEvent {
    SessionEvent::PermissionResolved {
        tool_call_id: pending.tool_call_id.clone(),
        selected_option_id: selected_option.map(|option| option.option_id.clone()),
        selected_option_kind: selected_option.map(|option| option.kind.clone()),
        selected_option_name: selected_option.map(|option| option.name.clone()),
    }
}

/// The same event with the session's origin written into it, when it is a
/// permission request.
///
/// Every field is named, deliberately: a pattern with `..` would silently drop
/// the next field this variant gains, and the card's provenance line is
/// written by whoever adds it. A compile error here is the reminder.
///
/// `origin` is not `Option`, so this **overwrites** whatever a provider client
/// put there: the placeholder cannot survive to the wire, and a peer session's
/// card cannot be mislabelled as this machine's own. Called from the broker
/// (the pending entry carries the truth for the per-peer card count) and from
/// `SessionRuntime::publish_agent_event_with_seq`, which is the one place a
/// request leaves for a subscriber.
pub(super) fn stamp_origin(request: SessionEvent, origin: SessionOrigin) -> SessionEvent {
    match request {
        SessionEvent::PermissionRequest {
            tool_call_id,
            title,
            description,
            command,
            args,
            cwd,
            env,
            options,
            origin: _,
        } => SessionEvent::PermissionRequest {
            tool_call_id,
            title,
            description,
            command,
            args,
            cwd,
            env,
            options,
            origin,
        },
        other => other,
    }
}

/// The device id of a request's origin, when it came from a paired device.
fn peer_origin_device(request: &SessionEvent) -> Option<String> {
    match request {
        SessionEvent::PermissionRequest { origin, .. } => (origin.kind == SessionOriginKind::Peer)
            .then(|| origin.device_id.clone())
            .flatten(),
        _ => None,
    }
}

fn validate_permission_request(
    tool_call_id: &str,
    request: &SessionEvent,
) -> Result<(), PermissionResponseError> {
    validate_permission_field("tool_call_id", tool_call_id)?;
    let SessionEvent::PermissionRequest {
        title,
        description,
        command,
        args,
        cwd,
        env,
        options,
        ..
    } = request
    else {
        return Err(PermissionResponseError::InvalidRequest(
            "not a permission request".to_string(),
        ));
    };
    validate_permission_field("title", title)?;
    for (field, value) in [
        ("description", description.as_deref()),
        ("command", command.as_deref()),
        ("cwd", cwd.as_deref()),
    ] {
        if let Some(value) = value {
            validate_permission_field(field, value)?;
        }
    }
    if let Some(args) = args {
        if args.len() > MAX_ACP_PERMISSION_ARGS {
            return Err(PermissionResponseError::InvalidRequest(format!(
                "permission request has more than the maximum of {MAX_ACP_PERMISSION_ARGS} args"
            )));
        }
        for arg in args {
            if arg.len() > MAX_ACP_PERMISSION_FIELD_BYTES {
                return Err(PermissionResponseError::InvalidRequest(format!(
                    "permission request arg exceeds {MAX_ACP_PERMISSION_FIELD_BYTES} bytes"
                )));
            }
        }
    }
    if let Some(env) = env {
        if env.len() > MAX_ACP_PERMISSION_ENV {
            return Err(PermissionResponseError::InvalidRequest(format!(
                "permission request has more than the maximum of {MAX_ACP_PERMISSION_ENV} env vars"
            )));
        }
        for variable in env {
            validate_permission_field("env name", &variable.name)?;
            if variable.value.len() > MAX_ACP_PERMISSION_FIELD_BYTES {
                return Err(PermissionResponseError::InvalidRequest(format!(
                    "permission request env value exceeds {MAX_ACP_PERMISSION_FIELD_BYTES} bytes"
                )));
            }
        }
    }
    if options.is_empty() {
        return Err(PermissionResponseError::InvalidRequest(
            "permission request has no options".to_string(),
        ));
    }
    if options.len() > MAX_ACP_PERMISSION_OPTIONS {
        return Err(PermissionResponseError::InvalidRequest(format!(
            "permission request has more than the maximum of {MAX_ACP_PERMISSION_OPTIONS} options"
        )));
    }
    for option in options {
        validate_permission_field("option_id", &option.option_id)?;
        validate_permission_field("option name", &option.name)?;
        validate_permission_field("option kind", &option.kind)?;
    }
    Ok(())
}

fn validate_permission_field(field: &str, value: &str) -> Result<(), PermissionResponseError> {
    if value.is_empty() {
        return Err(PermissionResponseError::InvalidRequest(format!(
            "permission request has an empty {field}"
        )));
    }
    if value.len() > MAX_ACP_PERMISSION_FIELD_BYTES {
        return Err(PermissionResponseError::InvalidRequest(format!(
            "permission request {field} exceeds {MAX_ACP_PERMISSION_FIELD_BYTES} bytes"
        )));
    }
    Ok(())
}

fn decision_from_outcome(journal_outcome: &str) -> HostDecision {
    // Invariant: only the allow kinds grant a spawn. Any new journal outcome
    // that is not mapped here is a deny (Cancelled) — the catch-all is
    // deliberate, not a leftover default.
    match journal_outcome {
        "allow_once" | "allow_always" => HostDecision::Allow,
        "deny" => HostDecision::Deny,
        "timeout" => HostDecision::Timeout,
        _ => HostDecision::Cancelled,
    }
}

fn select_option(
    options: &[PermissionOption],
    outcome: PermissionOutcome,
    option_id: Option<&str>,
) -> Result<Option<PermissionOption>, String> {
    if let Some(option_id) = option_id {
        let Some(option) = options.iter().find(|option| option.option_id == option_id) else {
            return Err(format!("Unknown permission option '{option_id}'."));
        };
        let valid = match outcome {
            PermissionOutcome::AllowOnce => option.kind.starts_with("allow"),
            PermissionOutcome::Deny => option.kind.starts_with("reject"),
        };
        if !valid {
            return Err(format!(
                "Permission option '{option_id}' cannot be used for {outcome:?}."
            ));
        }
        return Ok(Some(option.clone()));
    }
    let (once_kind, intent) = match outcome {
        PermissionOutcome::AllowOnce => ("allow_once", "allow"),
        PermissionOutcome::Deny => ("reject_once", "reject"),
    };
    if let Some(option) = options.iter().find(|option| option.kind == once_kind) {
        return Ok(Some(option.clone()));
    }
    // A durable option of the same intent is never answered implicitly: the
    // client has to name it, and the request stays pending until then.
    if options.iter().any(|option| option.kind.starts_with(intent)) {
        return Err(format!(
            "Permission request offers no '{once_kind}' option (offered: {}); the request stays pending",
            offered_kinds(options)
        ));
    }
    Ok(None)
}

/// Paseo's chooser rule: the same allow kind offered twice means the agent is
/// asking which one to use, so the request must reach the user.
fn is_allow_chooser(options: &[PermissionOption]) -> bool {
    let mut seen: Vec<&str> = Vec::new();
    for option in options
        .iter()
        .filter(|option| option.kind.starts_with("allow"))
    {
        if seen.contains(&option.kind.as_str()) {
            return true;
        }
        seen.push(&option.kind);
    }
    false
}

/// The auto-accept order Paseo uses: one-shot first, then durable.
fn select_allow_option(options: &[PermissionOption]) -> Option<&PermissionOption> {
    options
        .iter()
        .find(|option| option.kind == "allow_once")
        .or_else(|| options.iter().find(|option| option.kind == "allow_always"))
}

fn offered_kinds(options: &[PermissionOption]) -> String {
    options
        .iter()
        .map(|option| option.kind.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn unsupported_outcome_reason(options: &[PermissionOption], outcome: PermissionOutcome) -> String {
    let label = match outcome {
        PermissionOutcome::AllowOnce => "Allow once",
        PermissionOutcome::Deny => "Deny",
    };
    let intent = match outcome {
        PermissionOutcome::AllowOnce => "allow",
        PermissionOutcome::Deny => "reject",
    };
    format!(
        "Could not honor {label}: ACP did not offer any {intent} option (offered: {}); request was cancelled",
        offered_kinds(options)
    )
}

fn io_error(message: &str) -> PermissionResponseError {
    PermissionResponseError::Io(io::Error::other(message))
}

#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(test)]
pub(super) fn permission_with_kinds(tool_call_id: &str, kinds: &[(&str, &str)]) -> SessionEvent {
    SessionEvent::PermissionRequest {
        tool_call_id: tool_call_id.to_string(),
        title: "Run command".to_string(),
        description: None,
        command: Some("echo test".to_string()),
        args: None,
        cwd: None,
        env: None,
        options: kinds
            .iter()
            .map(|(option_id, kind)| PermissionOption {
                option_id: (*option_id).to_string(),
                name: (*kind).to_string(),
                kind: (*kind).to_string(),
            })
            .collect(),
        // A provider client writes `local` as a placeholder; the daemon
        // overwrites it with the session's own origin before the request leaves
        // for a subscriber.
        origin: SessionOrigin::local(),
    }
}

#[cfg(test)]
pub(super) fn permission(tool_call_id: &str) -> SessionEvent {
    permission_with_kinds(
        tool_call_id,
        &[("allow", "allow_once"), ("deny", "reject_once")],
    )
}

#[cfg(test)]
pub(super) fn permission_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "devboule-permission-{label}-{}-{nonce}.sqlite",
        std::process::id()
    ))
}

#[cfg(test)]
pub(super) fn test_broker() -> (Arc<PermissionBroker>, Arc<Mutex<SentResponses>>) {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let sent_for_sender = Arc::clone(&sent);
    let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
        sent_for_sender
            .lock()
            .expect("sent lock")
            .push((id, result));
        Ok(())
    });
    (PermissionBroker::for_test(sender), sent)
}

#[cfg(test)]
pub(super) type SentResponses = Vec<(u64, serde_json::Value)>;

#[cfg(test)]
mod tests {
    use super::SessionRuntime;
    use super::{
        permission, permission_path, permission_with_kinds, test_broker, PermissionBroker,
        PermissionSender, MAX_ACP_PERMISSION_ARGS, MAX_PENDING_ACP_PERMISSIONS,
        MAX_PENDING_FOR_PEER,
    };
    use crate::journal::Journal;
    use devboule_protocol::{PeerRole, PermissionOutcome, SessionEvent, SessionOrigin};
    use rusqlite::Connection;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;

    #[test]
    fn legacy_allow_without_a_one_shot_option_stays_pending() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                51,
                permission_with_kinds("durable-only", &[("always", "allow_always")]),
                &runtime,
            )
            .expect("register");

        let error = broker
            .respond("durable-only", PermissionOutcome::AllowOnce)
            .expect_err("a durable allow is never chosen implicitly");
        assert!(error.to_string().contains("allow_once"));
        assert_eq!(broker.pending_len(), 1);
        assert!(sent.lock().expect("sent lock").is_empty());

        broker
            .respond_with_option(
                "durable-only",
                PermissionOutcome::AllowOnce,
                Some("always".to_string()),
            )
            .expect("explicit durable option");
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].1["outcome"]["optionId"], "always");
        assert_eq!(broker.pending_len(), 0);
    }

    #[test]
    fn explicit_option_id_is_honoured_and_reported() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                59,
                permission_with_kinds(
                    "explicit",
                    &[("allow-once", "allow_once"), ("always", "allow_always")],
                ),
                &runtime,
            )
            .expect("register");

        broker
            .respond_with_option(
                "explicit",
                PermissionOutcome::AllowOnce,
                Some("allow-once".to_string()),
            )
            .expect("explicit option");
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent[0].1["outcome"]["optionId"], "allow-once");
    }

    #[test]
    fn explicit_option_with_wrong_intent_stays_pending() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                60,
                permission_with_kinds("wrong-intent", &[("deny", "reject_once")]),
                &runtime,
            )
            .expect("register");

        let error = broker
            .respond_with_option(
                "wrong-intent",
                PermissionOutcome::AllowOnce,
                Some("deny".to_string()),
            )
            .expect_err("wrong intent");
        assert!(error.to_string().contains("cannot be used"));
        assert_eq!(broker.pending_len(), 1);
        assert!(sent.lock().expect("sent lock").is_empty());
    }

    #[test]
    fn legacy_response_without_an_intent_cancels_with_a_reason() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                62,
                permission_with_kinds("no-allow", &[("deny", "reject_once")]),
                &runtime,
            )
            .expect("register");

        let error = broker
            .respond("no-allow", PermissionOutcome::AllowOnce)
            .expect_err("missing intent");
        assert!(error.to_string().contains("did not offer any allow option"));
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["outcome"],
            "cancelled"
        );
        assert_eq!(broker.pending_len(), 0);
    }

    #[test]
    fn bypass_mode_auto_answers_without_a_client_permission_request() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "bypass".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(53, permission("pi-bypass"), &runtime)
            .expect("register");

        assert!(broker.auto_answer("pi-bypass", &runtime).expect("answer"));
        assert_eq!(broker.pending_len(), 0);
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent[0].0, 53);
        assert_eq!(sent[0].1["outcome"]["optionId"], "allow");
    }

    #[test]
    fn bypass_mode_leaves_a_duplicate_allow_chooser_for_the_client() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "bypass".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(
                56,
                permission_with_kinds(
                    "pi-chooser",
                    &[("once", "allow_once"), ("once-again", "allow_once")],
                ),
                &runtime,
            )
            .expect("register");

        assert!(!broker
            .auto_answer("pi-chooser", &runtime)
            .expect("chooser policy"));
        assert_eq!(broker.pending_len(), 1);
        assert!(sent.lock().expect("sent lock").is_empty());
    }

    #[test]
    fn bypass_mode_auto_answers_the_standard_option_triple() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "bypass".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(
                63,
                permission_with_kinds(
                    "pi-standard",
                    &[
                        ("once", "allow_once"),
                        ("always", "allow_always"),
                        ("deny", "reject_once"),
                    ],
                ),
                &runtime,
            )
            .expect("register");

        assert!(broker.auto_answer("pi-standard", &runtime).expect("answer"));
        assert_eq!(broker.pending_len(), 0);
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["optionId"],
            "once"
        );
    }

    #[test]
    fn bypass_mode_leaves_a_request_without_an_allow_option_for_the_client() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "bypass".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(
                64,
                permission_with_kinds("pi-reject-only", &[("deny", "reject_once")]),
                &runtime,
            )
            .expect("register");

        assert!(!broker
            .auto_answer("pi-reject-only", &runtime)
            .expect("reject-only policy"));
        assert_eq!(broker.pending_len(), 1);
        assert!(sent.lock().expect("sent lock").is_empty());
    }

    #[test]
    fn every_allow_kind_is_an_allow_decision() {
        use super::{decision_from_outcome, HostDecision};
        assert_eq!(decision_from_outcome("allow_once"), HostDecision::Allow);
        assert_eq!(decision_from_outcome("allow_always"), HostDecision::Allow);
        assert_eq!(decision_from_outcome("deny"), HostDecision::Deny);
        assert_eq!(decision_from_outcome("timeout"), HostDecision::Timeout);
        assert_eq!(decision_from_outcome("cancelled"), HostDecision::Cancelled);
    }

    #[test]
    fn bypass_mode_journals_the_durable_allow_it_granted() {
        let path = permission_path("auto-answer");
        let journal = Arc::new(Journal::open(&path).expect("journal"));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sent_for_sender = Arc::clone(&sent);
        let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
            sent_for_sender
                .lock()
                .expect("sent lock")
                .push((id, result));
            Ok(())
        });
        let broker = PermissionBroker::with_sender(sender);
        let runtime = SessionRuntime::for_acp(
            "s.permission.auto".to_string(),
            Some(Arc::clone(&journal)),
            Arc::clone(&broker),
        );
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("grok".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "auto_accept".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(
                65,
                permission_with_kinds("durable-auto", &[("always", "allow_always")]),
                &runtime,
            )
            .expect("register");

        assert!(broker
            .auto_answer("durable-auto", &runtime)
            .expect("answer"));
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["optionId"],
            "always"
        );
        journal.flush().expect("journal flush");
        let conn = Connection::open(&path).expect("inspect journal");
        let outcome: String = conn
            .query_row(
                "SELECT outcome FROM permissions WHERE session_id = ?1 AND request_id = ?2",
                ["s.permission.auto", "durable-auto"],
                |row| row.get(0),
            )
            .expect("permission row");
        assert_eq!(outcome, "allow_always");
        drop(conn);
        journal.shutdown();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn legacy_deny_without_a_one_shot_option_stays_pending() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                57,
                permission_with_kinds("reject-always", &[("always", "reject_always")]),
                &runtime,
            )
            .expect("register");

        let error = broker
            .respond("reject-always", PermissionOutcome::Deny)
            .expect_err("a durable reject is never chosen implicitly");
        assert!(error.to_string().contains("reject_once"));
        assert_eq!(broker.pending_len(), 1);
        assert!(sent.lock().expect("sent lock").is_empty());
    }

    #[test]
    fn auto_answer_falls_back_to_a_definite_cancel_after_send_failure() {
        let first_attempt = Arc::new(AtomicBool::new(true));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let first_attempt_for_sender = Arc::clone(&first_attempt);
        let sent_for_sender = Arc::clone(&sent);
        let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
            if first_attempt_for_sender.swap(false, Ordering::SeqCst) {
                return Err(std::io::Error::other("synthetic Pi write failure"));
            }
            sent_for_sender
                .lock()
                .expect("sent lock")
                .push((id, result));
            Ok(())
        });
        let broker = PermissionBroker::for_test(sender);
        let runtime = Arc::new(SessionRuntime::new());
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "bypass".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(58, permission("send-failure"), &runtime)
            .expect("register");

        assert!(broker
            .auto_answer("send-failure", &runtime)
            .expect("fallback cancellation"));
        assert_eq!(broker.pending_len(), 0);
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent[0].1["outcome"]["outcome"], "cancelled");
    }

    #[test]
    fn ask_mode_leaves_permission_request_for_the_broker() {
        let (broker, _) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("grok".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "ask".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(54, permission("ask"), &runtime)
            .expect("register");

        assert!(!broker.auto_answer("ask", &runtime).expect("ask policy"));
        assert_eq!(broker.pending_len(), 1);
    }

    #[test]
    fn auto_accept_prefers_allow_once_then_allow_always() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("grok".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "auto_accept".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(
                55,
                permission_with_kinds("auto-accept", &[("always", "allow_always")]),
                &runtime,
            )
            .expect("register");

        assert!(broker.auto_answer("auto-accept", &runtime).expect("answer"));
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent[0].1["outcome"]["optionId"], "always");
    }

    #[test]
    fn invalid_response_interleaving_preserves_new_registration() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                52,
                permission_with_kinds("reused", &[("always", "allow_always")]),
                &runtime,
            )
            .expect("old register");
        let broker_for_hook = Arc::downgrade(&broker);
        let runtime_for_hook = Arc::clone(&runtime);
        broker.set_after_take_hook(Arc::new(move || {
            broker_for_hook
                .upgrade()
                .expect("broker")
                .register(53, permission("reused"), &runtime_for_hook)
                .expect("new registration");
        }));

        let error = broker
            .respond("reused", PermissionOutcome::Deny)
            .expect_err("old request has no one-shot deny option");
        assert!(error
            .to_string()
            .contains("did not offer any reject option"));
        broker
            .respond("reused", PermissionOutcome::AllowOnce)
            .expect("new registration remains answerable");

        let sent = sent.lock().expect("sent lock");
        assert!(sent
            .iter()
            .any(|(id, result)| { *id == 52 && result["outcome"]["outcome"] == "cancelled" }));
        assert!(sent
            .iter()
            .any(|(id, result)| { *id == 53 && result["outcome"]["optionId"] == "allow" }));
    }

    #[test]
    fn broker_rejects_permission_floods_at_the_per_session_limit() {
        let (broker, _) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        for index in 0..MAX_PENDING_ACP_PERMISSIONS {
            broker
                .register(
                    index as u64,
                    permission(&format!("flood-{index}")),
                    &runtime,
                )
                .expect("within limit");
        }
        let error = match broker.register(999, permission("flood-over-limit"), &runtime) {
            Ok(_) => panic!("limit must reject another request"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("maximum"));
        assert_eq!(broker.pending_len(), MAX_PENDING_ACP_PERMISSIONS);
    }

    #[test]
    fn permission_request_rejects_more_than_256_args() {
        let (broker, _) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        let mut event = permission("too-many-args");
        match &mut event {
            SessionEvent::PermissionRequest { args, .. } => {
                *args = Some(
                    (0..=MAX_ACP_PERMISSION_ARGS)
                        .map(|index| format!("a{index}"))
                        .collect(),
                );
            }
            _ => panic!("permission fixture is a PermissionRequest"),
        }
        let error = match broker.register(1, event, &runtime) {
            Ok(_) => panic!("{} args must be rejected", MAX_ACP_PERMISSION_ARGS + 1),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains(&MAX_ACP_PERMISSION_ARGS.to_string()),
            "rejection must name the arg cap: {error}"
        );
    }
    /// §8b A14: the origin the broker stamps is what the card's provenance
    /// line renders. A peer session's card names its device; a local one says
    /// `local`, which the app draws as no line at all.
    #[test]
    fn a_registered_request_carries_the_sessions_origin() {
        let (broker, _sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.set_origin(SessionOrigin::peer("device-phone", PeerRole::Client));
        broker
            .register(1, permission("origin-peer"), &runtime)
            .expect("register");
        let pending = broker.take("origin-peer", None).expect("pending");
        match &pending.request {
            SessionEvent::PermissionRequest { origin, .. } => assert_eq!(
                origin,
                &SessionOrigin::peer("device-phone", PeerRole::Client)
            ),
            other => panic!("permission fixture is a PermissionRequest: {other:?}"),
        }

        // A local session's request is stamped `local`, not left bare.
        let local_runtime = Arc::new(SessionRuntime::new());
        broker
            .register(2, permission("origin-local"), &local_runtime)
            .expect("register");
        let pending = broker.take("origin-local", None).expect("pending");
        match &pending.request {
            SessionEvent::PermissionRequest { origin, .. } => {
                assert_eq!(origin, &SessionOrigin::local())
            }
            other => panic!("permission fixture is a PermissionRequest: {other:?}"),
        }
    }

    /// A device may hold three undecided cards; the fourth is refused rather
    /// than stacked in the queue the person at this machine has to read.
    #[test]
    fn a_peer_may_hold_three_permission_cards_and_not_four() {
        let (broker, _sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.set_origin(SessionOrigin::peer("device-phone", PeerRole::Client));
        for index in 0..MAX_PENDING_FOR_PEER {
            broker
                .register(index as u64, permission(&format!("card-{index}")), &runtime)
                .expect("a card inside the allowance");
        }
        // `expect_err` would need `PendingPermission: Debug`, and the type holds
        // a responder that is not printable on purpose; a match is the honest
        // shape here.
        let error = match broker.register(9, permission("card-over"), &runtime) {
            Ok(_) => panic!("the fourth card for one device must be refused"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains(&MAX_PENDING_FOR_PEER.to_string()),
            "the refusal names the allowance: {error}"
        );
        assert_eq!(broker.pending_len(), MAX_PENDING_FOR_PEER);
    }

    /// The allowance is a peer's, not the local person's: a local session may
    /// still queue more than three, which is what the desktop has always done.
    #[test]
    fn a_local_session_is_not_under_the_per_peer_allowance() {
        let (broker, _sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        for index in 0..MAX_PENDING_FOR_PEER + 1 {
            broker
                .register(
                    index as u64,
                    permission(&format!("local-{index}")),
                    &runtime,
                )
                .expect("a local session is not capped per peer");
        }
        assert_eq!(broker.pending_len(), MAX_PENDING_FOR_PEER + 1);
    }

    #[test]
    fn two_permission_requests_correlate_independently() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(11, permission("first"), &runtime)
            .expect("first");
        broker
            .register(12, permission("second"), &runtime)
            .expect("second");
        broker
            .respond("second", PermissionOutcome::Deny)
            .expect("second response");
        broker
            .respond("first", PermissionOutcome::AllowOnce)
            .expect("first response");
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].0, 12);
        assert_eq!(sent[0].1["outcome"]["optionId"], "deny");
        assert_eq!(sent[1].0, 11);
        assert_eq!(sent[1].1["outcome"]["optionId"], "allow");
    }

    #[test]
    fn permission_response_races_timeout_with_one_journaled_reply() {
        let path = permission_path("race");
        let journal = Arc::new(Journal::open(&path).expect("journal"));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let recorded_before_send = Arc::new(AtomicBool::new(false));
        let sender_started = Arc::new(Barrier::new(2));
        let sender_release = Arc::new(Barrier::new(2));
        let path_for_sender = path.clone();
        let sent_for_sender = Arc::clone(&sent);
        let recorded_for_sender = Arc::clone(&recorded_before_send);
        let entered_for_sender = Arc::clone(&sender_started);
        let release_for_sender = Arc::clone(&sender_release);
        let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
            let conn = Connection::open(&path_for_sender).expect("inspect journal");
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM permissions WHERE session_id = ?1 AND request_id = ?2",
                    ["s.permission.race", "race"],
                    |row| row.get(0),
                )
                .expect("permission row count");
            recorded_for_sender.store(count == 1, Ordering::Release);
            sent_for_sender
                .lock()
                .expect("sent lock")
                .push((id, result));
            entered_for_sender.wait();
            release_for_sender.wait();
            Ok(())
        });
        let broker = PermissionBroker::for_test(sender);
        let runtime = Arc::new(SessionRuntime::for_acp(
            "s.permission.race".to_string(),
            Some(Arc::clone(&journal)),
            Arc::clone(&broker),
        ));
        let pending = broker
            .register(21, permission("race"), &runtime)
            .expect("register");
        let start = Arc::new(Barrier::new(3));
        let respond_broker = Arc::clone(&broker);
        let respond_start = Arc::clone(&start);
        let respond_thread = thread::spawn(move || {
            respond_start.wait();
            respond_broker.respond("race", PermissionOutcome::AllowOnce)
        });
        let expire_broker = Arc::clone(&broker);
        let expire_start = Arc::clone(&start);
        let expire_thread = thread::spawn(move || {
            expire_start.wait();
            expire_broker.expire("race", &pending)
        });
        start.wait();
        sender_started.wait();
        sender_release.wait();
        let _ = respond_thread.join().expect("respond thread");
        let _ = expire_thread.join().expect("expiry thread");

        journal.flush().expect("journal flush");
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent.iter().filter(|(id, _)| *id == 21).count(), 1);
        assert_eq!(sent.len(), 1);
        assert!(recorded_before_send.load(Ordering::Acquire));
        journal.shutdown();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn daemon_shutdown_cancels_outstanding_request_before_reconnect() {
        let (old, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        old.register(31, permission("dead"), &runtime)
            .expect("register");
        old.close();
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["outcome"],
            "cancelled"
        );
        assert_eq!(old.pending_len(), 0);
        drop(old);
    }

    #[test]
    fn cancel_pending_completes_but_leaves_the_broker_open() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(51, permission("soft-stop"), &runtime)
            .expect("register");
        broker.cancel_pending();
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["outcome"],
            "cancelled"
        );
        assert_eq!(broker.pending_len(), 0);
        broker
            .register(52, permission("after-stop"), &runtime)
            .expect("a soft interrupt must not close the broker");
    }

    #[test]
    fn close_completes_and_rejects_later_requests() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(61, permission("closing"), &runtime)
            .expect("register");
        broker.close();
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["outcome"],
            "cancelled"
        );
        assert_eq!(broker.pending_len(), 0);
        let error = match broker.register(62, permission("too-late"), &runtime) {
            Ok(_) => panic!("a closed broker must reject new requests"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("closed"), "{error}");
    }

    #[test]
    fn duplicate_or_conflicting_responses_are_rejected() {
        let (broker, _) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(41, permission("once"), &runtime)
            .expect("register");
        broker
            .respond("once", PermissionOutcome::AllowOnce)
            .expect("first response");
        assert!(matches!(
            broker.respond("once", PermissionOutcome::AllowOnce),
            Err(super::PermissionResponseError::NotFound)
        ));
        assert!(matches!(
            broker.respond("once", PermissionOutcome::Deny),
            Err(super::PermissionResponseError::NotFound)
        ));
    }
}
