//! Characterisation tests for the delegated answer road
//! (`answer_child_permission`), each naming the mutant it must catch. The
//! nine tests in `session_tests.rs` and `mcp_broker_tests.rs` keep their
//! per-check coverage; this file covers what they do not — the missing
//! caller's row, the ambiguity guard's place **before** the switch, the
//! owner scope of the card scan, and the attention tail (P6), which the
//! publish path can raise for real.

use super::tests::{insert_child, insert_live_agent};
use super::*;

pub(super) fn registry_with_journal() -> (std::path::PathBuf, SessionRegistry, Arc<Journal>) {
    let dir = crate::test_dirs::test_temp_dir("devboule-child-perm");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    let registry = SessionRegistry::new(RuntimePaths::from_dir(&dir), Some(Arc::clone(&journal)));
    (dir, registry, journal)
}

pub(super) fn test_owner(user: &str) -> OwnerId {
    OwnerId::new(user, "child-permission-client").expect("owner")
}

pub(super) fn park_card(runtime: &Arc<SessionRuntime>, card_id: &str) {
    let broker = runtime.permission_broker().expect("broker");
    broker
        .register(1, permission_broker::permission(card_id), runtime)
        .expect("the card parks");
}

pub(super) fn park_chooser_card(runtime: &Arc<SessionRuntime>, card_id: &str) {
    let broker = runtime.permission_broker().expect("broker");
    broker
        .register(
            1,
            permission_broker::permission_with_kinds(
                card_id,
                &[("once", "allow_once"), ("once-again", "allow_once")],
            ),
            runtime,
        )
        .expect("the chooser parks");
}

pub(super) fn answer(
    registry: &SessionRegistry,
    creator: &str,
    card_id: &str,
    outcome: PermissionOutcome,
    caps: Vec<String>,
) -> Result<(), String> {
    registry.answer_child_permission(creator, card_id, outcome, &|_device| caps.clone())
}

/// Raise attention the way the publish path does — the only raiser — so the
/// answer's attention tail has something real to clear.
pub(super) fn raise_permission_attention(runtime: &Arc<SessionRuntime>) {
    runtime.publish_agent_event(
        SessionEvent::PermissionRequest {
            tool_call_id: "tool-c3-attention".to_string(),
            title: "Run attention test".to_string(),
            description: None,
            command: None,
            args: None,
            cwd: None,
            env: None,
            options: Vec::new(),
            is_chooser: None,
            kind: None,
            questions: None,
            origin: SessionOrigin::local(),
            create_agent: None,
        },
        None,
    );
}

