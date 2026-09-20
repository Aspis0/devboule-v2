//! Characterisation tests for the child-profile move road
//! (`set_agent_child_profile`), each naming the mutant it must catch. The
//! eight tests in `session_tests.rs` keep the per-refusal coverage; this
//! file covers what they do not — the missing caller's row, the push a
//! landed move owes the transition sink, the roster's delegation column,
//! the addressing fallback to the title, and the live half of the marker
//! ratchet against lowering. The extracted phases are called directly in
//! `session_child_profile_phase_tests.rs`.

use super::tests::{insert_live_agent, insert_move_child};
use super::*;

pub(super) fn registry_with_journal() -> (std::path::PathBuf, SessionRegistry, Arc<Journal>) {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let process_id = std::process::id();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devboule-child-profile-{process_id}-{stamp}-{counter}"
    ));
    std::fs::create_dir(&dir).expect("tmp dir");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    let registry = SessionRegistry::new(RuntimePaths::from_dir(&dir), Some(Arc::clone(&journal)));
    (dir, registry, journal)
}

pub(super) fn test_owner(user: &str) -> OwnerId {
    OwnerId::new(user, "child-profile-client").expect("owner")
}

pub(super) fn facts(mode_id: &str, model: &str, profile_id: &str) -> ChildProfileFacts {
    ChildProfileFacts {
        profile_id: profile_id.to_string(),
        mode_id: mode_id.to_string(),
        model: model.to_string(),
        thinking_option_id: None,
    }
}

fn solo_resolve(name: &str) -> Result<ChildProfileFacts, String> {
    if name == "Solo" {
        Ok(facts("bypass", "model-b", "p-1"))
    } else {
        Err(format!(
            "unknown profile; call devboule_list_profiles ({name})"
        ))
    }
}

pub(super) fn journal_row_of(journal: &Arc<Journal>, id: &str) -> SessionRecord {
    journal
        .list()
        .expect("journal rows")
        .into_iter()
        .find(|record| record.id == id)
        .expect("the child's row")
}

pub(super) fn live_view_of(
    registry: &SessionRegistry,
    id: &str,
) -> (Option<String>, devboule_protocol::UnattendedState) {
    let live = registry
        .inner
        .lock()
        .expect("registry")
        .get(id)
        .and_then(RegistryEntry::as_peer_visible)
        .expect("live entry")
        .metadata
        .clone();
    (live.profile_id, live.unattended)
}

