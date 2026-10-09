//! The cleanup plan against provider trees shaped the way the platform spawns
//! them: a shim at the daemon's direct child with the agent and its helpers
//! beneath it. A member is a target only when its parent is proven to be
//! outside the tree; any missing or unproven link protects it.

use std::collections::HashMap;

use super::fakes::{identity, proof, refresh, FakeProbe};
use super::*;
use crate::process_plan::{CHAIN_UNPROVEN, MEMBER_UNPROVEN};
use crate::process_tree::JobObject;

/// The same OS answers, but a parent's exit severs the link, as on Unix where
/// an orphan is re-parented to init.
struct ReparentingProbe<'a>(&'a FakeProbe);

impl ProcessProbe for ReparentingProbe<'_> {
    fn members(&self, job: &JobObject) -> Result<Vec<u32>, String> {
        self.0.members(job)
    }

    fn identity(&self, pid: u32) -> Option<ProcessIdentity> {
        self.0.identity(pid)
    }

    fn listening_ports(&self) -> Vec<(u16, u32)> {
        self.0.listening_ports()
    }

    fn proof_kind(&self) -> &'static str {
        self.0.proof_kind()
    }

    fn parent_links_survive_exit(&self) -> bool {
        false
    }
}

fn named(plan: &CleanupPlan) -> Vec<(u32, &'static str)> {
    plan.excluded
        .iter()
        .map(|skip| (skip.pid, skip.reason))
        .collect()
}

