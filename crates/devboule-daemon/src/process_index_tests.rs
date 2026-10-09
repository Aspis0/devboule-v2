//! The index's answers against scripted probes: ownership by port and pid,
//! per-query refresh of ports and the exe line, root recording, and the
//! cleanup plan that never includes the agent's chain.

use std::collections::HashMap;

use super::fakes::{identity, identity_with, proof, refresh, FakeProbe};
use super::*;

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
    assert_eq!(first[0].started_at_ticks, 1_000);

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
        second[0].started_at_ticks, 2_000,
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
    assert_eq!(
        matches[0].0.started_at_ticks, 1_000,
        "identity did not move"
    );
}

/// Every daemon child in a session's membership is a provider root: a second
/// one cannot be told apart from a respawned provider, so it is protected too.
#[test]
fn every_daemon_child_of_a_session_is_a_protected_root() {
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

    let plan = index
        .cleanup_plan("session-e")
        .expect("a live session")
        .expect("a proven chain");
    let named: Vec<(u32, &str)> = plan
        .excluded
        .iter()
        .map(|skip| (skip.pid, skip.reason))
        .collect();
    assert_eq!(named, vec![(100, "agent_root"), (999, "agent_root")]);
    assert!(plan.targets.is_empty());
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

    let plan = index
        .cleanup_plan("session-f")
        .expect("a live session")
        .expect("a proven chain");
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
fn cleanup_plan_refuses_an_unvouched_member_and_names_nothing_to_signal() {
    let index = ProcessIndex::new();
    let mut probe = FakeProbe {
        members: vec![100, 300],
        identities: HashMap::from([
            // The root: our own direct child.
            (100, identity(1_000, std::process::id())),
            // 300 is a member the OS will not vouch for.
        ]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-c", None)], &mut probe);

    assert!(matches!(
        index.cleanup_plan("session-c"),
        Some(Err(reason)) if reason == crate::process_plan::MEMBER_UNPROVEN
    ));
}

/// An executable the process spelled as a credential never reaches an entry,
/// so neither the tool answers nor the cleanup plan (and its audit row) carry
/// it.
#[test]
fn a_credential_shaped_executable_is_masked_in_entries_and_the_plan() {
    let index = ProcessIndex::new();
    let mut probe = FakeProbe {
        members: vec![100, 200],
        identities: HashMap::from([
            (
                100,
                identity_with(1_000, std::process::id(), "/usr/local/bin/agent"),
            ),
            (200, identity_with(2_000, 77, "API_KEY=secret")),
        ]),
        ports: Vec::new(),
    };
    refresh(&index, vec![proof("session-a", None)], &mut probe);

    let entries = index.session_entries("session-a");
    let spelled = entries
        .iter()
        .find(|entry| entry.pid == 200)
        .expect("entry");
    assert_eq!(spelled.exe.as_deref(), Some("API_KEY=[redacted]"));
    let plan = index
        .cleanup_plan("session-a")
        .expect("a live session")
        .expect("a proven chain");
    assert!(
        plan.targets
            .iter()
            .all(|target| target.exe.as_deref() != Some("API_KEY=secret")),
        "the plan carries the redacted string"
    );
}
