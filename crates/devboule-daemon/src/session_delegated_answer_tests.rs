//! The delegated answer's switch road, moved whole out of `session_tests.rs`
//! lines 49-457: the switch read at the answer rather than at the park, the
//! unknown and already-resolved refusals, the not-the-callers-child and
//! row-shape refusals, the caller-origin pair, and the no-cap, no-pause shape.
//! Every line below is byte-identical to its text there apart from this header;
//! `child_runtime_of` travels with the nine tests that use it, and the six
//! fixtures they lean on are promoted to `pub(super)` in the file they stay in.

use super::tests::{
    answer, insert_child, insert_live_agent, park_card, test_owner, tmp_delete_registry,
};
use super::*;

/// C1 + C2: the switch is read **at the answer**, never at the park. Off
/// refuses with the card still pending; off-after-park refuses the same
/// way; on accepts. A cached flag fails one of these three.
#[test]
fn the_delegated_answer_reads_the_switch_at_the_moment_it_lands() {
    let (dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("s5b-del-switch", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    insert_live_agent(&registry, &creator, owner.clone());
    let child_runtime = insert_child(&registry, &child, owner.clone(), &creator);

    // C1: the switch was never turned on — refused, card pending.
    park_card(&registry, &child_runtime, "card-1");
    let error = answer(
        &registry,
        &creator,
        "card-1",
        PermissionOutcome::AllowOnce,
        vec![],
    )
    .expect_err("delegation is off");
    assert!(error.contains("delegation is off"), "{error}");
    assert_eq!(
        child_runtime
            .permission_broker()
            .expect("broker")
            .pending_len(),
        1,
        "the card stays pending for the human"
    );

    // C2: on when the card parked, off before the answer lands — the
    // answer is still refused, because the read is now.
    store.set(true).expect("set on");
    park_card(&registry, &child_runtime, "card-2");
    store.set(false).expect("set off");
    let error = answer(
        &registry,
        &creator,
        "card-2",
        PermissionOutcome::AllowOnce,
        vec![],
    )
    .expect_err("the switch went off after the park");
    assert!(error.contains("delegation is off"), "{error}");
    assert_eq!(
        child_runtime
            .permission_broker()
            .expect("broker")
            .pending_len(),
        2,
        "both cards untouched"
    );

    // And on: the same answer goes through.
    store.set(true).expect("set on");
    answer(
        &registry,
        &creator,
        "card-1",
        PermissionOutcome::AllowOnce,
        vec![],
    )
    .expect("with the switch on, the answer lands");
    let _ = std::fs::remove_dir_all(&dir);
}

/// C3: an invented id and a replayed id are two sentences, and neither
/// touches anything.
#[test]
fn an_unknown_card_and_an_already_resolved_card_are_two_distinct_refusals() {
    let (dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("s5b-del-c3", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
    insert_live_agent(&registry, &creator, owner.clone());
    let child_runtime = insert_child(&registry, &child, owner.clone(), &creator);

    let error = answer(
        &registry,
        &creator,
        "no-such-card",
        PermissionOutcome::Deny,
        vec![],
    )
    .expect_err("no such card exists");
    assert!(error.contains("unknown permission card"), "{error}");

    park_card(&registry, &child_runtime, "card-1");
    answer(
        &registry,
        &creator,
        "card-1",
        PermissionOutcome::AllowOnce,
        vec![],
    )
    .expect("first answer lands");
    let error = answer(
        &registry,
        &creator,
        "card-1",
        PermissionOutcome::AllowOnce,
        vec![],
    )
    .expect_err("the card is gone");
    assert!(error.contains("already been resolved"), "{error}");
    assert_eq!(
        child_runtime
            .permission_broker()
            .expect("broker")
            .pending_len(),
        0,
        "the replay resolved nothing twice"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The card's runtime, looked up the way the answer chain finds it.
fn child_runtime_of(registry: &SessionRegistry, child: &str) -> Arc<SessionRuntime> {
    registry.runtime(child).expect("child runtime")
}

/// C4: a sibling's card, and the creator's own session's card, are
/// refused with the card pending. Loosening `created_by` to same-owner
/// turns the first refusal red.
#[test]
fn a_card_that_is_not_the_callers_childs_is_refused_and_stays_pending() {
    let (dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("s5b-del-c4", "proc-1");
    let creator_a = compose_session_id(&owner.session_token(), "cra").expect("id");
    let creator_b = compose_session_id(&owner.session_token(), "crb").expect("id");
    let child_of_b = compose_session_id(&owner.session_token(), "chb").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
    let creator_a_runtime = insert_live_agent(&registry, &creator_a, owner.clone());
    insert_child(&registry, &child_of_b, owner.clone(), &creator_b);

    // B's child has a card; A reaches for it.
    let child_b_runtime = child_runtime_of(&registry, &child_of_b);
    park_card(&registry, &child_b_runtime, "card-b1");
    let error = answer(
        &registry,
        &creator_a,
        "card-b1",
        PermissionOutcome::Deny,
        vec![],
    )
    .expect_err("not A's child");
    assert!(error.contains("not your child"), "{error}");
    assert_eq!(
        child_b_runtime
            .permission_broker()
            .expect("broker")
            .pending_len(),
        1,
        "the card stays pending for whoever may answer it"
    );

    // A's own card — the creation-consent card A's own session raised —
    // is A's session, not A's child.
    park_card(&registry, &creator_a_runtime, "card-self");
    let error = answer(
        &registry,
        &creator_a,
        "card-self",
        PermissionOutcome::Deny,
        vec![],
    )
    .expect_err("a session may not answer its own card");
    assert!(error.contains("not your child"), "{error}");
    assert_eq!(
        creator_a_runtime
            .permission_broker()
            .expect("broker")
            .pending_len(),
        1
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// C5: a creator whose session belongs to a paired device answers only
/// while that device holds `answer_permissions`.
#[test]
fn a_peer_creator_answers_only_under_answer_permissions() {
    let (dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("s5b-del-c5", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
    insert_live_agent(&registry, &creator, owner.clone());
    let child_runtime = insert_child(&registry, &child, owner.clone(), &creator);
    {
        let mut map = registry.inner.lock().expect("registry");
        let live = map
            .get_mut(&creator)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("creator entry");
        live.metadata.origin = SessionOrigin::peer("device-c5", PeerRole::Client);
    }

    park_card(&registry, &child_runtime, "card-1");
    let error = answer(
        &registry,
        &creator,
        "card-1",
        PermissionOutcome::AllowOnce,
        vec!["view".to_string()],
    )
    .expect_err("the device holds no answer_permissions");
    assert!(error.contains("answer_permissions"), "{error}");
    assert_eq!(
        child_runtime
            .permission_broker()
            .expect("broker")
            .pending_len(),
        1
    );
    answer(
        &registry,
        &creator,
        "card-1",
        PermissionOutcome::AllowOnce,
        vec!["view".to_string(), "answer_permissions".to_string()],
    )
    .expect("with the capability, the answer lands");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The P0 door's instance in the session layer refuses with the policy's own
/// sentence, rendered as the wire renders it — not the hand-written copy
/// that used to live here.
#[test]
fn a_peer_answer_refusal_is_the_policy_sentence() {
    let (dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("s5b-del-policy-sentence", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
    insert_live_agent(&registry, &creator, owner.clone());
    let child_runtime = insert_child(&registry, &child, owner.clone(), &creator);
    {
        let mut map = registry.inner.lock().expect("registry");
        let live = map
            .get_mut(&creator)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("creator entry");
        live.metadata.origin = SessionOrigin::peer("device-policy", PeerRole::Client);
    }
    park_card(&registry, &child_runtime, "card-1");
    let error = answer(
        &registry,
        &creator,
        "card-1",
        PermissionOutcome::AllowOnce,
        vec!["view".to_string()],
    )
    .expect_err("the device holds no answer_permissions");
    assert_eq!(
        error, "capability 'answer_permissions' was not negotiated; the card stays pending",
        "the policy's own sentence, plus what happens to the card: {error}"
    );
    assert_eq!(
        child_runtime
            .permission_broker()
            .expect("broker")
            .pending_len(),
        1
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Fail-safe at the session layer: a peer-shaped row that names no device
/// (or no role) is an unknown, and the unknown is refused — never treated
/// as the local person.
#[test]
fn a_peer_shaped_row_without_a_device_is_refused() {
    let (dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("s5b-del-undevice", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
    insert_live_agent(&registry, &creator, owner.clone());
    let child_runtime = insert_child(&registry, &child, owner.clone(), &creator);
    {
        let mut map = registry.inner.lock().expect("registry");
        let live = map
            .get_mut(&creator)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("creator entry");
        live.metadata.origin = SessionOrigin {
            kind: SessionOriginKind::Peer,
            device_id: None,
            role: None,
        };
    }
    park_card(&registry, &child_runtime, "card-1");
    let error = answer(
        &registry,
        &creator,
        "card-1",
        PermissionOutcome::AllowOnce,
        vec![
            "view".to_string(),
            "answer_permissions".to_string(),
            "send".to_string(),
            "create_sessions".to_string(),
        ],
    )
    .expect_err("a device nobody named holds nothing");
    assert!(error.contains("unknown"), "{error}");
    assert_eq!(
        child_runtime
            .permission_broker()
            .expect("broker")
            .pending_len(),
        1
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The door's reader: absent rows (and an unreadable lock) are `None`, live
/// local rows are `Some(local)`, and a stored `Unknown` reads back as itself.
/// (Peer rows read back as themselves; the C5 tests above pin that half.)
#[test]
fn caller_origin_is_none_without_a_row_and_local_for_a_local_row() {
    let (dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("s5b-origin-reader", "proc-1");
    assert_eq!(registry.caller_origin("s.nobody.1"), None);
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    insert_live_agent(&registry, &creator, owner.clone());
    assert_eq!(
        registry.caller_origin(&creator),
        Some(SessionOrigin::local())
    );
    // A stored `Unknown` is a fact, not an absence: it reads back as itself
    // so the door refuses it hard rather than retryably.
    {
        let mut map = registry.inner.lock().expect("registry");
        let live = map
            .get_mut(&creator)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("creator entry");
        live.metadata.origin = SessionOrigin::unknown();
    }
    assert_eq!(
        registry.caller_origin(&creator),
        Some(SessionOrigin::unknown())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Fail-safe: a poisoned lock reads as no row — refused, never local.
#[test]
fn caller_origin_is_none_when_the_lock_is_poisoned() {
    let (dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("s5b-origin-poison", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    insert_live_agent(&registry, &creator, owner.clone());
    let inner = Arc::clone(&registry.inner);
    let _ = std::thread::spawn(move || {
        let _guard = inner.lock().unwrap();
        panic!("poison the registry lock");
    })
    .join();
    assert_eq!(registry.caller_origin(&creator), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// C7: the brake is deleted — no cap, no pause. As many answers as cards
/// land, one after another; any rate limit under this count turns red.
#[test]
fn delegated_answers_have_no_cap_and_no_pause() {
    let (dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("s5b-del-c7", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
    insert_live_agent(&registry, &creator, owner.clone());
    let child_runtime = insert_child(&registry, &child, owner.clone(), &creator);
    for index in 0..8 {
        let card = format!("card-{index}");
        park_card(&registry, &child_runtime, &card);
        answer(&registry, &creator, &card, PermissionOutcome::Deny, vec![])
            .unwrap_or_else(|error| panic!("answer {index} must land: {error}"));
    }
    assert_eq!(
        child_runtime
            .permission_broker()
            .expect("broker")
            .pending_len(),
        0
    );
    let _ = std::fs::remove_dir_all(&dir);
}
