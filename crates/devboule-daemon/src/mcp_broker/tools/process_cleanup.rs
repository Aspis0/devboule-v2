//! The cleanup tool: the mode-following card, the exact approved plan, and
//! the two-phase termination report.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::processes::{refreshed, reply, strict_arguments};
use crate::mcp_broker::caller::{audit_mcp_tool, McpCaller};
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::tools::first_use::{
    ensure_write_approved, Approval, GateMark, PROCESS_CLEANUP_GROUP,
};
use crate::mcp_broker::{McpBroker, RegisteredSession};
use crate::process_plan::{CleanupPlan, PlanTarget};
use crate::server::ServerState;

/// The default and the ceiling for `graceMs`: the graceful phase is a wait
/// the caller names, and neither it nor the forced phase may be unbounded.
const DEFAULT_GRACE_MS: u32 = 2_000;
const MAX_GRACE_MS: u32 = 30_000;

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

/// The exe names the card shows for the plan: unique basenames, at most
/// eight, with a marker when the plan holds more.
fn executables_line(targets: &[PlanTarget]) -> String {
    const SHOWN: usize = 8;
    let mut names: Vec<String> = Vec::new();
    let mut overflowed = false;
    for target in targets {
        let name = target
            .exe
            .as_deref()
            .map(|exe| {
                std::path::Path::new(exe)
                    .file_name()
                    .map(|base| base.to_string_lossy().into_owned())
                    .unwrap_or_else(|| exe.to_string())
            })
            .unwrap_or_else(|| "unknown".to_string());
        if names.contains(&name) {
            continue;
        }
        if names.len() == SHOWN {
            overflowed = true;
            break;
        }
        names.push(name);
    }
    if overflowed {
        names.push("…".to_string());
    }
    names.join(", ")
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
    refreshed(state, &id)?;
    let plan = state
        .process_index
        .cleanup_plan(&registration.session_id)
        .unwrap_or(CleanupPlan {
            targets: Vec::new(),
            excluded: Vec::new(),
            unproven: Vec::new(),
        });
    let label = state
        .process_index
        .session_label(&registration.session_id)
        .unwrap_or_else(|| registration.session_id.clone());
    let mut skipped: Vec<(u32, &'static str)> = plan
        .excluded
        .iter()
        .map(|entry| (entry.pid, entry.reason))
        .collect();
    if plan.targets.is_empty() {
        audit_mcp_tool(
            state,
            &caller,
            crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL,
            &registration.session_id,
            "ok",
        );
        skipped.sort_by_key(|(pid, _)| *pid);
        return cleanup_reply(id, Vec::new(), Vec::new(), plan.unproven, skipped);
    }
    let count = plan.targets.len();
    let count_text = count.to_string();
    let executables = executables_line(&plan.targets);
    let facts: [(&str, &str); 3] = [
        ("session", label.as_str()),
        ("processes", count_text.as_str()),
        ("executables", executables.as_str()),
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

    // The approved plan is what runs. One more membership read exists only
    // to name what appeared after approval — those are left alone.
    refreshed(state, &id)?;
    let current = state
        .process_index
        .session_entries(&registration.session_id);
    let approved: HashSet<u32> = plan
        .targets
        .iter()
        .map(|target| target.pid)
        .chain(plan.excluded.iter().map(|entry| entry.pid))
        .collect();
    for entry in &current {
        if !approved.contains(&entry.pid) {
            skipped.push((entry.pid, "not_in_approved_plan"));
        }
    }
    let unproven = state
        .process_index
        .cleanup_plan(&registration.session_id)
        .map(|fresh| fresh.unproven)
        .unwrap_or_default();
    let termination = match crate::process_terminate::terminate_all(
        &plan.targets,
        Duration::from_millis(u64::from(grace)),
        &crate::process_terminate::os_target_check,
    ) {
        Ok(termination) => termination,
        Err(error) => {
            audit_mcp_tool(
                state,
                &caller,
                crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL,
                &registration.session_id,
                "failed: platform_unavailable",
            );
            return Err(tool_error(&id, &format!("platform_unavailable: {error}")));
        }
    };
    skipped.extend(termination.skipped);
    skipped.sort_by_key(|(pid, _)| *pid);
    // An automatic mode raises no card, so the row is the only place the
    // approval is recorded: what was stopped, in which session's name.
    let outcome = match approval {
        Approval::Mode => format!(
            "ok; approved by automatic mode: {count} planned ({executables}), terminated {:?}",
            termination.terminated
        ),
        Approval::Person => "ok".to_string(),
    };
    audit_mcp_tool(
        state,
        &caller,
        crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL,
        &registration.session_id,
        &outcome,
    );
    cleanup_reply(
        id,
        termination.terminated,
        termination.still_running,
        unproven,
        skipped,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The card's executable line: unique basenames, at most eight, and a
    /// marker when the plan holds more.
    #[test]
    fn executables_line_shows_unique_basenames_capped_with_a_marker() {
        let target = |exe: Option<&str>| PlanTarget {
            pid: 1,
            started_at_ms: 0,
            exe: exe.map(str::to_string),
        };
        assert_eq!(
            executables_line(&[target(Some("/a/node")), target(Some("/b/node"))]),
            "node"
        );
        assert_eq!(
            executables_line(&[target(None), target(Some("/x/sh.exe"))]),
            "unknown, sh.exe"
        );
        let many: Vec<PlanTarget> = (0..12).map(|_| target(Some("/bin/tool"))).collect();
        assert_eq!(executables_line(&many), "tool", "the same exe counts once");
        let distinct: Vec<PlanTarget> = (0..10)
            .map(|index| target(Some(&format!("/bin/p{index}"))))
            .collect();
        assert_eq!(
            executables_line(&distinct),
            "p0, p1, p2, p3, p4, p5, p6, p7, …"
        );
    }
}
