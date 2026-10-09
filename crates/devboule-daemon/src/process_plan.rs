//! The cleanup plan: what one `devboule_cleanup_processes` call may act on,
//! and what it deliberately leaves — with a reason for every leave.

use std::collections::{HashMap, HashSet};

use crate::process_index::ProcessEntry;

/// One process the card will act on: identity carried to the signal.
#[derive(Clone, Debug)]
pub(crate) struct PlanTarget {
    pub(crate) pid: u32,
    pub(crate) started_at_ticks: u64,
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

/// Refusal when a session's provider tree cannot be proved: a cleanup then
/// signals nothing rather than guess which members are the caller's own.
pub(crate) const CHAIN_UNPROVEN: &str = "caller_chain_unproven";

/// Refusal when a session member's identity cannot be read: it could be the
/// link between a target and the provider root, so nothing is signalled.
pub(crate) const MEMBER_UNPROVEN: &str = "member_unproven";

/// Bounds the parent walk: the membership is finite, but parent links come
/// from the OS and are not trusted to terminate.
const MAX_CHAIN_DEPTH: usize = 64;

/// The provider tree of one session, each member named with why it is kept
/// out of the plan. Roots are the daemon's direct children in the session
/// (`is_agent`) plus the recorded root while the same process still holds its
/// pid and creation time. Ancestors and descendants of a root inside the
/// membership are protected too. A parent link counts only when the parent
/// was created no later than its child, so a reused pid is never a parent.
/// A session with members and no proven root is refused.
pub(crate) fn provider_tree(
    entries: &HashMap<u32, ProcessEntry>,
    recorded_root: Option<(u32, u64)>,
) -> Result<Vec<SkippedPlan>, &'static str> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let mut roots: Vec<u32> = entries
        .values()
        .filter(|entry| entry.is_agent)
        .map(|entry| entry.pid)
        .collect();
    if let Some((pid, started_at)) = recorded_root {
        let still_the_same_process = entries
            .get(&pid)
            .is_some_and(|entry| entry.started_at_ticks == started_at);
        if still_the_same_process && !roots.contains(&pid) {
            roots.push(pid);
        }
    }
    if roots.is_empty() {
        return Err(CHAIN_UNPROVEN);
    }

    let mut tree: HashMap<u32, &'static str> = HashMap::new();
    for root in &roots {
        tree.insert(*root, "agent_root");
    }
    for root in &roots {
        let mut current = *root;
        for _ in 0..MAX_CHAIN_DEPTH {
            let Some(parent) = entries
                .get(&current)
                .and_then(|child| parent_of(entries, child))
            else {
                break;
            };
            tree.entry(parent.pid).or_insert("agent_ancestor");
            current = parent.pid;
        }
    }

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for entry in entries.values() {
        if let Some(parent) = parent_of(entries, entry) {
            children.entry(parent.pid).or_default().push(entry.pid);
        }
    }
    let mut frontier = roots;
    let mut visited: HashSet<u32> = HashSet::new();
    while let Some(pid) = frontier.pop() {
        if !visited.insert(pid) {
            continue;
        }
        for child in children.get(&pid).into_iter().flatten() {
            tree.entry(*child).or_insert("agent_descendant");
            frontier.push(*child);
        }
    }

    let mut named: Vec<SkippedPlan> = tree
        .into_iter()
        .map(|(pid, reason)| SkippedPlan { pid, reason })
        .collect();
    named.sort_by_key(|skip| skip.pid);
    Ok(named)
}

/// The member that is this entry's parent, when the link can be real: the
/// parent is a member, is not the entry itself, and was created strictly
/// before it. Equal creation times are not proof, so they are not a link.
fn parent_of<'a>(
    entries: &'a HashMap<u32, ProcessEntry>,
    child: &ProcessEntry,
) -> Option<&'a ProcessEntry> {
    let parent = entries.get(&child.ppid)?;
    (parent.pid != child.pid && parent.started_at_ticks < child.started_at_ticks).then_some(parent)
}

/// Whether a member is proven outside the provider tree: its parent was alive
/// outside the membership, and created before it. Anything less — a missing or
/// reused parent, or a platform that re-parents orphans — leaves it protected.
pub(crate) fn outside_the_tree(entry: &ProcessEntry) -> bool {
    entry
        .outside_parent_started_at_ticks
        .is_some_and(|parent_started_at| parent_started_at < entry.started_at_ticks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pid: u32, started_at_ticks: u64, ppid: u32, is_agent: bool) -> ProcessEntry {
        ProcessEntry {
            pid,
            started_at_ticks,
            exe: None,
            argv: Vec::new(),
            ports: Vec::new(),
            proof: "job_member",
            ppid,
            is_agent,
            outside_parent_started_at_ticks: None,
        }
    }

    /// The recorded root, its ancestors inside the membership, and every
    /// descendant are named, each with its own reason; an unrelated member
    /// is not named.
    #[test]
    fn the_provider_tree_names_roots_ancestors_and_descendants() {
        let mut entries = HashMap::new();
        entries.insert(100, entry(100, 1_000, 200, false));
        entries.insert(200, entry(200, 900, 7, false));
        entries.insert(300, entry(300, 1_500, 100, false));
        entries.insert(999, entry(999, 2_000, 7, false));

        let tree = provider_tree(&entries, Some((100, 1_000))).expect("a proven root");
        let named: Vec<(&u32, &&str)> = tree.iter().map(|skip| (&skip.pid, &skip.reason)).collect();
        assert_eq!(
            named,
            vec![
                (&100, &"agent_root"),
                (&200, &"agent_ancestor"),
                (&300, &"agent_descendant"),
            ]
        );
    }

    /// The pair is the identity: a different creation time under the same
    /// pid is not the root, and with no proven root the chain is refused.
    #[test]
    fn a_reused_recorded_pid_leaves_the_chain_unproven() {
        let entries = HashMap::from([(100, entry(100, 1_000, 7, false))]);
        assert_eq!(
            provider_tree(&entries, Some((100, 1_001))).err(),
            Some(CHAIN_UNPROVEN)
        );
        assert_eq!(provider_tree(&entries, None).err(), Some(CHAIN_UNPROVEN));
    }

    /// A session with nothing in its membership has nothing to protect.
    #[test]
    fn an_empty_membership_needs_no_root() {
        assert!(provider_tree(&HashMap::new(), None)
            .expect("nothing to prove")
            .is_empty());
    }

    /// A child created before its claimed parent cannot be that parent's
    /// child: the pid was reused, so the link is not followed.
    #[test]
    fn a_parent_created_after_its_child_is_not_linked() {
        let entries = HashMap::from([
            (100, entry(100, 1_000, 7, true)),
            (200, entry(200, 500, 100, false)),
        ]);
        let tree = provider_tree(&entries, None).expect("a daemon-child root");
        let named: Vec<u32> = tree.iter().map(|skip| skip.pid).collect();
        assert_eq!(named, vec![100]);
    }
}
