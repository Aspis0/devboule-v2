//! Direct unit tests of the phases in `session_child_profile.rs`: each
//! phase is called on its own, so a phase that is wrong fails here even
//! before the through-the-road characterisation in
//! `session_child_profile_tests.rs` notices.

use super::session_child_profile::manifest_arrived;
use super::session_child_profile::model_ask_needed;
use super::session_child_profile_tests::{
    facts, journal_row_of, live_view_of, registry_with_journal, test_owner,
};
use super::tests::{insert_live_agent, insert_move_child};
use super::*;

/// Mutant: the predicate flipped — a manifest-less runtime would count as
/// arrived and the "cannot say yet" refusal would never fire.
#[test]
fn manifest_arrived_is_false_until_the_runtime_holds_a_manifest() {
    let runtime = SessionRuntime::with_journal("s.cp-manifest.1".to_string(), None);
    assert!(
        !manifest_arrived(&runtime),
        "no manifest delivered yet: the daemon cannot say"
    );
    runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: None,
        current_model_id: Some("model-a".to_string()),
        models: Vec::new(),
        modes: None,
    });
    assert!(manifest_arrived(&runtime), "the manifest arrived");
}

/// Mutant: the disjunction narrowed — a child on a different model, with no
/// current model at all, or with a thinking option to deliver, must each
/// still be asked; only "same model, nothing to deliver" asks nothing.
#[test]
fn model_ask_needed_asks_when_the_model_differs_or_thinking_is_set() {
    let manifest = SessionEvent::SessionManifest {
        provider_id: None,
        current_model_id: Some("model-a".to_string()),
        models: Vec::new(),
        modes: None,
    };
    assert!(
        model_ask_needed(Some(&manifest), &facts("bypass", "model-b", "p-1")),
        "a different model is an ask"
    );
    assert!(
        model_ask_needed(None, &facts("bypass", "model-a", "p-1")),
        "no current model known: the ask happens"
    );
    let thinking = ChildProfileFacts {
        thinking_option_id: Some("effort-high".to_string()),
        ..facts("bypass", "model-a", "p-1")
    };
    assert!(
        model_ask_needed(Some(&manifest), &thinking),
        "the same model with a thinking option to deliver is still an ask"
    );
    assert!(
        !model_ask_needed(Some(&manifest), &facts("bypass", "model-a", "p-1")),
        "already on the model, nothing to deliver: no ask"
    );
}

/// Mutant: the display-name arm dropped from the scan, or the id arm —
/// both ways of addressing the child must resolve, and every miss must
/// name its own reason.
#[test]
fn resolve_own_live_child_resolves_both_names_and_each_miss_names_itself() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("s5b-mv-seam-resolve");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let bystander = compose_session_id(&owner.session_token(), "bys").expect("id");
    insert_live_agent(&registry, &creator, owner.clone());
    insert_live_agent(&registry, &bystander, owner.clone());
    let (_runtime, _mode_calls, _model_calls, _order) = insert_move_child(
        &registry,
        &journal,
        &child,
        owner.clone(),
        &creator,
        "Worker",
        &["bypass"],
        Some("model-a"),
        true,
        false,
        false,
    );
    let (by_display, runtime, child_owner) = registry
        .resolve_own_live_child(&creator, "Worker")
        .expect("the display name resolves");
    assert_eq!(by_display.id, child);
    assert_eq!(runtime.session_id, child);
    assert_eq!(child_owner.user, owner.user);
    let (by_id, _, _) = registry
        .resolve_own_live_child(&creator, &child)
        .expect("the id resolves");
    assert_eq!(by_id.id, child);

    let error = registry
        .resolve_own_live_child(&creator, &bystander)
        .err()
        .expect("a same-owner non-child is told what it is");
    assert!(error.contains("not your child"), "{error}");
    let error = registry
        .resolve_own_live_child(&creator, &creator)
        .err()
        .expect("the caller is not its own child");
    assert!(error.contains("not its own child"), "{error}");
    let error = registry
        .resolve_own_live_child(&creator, "Nobody")
        .err()
        .expect("an invented name matches nobody");
    assert!(error.contains("none of your live children"), "{error}");
    let ghost = compose_session_id(&owner.session_token(), "ghost").expect("id");
    let error = registry
        .resolve_own_live_child(&ghost, "Worker")
        .err()
        .expect("a caller with no row is refused");
    assert_eq!(
        error,
        "the calling session is not registered on this daemon"
    );

    let twin = compose_session_id(&owner.session_token(), "twin").expect("id");
    insert_move_child(
        &registry,
        &journal,
        &twin,
        owner.clone(),
        &creator,
        "Worker",
        &["bypass"],
        Some("model-a"),
        true,
        false,
        false,
    );
    let error = registry
        .resolve_own_live_child(&creator, "Worker")
        .err()
        .expect("two children share the name");
    assert!(
        error.contains("more than one of your live children"),
        "{error}"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mutant: the partial arm recorded a profile id, or skipped the record —
/// the row must carry the ratchet only, and the sentence must say exactly
/// what stands.
#[test]
fn record_partial_move_records_the_ratchet_only_and_reports_what_stands() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("s5b-mv-seam-partial");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    insert_live_agent(&registry, &creator, owner.clone());
    let (_runtime, _mode_calls, _model_calls, _order) = insert_move_child(
        &registry,
        &journal,
        &child,
        owner.clone(),
        &creator,
        "Worker",
        &["bypass"],
        Some("model-a"),
        true,
        false,
        false,
    );
    let (child_session, _child_runtime, _child_owner) = registry
        .resolve_own_live_child(&creator, "Worker")
        .expect("the child resolves");
    let error = registry
        .record_partial_move(
            &child_session,
            &facts("bypass", "model-b", "p-1"),
            "the provider refused the model",
        )
        .expect_err("the partial arm is a refusal");
    assert!(
        error.contains("mode was switched to 'bypass'")
            && error.contains("no profile change is recorded"),
        "the answer reports exactly the partial state: {error}"
    );
    let record = journal_row_of(&journal, &child);
    assert_eq!(record.profile_id, None, "the ratchet only: no profile id");
    assert_eq!(
        record.unattended_state,
        devboule_protocol::UnattendedState::Yes,
        "the mode landed, so the ratchet fires"
    );
    let (profile_id, unattended) = live_view_of(&registry, &child);
    assert_eq!(profile_id, None);
    assert_eq!(unattended, devboule_protocol::UnattendedState::Yes);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
