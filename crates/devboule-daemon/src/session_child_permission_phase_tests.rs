//! Direct unit tests of the phases in `session_child_permission.rs`: each
//! phase is called on its own, so a phase that is wrong fails here even
//! before the through-the-road characterisation in
//! `session_child_permission_tests.rs` notices.

use super::session_child_permission::child_answer_caps_refusal;
use super::session_child_permission_tests::{
    answer, park_card, raise_permission_attention, registry_with_journal, test_owner,
};
use super::tests::{insert_child, insert_live_agent};
use super::*;

/// A registered row that is not live: the transcript arm of the registry,
/// the kind of row only a bare `map.get` may answer through.
fn insert_transcript_caller(registry: &SessionRegistry, id: &str, owner: OwnerId) {
    let metadata = Session {
        id: id.to_string(),
        workspace_id: None,
        cwd: None,
        kind: SessionKind::Terminal,
        title: "Terminal".to_string(),
        state: SessionState::Ended {
            generation: 1,
            code: Some(0),
            integrity: TranscriptIntegrity::Complete,
        },
        elapsed_ms: Some(0),
        provider: None,
        peer_session_id: None,
        created_at_ms: 1,
        origin: SessionOrigin::local(),
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: devboule_protocol::UnattendedState::No,
        labels: Default::default(),
        resumable: false,
    };
    let runtime = SessionRuntime::from_replay(
        id.to_string(),
        registry.journal.clone(),
        crate::journal::Replay {
            generation: 1,
            last_seq: 0,
            integrity: TranscriptIntegrity::Complete,
            event_seqs: Vec::new(),
            events: Vec::new(),
        },
    );
    registry.inner.lock().expect("registry").insert(
        id.to_string(),
        RegistryEntry::Transcript(Box::new(TranscriptSession {
            metadata,
            owner,
            runtime,
        })),
    );
}