/// Mutant: the caller-row lookup swapped for any row of the map — a ghost
/// caller would inherit a stranger's owner and the scan would answer
/// "none of your live children" instead of naming the missing row.
#[test]
fn a_move_from_an_unregistered_caller_names_the_missing_row() {
    let (dir, registry, _journal) = registry_with_journal();
    let ghost = test_owner("s5b-mv-p1-ghost");
    let stranger_owner = test_owner("s5b-mv-p1-stranger");
    let stranger = compose_session_id(&stranger_owner.session_token(), "str").expect("id");
    insert_live_agent(&registry, &stranger, stranger_owner);
    let ghost_session = compose_session_id(&ghost.session_token(), "ghost").expect("id");
    let error = registry
        .set_agent_child_profile(&ghost_session, "Worker", "Solo", &solo_resolve)
        .expect_err("the caller's row is gone");
    assert_eq!(
        error, "the calling session is not registered on this daemon",
        "the refusal names the row, not the scan's miss: {error}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mutants: the landed move's push dropped (the sink stays silent), or the
/// live-metadata write dropped (the roster serves the pre-move facts). Not
/// claimed: dropping the explicit roster-cache invalidation — the journal
/// invalidation just above it chains to the same clear, so no test can
/// tell the two lines apart.
#[test]
fn a_landed_move_pushes_the_fresh_row_to_the_transition_sink() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("s5b-mv-push");
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
    // The app is open and holds this roster already: after the move, the
    // client's next read must not serve the pre-move row.
    let _ = registry.state_snapshots(&owner);
    let sink_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let fired = Arc::clone(&sink_log);
    registry.set_transition_sink(Arc::new(move |pushed| {
        fired.lock().expect("sink log").push(pushed.user.clone());
    }));
    registry
        .set_agent_child_profile(&creator, "Worker", "Solo", &solo_resolve)
        .expect("the move lands");
    assert_eq!(
        *sink_log.lock().expect("sink log"),
        vec![owner.user.clone()],
        "the landed move pushes exactly one transition for the owner"
    );
    let row = registry
        .state_snapshots(&owner)
        .into_iter()
        .find(|row| row.id == child)
        .expect("the pushed row");
    assert_eq!(
        row.profile_id.as_deref(),
        Some("p-1"),
        "the roster the client reads next carries the recorded profile"
    );
    assert_eq!(row.unattended, devboule_protocol::UnattendedState::Yes);
    assert_eq!(
        registry.full_roster_build_count(),
        2,
        "the move invalidated the cached roster, so the read rebuilds \
         instead of serving the pre-move row"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mutant: the live marker raise dropped (or lowered to an assign) — the
/// roster's delegation column reads the live metadata, so a switch that is
/// on must flip from `active` to `unattended` when the move lands.
#[test]
fn a_landed_move_marks_the_rosters_delegation_column_unattended() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("s5b-mv-delegation");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
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
    let before = registry
        .state_snapshots(&owner)
        .into_iter()
        .find(|row| row.id == child)
        .expect("the child's row before the move");
    assert_eq!(
        before.delegation.expect("a child carries the column").state,
        devboule_protocol::DelegationRunState::Active,
        "the switch is on and the child has never run unattended"
    );
    registry
        .set_agent_child_profile(&creator, "Worker", "Solo", &solo_resolve)
        .expect("the move lands");
    let after = registry
        .state_snapshots(&owner)
        .into_iter()
        .find(|row| row.id == child)
        .expect("the child's row after the move");
    assert_eq!(
        after.delegation.expect("a child carries the column").state,
        devboule_protocol::DelegationRunState::Unattended,
        "the child has run in an auto-answering mode: the column says so"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mutant: the display name's fallback to the title dropped — a child with
/// no display name of its own stays addressable by the title the roster
/// shows beneath it.
#[test]
fn a_move_reaches_a_child_by_its_title_when_it_has_no_display_name() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("s5b-mv-title");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    insert_live_agent(&registry, &creator, owner.clone());
    let (_runtime, mode_calls, _model_calls, _order) = insert_move_child(
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
    {
        let mut map = registry.inner.lock().expect("registry");
        let live = map
            .get_mut(&child)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("live entry");
        live.metadata.display_name = None;
    }
    registry
        .set_agent_child_profile(&creator, "Agent", "Solo", &solo_resolve)
        .expect("the title addresses the child");
    assert_eq!(mode_calls.load(Ordering::Acquire), 1);
    assert_eq!(
        journal_row_of(&journal, &child).profile_id.as_deref(),
        Some("p-1")
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mutant: the profile resolve moved after the manifest pre-read — a caller
/// with an unticked profile AND a manifest-less child must hear the
/// profile's refusal (check 3 runs before check 4), and no ask may happen.
#[test]
fn the_profile_refusal_comes_before_the_manifest_cannot_say_yet() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("s5b-mv-order");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    insert_live_agent(&registry, &creator, owner.clone());
    let (_runtime, mode_calls, _model_calls, _order) = insert_move_child(
        &registry,
        &journal,
        &child,
        owner.clone(),
        &creator,
        "Worker",
        &[],
        Some("model-a"),
        false,
        false,
        false,
    );
    let error = registry
        .set_agent_child_profile(&creator, "Worker", "Solo", &|_name| {
            Err("unknown profile; call devboule_list_profiles".to_string())
        })
        .expect_err("the profile is not ticked");
    assert!(
        error.contains("unknown profile"),
        "check 3's refusal, not the manifest's: {error}"
    );
    assert_eq!(
        mode_calls.load(Ordering::Acquire),
        0,
        "nothing is asked of the child before the profile resolves"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mutant: the live ratchet assigned the delivery's own judgement instead
/// of comparing ranks — a child already marked `Yes` that moves onto an
/// unjudgeable mode (`unknown`) must keep `Yes` in the live metadata the
/// snapshot serves; the journal's SQL MAX cannot catch this half, which is
/// why the assertion reads the live copy and not the row.
#[test]
fn the_live_marker_ratchet_never_lowers_for_a_lower_judging_delivery() {
    let (dir, registry, journal) = registry_with_journal();
    let owner = test_owner("s5b-mv-ratchet-live");
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
        &["bypass", "deep-work"],
        Some("model-a"),
        true,
        false,
        false,
    );
    let resolve = |name: &str| match name {
        "Solo" => Ok(facts("bypass", "model-b", "p-yes")),
        "Deep" => Ok(facts("deep-work", "model-a", "p-unknown")),
        other => Err(format!("unknown profile ({other})")),
    };
    registry
        .set_agent_child_profile(&creator, "Worker", "Solo", &resolve)
        .expect("the move onto the auto-answering mode lands");
    registry
        .set_agent_child_profile(&creator, "Worker", "Deep", &resolve)
        .expect("the move onto the unjudgeable mode lands");
    let (profile_id, unattended) = live_view_of(&registry, &child);
    assert_eq!(
        profile_id.as_deref(),
        Some("p-unknown"),
        "the lower-judging move itself landed"
    );
    assert_eq!(
        unattended,
        devboule_protocol::UnattendedState::Yes,
        "the live marker stays at what the child earned: a delivery judged \
         unknown must not lower it"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
