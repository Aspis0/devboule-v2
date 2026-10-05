//! The index's answers against scripted probes: ownership by port and pid,
//! identity replacement when a pid is reused, and the cleanup plan that
//! never includes the session's own root.

use std::collections::HashMap;
use std::sync::Arc;

use crate::process_tree::JobObject;

use super::*;

/// One probe's scripted reality: the members the proof admits, what the OS
/// says about each, and the port table.
struct FakeProbe {
    members: Vec<u32>,
    identities: HashMap<u32, ProcessIdentity>,
    ports: Vec<(u16, u32)>,
}

impl ProcessProbe for FakeProbe {
    fn members(&self, _job: &JobObject) -> Vec<u32> {
        self.members.clone()
    }

    fn identity(&self, pid: u32) -> Option<ProcessIdentity> {
        self.identities.get(&pid).cloned()
    }

    fn listening_ports(&self) -> Vec<(u16, u32)> {
        self.ports.clone()
    }

    fn proof_kind(&self) -> &'static str {
        "job_member"
    }
}

fn identity(started_at_ms: u64, ppid: u32) -> ProcessIdentity {
    ProcessIdentity {
        started_at_ms,
        ppid,
        exe: Some("/usr/local/bin/tool".to_string()),
        argv: vec!["/usr/local/bin/tool".to_string(), "--serve".to_string()],
    }
}

fn proof(id: &str, workspace: Option<&str>) -> SessionProof {
    SessionProof {
        id: id.to_string(),
        workspace_id: workspace.map(str::to_string),
        label: format!("agent {id}"),
        job: Arc::new(JobObject::new().expect("a proof handle")),
    }
}

#[test]
fn process_port_returns_only_owned_member() {
    let index = ProcessIndex::new();
    let probe = FakeProbe {
        members: vec![100],
        identities: HashMap::from([(100, identity(1_000, 7))]),
        ports: vec![(1420, 100), (8080, 4242)],
    };
    index.refresh_with(vec![proof("session-a", Some("ws-1"))], &probe);

    let matches = index.owner_matches(Some(1420), None);
    assert_eq!(matches.len(), 1, "the member owns its port");
    let (entry, session_id, agent, workspace) = &matches[0];
    assert_eq!(entry.pid, 100);
    assert_eq!(session_id, "session-a");
    assert_eq!(agent, "agent session-a");
    assert_eq!(workspace.as_deref(), Some("ws-1"));
    assert_eq!(entry.proof, "job_member");

    assert!(
        index.owner_matches(Some(8080), None).is_empty(),
        "a port owned by a non-member has no owner here"
    );
    assert_eq!(index.owner_matches(None, Some(100)).len(), 1);
    assert!(index.owner_matches(None, Some(4242)).is_empty());
    assert!(
        index.owner_matches(Some(1420), Some(100)).is_empty(),
        "asking both at once is the tool's refusal; the index answers nothing"
    );
}

#[test]
fn process_list_detects_pid_reuse() {
    let index = ProcessIndex::new();
    let probe = FakeProbe {
        members: vec![100],
        identities: HashMap::from([(100, identity(1_000, 9))]),
        ports: vec![],
    };
    index.refresh_with(vec![proof("session-b", None)], &probe);
    let first = index.session_entries("session-b");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].started_at_ms, 1_000);

    // The same pid comes back with a different creation time: the old entry
    // is the old process, and it must never answer as the new one.
    let probe = FakeProbe {
        members: vec![100],
        identities: HashMap::from([(100, identity(2_000, 9))]),
        ports: vec![(1420, 100)],
    };
    index.refresh_with(vec![proof("session-b", None)], &probe);
    let second = index.session_entries("session-b");
    assert_eq!(second.len(), 1);
    assert_eq!(
        second[0].started_at_ms, 2_000,
        "replaced by what the OS says is there now"
    );
    assert_eq!(index.owner_matches(Some(1420), None).len(), 1);

    // And once the pid leaves the proof entirely, nothing of it remains.
    let probe = FakeProbe {
        members: vec![],
        identities: HashMap::new(),
        ports: vec![],
    };
    index.refresh_with(vec![proof("session-b", None)], &probe);
    assert!(index.session_entries("session-b").is_empty());
    assert!(index.owner_matches(None, Some(100)).is_empty());
}

#[test]
fn cleanup_targets_exclude_the_sessions_own_agent_root() {
    let index = ProcessIndex::new();
    let probe = FakeProbe {
        members: vec![100, 200, 300],
        identities: HashMap::from([
            // The root: our own direct child.
            (100, identity(1_000, std::process::id())),
            (200, identity(2_000, 100)),
            // 300 is a member the OS will not vouch for.
        ]),
        ports: vec![],
    };
    index.refresh_with(vec![proof("session-c", None)], &probe);

    let plan = index.cleanup_plan("session-c").expect("a live session");
    assert_eq!(plan.targets, vec![200], "a member, never the agent root");
    assert_eq!(
        plan.unproven,
        vec![300],
        "an unvouched member is reported, not acted on"
    );
    assert!(
        !plan.targets.contains(&100) && !plan.unproven.contains(&100),
        "the agent root is in neither list"
    );
}
