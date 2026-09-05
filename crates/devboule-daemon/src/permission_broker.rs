//! Shared permission broker for ACP and Claude stream-json sessions.

use std::collections::HashMap;
use std::fmt;
use std::io;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_protocol::{PermissionOption, PermissionOutcome, SessionEvent};

use super::SessionRuntime;

/// Two minutes gives a person enough time to inspect a command while still
/// bounding an ACP agent that is waiting on a viewer who has gone away. ACP
/// has no permission deadline of its own, so expiry sends `Cancelled`: no
/// operation was granted and the agent already understands that state.
pub const ACP_PERMISSION_TIMEOUT: Duration = Duration::from_secs(120);

const MAX_PENDING_ACP_PERMISSIONS: usize = 32;
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

struct PermissionTable {
    entries: HashMap<String, Arc<PendingPermission>>,
    closed: bool,
}

pub(crate) struct PermissionBroker {
    sender: Arc<PermissionSender>,
    pending: Mutex<PermissionTable>,
    require_journal: bool,
    #[cfg(test)]
    timeout: Mutex<Duration>,
    #[cfg(test)]
    after_take_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    fail_next_timeout_spawn: AtomicBool,
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
            timeout: Mutex::new(ACP_PERMISSION_TIMEOUT),
            #[cfg(test)]
            after_take_hook: Mutex::new(None),
            #[cfg(test)]
            fail_next_timeout_spawn: AtomicBool::new(false),
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
            timeout: Mutex::new(ACP_PERMISSION_TIMEOUT),
            #[cfg(test)]
            after_take_hook: Mutex::new(None),
            #[cfg(test)]
            fail_next_timeout_spawn: AtomicBool::new(false),
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
        let pending = self.take(tool_call_id, None)?;
        #[cfg(test)]
        self.run_after_take_hook();
        let option = match &pending.request {
            SessionEvent::PermissionRequest { options, .. } => select_option(options, outcome),
            _ => None,
        };
        let Some(option) = option else {
            let reason = unsupported_outcome_reason(&pending, outcome);
            return match self.complete(
                &pending,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
                "cancelled",
            ) {
                Ok(()) => Err(PermissionResponseError::InvalidRequest(reason)),
                Err(error) => Err(error),
            };
        };
        // Only the exact one-shot ACP option is selectable. A durable option
        // is never substituted for the label the user saw; if it is the only
        // option, the request is cancelled and the UI receives the reason.
        let result = serde_json::json!({
            "outcome": { "outcome": "selected", "optionId": option.option_id }
        });
        self.complete(
            &pending,
            result,
            match outcome {
                PermissionOutcome::AllowOnce => "allow_once",
                PermissionOutcome::Deny => "deny",
            },
        )
    }

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
            journal_outcome,
        )
        .is_ok()
    }

    pub(super) fn cancel_all(&self) {
        let pending = self
            .pending
            .lock()
            .map(|mut table| {
                table.closed = true;
                table.entries.drain().map(|(_, pending)| pending).collect()
            })
            .unwrap_or_else(|_| Vec::new());
        for pending in pending {
            let _ = self.complete(
                &pending,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
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

    fn complete(
        &self,
        pending: &Arc<PendingPermission>,
        result: serde_json::Value,
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
            let send_result = self.dispatch_responder(
                pending,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
            );
            if let Some(runtime) = runtime {
                runtime.remove_permission_request(&pending.tool_call_id);
                let _ = runtime.publish_agent_event(
                    SessionEvent::PermissionResolved {
                        tool_call_id: pending.tool_call_id.clone(),
                    },
                    None,
                );
            }
            self.mark_done(pending, decision);
            return match send_result {
                Ok(()) => Err(PermissionResponseError::Io(io::Error::other(
                    "permission decision was not journaled; ACP request was cancelled",
                ))),
                Err(error) => Err(PermissionResponseError::Io(error)),
            };
        }
        let send_result = self.dispatch_responder(pending, result);
        if let Some(runtime) = runtime {
            runtime.remove_permission_request(&pending.tool_call_id);
            let _ = runtime.publish_agent_event(
                SessionEvent::PermissionResolved {
                    tool_call_id: pending.tool_call_id.clone(),
                },
                None,
            );
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
    /// user (or timeout / cancel) decides. The ACP agent is not written to.
    pub(super) fn request_host_permission(
        self: &Arc<Self>,
        request: SessionEvent,
        runtime: &Arc<SessionRuntime>,
    ) -> HostDecision {
        let pending = match self.register_host(request.clone(), runtime) {
            Ok(pending) => pending,
            Err(_) => return HostDecision::Cancelled,
        };
        if runtime.permission_delivery_enabled() == Some(false) {
            let _ = self.cancel(&pending.tool_call_id, &pending, "capability_not_supported");
            return HostDecision::Cancelled;
        }
        if self.arm_timeout(Arc::clone(&pending)).is_err() {
            let _ = self.cancel(&pending.tool_call_id, &pending, "timeout_spawn_failed");
            return HostDecision::Cancelled;
        }
        let _ = runtime.publish_agent_event(request, None);
        self.wait_for_decision(&pending)
    }

    pub(super) fn arm_timeout(self: &Arc<Self>, pending: Arc<PendingPermission>) -> io::Result<()> {
        #[cfg(test)]
        if self.take_timeout_spawn_failure() {
            return Err(io::Error::other("test timeout spawn failure"));
        }
        let timeout = self.permission_timeout();
        let broker = Arc::clone(self);
        let tool_call_id = pending.tool_call_id.clone();
        std::thread::Builder::new()
            .name("acp-permission-timeout".to_string())
            .spawn(move || {
                let (done, wake) = &*pending.done;
                let deadline = Instant::now() + timeout;
                let Ok(mut completed) = done.lock() else {
                    return;
                };
                while !completed.done {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    let Ok((next, timed_out)) = wake.wait_timeout(completed, remaining) else {
                        return;
                    };
                    completed = next;
                    if timed_out.timed_out() {
                        break;
                    }
                }
                if !completed.done {
                    drop(completed);
                    let _ = broker.expire(&tool_call_id, &pending);
                }
            })
            .map(|_| ())
    }

    fn wait_for_decision(&self, pending: &PendingPermission) -> HostDecision {
        let cap = self.permission_timeout() + Duration::from_secs(30);
        let deadline = Instant::now() + cap;
        let (done, wake) = &*pending.done;
        let Ok(mut completed) = done.lock() else {
            return HostDecision::Cancelled;
        };
        while !completed.done {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return HostDecision::Cancelled;
            }
            let Ok((next, timed_out)) = wake.wait_timeout(completed, remaining) else {
                return HostDecision::Cancelled;
            };
            completed = next;
            if timed_out.timed_out() && !completed.done {
                return HostDecision::Cancelled;
            }
        }
        completed.decision.unwrap_or(HostDecision::Cancelled)
    }

    // The cfg-split makes the block unable to be a tail expression in both
    // configurations; the `return` is load-bearing.
    #[allow(clippy::needless_return)]
    fn permission_timeout(&self) -> Duration {
        #[cfg(test)]
        {
            return self
                .timeout
                .lock()
                .ok()
                .map(|guard| *guard)
                .unwrap_or(ACP_PERMISSION_TIMEOUT);
        }
        #[cfg(not(test))]
        ACP_PERMISSION_TIMEOUT
    }

    #[cfg(test)]
    pub(super) fn set_timeout(&self, timeout: Duration) {
        if let Ok(mut stored) = self.timeout.lock() {
            *stored = timeout;
        }
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

    #[cfg(test)]
    pub(super) fn fail_next_timeout_spawn(&self) {
        self.fail_next_timeout_spawn.store(true, Ordering::Release);
    }

    #[cfg(test)]
    fn take_timeout_spawn_failure(&self) -> bool {
        self.fail_next_timeout_spawn.swap(false, Ordering::AcqRel)
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
    // Invariant: only `allow_once` grants a spawn. Any new journal outcome
    // that is not mapped here is a deny (Cancelled) — the catch-all is
    // deliberate, not a leftover default.
    match journal_outcome {
        "allow_once" => HostDecision::Allow,
        "deny" => HostDecision::Deny,
        "timeout" => HostDecision::Timeout,
        _ => HostDecision::Cancelled,
    }
}

fn select_option(
    options: &[PermissionOption],
    outcome: PermissionOutcome,
) -> Option<&PermissionOption> {
    let kind = match outcome {
        PermissionOutcome::AllowOnce => "allow_once",
        PermissionOutcome::Deny => "reject_once",
    };
    options.iter().find(|option| option.kind == kind)
}

fn unsupported_outcome_reason(pending: &PendingPermission, outcome: PermissionOutcome) -> String {
    let (label, required_kind) = match outcome {
        PermissionOutcome::AllowOnce => ("Allow once", "allow_once"),
        PermissionOutcome::Deny => ("Deny", "reject_once"),
    };
    let offered = match &pending.request {
        SessionEvent::PermissionRequest { options, .. } => options
            .iter()
            .map(|option| option.kind.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    };
    format!(
        "Could not honor {label}: ACP did not offer the exact one-shot option '{required_kind}' (offered: {offered}); request was cancelled"
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
        PermissionSender, ACP_PERMISSION_TIMEOUT, MAX_ACP_PERMISSION_ARGS,
        MAX_PENDING_ACP_PERMISSIONS,
    };
    use crate::journal::Journal;
    use devboule_protocol::{PermissionOutcome, SessionEvent};
    use rusqlite::Connection;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;

    #[test]
    fn durable_only_permission_is_cancelled_and_reports_why() {
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
            .expect_err("a durable option must not satisfy allow once");
        assert!(error.to_string().contains("allow_once"));
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, 51);
        assert_eq!(sent[0].1["outcome"]["outcome"], "cancelled");
        assert_eq!(broker.pending_len(), 0);
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
        assert!(error.to_string().contains("reject_once"));
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
        old.cancel_all();
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["outcome"],
            "cancelled"
        );
        assert_eq!(old.pending_len(), 0);
        drop(old);
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
        assert_eq!(ACP_PERMISSION_TIMEOUT.as_secs(), 120);
    }
}
