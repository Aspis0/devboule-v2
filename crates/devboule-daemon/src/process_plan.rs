//! The cleanup plan: what one `devboule_cleanup_processes` call may act on,
//! and what it deliberately leaves — with a reason for every leave.

use std::collections::HashMap;

use crate::process_index::ProcessEntry;

/// One process the card will act on: identity carried to the signal.
#[derive(Clone, Debug)]
pub(crate) struct PlanTarget {
    pub(crate) pid: u32,
    pub(crate) started_at_ms: u64,
    pub(crate) exe: Option<String>,
}

/// One member the plan leaves alone, and why — nothing is dropped silently.
#[derive(Clone, Debug)]
pub(crate) struct SkippedPlan {
    pub(crate) pid: u32,
    pub(crate) reason: &'static str,
}

/// What a cleanup is allowed to act on, and what it deliberately leaves.
pub(crate) struct CleanupPlan {
    pub(crate) targets: Vec<PlanTarget>,
    pub(crate) excluded: Vec<SkippedPlan>,
    pub(crate) unproven: Vec<u32>,
}

/// The agent's chain inside the membership: the recorded root — the pid and
/// creation time the index saw the daemon's direct child at — plus any
/// member whose parent chain reaches that root without leaving the
/// membership. The pair matters: a member merely re-parented to the daemon
/// is a stray, not the agent, and stays a target. Each exclusion carries its
/// reason; an unexplained member is never excluded.
pub(crate) fn excluded_agent_chain(
    entries: &HashMap<u32, ProcessEntry>,
    agent_root: Option<(u32, u64)>,
) -> Vec<SkippedPlan> {
    let Some((root_pid, root_started_at)) = agent_root else {
        return Vec::new();
    };
    let recorded_root_is_a_member = entries
        .get(&root_pid)
        .is_some_and(|entry| entry.started_at_ms == root_started_at);
    if !recorded_root_is_a_member {
        return Vec::new();
    }
    let mut excluded: HashMap<u32, &'static str> = HashMap::new();
    excluded.insert(root_pid, "agent_root");
    // Walk up from the root: whatever member sits above it is an ancestor of
    // the agent inside the group. The walk is bounded — a membership is
    // finite, and a parent chain must not be trusted to terminate.
    let mut current = root_pid;
    for _ in 0..64 {
        let Some(entry) = entries.get(&current) else {
            break;
        };
        let parent = entry.ppid;
        if parent == current {
            break;
        }
        let Some(parent_entry) = entries.get(&parent) else {
            break;
        };
        excluded.insert(parent_entry.pid, "agent_ancestor");
        current = parent_entry.pid;
    }
    let mut skipped: Vec<SkippedPlan> = excluded
        .into_iter()
        .map(|(pid, reason)| SkippedPlan { pid, reason })
        .collect();
    skipped.sort_by_key(|skip| skip.pid);
    skipped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pid: u32, started_at_ms: u64, ppid: u32, is_agent: bool) -> ProcessEntry {
        ProcessEntry {
            pid,
            started_at_ms,
            exe: None,
            argv: Vec::new(),
            ports: Vec::new(),
            proof: "job_member",
            ppid,
            is_agent,
        }
    }

    /// The recorded root and every member above it are excluded, each with
    /// its own reason; a descendant and a sibling daemon-child are not part
    /// of the chain the pair names.
    #[test]
    fn the_agent_chain_names_the_root_and_its_ancestors() {
        let mut entries = HashMap::new();
        entries.insert(100, entry(100, 1_000, 200, true));
        entries.insert(200, entry(200, 900, 7, false));
        entries.insert(300, entry(300, 1_500, 100, false));
        entries.insert(999, entry(999, 2_000, 7, true));

        let excluded = excluded_agent_chain(&entries, Some((100, 1_000)));
        let named: Vec<(&u32, &&str)> = excluded
            .iter()
            .map(|skip| (&skip.pid, &skip.reason))
            .collect();
        assert_eq!(
            named,
            vec![(&100, &"agent_root"), (&200, &"agent_ancestor")]
        );
    }

    /// The pair is the identity: a different creation time under the same
    /// pid is not the agent, and without a recorded root nothing is excluded.
    #[test]
    fn a_different_creation_time_is_not_the_agent() {
        let entries = HashMap::from([(100, entry(100, 1_000, 7, true))]);
        assert!(excluded_agent_chain(&entries, Some((100, 1_001))).is_empty());
        assert!(excluded_agent_chain(&entries, None).is_empty());
    }
}
