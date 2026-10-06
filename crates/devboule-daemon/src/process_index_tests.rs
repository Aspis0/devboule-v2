//! The index's answers against scripted probes: ownership by port and pid,
//! per-query refresh of ports and the exe line, root recording, and the
//! cleanup plan that never includes the agent's chain.

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
    identity_with(started_at_ms, ppid, "/usr/local/bin/tool")
}

fn identity_with(started_at_ms: u64, ppid: u32, exe: &str) -> ProcessIdentity {
    ProcessIdentity {
        started_at_ms,
        ppid,
        exe: Some(exe.to_string()),
        argv: vec![exe.to_string(), "--serve".to_string()],
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

fn refresh(index: &ProcessIndex, roots: Vec<SessionProof>, probe: &mut FakeProbe) {
    index.refresh_with(roots, probe).expect("fake probe works");
}

#[test]
fn process_port_returns_only_owned_member() {
    let index = ProcessIndex::new();
    let mut probe = FakeProbe {
        members: vec![100],
        identities: HashMap::from([(100, identity(1_000, 7))]),
        ports: vec![(1420, 100), (8080, 4242)],
    };
    refresh(&index, vec![proof("session-a", Some("ws-1"))], &mut probe);

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
    let mut probe = FakeProbe {
        members: vec![100],
        identities: HashMap::from([(100, identity(1_000, 9))]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-b", None)], &mut probe);
    let first = index.session_entries("session-b");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].started_at_ms, 1_000);

    // The same pid comes back with a different creation time: the old entry
    // is the old process, and it must never answer as the new one.
    let mut probe = FakeProbe {
        members: vec![100],
        identities: HashMap::from([(100, identity(2_000, 9))]),
        ports: vec![(1420, 100)],
    };
    refresh(&index, vec![proof("session-b", None)], &mut probe);
    let second = index.session_entries("session-b");
    assert_eq!(second.len(), 1);
    assert_eq!(
        second[0].started_at_ms, 2_000,
        "replaced by what the OS says is there now"
    );
    assert_eq!(index.owner_matches(Some(1420), None).len(), 1);

    // And once the pid leaves the proof entirely, nothing of it remains.
    let mut probe = FakeProbe {
        members: vec![],
        identities: HashMap::new(),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-b", None)], &mut probe);
    assert!(index.session_entries("session-b").is_empty());
    assert!(index.owner_matches(None, Some(100)).is_empty());
}

/// A member that binds its port after first sight is found by that port on
/// the next query: ports and the exe line are rebuilt every refresh, never
/// frozen at the first read.
#[test]
fn ports_and_exe_refresh_on_every_query() {
    let index = ProcessIndex::new();
    let mut probe = FakeProbe {
        members: vec![100],
        identities: HashMap::from([(100, identity_with(1_000, 7, "/bin/before"))]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-d", None)], &mut probe);
    assert!(
        index.owner_matches(Some(1420), None).is_empty(),
        "no port yet at first sight"
    );

    let mut probe = FakeProbe {
        members: vec![100],
        identities: HashMap::from([(100, identity_with(1_000, 7, "/bin/after"))]),
        ports: vec![(1420, 100)],
    };
    refresh(&index, vec![proof("session-d", None)], &mut probe);

    let matches = index.owner_matches(Some(1420), None);
    assert_eq!(matches.len(), 1, "the port it bound since is its port now");
    assert_eq!(
        matches[0].0.exe.as_deref(),
        Some("/bin/after"),
        "the exe line is fresh too"
    );
    assert_eq!(matches[0].0.started_at_ms, 1_000, "identity did not move");
}

/// The agent exclusion is the recorded (pid, creation time) pair: a member
/// merely re-parented to the daemon is a stray, not the agent, and stays in
/// the plan.
#[test]
fn a_stray_reparented_to_the_daemon_is_a_target_not_the_agent() {
    let index = ProcessIndex::new();
    let daemon = std::process::id();
    let mut probe = FakeProbe {
        members: vec![100, 999],
        identities: HashMap::from([
            (100, identity(1_000, daemon)),
            (999, identity(3_000, daemon)),
        ]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-e", None)], &mut probe);

    let plan = index.cleanup_plan("session-e").expect("a live session");
    // Both are daemon children on sight; the lowest pid is the session's own
    // spawn, so the pair recorded is (100, 1_000).
    assert_eq!(plan.excluded.len(), 1);
    assert_eq!(plan.excluded[0].pid, 100);
    assert_eq!(plan.excluded[0].reason, "agent_root");
    let pids: Vec<u32> = plan.targets.iter().map(|target| target.pid).collect();
    assert_eq!(pids, vec![999], "the stray is cleaned, not protected");
}

/// The recorded pair outlives the shape the entries take later: if the root
/// is re-parented at a later refresh (its `is_agent` flips off), the
/// recorded pid+creation time still excludes exactly that process.
#[test]
fn the_agent_root_is_excluded_by_its_recorded_creation_time() {
    let index = ProcessIndex::new();
    let daemon = std::process::id();
    let mut probe = FakeProbe {
        members: vec![100],
        identities: HashMap::from([(100, identity(1_000, daemon))]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-f", None)], &mut probe);

    // The same pid, still ours, but its parent now reads as something else.
    let mut probe = FakeProbe {
        members: vec![100],
        identities: HashMap::from([(100, identity(1_000, 4242))]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-f", None)], &mut probe);

    let plan = index.cleanup_plan("session-f").expect("a live session");
    assert_eq!(
        plan.excluded.len(),
        1,
        "the recorded pair still excludes it"
    );
    assert_eq!(plan.excluded[0].pid, 100);
    assert!(
        plan.targets.is_empty(),
        "the agent itself is never a target"
    );
}

#[test]
fn cleanup_plan_excludes_the_agent_root_and_unvouched_members() {
    let index = ProcessIndex::new();
    let mut probe = FakeProbe {
        members: vec![100, 200, 300],
        identities: HashMap::from([
            // The root: our own direct child.
            (100, identity(1_000, std::process::id())),
            (200, identity(2_000, 100)),
            // 300 is a member the OS will not vouch for.
        ]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-c", None)], &mut probe);

    let plan = index.cleanup_plan("session-c").expect("a live session");
    let pids: Vec<u32> = plan.targets.iter().map(|target| target.pid).collect();
    assert_eq!(pids, vec![200], "a member, never the agent root");
    assert_eq!(
        plan.excluded
            .iter()
            .map(|entry| entry.pid)
            .collect::<Vec<u32>>(),
        vec![100],
        "and the root's exclusion says why"
    );
    assert_eq!(plan.excluded[0].reason, "agent_root");
    assert_eq!(
        plan.unproven,
        vec![300],
        "an unvouched member is reported, not acted on"
    );
}