/// cmd.exe (daemon child) -> node.exe (the agent, which calls the tool) ->
/// node.exe (its helper) -> node.exe (a grandchild), plus an orphan whose
/// parent is gone: the tree is protected and the orphan is unproven, so
/// nothing is a target.
#[test]
fn a_windows_shim_tree_keeps_the_callers_whole_chain_out_of_the_plan() {
    let index = ProcessIndex::new();
    let daemon = std::process::id();
    let mut probe = FakeProbe {
        members: vec![100, 200, 300, 400, 500],
        identities: HashMap::from([
            (100, identity(1_000, daemon)),
            (200, identity(1_100, 100)),
            (300, identity(1_200, 200)),
            (400, identity(1_300, 300)),
            (500, identity(1_400, 77)),
        ]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-win", None)], &mut probe);

    let plan = index
        .cleanup_plan("session-win")
        .expect("a live session")
        .expect("a proven chain");
    assert!(
        plan.targets.is_empty(),
        "no member has a parent proven outside the tree"
    );
    assert_eq!(
        named(&plan),
        vec![
            (100, "agent_root"),
            (200, "agent_descendant"),
            (300, "agent_descendant"),
            (400, "agent_descendant"),
            (500, "lineage_unproven"),
        ]
    );
}

/// A member whose parent is alive outside the membership, and started before
/// it, is proven outside the provider tree: the one path to a target.
#[test]
fn a_member_under_a_live_outside_parent_is_a_target() {
    let index = ProcessIndex::new();
    let daemon = std::process::id();
    let mut probe = FakeProbe {
        members: vec![100, 200],
        identities: HashMap::from([
            (100, identity(1_000, daemon)),
            (200, identity(2_000, 300)),
            (300, identity(500, 7)),
        ]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-outside", None)], &mut probe);

    let plan = index
        .cleanup_plan("session-outside")
        .expect("a live session")
        .expect("a proven chain");
    let targets: Vec<u32> = plan.targets.iter().map(|target| target.pid).collect();
    assert_eq!(targets, vec![200]);
}

/// The outside parent was created after the child: the ppid was reused, so it
/// is not the parent and the member stays protected.
#[test]
fn a_parent_created_after_its_child_is_not_proof() {
    let index = ProcessIndex::new();
    let daemon = std::process::id();
    let mut probe = FakeProbe {
        members: vec![100, 200],
        identities: HashMap::from([
            (100, identity(1_000, daemon)),
            (200, identity(2_000, 300)),
            (300, identity(3_000, 7)),
        ]),
        ports: vec![],
    };
    refresh(
        &index,
        vec![proof("session-reused-parent", None)],
        &mut probe,
    );

    let plan = index
        .cleanup_plan("session-reused-parent")
        .expect("a live session")
        .expect("a proven chain");
    assert!(plan.targets.is_empty());
    assert_eq!(
        named(&plan),
        vec![(100, "agent_root"), (200, "lineage_unproven")]
    );
}

/// A member a parent-less orphan: the parent pid names nothing alive, so the
/// link cannot be proven and the member is protected, never an orphan target.
#[test]
fn an_orphan_whose_parent_is_gone_is_protected() {
    let index = ProcessIndex::new();
    let daemon = std::process::id();
    let mut probe = FakeProbe {
        members: vec![100, 200],
        identities: HashMap::from([(100, identity(1_000, daemon)), (200, identity(2_000, 77))]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-gone", None)], &mut probe);

    let plan = index
        .cleanup_plan("session-gone")
        .expect("a live session")
        .expect("a proven chain");
    assert!(plan.targets.is_empty());
    assert_eq!(
        named(&plan),
        vec![(100, "agent_root"), (200, "lineage_unproven")]
    );
}

/// The same live-parent member, read on a platform where parent links do not
/// survive the parent's exit: its ppid proves nothing, so it is protected.
#[test]
fn a_reparented_orphan_is_protected_where_links_do_not_survive() {
    let daemon = std::process::id();
    let mut probe = FakeProbe {
        members: vec![100, 200],
        identities: HashMap::from([
            (100, identity(1_000, daemon)),
            (200, identity(2_000, 1)),
            (1, identity(1, 0)),
        ]),
        ports: vec![],
    };

    let linked = ProcessIndex::new();
    refresh(&linked, vec![proof("session-linked", None)], &mut probe);
    let plan = linked
        .cleanup_plan("session-linked")
        .expect("a live session")
        .expect("a proven chain");
    assert_eq!(
        plan.targets.len(),
        1,
        "with links kept, init is a proven parent"
    );

    let reparented = ProcessIndex::new();
    reparented
        .refresh_with(
            vec![proof("session-reparented", None)],
            &mut ReparentingProbe(&probe),
        )
        .expect("fake probe works");
    let plan = reparented
        .cleanup_plan("session-reparented")
        .expect("a live session")
        .expect("a proven chain");
    assert!(plan.targets.is_empty());
    assert_eq!(
        named(&plan),
        vec![(100, "agent_root"), (200, "lineage_unproven")]
    );
}

/// A member whose identity cannot be read is unproven: a plan with one is
/// refused, because it could be the bridge between a target and the root.
#[test]
fn an_unproven_member_refuses_the_plan() {
    let index = ProcessIndex::new();
    let daemon = std::process::id();
    let mut probe = FakeProbe {
        members: vec![100, 200],
        identities: HashMap::from([(100, identity(1_000, daemon))]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-unproven", None)], &mut probe);

    assert!(matches!(
        index.cleanup_plan("session-unproven"),
        Some(Err(reason)) if reason == MEMBER_UNPROVEN
    ));
}

/// The intermediate wrapper's identity is unreadable while its child is
/// readable and names it as parent: the child is not bridged to a root by
/// any proof, so the whole plan is refused rather than the child signalled.
#[test]
fn an_unproven_intermediate_refuses_the_plan() {
    let index = ProcessIndex::new();
    let daemon = std::process::id();
    let mut probe = FakeProbe {
        members: vec![100, 150, 200],
        identities: HashMap::from([(100, identity(1_000, daemon)), (200, identity(2_000, 150))]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-bridge", None)], &mut probe);

    assert!(matches!(
        index.cleanup_plan("session-bridge"),
        Some(Err(reason)) if reason == MEMBER_UNPROVEN
    ));
}

/// A reused recorded root's pid, with no daemon parent: the chain is not
/// proven, so the plan is refused and nothing is guessed.
#[test]
fn a_reused_root_pid_refuses_the_plan() {
    let index = ProcessIndex::new();
    let daemon = std::process::id();
    let mut probe = FakeProbe {
        members: vec![100],
        identities: HashMap::from([(100, identity(1_000, daemon))]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-reuse", None)], &mut probe);

    let mut probe = FakeProbe {
        members: vec![100, 200],
        identities: HashMap::from([(100, identity(5_000, 7)), (200, identity(6_000, 100))]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-reuse", None)], &mut probe);

    assert!(matches!(
        index.cleanup_plan("session-reuse"),
        Some(Err(reason)) if reason == CHAIN_UNPROVEN
    ));
}

/// A session with members and no daemon child at all cannot name its
/// provider, so it is refused rather than cleaned blind.
#[test]
fn a_session_without_a_provider_root_is_refused() {
    let index = ProcessIndex::new();
    let mut probe = FakeProbe {
        members: vec![200],
        identities: HashMap::from([(200, identity(2_000, 77))]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-rootless", None)], &mut probe);

    assert!(matches!(
        index.cleanup_plan("session-rootless"),
        Some(Err(reason)) if reason == CHAIN_UNPROVEN
    ));
}

/// The same OS answers, but the job's member list cannot be read at all.
struct UnreadableJobProbe<'a>(&'a FakeProbe);

impl ProcessProbe for UnreadableJobProbe<'_> {
    fn members(&self, _job: &JobObject) -> Result<Vec<u32>, String> {
        Err("the job's member list could not be read".to_string())
    }

    fn identity(&self, pid: u32) -> Option<ProcessIdentity> {
        self.0.identity(pid)
    }

    fn listening_ports(&self) -> Vec<(u16, u32)> {
        self.0.listening_ports()
    }

    fn proof_kind(&self) -> &'static str {
        self.0.proof_kind()
    }
}

/// A job whose member list cannot be read is no proof of anything: the plan is
/// refused, not drawn from an empty membership that would look like a clean one.
#[test]
fn an_unreadable_job_refuses_the_plan() {
    let daemon = std::process::id();
    let probe = FakeProbe {
        members: vec![100, 200],
        identities: HashMap::from([(100, identity(1_000, daemon)), (200, identity(2_000, 100))]),
        ports: vec![],
    };
    let index = ProcessIndex::new();
    index
        .refresh_with(
            vec![proof("session-blind", None)],
            &mut UnreadableJobProbe(&probe),
        )
        .expect("the refresh itself works");

    assert!(matches!(
        index.cleanup_plan("session-blind"),
        Some(Err(reason)) if reason == MEMBER_UNPROVEN
    ));
}

/// The daemon never appears as a cleanup target, whatever its parent reads.
#[test]
fn the_daemon_itself_is_never_a_target() {
    let index = ProcessIndex::new();
    let daemon = std::process::id();
    let mut probe = FakeProbe {
        members: vec![100, daemon],
        identities: HashMap::from([(100, identity(1_000, daemon)), (daemon, identity(900, 7))]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-self", None)], &mut probe);

    let plan = index
        .cleanup_plan("session-self")
        .expect("a live session")
        .expect("a proven chain");
    assert!(plan.targets.iter().all(|target| target.pid != daemon));
    assert!(plan
        .excluded
        .iter()
        .any(|skip| skip.pid == daemon && skip.reason == "daemon"));
}
