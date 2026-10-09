//! The cleanup tool: the mode-following card, the exact approved plan, and
//! the two-phase termination report.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::process_cleanup_audit as audit;
use super::processes::{refreshed, reply, strict_arguments};
use crate::mcp_broker::caller::{audit_mcp_tool, McpCaller};
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::tools::first_use::{ensure_write_approved, GateMark, PROCESS_CLEANUP_GROUP};
use crate::mcp_broker::{McpBroker, RegisteredSession};
use crate::process_plan::{CleanupPlan, PlanTarget};
use crate::process_terminate::{owned_target_check, terminate_all, Termination};
use crate::server::ServerState;

/// The default and the ceiling for `graceMs`: the graceful phase is a wait
/// the caller names, and neither it nor the forced phase may be unbounded.
const DEFAULT_GRACE_MS: u32 = 2_000;
const MAX_GRACE_MS: u32 = 30_000;

/// macOS signals name a pid, and the check that confirms the process cannot be
/// bound to the signal: a process can be replaced between the two. Nothing is
/// signalled on such a platform until a signal carries the verified identity.
const SIGNALS_BIND_IDENTITY: bool = !cfg!(target_os = "macos");

/// The refusal for a platform whose signals cannot be bound to the confirmed
/// process: `None` where they can.
fn platform_refusal(signals_bind_identity: bool) -> Option<&'static str> {
    (!signals_bind_identity).then_some("signals_cannot_bind_identity")
}

/// The refusal when the plan holds more processes than the card can list.
const TOO_MANY_SENTENCE: &str =
    "refused: the plan holds more processes than the card can list exactly; nothing was stopped";

/// The refusal when this platform cannot bind a signal to a verified process.
const PLATFORM_SENTENCE: &str =
    "refused: this platform cannot bind a signal to the verified process; nothing was stopped";

/// The refusal when the session's own provider tree cannot be proved: the
/// caller is told nothing was stopped, and why.
const UNPROVEN_SENTENCE: &str =
    "refused: the caller's own provider process tree cannot be proved; nothing was stopped";