/// Mutant: P1's bare `map.get` unified with the scan's live-only probe — a
/// caller whose row is a transcript would be refused "not registered"
/// instead of read; any row carries the caller's identity.
#[test]
fn caller_identity_reads_any_row_and_refuses_only_an_absent_one() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("c3-ph-p1");
    let live = compose_session_id(&owner.session_token(), "live").expect("id");
    insert_live_agent(&registry, &live, owner.clone());
    let (user, origin) = registry
        .caller_identity(&live)
        .expect("a live row reads back");
    assert_eq!(user, owner.user);
    assert_eq!(origin, SessionOrigin::local());

    let transcript = compose_session_id(&owner.session_token(), "trans").expect("id");
    insert_transcript_caller(&registry, &transcript, owner.clone());
    let (user, origin) = registry
        .caller_identity(&transcript)
        .expect("a transcript row reads back too");
    assert_eq!(user, owner.user);
    assert_eq!(origin, SessionOrigin::local());

    let ghost = compose_session_id(&owner.session_token(), "ghost").expect("id");
    let error = registry
        .caller_identity(&ghost)
        .expect_err("no row at all is the refusal");
    assert_eq!(
        error, "the calling session is not registered on this daemon",
        "{error}"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mutants: the owner scope dropped from the scan (another owner's card
/// would be found), the child counter dropped (an ambiguous id would be
/// answered against the first child the map yields), or the child-over-
/// non-child preference dropped (killed in 6 of 7 map orders; the seventh
/// order — the child yielded first — is the one where the preference is a
/// no-op and no test can tell, which the crowd of non-child holders below
/// makes as rare as the map allows).
#[test]
fn find_card_holder_scopes_the_owner_and_prefers_a_child() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("c3-ph-p2");
    let other = test_owner("c3-ph-p2-other");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let bystander = compose_session_id(&owner.session_token(), "bye").expect("id");
    let stranger = compose_session_id(&other.session_token(), "str").expect("id");
    let stranger_child = compose_session_id(&other.session_token(), "sch").expect("id");
    insert_live_agent(&registry, &creator, owner.clone());
    let child_runtime = insert_child(&registry, &child, owner.clone(), &creator);
    let bystander_runtime = insert_live_agent(&registry, &bystander, owner.clone());
    let stranger_child_runtime = insert_child(&registry, &stranger_child, other, &stranger);

    let (found, holders) = registry
        .find_card_holder(&owner.user, &creator, "card-mine")
        .expect("the lock holds");
    assert!(found.is_none() && holders == 0, "no card anywhere yet");

    // The child's card, with the caller's own non-child cards parked on the
    // same id first: whichever order the map yields, the child must win —
    // and with several non-child holders only the preference (not the map's
    // order) can pick the child, except in the one order where the child is
    // yielded first.
    park_card(&bystander_runtime, "card-mine");
    park_card(&child_runtime, "card-mine");
    let crowd = ["c1", "c2", "c3", "c4", "c5"];
    let mut crowd_runtimes = Vec::new();
    for name in crowd {
        let id = compose_session_id(&owner.session_token(), name).expect("id");
        crowd_runtimes.push(insert_live_agent(&registry, &id, owner.clone()));
    }
    for runtime in &crowd_runtimes {
        park_card(runtime, "card-mine");
    }
    let (found, holders) = registry
        .find_card_holder(&owner.user, &creator, "card-mine")
        .expect("the lock holds");
    assert_eq!(holders, 1, "one live child holds the id");
    let child_broker = child_runtime.permission_broker().expect("broker");
    assert!(
        found.is_some_and(|found| std::sync::Arc::ptr_eq(&found, &child_broker)),
        "the child holder is preferred over the non-child crowd"
    );

    // Two children holding the same id: the count is what the ambiguity
    // guard refuses on.
    let twin = compose_session_id(&owner.session_token(), "twn").expect("id");
    let twin_runtime = insert_child(&registry, &twin, owner.clone(), &creator);
    park_card(&twin_runtime, "card-mine");
    let (_found, holders) = registry
        .find_card_holder(&owner.user, &creator, "card-mine")
        .expect("the lock holds");
    assert_eq!(holders, 2, "both live children counted");

    // Another owner's card is invisible to the scan.
    park_card(&stranger_child_runtime, "card-theirs");
    let (found, holders) = registry
        .find_card_holder(&owner.user, &creator, "card-theirs")
        .expect("the lock holds");
    assert!(
        found.is_none() && holders == 0,
        "the scan never leaves the owner's rows"
    );

    // Only a non-child holds an id: still found — the child check in the
    // chain is what refuses it, with its own sentence.
    park_card(&bystander_runtime, "card-bystander");
    let (found, holders) = registry
        .find_card_holder(&owner.user, &creator, "card-bystander")
        .expect("the lock holds");
    let bystander_broker = bystander_runtime.permission_broker().expect("broker");
    assert_eq!(holders, 0, "a non-child is no child holder");
    assert!(
        found.is_some_and(|found| std::sync::Arc::ptr_eq(&found, &bystander_broker)),
        "the non-child's card stays findable for the chain to refuse"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mutants: the live-view guard dropped (a dead session would pass as the
/// card's session), the child predicate dropped or flipped (any visible
/// session would pass), or the two refusal sentences swapped.
#[test]
fn child_answer_target_names_each_miss_and_resolves_a_live_child() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("c3-ph-p4");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let sibling = compose_session_id(&owner.session_token(), "sib").expect("id");
    insert_live_agent(&registry, &creator, owner.clone());
    insert_child(&registry, &child, owner.clone(), &creator);
    insert_live_agent(&registry, &sibling, owner.clone());

    let target = registry
        .child_answer_target(&child, &creator)
        .expect("a live child resolves");
    assert_eq!(target, child, "the remembered id is the card's session");

    let error = registry
        .child_answer_target(&sibling, &creator)
        .expect_err("a same-owner non-child is not the caller's child");
    assert_eq!(
        error,
        format!(
            "permission card {sibling} belongs to a session that is not your child; \
             it stays pending for whoever may answer it"
        ),
        "{error}"
    );
    let error = registry
        .child_answer_target(&creator, &creator)
        .expect_err("the caller itself is not its own child");
    assert!(error.contains("not your child"), "{error}");

    let error = registry
        .child_answer_target("s.nowhere.1", &creator)
        .expect_err("an invented id matches no view");
    assert_eq!(
        error, "permission card s.nowhere.1 is not pending on one of your live sessions",
        "{error}"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mutants: an unreadable journal read as `true` (every unknown card would
/// become "already been resolved"), or an absent journal read as `true`.
#[test]
fn permission_already_recorded_reads_false_when_the_ledger_cannot_answer() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("c3-ph-p3");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
    insert_live_agent(&registry, &creator, owner.clone());
    let child_runtime = insert_child(&registry, &child, owner.clone(), &creator);
    assert!(
        !registry.permission_already_recorded("card-never"),
        "an unknown id is not in the ledger"
    );
    park_card(&child_runtime, "card-ledger");
    answer(
        &registry,
        &creator,
        "card-ledger",
        PermissionOutcome::Deny,
        vec![],
    )
    .expect("the answer lands");
    assert!(
        registry.permission_already_recorded("card-ledger"),
        "the resolved card is a row in the ledger"
    );
    journal.shutdown();
    assert!(
        !registry.permission_already_recorded("card-ledger"),
        "a journal that cannot answer reads false, never true"
    );

    let bare_dir = dir.join("bare");
    std::fs::create_dir(&bare_dir).expect("tmp dir");
    let bare = SessionRegistry::new(RuntimePaths::from_dir(&bare_dir), None);
    assert!(
        !bare.permission_already_recorded("card-ledger"),
        "no journal: the read is false, which renders the sentence inert"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mutants: the peer shape answered for a local creator (the gate would
/// consult a device the caller does not have), or the caps fetched before
/// the shape is known (a device would be asked for caps it is never judged
/// against).
#[test]
fn child_answer_caps_refusal_skips_locals_and_judges_peers_by_the_wire_gate() {
    let local_calls = Arc::new(AtomicU64::new(0));
    let counted_local = {
        let local_calls = Arc::clone(&local_calls);
        move |device: &str| {
            local_calls.fetch_add(1, Ordering::Acquire);
            vec![format!("caps-of-{device}")]
        }
    };
    assert!(
        child_answer_caps_refusal(&SessionOrigin::local(), &counted_local).is_ok(),
        "a local creator is the person at this machine: no gate, no device ask"
    );
    assert_eq!(
        local_calls.load(Ordering::Acquire),
        0,
        "the device is never asked for a local creator"
    );

    let peer_calls = Arc::new(AtomicU64::new(0));
    let counted_peer = {
        let peer_calls = Arc::clone(&peer_calls);
        move |_device: &str| {
            peer_calls.fetch_add(1, Ordering::Acquire);
            vec!["view".to_string(), "answer_permissions".to_string()]
        }
    };
    let peer = SessionOrigin::peer("device-ph", PeerRole::Client);
    assert!(
        child_answer_caps_refusal(&peer, &counted_peer).is_ok(),
        "a peer holding answer_permissions may answer"
    );
    assert_eq!(
        peer_calls.load(Ordering::Acquire),
        1,
        "the gate judged the device's own caps, fetched once"
    );

    let error = child_answer_caps_refusal(&peer, &|_: &str| vec!["view".to_string()])
        .expect_err("no answer_permissions");
    assert_eq!(
        error, "capability 'answer_permissions' was not negotiated; the card stays pending",
        "the policy's own sentence, plus what happens to the card: {error}"
    );

    let shapeless = SessionOrigin {
        kind: SessionOriginKind::Peer,
        device_id: None,
        role: None,
    };
    let shapeless_calls = Arc::new(AtomicU64::new(0));
    let counted_shapeless = {
        let shapeless_calls = Arc::clone(&shapeless_calls);
        move |device: &str| {
            shapeless_calls.fetch_add(1, Ordering::Acquire);
            vec![format!("caps-of-{device}")]
        }
    };
    let error = child_answer_caps_refusal(&shapeless, &counted_shapeless)
        .expect_err("a peer-shaped row without a device is an unknown");
    assert_eq!(
        error, "the calling session's origin is unknown; the card stays pending",
        "the unknown never renders as the benign one: {error}"
    );
    assert_eq!(
        shapeless_calls.load(Ordering::Acquire),
        0,
        "a device nobody named is never asked"
    );
}

/// Mutants: the `clear_attention` gate dropped (a session waiting for
/// nothing would push a transition), the clear dropped (the attention
/// would outlive its card), or the transition push dropped.
#[test]
fn clear_child_attention_after_answer_clears_only_raised_attention() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("c3-ph-p6");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let runtime = insert_child(&registry, &child, owner.clone(), &creator);
    // The sink is live before the first call, so the empty case is a real
    // observation: the tail ran and pushed nothing.
    let sink_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let fired = Arc::clone(&sink_log);
    registry.set_transition_sink(Arc::new(move |pushed| {
        fired.lock().expect("sink log").push(pushed.user.clone());
    }));
    registry.clear_child_attention_after_answer(&child);
    assert!(
        runtime.attention().is_none(),
        "nothing was raised: nothing to clear"
    );
    assert!(
        sink_log.lock().expect("sink log").is_empty(),
        "nothing was raised: the tail pushes nothing"
    );

    raise_permission_attention(&runtime);
    assert!(runtime.attention().is_some(), "the raise worked");
    // A fresh sink: the raise pushed into the old one; only the tail's own
    // push may appear here.
    let raised_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let counted = Arc::clone(&raised_log);
    registry.set_transition_sink(Arc::new(move |pushed| {
        counted.lock().expect("sink log").push(pushed.user.clone());
    }));
    registry.clear_child_attention_after_answer(&child);
    assert!(
        runtime.attention().is_none(),
        "the attention the card raised is cleared"
    );
    assert_eq!(
        *raised_log.lock().expect("sink log"),
        vec![owner.user.clone()],
        "the client is owed the transition"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