/// Mutant: P1's absent-row refusal skipped — the caller's OWN row must be
/// the one read; a first-row fallback answers with some other owner's
/// identity and reaches the switch check with a stranger's scope, which is
/// a different refusal.
#[test]
fn an_unregistered_caller_is_refused_by_name() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("c3-p1-ghost");
    let bystander = test_owner("c3-p1-bystander");
    let bystander_session = compose_session_id(&bystander.session_token(), "bye").expect("id");
    insert_live_agent(&registry, &bystander_session, bystander);
    let ghost = compose_session_id(&owner.session_token(), "ghost").expect("id");
    let error = answer(&registry, &ghost, "card-1", PermissionOutcome::Deny, vec![])
        .expect_err("no row for the caller");
    assert_eq!(
        error, "the calling session is not registered on this daemon",
        "P1's own refusal, not a card-shaped sentence: {error}"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mutant: the ambiguity guard moved below the broker's switch check (or
/// into it) — with the switch off, an ambiguous id must still be refused
/// ambiguous, not told the delegation is off.
#[test]
fn an_ambiguous_card_is_ambiguous_even_with_the_switch_off() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("c3-p3-order");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child_a = compose_session_id(&owner.session_token(), "cha").expect("id");
    let child_b = compose_session_id(&owner.session_token(), "chb").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    insert_live_agent(&registry, &creator, owner.clone());
    let runtime_a = insert_child(&registry, &child_a, owner.clone(), &creator);
    let runtime_b = insert_child(&registry, &child_b, owner.clone(), &creator);
    park_card(&runtime_a, "card-dup");
    park_card(&runtime_b, "card-dup");

    let error = answer(
        &registry,
        &creator,
        "card-dup",
        PermissionOutcome::Deny,
        vec![],
    )
    .expect_err("two children hold the same card id");
    assert!(
        error.contains("more than one of your live children"),
        "the ambiguity guard runs before the switch is ever read: {error}"
    );
    for (name, runtime) in [("a", &runtime_a), ("b", &runtime_b)] {
        assert_eq!(
            runtime.permission_broker().expect("broker").pending_len(),
            1,
            "child {name}'s card stays pending for the human"
        );
    }
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mutant: the scan's owner scope dropped — a card on another owner's child
/// would be found and then refused at the child check as "not your child",
/// telling the caller the card exists; the scan must not find it at all.
#[test]
fn a_card_on_another_owners_session_is_unknown_not_not_your_child() {
    let (dir, registry, journal) = registry_with_journal();
    let owner_a = test_owner("c3-p2-owner-a");
    let owner_b = test_owner("c3-p2-owner-b");
    let creator_a = compose_session_id(&owner_a.session_token(), "cr1").expect("id");
    let creator_b = compose_session_id(&owner_b.session_token(), "cr2").expect("id");
    let child_b = compose_session_id(&owner_b.session_token(), "chb").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
    insert_live_agent(&registry, &creator_a, owner_a);
    insert_live_agent(&registry, &creator_b, owner_b.clone());
    let child_b_runtime = insert_child(&registry, &child_b, owner_b, &creator_b);
    park_card(&child_b_runtime, "card-other");

    let error = answer(
        &registry,
        &creator_a,
        "card-other",
        PermissionOutcome::Deny,
        vec![],
    )
    .expect_err("the card belongs to another owner's session");
    assert_eq!(
        error, "unknown permission card card-other",
        "the scan never saw the other owner's card: {error}"
    );
    assert_eq!(
        child_b_runtime
            .permission_broker()
            .expect("broker")
            .pending_len(),
        1,
        "the other owner's card is untouched"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mutants: the attention tail dropped, its `clear_attention` dropped, or
/// its transition push dropped — the attention a parked card raised is the
/// tail's to clear, and the client is owed the transition.
#[test]
fn the_attention_a_parked_card_raised_clears_when_the_delegated_answer_lands() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("c3-p6-clear");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
    insert_live_agent(&registry, &creator, owner.clone());
    let child_runtime = insert_child(&registry, &child, owner.clone(), &creator);
    raise_permission_attention(&child_runtime);
    assert!(
        child_runtime.attention().is_some(),
        "the raise worked, or the tail has nothing to clear"
    );
    park_card(&child_runtime, "card-p6");
    // Installed after the park, so only the answer's own pushes are
    // counted: the park's surfacing to the creator raises attention of its
    // own, which is not the tail's doing.
    let sink_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let fired = Arc::clone(&sink_log);
    registry.set_transition_sink(Arc::new(move |pushed, _snapshots| {
        fired.lock().expect("sink log").push(pushed.user.clone());
    }));
    answer(
        &registry,
        &creator,
        "card-p6",
        PermissionOutcome::AllowOnce,
        vec![],
    )
    .expect("the answer lands");
    assert!(
        child_runtime.attention().is_none(),
        "the tail cleared the attention the card raised"
    );
    assert_eq!(
        // Answering a delegated card moves two facts on one row: the attention
        // the card stood under clears, and the turn status stops being `blocked`.
        // Since the roster began carrying that status (`SessionStateSnapshot::
        // activity`), the tail is owed one transition per fact rather than one
        // per moment — the same owner, pushed for each thing that changed.
        sink_log.lock().expect("sink log").len(),
        2,
        "the tail pushed for the owner: once for the cleared raise, once for the \
         status the card's leaving changed"
    );
    assert!(
        sink_log
            .lock()
            .expect("sink log")
            .iter()
            .all(|pushed| pushed == &owner.user),
        "and for no other owner"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mutant: the tail consuming the remembered child before the broker
/// returns — the child check runs before the capability check, so a refused
/// answer still leaves the cell filled; only an `Ok` may clear the
/// attention the card was waiting under.
#[test]
fn a_refused_answer_leaves_the_childs_attention_up() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("c3-p6-refused");
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
        live.metadata.origin = SessionOrigin::peer("device-c3", PeerRole::Client);
    }
    raise_permission_attention(&child_runtime);
    park_card(&child_runtime, "card-refused");
    // Same discipline: the sink sees only what the answer itself does.
    let sink_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let fired = Arc::clone(&sink_log);
    registry.set_transition_sink(Arc::new(move |pushed, _snapshots| {
        fired.lock().expect("sink log").push(pushed.user.clone());
    }));
    let error = answer(
        &registry,
        &creator,
        "card-refused",
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
        1,
        "the card stays pending for the human"
    );
    assert!(
        child_runtime.attention().is_some(),
        "a refused answer must not clear the attention the card raised"
    );
    assert!(
        sink_log.lock().expect("sink log").is_empty(),
        "a refused answer pushes no transition"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mutant: the refusal bound to the pending card's session id instead of
/// the card id the caller passed — the C3 regression the audit caught. The
/// broker hands check 4 `pending.session_id`; the sentence must name the
/// tool call id the chain looked the card up by, which is the only card id
/// the caller has ever seen.
#[test]
fn a_not_your_child_refusal_names_the_card_id_the_caller_passed() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("c3-pin-c4");
    let creator_a = compose_session_id(&owner.session_token(), "cra").expect("id");
    let creator_b = compose_session_id(&owner.session_token(), "crb").expect("id");
    let child_of_b = compose_session_id(&owner.session_token(), "chb").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
    insert_live_agent(&registry, &creator_a, owner.clone());
    let child_b_runtime = insert_child(&registry, &child_of_b, owner.clone(), &creator_b);
    park_card(&child_b_runtime, "card-b1");

    let error = answer(
        &registry,
        &creator_a,
        "card-b1",
        PermissionOutcome::Deny,
        vec![],
    )
    .expect_err("the card belongs to a stranger's child");
    assert_eq!(
        error,
        "permission card card-b1 belongs to a session that is not your child; \
         it stays pending for whoever may answer it",
        "the caller's own card id in the sentence: {error}"
    );
    assert_eq!(
        child_b_runtime
            .permission_broker()
            .expect("broker")
            .pending_len(),
        1,
        "the card stays pending for whoever may answer it"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The card's session is not a peer-visible view when check 4 runs, while
/// the scan still found the broker through the entry's runtime. Reachable
/// through the road: a card parked on a not-yet-visible (configuring) child
/// answers here, and so does a child that ends in the window between the
/// scan and the check — the two take separate `inner` holds on purpose.
/// Mutant: the sentence bound to the pending card's session id instead of
/// the caller's card id.
#[test]
fn a_card_whose_session_is_not_live_names_the_callers_card_id() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("c3-pin-gone");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let gone = compose_session_id(&owner.session_token(), "gone").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
    insert_live_agent(&registry, &creator, owner.clone());
    let gone_runtime = insert_live_agent(&registry, &gone, owner.clone());
    park_card(&gone_runtime, "card-gone");
    {
        let mut map = registry.inner.lock().expect("registry");
        let entry = map.remove(&gone).expect("the holder's entry");
        match entry {
            RegistryEntry::Live(session) => {
                map.insert(gone.clone(), RegistryEntry::Configuring(session));
            }
            _ => panic!("the entry that parked the card was live"),
        }
    }

    let error = answer(
        &registry,
        &creator,
        "card-gone",
        PermissionOutcome::Deny,
        vec![],
    )
    .expect_err("the card's session is not a live view");
    assert_eq!(
        error, "permission card card-gone is not pending on one of your live sessions",
        "the caller's own card id in the sentence: {error}"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Review A2a P1 — the delegated door's own first-pick: a chooser passes
/// `select_option` because the FIRST `allow_once` matches, so a creator's
/// answer would grant an option nobody chose. The MCP tool carries no
/// option id and the envelope the creator saw lists no options, so no
/// answer this door can give is explicit: the card is refused here and
/// stays pending with the person, where the chooser rule says it goes.
#[test]
fn a_creator_cannot_answer_a_chooser_and_the_card_stays_pending() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("c3-chooser");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("creator id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("child id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
    insert_live_agent(&registry, &creator, owner.clone());
    let runtime = insert_child(&registry, &child, owner.clone(), &creator);
    park_chooser_card(&runtime, "card-chooser");

    let expected = "permission card card-chooser is a chooser; only a person can choose between its options, so it stays pending";
    let allow_error = answer(
        &registry,
        &creator,
        "card-chooser",
        PermissionOutcome::AllowOnce,
        vec![],
    )
    .expect_err("a chooser has no answer this tool can give");
    assert_eq!(allow_error, expected, "the creator hears why, exactly");
    let deny_error = answer(
        &registry,
        &creator,
        "card-chooser",
        PermissionOutcome::Deny,
        vec![],
    )
    .expect_err("nor can this door refuse a chooser");
    assert_eq!(deny_error, expected, "neither outcome answers it");
    assert_eq!(
        runtime.permission_broker().expect("broker").pending_len(),
        1,
        "the request is still the human's to answer"
    );
    store.set(false).expect("set off");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