/// The cleanup answer: what is gone, what still answers, what the OS would
/// not vouch for, and every member no signal was sent to — with its reason.
fn cleanup_reply(
    id: Value,
    terminated: Vec<u32>,
    still_running: Vec<u32>,
    unproven: Vec<u32>,
    skipped: Vec<(u32, &'static str)>,
) -> Result<Option<Value>, Value> {
    let skipped = skipped
        .into_iter()
        .map(|(pid, reason)| json!({"pid": pid, "reason": reason}))
        .collect::<Vec<_>>();
    reply(
        id,
        json!({
            "terminated": terminated,
            "stillRunning": still_running,
            "unproven": unproven,
            "skipped": skipped,
        }),
        false,
    )
}

/// The most targets a card lists one by one. A larger plan is refused: a
/// person must not approve a list that is shown only in part.
const LISTED_TARGET_LIMIT: usize = 32;

fn exact_list_fits(count: usize) -> bool {
    count <= LISTED_TARGET_LIMIT
}

/// The card's exact target line: every target's pid and image basename, in
/// pid order, so two processes of the same image are two entries.
fn targets_line(targets: &[PlanTarget]) -> String {
    let mut ordered: Vec<&PlanTarget> = targets.iter().collect();
    ordered.sort_by_key(|target| target.pid);
    ordered
        .iter()
        .map(|target| {
            let image = target
                .exe
                .as_deref()
                .map(|exe| {
                    std::path::Path::new(exe)
                        .file_name()
                        .map(|base| base.to_string_lossy().into_owned())
                        .unwrap_or_else(|| exe.to_string())
                })
                .unwrap_or_else(|| "unknown".to_string());
            format!("{} {image}", target.pid)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `devboule_cleanup_processes`: the caller's own session's proven members,
/// graceful then forced, behind the same mode-following card every
/// Devboule write uses — an automatic mode approves with no card and the
/// audit row says so; an asking mode shows it, each time, listing the
/// exact processes. The plan approved is the plan executed: members that
/// appear later are left alone and reported, and no pid argument exists.
pub(in crate::mcp_broker) fn cleanup(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let arguments = strict_arguments(&id, message, &["graceMs"])?;
    let grace = match arguments.get("graceMs") {
        None | Some(Value::Null) => DEFAULT_GRACE_MS,
        Some(value) => value
            .as_u64()
            .filter(|millis| *millis <= MAX_GRACE_MS as u64)
            .map(|millis| millis as u32)
            .ok_or_else(|| {
                rpc_error(
                    id.clone(),
                    -32602,
                    "graceMs must be an integer of at most 30000",
                )
            })?,
    };
    if let Some(reason) = platform_refusal(SIGNALS_BIND_IDENTITY) {
        audit_mcp_tool(
            state,
            &caller,
            crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL,
            &registration.session_id,
            &audit::refused(reason),
        );
        return Err(tool_error(&id, PLATFORM_SENTENCE));
    }
    if let Err(error) = refreshed(state, &id) {
        audit_mcp_tool(
            state,
            &caller,
            crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL,
            &registration.session_id,
            &audit::refused("refresh_unavailable"),
        );
        return Err(error);
    }
    let plan = match state.process_index.cleanup_plan(&registration.session_id) {
        None => CleanupPlan {
            targets: Vec::new(),
            excluded: Vec::new(),
            unproven: Vec::new(),
        },
        Some(Ok(plan)) => plan,
        Some(Err(reason)) => {
            audit_mcp_tool(
                state,
                &caller,
                crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL,
                &registration.session_id,
                &audit::refused(reason),
            );
            return Err(tool_error(&id, UNPROVEN_SENTENCE));
        }
    };
    let label = state
        .process_index
        .session_label(&registration.session_id)
        .unwrap_or_else(|| registration.session_id.clone());
    let mut skipped: Vec<(u32, &'static str)> = plan
        .excluded
        .iter()
        .map(|entry| (entry.pid, entry.reason))
        .collect();
    skipped.sort_by_key(|(pid, _)| *pid);
    if plan.targets.is_empty() {
        audit_mcp_tool(
            state,
            &caller,
            crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL,
            &registration.session_id,
            &audit::nothing_to_stop(&skipped, &plan.unproven),
        );
        return cleanup_reply(id, Vec::new(), Vec::new(), plan.unproven, skipped);
    }
    let count = plan.targets.len();
    if !exact_list_fits(count) {
        audit_mcp_tool(
            state,
            &caller,
            crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL,
            &registration.session_id,
            &audit::refused("too_many_to_list"),
        );
        return Err(tool_error(&id, TOO_MANY_SENTENCE));
    }
    let count_text = count.to_string();
    let targets = targets_line(&plan.targets);
    let facts: [(&str, &str); 3] = [
        ("session", label.as_str()),
        ("processes", count_text.as_str()),
        ("targets", targets.as_str()),
    ];
    let subject = format!("stop {count} processes of {label}");
    let approval = match ensure_write_approved(
        state,
        broker,
        &registration.session_id,
        &registration.owner,
        PROCESS_CLEANUP_GROUP,
        &subject,
        &facts,
    ) {
        Ok(approval) => approval,
        Err(sentence) => {
            audit_mcp_tool(
                state,
                &caller,
                crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL,
                &registration.session_id,
                "denied",
            );
            return Err(tool_error(&id, &sentence));
        }
    };
    // The card can grant for the whole session; cleanup must ask again next
    // time, so the grant is spent the moment this call holds it.
    broker.remember_gate_mark(
        &registration.session_id,
        PROCESS_CLEANUP_GROUP,
        GateMark::None,
    );

    // From here the approval is spent: whatever happens, the row says so.
    let executed = match execute_plan(state, registration, &id, &plan, grace) {
        Ok(executed) => executed,
        Err((reason, error)) => {
            audit_mcp_tool(
                state,
                &caller,
                crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL,
                &registration.session_id,
                &audit::failed(approval, count, reason),
            );
            return Err(error);
        }
    };
    let termination = executed.termination;
    audit_mcp_tool(
        state,
        &caller,
        crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL,
        &registration.session_id,
        &audit::finished(
            approval,
            count,
            &targets,
            &termination,
            &executed.unproven,
            &executed.late,
        ),
    );
    skipped.extend(executed.late);
    skipped.extend(termination.skipped);
    skipped.sort_by_key(|(pid, _)| *pid);
    cleanup_reply(
        id,
        termination.terminated,
        termination.still_running,
        executed.unproven,
        skipped,
    )
}

/// What an approved plan left, and the members that appeared after approval.
struct Executed {
    termination: Termination,
    unproven: Vec<u32>,
    late: Vec<(u32, &'static str)>,
}

/// Run the approved plan, and only it. A fresh plan names what appeared after
/// approval and what a protected tree now holds — both left alone — and a
/// session whose tree cannot be proved any longer signals nothing. Every
/// signal is still preceded by a check against the session's job as it is
/// then. A failure names the step that failed, for the audit row.
fn execute_plan(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    id: &Value,
    plan: &CleanupPlan,
    grace: u32,
) -> Result<Executed, (&'static str, Value)> {
    refreshed(state, id).map_err(|error| ("refresh_unavailable", error))?;
    let approved: HashSet<u32> = plan
        .targets
        .iter()
        .map(|target| target.pid)
        .chain(plan.excluded.iter().map(|entry| entry.pid))
        .collect();
    let fresh = match state.process_index.cleanup_plan(&registration.session_id) {
        Some(Ok(fresh)) => fresh,
        _ => return Err(("chain_unproven", tool_error(id, UNPROVEN_SENTENCE))),
    };
    let mut late: Vec<(u32, &'static str)> = state
        .process_index
        .session_entries(&registration.session_id)
        .into_iter()
        .filter(|entry| !approved.contains(&entry.pid))
        .map(|entry| (entry.pid, "not_in_approved_plan"))
        .collect();
    let protected: HashSet<u32> = fresh.excluded.iter().map(|skip| skip.pid).collect();
    let (signal, now_protected): (Vec<PlanTarget>, Vec<PlanTarget>) = plan
        .targets
        .iter()
        .cloned()
        .partition(|target| !protected.contains(&target.pid));
    late.extend(
        now_protected
            .iter()
            .map(|target| (target.pid, "protected_at_execution")),
    );
    let unproven = fresh.unproven;
    let job = state
        .sessions
        .live_process_roots()
        .into_iter()
        .find(|proof| proof.id == registration.session_id)
        .map(|proof| proof.job);
    let termination = terminate_all(
        &signal,
        Duration::from_millis(u64::from(grace)),
        &|pid, planned| owned_target_check(job.as_deref(), pid, planned),
    )
    .map_err(|error| {
        (
            "platform_unavailable",
            tool_error(id, &format!("platform_unavailable: {error}")),
        )
    })?;
    Ok(Executed {
        termination,
        unproven,
        late,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A platform whose signal names a pid cannot bind it to the process the
    /// check confirmed: nothing is planned or signalled there.
    #[test]
    fn a_platform_that_cannot_bind_signals_refuses_before_planning() {
        assert_eq!(
            platform_refusal(false),
            Some("signals_cannot_bind_identity")
        );
        assert_eq!(platform_refusal(true), None);
    }

    /// The card names every target by pid and image: two processes with the
    /// same image are two entries, in pid order.
    #[test]
    fn targets_line_names_each_target_by_pid_and_image() {
        let target = |pid: u32, exe: Option<&str>| PlanTarget {
            pid,
            started_at_ticks: 0,
            exe: exe.map(str::to_string),
        };
        assert_eq!(
            targets_line(&[target(12, Some("/b/node")), target(9, Some("/a/node"))]),
            "9 node, 12 node"
        );
        assert_eq!(targets_line(&[target(7, None)]), "7 unknown");
    }

    /// A plan the card cannot list in full is refused, never shown in part.
    #[test]
    fn a_plan_too_large_to_list_exactly_is_refused() {
        assert!(exact_list_fits(LISTED_TARGET_LIMIT));
        assert!(!exact_list_fits(LISTED_TARGET_LIMIT + 1));
    }
}
