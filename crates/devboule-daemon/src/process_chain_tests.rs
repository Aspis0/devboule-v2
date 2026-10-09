//! The cleanup plan against provider trees shaped the way the platform spawns
//! them: a shim at the daemon's direct child with the agent and its helpers
//! beneath it. The caller's whole chain must stay out of every plan.

use std::collections::HashMap;

use super::fakes::{identity, proof, refresh, FakeProbe};
use super::*;
use crate::process_plan::CHAIN_UNPROVEN;

/// cmd.exe (daemon child) -> node.exe (the agent, which calls the tool) ->
/// node.exe (its helper) -> node.exe (a grandchild), plus an orphan whose
/// parent is not a member: only the orphan is a target.
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
    let targets: Vec<u32> = plan.targets.iter().map(|target| target.pid).collect();
    assert_eq!(targets, vec![500], "only the unrooted orphan is a target");
    let protected: Vec<u32> = plan.excluded.iter().map(|skip| skip.pid).collect();
    assert_eq!(protected, vec![100, 200, 300, 400]);
}

/// The recorded root's pid was reused by a process with another creation
/// time and no daemon parent: the chain is not proven, so the plan is refused
/// and nothing is guessed.
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

/// A child created before its claimed parent cannot be that parent's child:
/// the pid was reused, so the link is not followed and the child is a target.
#[test]
fn a_child_older_than_its_claimed_parent_is_not_a_descendant() {
    let index = ProcessIndex::new();
    let daemon = std::process::id();
    let mut probe = FakeProbe {
        members: vec![100, 200],
        identities: HashMap::from([(100, identity(1_000, daemon)), (200, identity(900, 100))]),
        ports: vec![],
    };
    refresh(&index, vec![proof("session-age", None)], &mut probe);

    let plan = index
        .cleanup_plan("session-age")
        .expect("a live session")
        .expect("a proven chain");
    let targets: Vec<u32> = plan.targets.iter().map(|target| target.pid).collect();
    assert_eq!(targets, vec![200]);
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
