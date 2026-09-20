use super::*;
use crate::raster_metadata::clean_png;
use devboule_protocol::ClientMessage;

// ------------------------------------------------------------------
// Slice 5b — the delegation switch's reader side: the delegated
// answer, the surfacing envelope, and the snapshot facts.
// ------------------------------------------------------------------

/// A live agent session that is somebody's child, with a display name.
pub(super) fn insert_child(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
    creator: &str,
) -> Arc<SessionRuntime> {
    let runtime = insert_live_agent(registry, id, owner);
    {
        let mut map = registry.inner.lock().expect("registry");
        let live = map
            .get_mut(id)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("live entry");
        live.metadata.created_by = Some(creator.to_string());
        live.metadata.display_name = Some("child".to_string());
    }
    runtime
}

pub(super) fn park_card(_registry: &SessionRegistry, runtime: &Arc<SessionRuntime>, card_id: &str) {
    let broker = runtime.permission_broker().expect("broker");
    broker
        .register(1, permission_broker::permission(card_id), runtime)
        .expect("the card parks");
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

// ------------------------------------------------------------------
// Pass A — `devboule_set_agent_profile`: a creator moves its own live
// child onto a ticked profile (slice 5b §2).
// ------------------------------------------------------------------

/// A switcher the move tests can watch and aim: both asks counted, in
/// order, each side failable on demand.
struct MoveSwitcher {
    order: Arc<Mutex<Vec<&'static str>>>,
    mode_calls: Arc<AtomicU64>,
    model_calls: Arc<AtomicU64>,
    mode_fails: bool,
    model_fails: bool,
}

impl ModelSwitcher for MoveSwitcher {
    fn set_model(&self, _model_id: Option<&str>, _effort: Option<&str>) -> Result<(), WireError> {
        self.model_calls.fetch_add(1, Ordering::AcqRel);
        self.order.lock().expect("order").push("model");
        if self.model_fails {
            Err(WireError::new(
                ErrorCode::InvalidRequest,
                "the provider refused the model".to_string(),
            ))
        } else {
            Ok(())
        }
    }

    fn set_mode(&self, _mode_id: &str) -> Result<(), WireError> {
        self.mode_calls.fetch_add(1, Ordering::AcqRel);
        self.order.lock().expect("order").push("mode");
        if self.mode_fails {
            Err(WireError::new(
                ErrorCode::InvalidRequest,
                "the provider refused the mode".to_string(),
            ))
        } else {
            Ok(())
        }
    }

    fn clone_switcher(&self) -> Box<dyn ModelSwitcher> {
        Box::new(Self {
            order: Arc::clone(&self.order),
            mode_calls: Arc::clone(&self.mode_calls),
            model_calls: Arc::clone(&self.model_calls),
            mode_fails: self.mode_fails,
            model_fails: self.model_fails,
        })
    }
}

/// A live agent child of `creator`: display name, manifest, an aimable
/// switcher, and the journal row a move's recording updates. Without the
/// manifest (`advertise_manifest: false`) it is the third state — a child
/// the daemon cannot yet judge.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn insert_move_child(
    registry: &SessionRegistry,
    journal: &Arc<Journal>,
    id: &str,
    owner: OwnerId,
    creator: &str,
    display_name: &str,
    available_modes: &[&str],
    current_model: Option<&str>,
    advertise_manifest: bool,
    mode_fails: bool,
    model_fails: bool,
) -> (
    Arc<SessionRuntime>,
    Arc<AtomicU64>,
    Arc<AtomicU64>,
    Arc<Mutex<Vec<&'static str>>>,
) {
    let runtime = Arc::new(SessionRuntime::with_journal(
        id.to_string(),
        registry.journal.clone(),
    ));
    if advertise_manifest {
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("test-agent".to_string()),
            current_model_id: current_model.map(str::to_string),
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: available_modes
                    .first()
                    .map(|mode| (*mode).to_string())
                    .unwrap_or_default(),
                available_modes: available_modes
                    .iter()
                    .map(|mode| devboule_protocol::SessionModeView {
                        id: (*mode).to_string(),
                        name: (*mode).to_string(),
                        description: None,
                    })
                    .collect(),
            }),
        });
    }
    let mode_calls = Arc::new(AtomicU64::new(0));
    let model_calls = Arc::new(AtomicU64::new(0));
    let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let metadata = Session {
        id: id.to_string(),
        workspace_id: None,
        cwd: None,
        kind: SessionKind::Acp,
        title: "Agent".to_string(),
        state: SessionState::Live { generation: 1 },
        elapsed_ms: Some(0),
        provider: Some("test-agent".to_string()),
        peer_session_id: None,
        created_at_ms: 1,
        origin: SessionOrigin::local(),
        display_name: Some(display_name.to_string()),
        created_by: Some(creator.to_string()),
        profile_id: None,
        context_id: None,
        unattended: devboule_protocol::UnattendedState::Unknown,
        labels: Default::default(),
        resumable: false,
    };
    let session = PtySession {
        metadata,
        owner: owner.clone(),
        process_job: Arc::new(JobObject::new().expect("job")),
        master: None,
        killer: Box::new(NoopKiller),
        steerer: Box::new(UnsupportedSteerer),
        switcher: Some(Box::new(MoveSwitcher {
            order: Arc::clone(&order),
            mode_calls: Arc::clone(&mode_calls),
            model_calls: Arc::clone(&model_calls),
            mode_fails,
            model_fails,
        })),
        stderr_handle: None,
        child_wait: None,
        writer: Arc::new(Mutex::new(Box::new(std::io::sink()))),
        image_sink: None,
        static_image_sink: None,
        reader_handle: None,
        coalesce_handle: None,
        runtime: Arc::clone(&runtime),
        mcp_session: None,
        exited: Arc::new(AtomicBool::new(false)),
        preserve_on_exit: Arc::new(AtomicBool::new(false)),
    };
    registry
        .inner
        .lock()
        .expect("registry")
        .insert(id.to_string(), RegistryEntry::Live(Box::new(session)));
    // The birth's row: a move's recording is a targeted UPDATE, so it
    // lands only on a row that exists — and the birth journals before
    // spawn, which is what puts one there in production.
    let mut record = crate::journal::new_session_record(
        id.to_string(),
        owner.user.clone(),
        None,
        SessionKind::Acp,
        "Agent",
    );
    record.display_name = Some(display_name.to_string());
    record.created_by = Some(creator.to_string());
    journal.upsert_blocking(record).expect("birth row");
    (runtime, mode_calls, model_calls, order)
}

/// The facts one test profile carries.
fn move_facts(mode_id: &str, model: &str, profile_id: &str) -> ChildProfileFacts {
    ChildProfileFacts {
        profile_id: profile_id.to_string(),
        mode_id: mode_id.to_string(),
        model: model.to_string(),
        thinking_option_id: None,
    }
}

fn journal_row(journal: &Arc<Journal>, id: &str) -> SessionRecord {
    journal
        .list()
        .expect("journal rows")
        .into_iter()
        .find(|record| record.id == id)
        .expect("the child's row")
}

fn move_live_view(
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

/// The happy path, in order: the mode ask lands first, the model ask
/// second, and the child's row records the profile and the raised marker —
/// the delivered mode's own judgement through the one predicate the birth
/// calls.
#[test]
fn a_move_switches_the_mode_then_the_model_and_records_the_row() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("s5b-move-ok", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    insert_live_agent(&registry, &creator, owner.clone());
    let (_runtime, mode_calls, model_calls, order) = insert_move_child(
        &registry,
        &journal,
        &child,
        owner.clone(),
        &creator,
        "Worker",
        &["bypass", "ask"],
        Some("model-a"),
        true,
        false,
        false,
    );
    registry
        .set_agent_child_profile(&creator, "Worker", "Solo", &|name| {
            if name == "Solo" {
                Ok(move_facts("bypass", "model-b", "p-1"))
            } else {
                Err("unknown profile; call devboule_list_profiles".to_string())
            }
        })
        .expect("the move lands");
    assert_eq!(mode_calls.load(Ordering::Acquire), 1, "the mode is asked");
    assert_eq!(model_calls.load(Ordering::Acquire), 1, "then the model");
    assert_eq!(
        *order.lock().expect("order"),
        vec!["mode", "model"],
        "the mode is asked and applied first; the model only after it succeeds"
    );
    let record = journal_row(&journal, &child);
    assert_eq!(record.profile_id.as_deref(), Some("p-1"));
    assert_eq!(
        record.unattended_state,
        devboule_protocol::UnattendedState::Yes,
        "a child that has run in a broker-answered mode carries the marker"
    );
    let (profile_id, unattended) = move_live_view(&registry, &child);
    assert_eq!(profile_id.as_deref(), Some("p-1"));
    assert_eq!(unattended, devboule_protocol::UnattendedState::Yes);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The link check (A1): a sibling, a grandchild, the caller itself and an
/// invented name are each refused with the sentence that case earns — the
/// child untouched, and the profile resolver never consulted for a child
/// that did not resolve.
#[test]
fn a_move_refuses_a_target_that_is_not_the_callers_own_live_child() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("s5b-move-a1", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let sibling = compose_session_id(&owner.session_token(), "sib").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let grandchild = compose_session_id(&owner.session_token(), "gch").expect("id");
    insert_live_agent(&registry, &creator, owner.clone());
    insert_live_agent(&registry, &sibling, owner.clone());
    {
        // A sibling the caller's own roster already shows, with a name.
        let mut map = registry.inner.lock().expect("registry");
        let live = map
            .get_mut(&sibling)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("sibling entry");
        live.metadata.display_name = Some("Bystander".to_string());
    }
    let (_child_runtime, mode_calls, _model_calls, _order) = insert_move_child(
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
    insert_move_child(
        &registry,
        &journal,
        &grandchild,
        owner.clone(),
        &child,
        "Grandkid",
        &["bypass"],
        Some("model-a"),
        true,
        false,
        false,
    );
    let resolves = Arc::new(AtomicU64::new(0));
    let resolve = |_name: &str| {
        resolves.fetch_add(1, Ordering::AcqRel);
        Ok(move_facts("bypass", "model-b", "p-1"))
    };
    let error = registry
        .set_agent_child_profile(&creator, "Bystander", "Solo", &resolve)
        .expect_err("a sibling is not the caller's child");
    assert!(error.contains("not your child"), "{error}");
    let error = registry
        .set_agent_child_profile(&creator, "Grandkid", "Solo", &resolve)
        .expect_err("a grandchild is not the caller's child");
    assert!(error.contains("not your child"), "{error}");
    let error = registry
        .set_agent_child_profile(&creator, &creator, "Solo", &resolve)
        .expect_err("the caller is not its own child");
    assert!(error.contains("not its own child"), "{error}");
    let error = registry
        .set_agent_child_profile(&creator, "Nobody", "Solo", &resolve)
        .expect_err("an invented name matches nobody");
    assert!(error.contains("none of your live children"), "{error}");
    assert_eq!(
        resolves.load(Ordering::Acquire),
        0,
        "the profile is never consulted for a child that did not resolve"
    );
    assert_eq!(
        mode_calls.load(Ordering::Acquire),
        0,
        "the child is untouched"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two live children sharing a display name are refused ambiguous —
/// neither is moved, and the sentence says to use the id.
#[test]
fn a_move_refuses_ambiguous_child_names_without_picking_one() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("s5b-move-twins", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let twin_a = compose_session_id(&owner.session_token(), "twa").expect("id");
    let twin_b = compose_session_id(&owner.session_token(), "twb").expect("id");
    insert_live_agent(&registry, &creator, owner.clone());
    let (_runtime_a, mode_calls, _model_calls_a, _order_a) = insert_move_child(
        &registry,
        &journal,
        &twin_a,
        owner.clone(),
        &creator,
        "Worker",
        &["bypass"],
        Some("model-a"),
        true,
        false,
        false,
    );
    insert_move_child(
        &registry,
        &journal,
        &twin_b,
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
        .set_agent_child_profile(&creator, "Worker", "Solo", &|_name| {
            Ok(move_facts("bypass", "model-b", "p-1"))
        })
        .expect_err("two children share the name");
    assert!(
        error.contains("more than one of your live children"),
        "{error}"
    );
    assert_eq!(
        mode_calls.load(Ordering::Acquire),
        0,
        "neither twin is asked: ambiguous refuses without picking"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// §1.2's third state at the mode ask: a child whose manifest has not
/// arrived cannot be judged, and the refusal says so — it never renders
/// the unknown as "the mode is unavailable".
#[test]
fn a_child_whose_manifest_has_not_arrived_is_a_cannot_say_yet_refusal() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("s5b-move-absent", "proc-1");
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
            Ok(move_facts("bypass", "model-b", "p-1"))
        })
        .expect_err("no manifest, no judgement");
    assert!(error.contains("cannot say yet"), "{error}");
    assert_eq!(
        mode_calls.load(Ordering::Acquire),
        0,
        "the child is untouched"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The provider dimension: the child's **own manifest** decides what is
/// available. A mode it does not advertise is refused before any ask, with
/// the child untouched and nothing recorded.
#[test]
fn a_mode_the_child_does_not_advertise_is_refused_before_any_ask() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("s5b-move-modes", "proc-1");
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
        &["ask"],
        Some("model-a"),
        true,
        false,
        false,
    );
    let error = registry
        .set_agent_child_profile(&creator, "Worker", "Solo", &|_name| {
            Ok(move_facts("bypass", "model-b", "p-1"))
        })
        .expect_err("the manifest does not advertise bypass");
    assert!(error.contains("'bypass' is not available"), "{error}");
    assert_eq!(mode_calls.load(Ordering::Acquire), 0);
    let record = journal_row(&journal, &child);
    assert_eq!(record.profile_id, None, "nothing is recorded");
    assert_eq!(
        record.unattended_state,
        devboule_protocol::UnattendedState::Unknown,
        "and the marker does not move"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A4: a provider that refuses the mode on its own wire refuses the move;
/// the child is untouched, nothing is recorded, and there is no restart
/// fallback to hide behind.
#[test]
fn a_provider_that_refuses_the_mode_refuses_the_move_without_a_restart() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("s5b-move-a4", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    insert_live_agent(&registry, &creator, owner.clone());
    let (_runtime, mode_calls, model_calls, _order) = insert_move_child(
        &registry,
        &journal,
        &child,
        owner.clone(),
        &creator,
        "Worker",
        &["bypass"],
        Some("model-a"),
        true,
        true,
        false,
    );
    let error = registry
        .set_agent_child_profile(&creator, "Worker", "Solo", &|_name| {
            Ok(move_facts("bypass", "model-b", "p-1"))
        })
        .expect_err("the provider refused the mode");
    assert!(error.contains("the provider refused the mode"), "{error}");
    assert_eq!(mode_calls.load(Ordering::Acquire), 1, "the ask happened");
    assert_eq!(
        model_calls.load(Ordering::Acquire),
        0,
        "and the model is never asked after a refused mode"
    );
    let record = journal_row(&journal, &child);
    assert_eq!(record.profile_id, None, "nothing is recorded");
    assert_eq!(
        record.unattended_state,
        devboule_protocol::UnattendedState::Unknown,
        "and no ratchet: the mode never landed"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A8: the partial state. The mode landed, the model ask was refused: the
/// answer reports exactly that, the row records no profile change — and
/// the ratchet still fires, because the child has in fact been able to run
/// in that mode and that cannot be un-lived.
#[test]
fn a_model_refusal_after_a_landed_mode_reports_the_partial_state_and_still_ratchets() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("s5b-move-a8", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    insert_live_agent(&registry, &creator, owner.clone());
    let (_runtime, mode_calls, model_calls, _order) = insert_move_child(
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
        true,
    );
    let error = registry
        .set_agent_child_profile(&creator, "Worker", "Solo", &|_name| {
            Ok(move_facts("bypass", "model-b", "p-1"))
        })
        .expect_err("the model ask is refused");
    assert!(
        error.contains("mode was switched to 'bypass'")
            && error.contains("no profile change is recorded"),
        "the answer reports exactly the partial state: {error}"
    );
    assert_eq!(mode_calls.load(Ordering::Acquire), 1);
    assert_eq!(model_calls.load(Ordering::Acquire), 1);
    let record = journal_row(&journal, &child);
    assert_eq!(record.profile_id, None, "no profile change is recorded");
    assert_eq!(
        record.unattended_state,
        devboule_protocol::UnattendedState::Yes,
        "and the ratchet still fires: the mode landed and cannot be un-lived"
    );
    let (profile_id, unattended) = move_live_view(&registry, &child);
    assert_eq!(profile_id, None);
    assert_eq!(unattended, devboule_protocol::UnattendedState::Yes);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A6/A7: the marker ratchets upward and is the delivered mode's own
/// judgement through the shared predicate — moving back never clears it,
/// and a mode the daemon cannot judge reads `unknown`, never a certainty
/// in either direction.
#[test]
fn the_marker_ratchets_upward_and_reads_the_delivered_mode() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("s5b-move-ratchet", "proc-1");
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
        "Solo" => Ok(move_facts("bypass", "model-b", "p-yes")),
        "Deep" => Ok(move_facts("deep-work", "model-a", "p-unknown")),
        other => Err(format!("unknown profile ({other})")),
    };
    registry
        .set_agent_child_profile(&creator, "Worker", "Solo", &resolve)
        .expect("the move onto the auto-answering mode lands");
    assert_eq!(
        journal_row(&journal, &child).unattended_state,
        devboule_protocol::UnattendedState::Yes
    );
    registry
        .set_agent_child_profile(&creator, "Worker", "Deep", &resolve)
        .expect("the move onto the unjudgeable mode lands");
    let record = journal_row(&journal, &child);
    assert_eq!(record.profile_id.as_deref(), Some("p-unknown"));
    assert_eq!(
        record.unattended_state,
        devboule_protocol::UnattendedState::Yes,
        "moving back never clears the marker: the child ran unattended and that cannot be un-lived"
    );

    // A child born `no` (its metadata carries the lowest rank), moved onto
    // the unjudgeable mode: the shared predicate answers `unknown` — an
    // absence of knowledge — so the live marker RAISES to `unknown`. A
    // table that answered `no` here would leave it at `no`, which is how
    // the A7 mutant was caught: the journal row cannot distinguish (the
    // MAX ratchet hides the predicate's answer under the old value), the
    // live metadata can.
    let fresh = compose_session_id(&owner.session_token(), "ch2").expect("id");
    insert_move_child(
        &registry,
        &journal,
        &fresh,
        owner.clone(),
        &creator,
        "Fresh",
        &["deep-work"],
        Some("model-a"),
        true,
        false,
        false,
    );
    {
        let mut map = registry.inner.lock().expect("registry");
        let live = map
            .get_mut(&fresh)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("fresh entry");
        live.metadata.unattended = devboule_protocol::UnattendedState::No;
    }
    registry
        .set_agent_child_profile(&creator, "Fresh", "Deep", &resolve)
        .expect("the fresh move lands");
    let record = journal_row(&journal, &fresh);
    assert_eq!(record.profile_id.as_deref(), Some("p-unknown"));
    let (_profile_id, fresh_unattended) = move_live_view(&registry, &fresh);
    assert_eq!(
        fresh_unattended,
        devboule_protocol::UnattendedState::Unknown,
        "an unauthored mode is an absence of knowledge, never a no"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The card id is provider-chosen and carries no session qualifier: two
/// live children of one creator holding the same id must refuse ambiguous
/// and leave BOTH cards pending — the answer must not land on whichever
/// child the registry yields first.
#[test]
fn a_card_id_held_by_two_children_is_ambiguous_and_leaves_both_pending() {
    let (dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("s5b-card-dup", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child_a = compose_session_id(&owner.session_token(), "cha").expect("id");
    let child_b = compose_session_id(&owner.session_token(), "chb").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
    insert_live_agent(&registry, &creator, owner.clone());
    let runtime_a = insert_child(&registry, &child_a, owner.clone(), &creator);
    let runtime_b = insert_child(&registry, &child_b, owner.clone(), &creator);
    park_card(&registry, &runtime_a, "card-dup");
    park_card(&registry, &runtime_b, "card-dup");

    let error = answer(
        &registry,
        &creator,
        "card-dup",
        PermissionOutcome::AllowOnce,
        vec![],
    )
    .expect_err("two children hold the same card id");
    assert!(
        error.contains("more than one of your live children"),
        "{error}"
    );
    for (name, runtime) in [("a", &runtime_a), ("b", &runtime_b)] {
        assert_eq!(
            runtime.permission_broker().expect("broker").pending_len(),
            1,
            "child {name}'s card stays pending for the human"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A card parked on the owner's own NON-child session cannot shadow the
/// caller's real child: the scan resolves among the caller's own children
/// only, the real card resolves, and the non-child's card is untouched.
#[test]
fn a_non_child_holding_the_same_card_id_cannot_shadow_the_real_child() {
    let (dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("s5b-card-shadow", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let bystander = compose_session_id(&owner.session_token(), "bye").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
    insert_live_agent(&registry, &creator, owner.clone());
    let child_runtime = insert_child(&registry, &child, owner.clone(), &creator);
    let bystander_runtime = insert_live_agent(&registry, &bystander, owner.clone());
    park_card(&registry, &bystander_runtime, "card-shadow");
    park_card(&registry, &child_runtime, "card-shadow");

    answer(
        &registry,
        &creator,
        "card-shadow",
        PermissionOutcome::Deny,
        vec![],
    )
    .expect("the caller's own child's card resolves");
    assert_eq!(
        child_runtime
            .permission_broker()
            .expect("broker")
            .pending_len(),
        0,
        "the real child's card is the one that resolved"
    );
    assert_eq!(
        bystander_runtime
            .permission_broker()
            .expect("broker")
            .pending_len(),
        1,
        "the non-child's card is untouched: it stays pending for the human"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// C10: the delegated answer journals its attribution, on the resolved
/// event and the durable record; the human path answers unattributed.
#[test]
fn a_delegated_answer_journals_its_attribution_and_the_humans_stays_none() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("s5b-del-c10", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    store.set(true).expect("set on");
    insert_live_agent(&registry, &creator, owner.clone());
    let child_runtime = insert_child(&registry, &child, owner.clone(), &creator);
    let conn = ConnHandle::new(1);
    attach_tracked(&child_runtime, &conn);

    park_card(&registry, &child_runtime, "card-delegated");
    answer(
        &registry,
        &creator,
        "card-delegated",
        PermissionOutcome::AllowOnce,
        vec![],
    )
    .expect("the delegated answer lands");

    let events = drain(&conn);
    let answered = events.iter().find_map(|event| match event {
        SessionEvent::PermissionAnswered {
            card_id,
            answered_by,
            outcome,
        } => Some((card_id.clone(), answered_by.clone(), outcome.clone())),
        _ => None,
    });
    assert_eq!(
        answered,
        Some((
            "card-delegated".to_string(),
            Some(creator.clone()),
            "allow_once".to_string()
        )),
        "the live record names its creator: {events:?}"
    );
    let resolved = events.iter().find_map(|event| match event {
        SessionEvent::PermissionResolved { answered_by, .. } => answered_by.clone(),
        _ => None,
    });
    assert_eq!(
        resolved.as_deref(),
        Some(creator.as_str()),
        "the resolved event carries the same attribution"
    );
    assert_eq!(
        journal.permission_count(&child).expect("count"),
        1,
        "the ledger the replay reads back counts it"
    );

    // The human path through the same broker: answered, but nobody to
    // attribute it to.
    park_card(&registry, &child_runtime, "card-human");
    child_runtime
        .permission_broker()
        .expect("broker")
        .respond("card-human", PermissionOutcome::Deny)
        .expect("human answer");
    let events = drain(&conn);
    let answered = events.iter().find_map(|event| match event {
        SessionEvent::PermissionAnswered {
            card_id,
            answered_by,
            ..
        } => Some((card_id.clone(), answered_by.clone())),
        _ => None,
    });
    assert_eq!(
        answered,
        Some(("card-human".to_string(), None)),
        "a person's answer is unattributed"
    );
    assert_eq!(
        journal.permission_count(&child).expect("count"),
        2,
        "both resolutions are in the ledger"
    );

    // The ledger is the durable thing the replay reads back: a restart
    // (close, reopen from disk) sees the same two.
    journal.shutdown();
    let reopened = Journal::open(&dir.join("journal.db")).expect("reopen journal");
    assert_eq!(
        reopened.permission_count(&child).expect("count"),
        2,
        "the count survives a restart"
    );
    reopened.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// C11: the snapshot's delegation facts. Absent for a session that is
/// not an agent-created child; `off`/`active` follow the switch as it is
/// **now**; `unattended` is the birth fact and survives the switch going
/// off; the answered count is the ledger's.
#[test]
fn the_snapshot_carries_delegation_facts_per_child() {
    let (dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("s5b-del-c11", "proc-1");
    let creator = compose_session_id(&owner.session_token(), "cr1").expect("id");
    let child = compose_session_id(&owner.session_token(), "ch1").expect("id");
    let unattended = compose_session_id(&owner.session_token(), "chu").expect("id");
    let bystander = compose_session_id(&owner.session_token(), "bye").expect("id");
    let store = Arc::new(crate::delegation_store::DelegationStore::load(&dir));
    registry.attach_delegation(Arc::clone(&store));
    insert_child(&registry, &child, owner.clone(), &creator);
    insert_live_agent(&registry, &bystander, owner.clone());
    insert_child(&registry, &unattended, owner.clone(), &creator);
    {
        let mut map = registry.inner.lock().expect("registry");
        let live = map
            .get_mut(&unattended)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("live entry");
        live.metadata.unattended = devboule_protocol::UnattendedState::Yes;
    }

    let delegation_state_of = |id: &str| {
        registry
            .state_snapshots(&owner)
            .into_iter()
            .find(|row| row.id == id)
            .expect("row")
            .delegation
    };

    // Switch off: a child is `off`, never absent; a bystander is absent,
    // never `off`.
    assert_eq!(
        delegation_state_of(&child),
        Some(DelegationState {
            answered: 0,
            state: DelegationRunState::Off
        })
    );
    assert_eq!(delegation_state_of(&bystander), None);

    // Switch on: active, and the unattended child stays unattended — the
    // birth fact outranks the live switch.
    store.set(true).expect("set on");
    registry.invalidate_state_roster_cache();
    assert_eq!(
        delegation_state_of(&child).map(|facts| facts.state),
        Some(DelegationRunState::Active)
    );
    assert_eq!(
        delegation_state_of(&unattended).map(|facts| facts.state),
        Some(DelegationRunState::Unattended)
    );

    // Switch off again: the child goes back to `off`, the unattended
    // child is STILL unattended — the row is the only thing telling the
    // human which sessions run without asking.
    store.set(false).expect("set off");
    registry.invalidate_state_roster_cache();
    assert_eq!(
        delegation_state_of(&child).map(|facts| facts.state),
        Some(DelegationRunState::Off)
    );
    assert_eq!(
        delegation_state_of(&unattended).map(|facts| facts.state),
        Some(DelegationRunState::Unattended)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A Write sink that records everything, standing in for the PTY input
/// side so the DSR fast path is observable without a ConPTY.
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn sink_runtime() -> (Arc<SessionRuntime>, Arc<Mutex<Vec<u8>>>) {
    let runtime = Arc::new(SessionRuntime::new());
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink: Arc<Mutex<Box<dyn Write + Send>>> =
        Arc::new(Mutex::new(Box::new(SharedSink(Arc::clone(&received)))));
    runtime
        .pty_writer
        .set(sink)
        .ok()
        .expect("sink registered once");
    (runtime, received)
}

/// Pull until the session queue is empty, recording delivery like the
/// connection writer does.
pub(super) fn drain(conn: &ConnHandle) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    loop {
        let batch = conn.pull_events();
        if batch.is_empty() {
            return events;
        }
        for event in &batch {
            conn.event_sent(event);
        }
        events.extend(batch.into_iter().map(|pending| pending.envelope.event));
    }
}

fn apply_snapshot_state(screen: &mut Screen, event: &SessionEvent) {
    let SessionEvent::Snapshot {
        data,
        cursor,
        bracketed_paste,
        line_wrap,
        title,
        ..
    } = event
    else {
        return;
    };
    screen.process(data.as_bytes());
    let shape = match cursor.shape {
        CursorShape::Block => 1,
        CursorShape::Underline => 3,
        CursorShape::Bar => 5,
    } + u16::from(!cursor.blinking);
    let state = format!(
        "\x1b[{};{}H\x1b[?25{}\x1b[{shape} q\x1b[?2004{}\x1b[?7{}{}",
        cursor.row + 1,
        cursor.col + 1,
        if cursor.visible { 'h' } else { 'l' },
        if *bracketed_paste { 'h' } else { 'l' },
        if *line_wrap { 'h' } else { 'l' },
        title
            .as_deref()
            .map(|title| format!("\x1b]2;{}\x1b\\", title))
            .unwrap_or_default(),
    );
    screen.process(state.as_bytes());
}

/// Deterministic flood chunk with attributes, cursor motion, CJK and a
/// line break, so screen equality is exercised beyond plain text.
fn flood_chunk(index: usize) -> String {
    let shade = 31 + (index % 7);
    format!("\x1b[{shade}mchunk {index:06}\x1b[0m \u{754c}\r\n")
}

pub(super) fn attach_tracked(runtime: &Arc<SessionRuntime>, conn: &Arc<ConnHandle>) -> u64 {
    let outcome = runtime
        .try_attach_with_replay(None, conn, false)
        .expect("attach");
    let transcript = runtime.is_transcript();
    conn.track_with_agent_replay(
        "s.a.1",
        Arc::clone(runtime),
        transcript,
        Some(0),
        outcome.generation,
        outcome.live_agent_replay,
    );
    outcome.generation
}

#[test]
fn attach_delivers_snapshot_then_live_with_exact_boundary() {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.publish_output("before");
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);
    assert_eq!(runtime.last_applied_seq(), 1);

    let events = drain(&conn);
    let [SessionEvent::Snapshot {
        as_of_seq, data, ..
    }] = &events[..]
    else {
        panic!("expected a single snapshot, got {events:?}");
    };
    assert_eq!(*as_of_seq, 1);
    assert!(data.contains("before"), "snapshot data: {data:?}");

    runtime.publish_output("after");
    let events = drain(&conn);
    assert_eq!(
        events,
        vec![SessionEvent::Output {
            seq: 2,
            data: "after".to_string()
        }]
    );
}

#[test]
fn attach_during_flood_never_duplicates_or_skips() {
    let runtime = Arc::new(SessionRuntime::new());
    let flood_runtime = Arc::clone(&runtime);
    let flood = std::thread::Builder::new()
        .name("flood".into())
        .spawn(move || {
            for index in 1..=4_000 {
                flood_runtime.publish_output(&flood_chunk(index));
                if index % 32 == 0 {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        })
        .expect("flood thread");

    let mut seen = std::collections::HashSet::new();
    let mut covered_to = 0u64;
    for epoch in 0u64..25 {
        let conn = ConnHandle::new(epoch + 1);
        attach_tracked(&runtime, &conn);
        let events = drain(&conn);
        assert!(
            !events.is_empty(),
            "epoch {epoch} saw nothing: attach must enqueue a snapshot"
        );
        // Spread the attach epochs across the flood's lifetime.
        std::thread::sleep(Duration::from_millis(4));
        let mut expected = None;
        for event in &events {
            match event {
                SessionEvent::Snapshot { as_of_seq, .. } => {
                    assert!(
                        *as_of_seq >= covered_to,
                        "snapshot boundary moved backwards at epoch {epoch}"
                    );
                    covered_to = (*as_of_seq).max(covered_to);
                    expected = Some(as_of_seq + 1);
                }
                SessionEvent::Output { seq, .. } => {
                    if let Some(expected_seq) = expected {
                        assert_eq!(
                            *seq, expected_seq,
                            "output skipped or duplicated at epoch {epoch}"
                        );
                    }
                    expected = Some(seq + 1);
                    assert!(seen.insert(*seq), "sequence {seq} delivered twice");
                    covered_to = (*seq).max(covered_to);
                }
                SessionEvent::Exit { .. } => {}
                other => panic!("unexpected event at epoch {epoch}: {other:?}"),
            }
        }
        runtime.detach_if_conn(conn.id);
    }
    flood.join().expect("flood thread joins");

    // The flood is complete: one final attach must now deliver (or
    // subsume) everything it published.
    let conn = ConnHandle::new(999);
    attach_tracked(&runtime, &conn);
    for event in drain(&conn) {
        match event {
            SessionEvent::Snapshot { as_of_seq, .. } => covered_to = as_of_seq.max(covered_to),
            SessionEvent::Output { seq, .. } => {
                assert!(seen.insert(seq), "sequence {seq} delivered twice");
                covered_to = seq.max(covered_to);
            }
            _ => {}
        }
    }
    assert_eq!(
        covered_to, 4_000,
        "the flood was not fully delivered or subsumed"
    );
}

#[test]
fn reattach_mid_flood_screen_equals_a_fresh_emulator() {
    let runtime = Arc::new(SessionRuntime::new());
    let mut reference = Screen::new(INITIAL_COLS, INITIAL_ROWS);

    fn apply(screen: &mut Screen, event: &SessionEvent) {
        match event {
            SessionEvent::Snapshot { .. } => apply_snapshot_state(screen, event),
            SessionEvent::Output { data, .. } => screen.process(data.as_bytes()),
            _ => {}
        }
    }

    // Phase 1: publish while detached, then attach and synchronise.
    for index in 1..=60 {
        let chunk = flood_chunk(index);
        runtime.publish_output(&chunk);
        reference.process(chunk.as_bytes());
    }
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);
    let mut client = Screen::new(INITIAL_COLS, INITIAL_ROWS);
    for event in drain(&conn) {
        apply(&mut client, &event);
    }
    assert_eq!(
        client.snapshot(),
        reference.snapshot(),
        "snapshot state must equal the emulator after phase 1"
    );

    // Phase 2: live chunks while attached, then reattach from scratch.
    for index in 61..=120 {
        let chunk = flood_chunk(index);
        runtime.publish_output(&chunk);
        reference.process(chunk.as_bytes());
    }
    for event in drain(&conn) {
        apply(&mut client, &event);
    }
    assert_eq!(client.snapshot(), reference.snapshot());

    runtime.detach_if_conn(conn.id);
    let conn = ConnHandle::new(2);
    attach_tracked(&runtime, &conn);
    let mut client = Screen::new(INITIAL_COLS, INITIAL_ROWS);
    for event in drain(&conn) {
        apply(&mut client, &event);
    }
    assert_eq!(
        client.snapshot(),
        reference.snapshot(),
        "snapshot + subsequent events must equal a fresh emulator fed the whole stream"
    );
}

#[test]
fn slow_client_is_resynchronised_with_a_snapshot() {
    let runtime = Arc::new(SessionRuntime::new());
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);

    // Stop reading: publish well past the frame budget without pulling.
    for index in 1..=200 {
        runtime.publish_output(&format!("slow-{index:04}\r\n"));
    }

    let events = drain(&conn);
    let mut expected = None;
    let mut client = Screen::new(INITIAL_COLS, INITIAL_ROWS);
    let mut reference = Screen::new(INITIAL_COLS, INITIAL_ROWS);
    let mut seen = std::collections::HashSet::new();
    for event in &events {
        match event {
            SessionEvent::Snapshot { as_of_seq, .. } => {
                expected = Some(as_of_seq + 1);
                apply_snapshot_state(&mut client, event);
            }
            SessionEvent::Output { seq, data } => {
                assert_eq!(*seq, expected.expect("outputs follow the snapshot"));
                expected = Some(seq + 1);
                assert!(seen.insert(*seq), "sequence {seq} delivered twice");
                client.process(data.as_bytes());
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
    for index in 1..=200 {
        reference.process(format!("slow-{index:04}\r\n").as_bytes());
    }
    assert_eq!(
        client.snapshot(),
        reference.snapshot(),
        "the resynchronised screen must still be the true screen"
    );
}

#[test]
fn pending_queue_never_exceeds_byte_or_frame_budget() {
    let runtime = Arc::new(SessionRuntime::new());
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);
    let payload = "x".repeat(COALESCE_MAX_BYTES);

    for _ in 0..200 {
        runtime.publish_output(&payload);
        let stream = runtime.stream.lock().expect("stream lock");
        assert!(stream
            .observers
            .values()
            .all(|attachment| attachment.pending_bytes <= PENDING_OUTPUT_BUDGET_BYTES));
        assert!(stream
            .observers
            .values()
            .all(|attachment| attachment.pending_frames <= PENDING_OUTPUT_BUDGET_FRAMES));
    }
}

#[test]
fn dsr_reply_is_written_straight_to_the_pty() {
    let (runtime, received) = sink_runtime();
    // No attachment, no journal, no snapshot: the query is answered on
    // the publish path itself.
    runtime.publish_output("\x1b[2;3H\x1b[6n");
    assert_eq!(
        String::from_utf8(received.lock().unwrap().clone()).expect("utf8"),
        "\x1b[2;3R",
        "one one-based CPR reply, routed to the PTY writer"
    );
    runtime.publish_output("plain");
    assert_eq!(received.lock().unwrap().len(), 6, "no extra replies");
}

#[test]
fn control_path_stays_responsive_under_flood() {
    let runtime = Arc::new(SessionRuntime::new());
    let flood_runtime = Arc::clone(&runtime);
    let stop = Arc::new(AtomicBool::new(false));
    let flood_stop = Arc::clone(&stop);
    let flood = std::thread::Builder::new()
        .name("flood".into())
        .spawn(move || {
            let chunk = "x".repeat(COALESCE_MAX_BYTES);
            while !flood_stop.load(Ordering::Acquire) {
                for _ in 0..16 {
                    flood_runtime.publish_output(&chunk);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        })
        .expect("flood thread");

    let mut worst = Duration::ZERO;
    for epoch in 0..200u64 {
        let started = Instant::now();
        let conn = ConnHandle::new(epoch + 1);
        runtime
            .try_attach_with_replay(None, &conn, false)
            .expect("attach under flood");
        runtime.detach_if_conn(conn.id);
        worst = worst.max(started.elapsed());
    }
    stop.store(true, Ordering::Release);
    flood.join().expect("flood thread joins");
    // Screen capture + registration is two grid copies under the lock;
    // if the publish path ever held the mutex across slow work, this
    // would blow far past the bound. 1 s is orders of magnitude above
    // the observed cost and 30x below the RPC timeout this milestone
    // exists to fix.
    assert!(
        worst < Duration::from_secs(1),
        "state lock starved under flood: {worst:?}"
    );
}

#[test]
fn two_observers_receive_the_same_output() {
    let runtime = Arc::new(SessionRuntime::new());
    let first = ConnHandle::new(1);
    let second = ConnHandle::new(2);
    let first_outcome = runtime
        .try_attach_with_subscription(101, None, &first, false)
        .expect("first observer");
    first
        .track_with_subscription(
            101,
            Arc::clone(&runtime),
            false,
            None,
            first_outcome.generation,
            first_outcome.live_agent_replay,
        )
        .expect("first subscription");
    let second_outcome = runtime
        .try_attach_with_subscription(202, None, &second, false)
        .expect("second observer");
    second
        .track_with_subscription(
            202,
            Arc::clone(&runtime),
            false,
            None,
            second_outcome.generation,
            second_outcome.live_agent_replay,
        )
        .expect("second subscription");
    runtime
        .claim_resize(first.id, 101)
        .expect("first observer claims resize control");
    let _ = drain(&first);
    let _ = drain(&second);

    runtime.publish_output("shared");

    assert_eq!(
        drain(&first),
        vec![SessionEvent::Output {
            seq: 1,
            data: "shared".to_string(),
        }]
    );
    assert_eq!(
        drain(&second),
        vec![SessionEvent::Output {
            seq: 1,
            data: "shared".to_string(),
        }]
    );
    assert_eq!(runtime.resize_owner_conn_id(), Some(1));
}

#[test]
fn same_connection_can_reattach() {
    let runtime = SessionRuntime::new();
    let conn = ConnHandle::new(7);
    runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("first");
    runtime
        .try_attach_with_replay(
            Some(Cursor {
                generation: 1,
                seq: 0,
            }),
            &conn,
            false,
        )
        .expect("reattach");
    assert_eq!(runtime.resize_owner_conn_id(), Some(7));
}

#[test]
fn detaching_one_observer_leaves_the_other_live() {
    let runtime = Arc::new(SessionRuntime::new());
    let first = ConnHandle::new(3);
    let second = ConnHandle::new(4);
    let first_outcome = runtime
        .try_attach_with_subscription(301, None, &first, false)
        .expect("first observer");
    first
        .track_with_subscription(
            301,
            Arc::clone(&runtime),
            false,
            None,
            first_outcome.generation,
            first_outcome.live_agent_replay,
        )
        .expect("first subscription");
    let second_outcome = runtime
        .try_attach_with_subscription(402, None, &second, false)
        .expect("second observer");
    second
        .track_with_subscription(
            402,
            Arc::clone(&runtime),
            false,
            None,
            second_outcome.generation,
            second_outcome.live_agent_replay,
        )
        .expect("second subscription");
    let _ = drain(&first);
    let _ = drain(&second);

    runtime.detach_subscription(first.id, 301);
    first.untrack_subscription(301);
    runtime.publish_output("still-live");

    assert!(drain(&first).is_empty());
    assert_eq!(
        drain(&second),
        vec![SessionEvent::Output {
            seq: 1,
            data: "still-live".to_string(),
        }]
    );
}

#[test]
fn typed_permission_request_reaches_a_late_observer() {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.stream.lock().unwrap().screen = None;
    let first = ConnHandle::new(5);
    let first_outcome = runtime
        .try_attach_with_subscription(501, None, &first, true)
        .expect("first observer");
    first
        .track_with_subscription(
            501,
            Arc::clone(&runtime),
            false,
            None,
            first_outcome.generation,
            first_outcome.live_agent_replay,
        )
        .expect("first subscription");

    runtime.publish_agent_event(permission_attention_event(), None);
    let first_events = first.pull_events();
    assert!(first_events.iter().any(|event| matches!(
        event.envelope.event,
        SessionEvent::PermissionRequest { ref tool_call_id, .. } if tool_call_id == "tool-attention"
    )));
    for event in &first_events {
        first.event_sent(event);
    }

    let second = ConnHandle::new(6);
    let second_outcome = runtime
        .try_attach_with_subscription(602, None, &second, true)
        .expect("late observer");
    second
        .track_with_subscription(
            602,
            Arc::clone(&runtime),
            false,
            None,
            second_outcome.generation,
            second_outcome.live_agent_replay,
        )
        .expect("second subscription");
    let second_events = second.pull_events();
    assert!(second_events.iter().any(|event| matches!(
        event.envelope.event,
        SessionEvent::PermissionRequest { ref tool_call_id, .. } if tool_call_id == "tool-attention"
    )));
}

#[test]
fn detached_permission_request_reaches_a_late_observer_once() {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.stream.lock().unwrap().screen = None;
    let first = ConnHandle::new(7);
    runtime
        .try_attach_with_subscription(701, None, &first, true)
        .expect("first observer");

    runtime.publish_agent_event(permission_attention_event(), None);
    runtime.detach_subscription(first.id, 701);

    let second = ConnHandle::new(8);
    let second_outcome = runtime
        .try_attach_with_subscription(802, None, &second, true)
        .expect("late observer");
    second
        .track_with_subscription(
            802,
            Arc::clone(&runtime),
            false,
            None,
            second_outcome.generation,
            second_outcome.live_agent_replay,
        )
        .expect("second subscription");
    let events = second.pull_events();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.envelope.event,
                SessionEvent::PermissionRequest { ref tool_call_id, .. }
                    if tool_call_id == "tool-attention"
            ))
            .count(),
        1
    );
}

#[test]
fn last_detach_keeps_runtime_and_allows_later_attach() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-last-detach", "process-last-detach");
    let session_id = "s.last-detach.1";
    // `insert_live` seats the runtime but writes no `sessions` row, so every
    // frame this session appended failed ("No session with that id.") and only
    // the observation of the failure raced the writer. The row is what makes
    // the assertion below about delivery instead of about that race.
    journal
        .upsert_blocking(crate::journal::new_session_record(
            session_id,
            owner.user.clone(),
            None,
            SessionKind::Terminal,
            "Terminal",
        ))
        .expect("journal row");
    insert_live(&registry, session_id, owner.clone());
    let runtime = registry.runtime(session_id).expect("runtime");
    let first = ConnHandle::new(5);
    registry
        .attach_with_subscription(session_id, 501, None, &first, &owner, false)
        .expect("first observer");
    let _ = drain(&first);

    registry
        .detach_with_subscription(session_id, 501, &first, &owner)
        .expect("first observer detaches");
    assert!(!runtime.process_exited());
    assert!(registry.runtime(session_id).is_ok());
    assert!(runtime.stream.lock().expect("stream").observers.is_empty());

    let third = ConnHandle::new(6);
    registry
        .attach_with_subscription(session_id, 603, None, &third, &owner, false)
        .expect("later observer");
    let _ = drain(&third);
    runtime.publish_output("after-detach");

    assert_eq!(
        drain(&third),
        vec![SessionEvent::Output {
            seq: 1,
            data: "after-detach".to_string(),
        }],
        "the frame reached a durable session: no degradation frame follows it"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn last_transcript_detach_removes_idle_registry_entry() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-transcript-idle", "process-transcript-idle");
    let session_id = "s.transcript-idle.1";
    journal
        .upsert_blocking(ended_record(session_id, &owner.user))
        .expect("journal row");
    insert_transcript(&registry, session_id, owner.clone());

    let conn = ConnHandle::new(7);
    registry
        .attach_with_subscription(session_id, 701, None, &conn, &owner, false)
        .expect("transcript observer attaches");
    registry
        .detach_with_subscription(session_id, 701, &conn, &owner)
        .expect("transcript observer detaches");

    assert!(registry.runtime(session_id).is_err());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn delivered_transcript_exit_removes_the_idle_registry_entry() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-transcript-exit", "process-transcript-exit");
    let session_id = "s.transcript-exit.1";
    journal
        .upsert_blocking(ended_record(session_id, &owner.user))
        .expect("journal row");
    insert_transcript(&registry, session_id, owner.clone());

    let conn = ConnHandle::new(8);
    registry
        .attach_with_subscription(session_id, 801, None, &conn, &owner, false)
        .expect("transcript observer attaches");
    let events = conn.pull_events();
    assert!(events
        .iter()
        .any(|event| matches!(event.envelope.event, SessionEvent::Exit { .. })));
    for event in &events {
        conn.event_sent(event);
    }
    registry.subscription_event_sent(session_id);

    assert!(registry.runtime(session_id).is_err());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The transcript store holds the whole history whatever the cursor says.
/// A reattaching reader presents a cursor that is a position inside the
/// current generation only — history does not advance cursors, so a cursor
/// can never certify the history was read — and the hydration behind the
/// store must therefore read unpositioned: the pull, through the owed-row
/// predicate, decides what is delivered.
#[test]
fn the_transcript_store_holds_the_whole_history_whatever_the_cursor_says() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-1", "probe");
    let session_id = "s.store.all.1";
    journal
        .upsert_blocking(crate::journal::new_session_record(
            session_id,
            &owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("journal row");
    let ledger_row = crate::journal::output_record(session_id, 1, 1, "gen-1 ledger".as_bytes());
    journal.append_blocking(ledger_row).expect("gen-1 ledger");
    let user_row = crate::journal::agent_report_record(
        session_id,
        1,
        2,
        &SessionEvent::AgentUserMessage {
            message_id: Some("m1".into()),
            text: "gen-1 user".into(),
            author: devboule_protocol::UserMessageAuthor::Human,
            message_kind: devboule_protocol::UserMessageKind::Unknown,
        },
    )
    .unwrap();
    journal.append_blocking(user_row).expect("gen-1 user row");
    journal.start_generation(session_id, 2).expect("resume");
    let ledger_after = crate::journal::output_record(session_id, 2, 6, "gen-2 ledger".as_bytes());
    journal.append_blocking(ledger_after).expect("gen-2 ledger");
    let answer_row = crate::journal::agent_report_record(
        session_id,
        2,
        7,
        &SessionEvent::AgentMessage {
            message_id: Some("m2".into()),
            text: "after cursor".into(),
            parent_tool_use_id: None,
            spawn_depth: None,
        },
    )
    .unwrap();
    journal.append_blocking(answer_row).expect("gen-2 answer");

    // A reattaching reader whose cursor says it is at seq 5 of generation 2.
    let conn = ConnHandle::new(9);
    registry
        .attach_with_subscription(
            session_id,
            901,
            Some(Cursor {
                generation: 2,
                seq: 5,
            }),
            &conn,
            &owner,
            false,
        )
        .expect("transcript observer attaches");
    let mut events = conn.pull_events();
    for event in &events {
        conn.event_sent(event);
    }
    loop {
        let more = conn.pull_events();
        if more.is_empty() {
            break;
        }
        for event in &more {
            conn.event_sent(event);
        }
        events.extend(more);
    }
    let transcript: Vec<String> = events
        .iter()
        .filter_map(|event| match &event.envelope.event {
            SessionEvent::Output { data, .. } => Some(data.clone()),
            SessionEvent::AgentUserMessage { text, .. } => Some(text.clone()),
            SessionEvent::AgentMessage { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        transcript,
        vec!["gen-1 ledger", "gen-1 user", "gen-2 ledger", "after cursor",],
        "the store must hold the whole history whatever the cursor says: {transcript:?}"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn stale_generation_is_rejected() {
    let runtime = SessionRuntime::new();
    runtime.bump_generation();
    let conn = ConnHandle::new(1);
    let err = runtime
        .try_attach_with_replay(
            Some(Cursor {
                generation: 1,
                seq: 0,
            }),
            &conn,
            false,
        )
        .err()
        .expect("stale generation must be rejected");
    assert_eq!(err.code, ErrorCode::SessionGenerationMismatch);
}

#[test]
fn detach_clears_only_this_connection() {
    let runtime = SessionRuntime::new();
    let conn = ConnHandle::new(3);
    runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach");
    runtime.detach_if_conn(3);
    assert_eq!(runtime.resize_owner_conn_id(), None);
}

#[test]
fn journal_keeps_drain_bytes_after_reap() {
    let dir = std::env::temp_dir().join(format!(
        "devboule-drain-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    journal
        .upsert_blocking(new_session_record(
            "s.drain.1",
            "S-1-5-21-1",
            None,
            SessionKind::Terminal,
            "Terminal",
        ))
        .unwrap();
    let runtime = Arc::new(SessionRuntime::with_journal(
        "s.drain.1".into(),
        Some(Arc::clone(&journal)),
    ));
    runtime.publish_output("HEAD");
    journal.flush().unwrap();
    runtime.mark_exited(Some(0));
    journal.flush().unwrap();
    let tail = "X".repeat(3953);
    runtime.publish_output(&tail);
    journal.flush().unwrap();
    runtime.close_output();
    journal.try_mark_ended("s.drain.1", 1, Some(0));
    journal.flush().unwrap();
    assert_eq!(runtime.published_frames.load(Ordering::Relaxed), 2);
    let stats = journal.stats();
    assert_eq!(stats.accepted_frames, 2);
    assert_eq!(stats.committed_frames, 2);
    assert_eq!(stats.failed_frames, 0);
    let replay = journal.replay("s.drain.1").unwrap();
    let replay_bytes: usize = replay
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { data, .. } => Some(data.len()),
            _ => None,
        })
        .sum();
    assert_eq!(replay_bytes, 4 + 3953, "journal silently lost drain bytes");
    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn coalesce_constants_are_small_enough_for_echo() {
    const {
        assert!(COALESCE_MAX_BYTES <= 16 * 1024);
    }
    const {
        assert!(COALESCE_MAX_BYTES >= 1024);
    }
    assert!(COALESCE_FLUSH <= Duration::from_millis(16));
}

#[test]
fn pty_error_exposes_only_the_os_code_to_clients() {
    let detail = "CreateProcessW command=C:\\Users\\secret\\shell.exe (os error 1450)";
    let code = extract_os_error_code(detail).expect("OS error code");
    assert_eq!(code, 1450);
    assert_eq!(os_error_description(code), "no system resources");
    let wire = pty_wire_error("Could not start the terminal shell.", detail);
    assert_eq!(
        wire.message,
        "Could not start the terminal shell. (OS error 1450: no system resources)."
    );
    assert!(!wire.message.contains("secret"));
}

pub(super) fn tmp_delete_registry() -> (std::path::PathBuf, SessionRegistry, Arc<Journal>) {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let process_id = std::process::id();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devboule-delete-session-{process_id}-{stamp}-{counter}"
    ));
    std::fs::create_dir(&dir).expect("tmp dir");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    let registry = SessionRegistry::new(RuntimePaths::from_dir(&dir), Some(Arc::clone(&journal)));
    (dir, registry, journal)
}

pub(super) fn test_owner(user: &str, client: &str) -> OwnerId {
    OwnerId::new(user, client).expect("owner")
}

pub(super) fn permission_attention_event() -> SessionEvent {
    SessionEvent::PermissionRequest {
        tool_call_id: "tool-attention".to_string(),
        title: "Run attention test".to_string(),
        description: None,
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: Vec::new(),
        // A provider client writes `local` here; the daemon overwrites it
        // with the session's stored origin on the way out.
        origin: SessionOrigin::local(),
        create_agent: None,
    }
}

pub(super) fn ended_record(id: &str, user: &str) -> crate::journal::SessionRecord {
    let mut record = new_session_record(id, user, None, SessionKind::Terminal, "Terminal");
    record.status = PersistStatus::Ended;
    record
}

pub(super) fn insert_transcript(registry: &SessionRegistry, id: &str, owner: OwnerId) {
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

pub(super) struct NoopKiller;

impl SessionKiller for NoopKiller {
    fn kill(&mut self) {}
    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(NoopKiller)
    }
}

struct RecordingSwitcher(Arc<AtomicU64>);

impl ModelSwitcher for RecordingSwitcher {
    fn set_model(&self, _model_id: Option<&str>, _effort: Option<&str>) -> Result<(), WireError> {
        self.0.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn clone_switcher(&self) -> Box<dyn ModelSwitcher> {
        Box::new(Self(Arc::clone(&self.0)))
    }
}

pub(super) struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "forced writer failure",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) struct RecordingWriter(pub(super) Arc<Mutex<Vec<u8>>>);

impl Write for RecordingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("recording writer lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct BytewiseRecordingWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
    first_write: Arc<Barrier>,
    first_write_seen: AtomicBool,
}

impl Write for BytewiseRecordingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let Some(byte) = bytes.first() else {
            return Ok(0);
        };
        self.bytes
            .lock()
            .expect("recording writer lock")
            .push(*byte);
        if !self.first_write_seen.swap(true, Ordering::AcqRel) {
            self.first_write.wait();
        }
        Ok(1)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn insert_live_agent(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
) -> Arc<SessionRuntime> {
    insert_live_agent_with_kind_and_writer(
        registry,
        id,
        owner,
        SessionKind::Acp,
        Box::new(FailingWriter) as Box<dyn Write + Send>,
    )
}

pub(super) fn insert_live_agent_with_writer(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
    writer: Box<dyn Write + Send>,
) -> Arc<SessionRuntime> {
    insert_live_agent_with_kind_and_writer(registry, id, owner, SessionKind::Acp, writer)
}

pub(super) fn insert_live_agent_with_kind_and_writer(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
    kind: SessionKind,
    writer: Box<dyn Write + Send>,
) -> Arc<SessionRuntime> {
    insert_live_agent_with_kind_writer_and_sink(registry, id, owner, kind, writer, None, None)
}

/// The insert every other helper goes through, with the collaborators the
/// steer path decides with — the killer a refused steer may fall back to, and
/// the steerer itself — plus the optional structured prompt routes.
#[allow(clippy::too_many_arguments)]
pub(super) fn insert_live_agent_with_turn_control(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
    kind: SessionKind,
    writer: Box<dyn Write + Send>,
    image_sink: Option<Arc<AcpPromptSink>>,
    static_image_sink: Option<Arc<dyn StaticImageSink>>,
    killer: Box<dyn SessionKiller>,
    steerer: Box<dyn SessionSteerer>,
) -> Arc<SessionRuntime> {
    let metadata = Session {
        id: id.to_string(),
        workspace_id: None,
        cwd: None,
        kind,
        title: "Agent".to_string(),
        state: SessionState::Live { generation: 1 },
        elapsed_ms: Some(0),
        provider: Some("test-agent".to_string()),
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
    let (broker, _) = permission_broker::test_broker();
    let runtime = SessionRuntime::for_acp(id.to_string(), registry.journal.clone(), broker);
    registry.configure_runtime_attention(&runtime, &owner);
    let session = PtySession {
        metadata,
        owner,
        process_job: Arc::new(JobObject::new().expect("job")),
        master: None,
        killer,
        steerer,
        switcher: None,
        stderr_handle: None,
        child_wait: None,
        writer: Arc::new(Mutex::new(writer)),
        // Test sessions default to the fallback world: no structured
        // route unless the test installs one, so the path-line
        // assertions below pin the honest fallback.
        image_sink,
        static_image_sink,
        reader_handle: None,
        coalesce_handle: None,
        runtime: Arc::clone(&runtime),
        mcp_session: None,
        exited: Arc::new(AtomicBool::new(false)),
        preserve_on_exit: Arc::new(AtomicBool::new(false)),
    };
    registry
        .inner
        .lock()
        .expect("registry")
        .insert(id.to_string(), RegistryEntry::Live(Box::new(session)));
    runtime
}

/// The insert behind the helpers above, with the optional structured
/// prompt routes: `image_sink` is the ACP sibling, `static_image_sink`
/// the route the three static providers carry. `None` for either is the
/// fallback world: no structured route unless a test installs one, so the
/// path-line assertions below pin the honest fallback.
fn insert_live_agent_with_kind_writer_and_sink(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
    kind: SessionKind,
    writer: Box<dyn Write + Send>,
    image_sink: Option<Arc<AcpPromptSink>>,
    static_image_sink: Option<Arc<dyn StaticImageSink>>,
) -> Arc<SessionRuntime> {
    insert_live_agent_with_turn_control(
        registry,
        id,
        owner,
        kind,
        writer,
        image_sink,
        static_image_sink,
        Box::new(NoopKiller),
        Box::new(UnsupportedSteerer),
    )
}

pub(super) fn attach_live_agent_for_test(
    runtime: &Arc<SessionRuntime>,
    session_id: &str,
    conn_id: u64,
) -> Arc<ConnHandle> {
    let conn = ConnHandle::new(conn_id);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    conn
}

// --- prompt attachments ------------------------------------------------

pub(super) fn attachment(name: &str, mime_type: &str, bytes: &[u8]) -> PromptAttachment {
    use base64::Engine;
    PromptAttachment {
        name: name.to_string(),
        mime_type: mime_type.to_string(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

/// A live agent session, attached, with a writer that swallows the prompt.
pub(super) fn agent_ready_for_attachment(
    registry: &SessionRegistry,
    session_id: &str,
    owner: &OwnerId,
    conn_id: u64,
) -> Arc<ConnHandle> {
    let runtime = insert_live_agent_with_writer(
        registry,
        session_id,
        owner.clone(),
        Box::new(std::io::sink()),
    );
    attach_live_agent_for_test(&runtime, session_id, conn_id)
}

/// The folder an attachment send is expected to fill.
pub(super) fn attachment_folder(registry: &SessionRegistry, session_id: &str) -> PathBuf {
    registry.runtime_dir().join("attachments").join(session_id)
}

/// Every file under `dir`, recursively. A missing `dir` is zero files,
/// which is what "wrote nothing" looks like when the folder was never made.
pub(super) fn files_under(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

pub(super) fn attachment_message(error: &WireError) -> &str {
    assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    &error.message
}

// --- stored references, on the send side -------------------------------
//
// A reference is the half of an attachment that does not travel in the
// frame: the client deposited the bytes earlier and now names the digest
// and the size it was answered with. These pin the resolution — the wire's
// rules first, the store after, the size compared — and the shape a
// resolved reference leaves in the prompt.

/// The path a reference's file must be at, computed from the digest the
/// deposit answered with. The store names a stored file `{digest}.{ext}`,
/// and the send resolves it to that path inside the session's folder.
fn stored_path(registry: &SessionRegistry, session_id: &str, digest: &str) -> PathBuf {
    attachment_folder(registry, session_id).join(format!("{digest}.png"))
}

/// The prompt the plain-text writer received, as a string.
fn written_prompt(received: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8(received.lock().expect("writer").clone()).expect("utf8")
}

#[test]
fn a_deposited_reference_reaches_the_provider_as_a_path_line() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-ref-deposited", "process-ref-deposited");
    let session_id = "ref-deposited";
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        session_id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 61);
    let deck = clean_png(0x31);
    let request = attachment("deck.png", "image/png", &deck);

    let reference = registry
        .deposit(session_id, &owner, &conn, &request)
        .expect("the owner may deposit into their own session");

    registry
        .send_with_subscription(
            session_id,
            61,
            "read the deck",
            &[],
            std::slice::from_ref(&reference),
            &owner,
            &conn,
        )
        .expect("a send naming a reference that was really deposited");

    let stored = files_under(&attachment_folder(&registry, session_id));
    assert_eq!(stored.len(), 1, "the deposit wrote one file");
    assert_eq!(
        stored_path(&registry, session_id, &reference.digest),
        stored[0],
        "the reference resolves to the deposited file"
    );
    assert_eq!(
        written_prompt(&received),
        format!(
            "read the deck\n\n[Image available at: {}]",
            stored[0].display()
        ),
        "the provider is handed the stored file's path, not its bytes"
    );
    assert!(
        !written_prompt(&received).contains(&request.data),
        "a reference exists so the bytes do not travel in the frame"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_reference_whose_stored_bytes_disagree_with_the_file_is_refused() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-ref-size", "process-ref-size");
    let session_id = "ref-size";
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        session_id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 62);

    let mut reference = registry
        .deposit(
            session_id,
            &owner,
            &conn,
            &attachment("deck.png", "image/png", &clean_png(0x32)),
        )
        .expect("deposit");
    let real_size = reference.stored_bytes;
    // The client's copy of the size is off by one: it is naming a file it
    // did not deposit, or a file that changed under it.
    reference.stored_bytes = real_size + 1;

    let error = registry
        .send_with_subscription(
            session_id,
            62,
            "read the deck",
            &[],
            std::slice::from_ref(&reference),
            &owner,
            &conn,
        )
        .expect_err("a size that disagrees with the file is a refusal, not a warning");

    let message = attachment_message(&error);
    assert!(message.contains(&reference.digest), "{message}");
    assert!(message.contains(&real_size.to_string()), "{message}");
    assert!(message.contains(&(real_size + 1).to_string()), "{message}");
    assert!(
        received.lock().expect("writer").is_empty(),
        "a refused reference must not leave a prompt half-sent"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The wire's session rule comes before the store, which is the order the
/// whole path keeps. The discriminating half of the assertion is that the
/// store *could* have answered: the reference names a file that really is
/// on disk, in the other session's folder. If resolution ran first, this
/// request would be refused for a digest the store cannot find in the
/// request's session, and the session sentence would never be reached.
#[test]
fn a_reference_naming_another_session_is_refused_by_the_wires_own_rule() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-ref-foreign", "process-ref-foreign");
    let session_id = "ref-foreign";
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        session_id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 63);
    let other_id = compose_session_id(&owner.session_token(), "ref02").expect("id");
    insert_live_agent_with_writer(
        &registry,
        &other_id,
        owner.clone(),
        Box::new(std::io::sink()),
    );
    let other = registry
        .deposit(
            &other_id,
            &owner,
            // A connection of its own: this deposit is not the request's,
            // and the request's session is the one that must stay empty.
            &ConnHandle::new(640),
            &attachment("deck.png", "image/png", &clean_png(0x33)),
        )
        .expect("the owner deposits into the other session too");
    assert_eq!(
        other.session_id, other_id,
        "the reference names the other session"
    );
    assert!(
        files_under(&attachment_folder(&registry, session_id)).is_empty(),
        "only the other session was deposited into, so the store has nothing to resolve \
         against the request's session"
    );

    let error = registry
        .send_with_subscription(
            session_id,
            63,
            "read the deck",
            &[],
            std::slice::from_ref(&other),
            &owner,
            &conn,
        )
        .expect_err("a reference to another session is refused");

    let message = attachment_message(&error);
    assert!(message.contains("belongs to session"), "{message}");
    assert!(
        message.contains(&other_id),
        "the refusal says which session the reference belongs to: {message}"
    );
    assert!(
        !message.contains("holds no attachment"),
        "the store's sentence means the store was asked first: {message}"
    );
    assert!(received.lock().expect("writer").is_empty());

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The regression the deleted dispatch guard stood in for. Before the
/// resolution existed, a request naming references was refused whole; what
/// must not happen now is a send that answers `Ok` while the file it named
/// was quietly dropped out of the prompt, which is the vanishing deck the
/// whole feature exists to prevent.
#[test]
fn a_reference_whose_digest_was_never_deposited_is_refused_not_dropped() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-ref-phantom", "process-ref-phantom");
    let session_id = "ref-phantom";
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        session_id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 64);
    let phantom = AttachmentReference {
        session_id: session_id.to_string(),
        digest: "a".repeat(64),
        stored_bytes: 4096,
    };

    let error = registry
        .send_with_subscription(
            session_id,
            64,
            "read the deck",
            &[],
            std::slice::from_ref(&phantom),
            &owner,
            &conn,
        )
        .expect_err("a digest with no file behind it is refused");

    let message = attachment_message(&error);
    assert!(
        message.contains("holds no attachment"),
        "the store's own sentence is the one that must come back: {message}"
    );
    assert!(
        received.lock().expect("writer").is_empty(),
        "nothing was sent, so nothing was silently missing from it"
    );
    assert!(
        files_under(&attachment_folder(&registry, session_id)).is_empty(),
        "a refused send writes nothing either"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn inline_attachments_and_references_in_one_send_keep_the_order_the_client_gave() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-ref-order", "process-ref-order");
    let session_id = "ref-order";
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        session_id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 65);
    let inline_bytes = clean_png(0x34);
    // Two stored decks, deposited in the opposite order to the one the
    // request names them in: the prompt must follow the request, not the
    // store's write order.
    let second = registry
        .deposit(
            session_id,
            &owner,
            &conn,
            &attachment("second.png", "image/png", &clean_png(0x35)),
        )
        .expect("deposit the deck the request names second");
    let first = registry
        .deposit(
            session_id,
            &owner,
            &conn,
            &attachment("first.png", "image/png", &clean_png(0x36)),
        )
        .expect("deposit the deck the request names first");

    registry
        .send_with_subscription(
            session_id,
            65,
            "two files",
            &[attachment("inline.png", "image/png", &inline_bytes)],
            &[first.clone(), second.clone()],
            &owner,
            &conn,
        )
        .expect("one inline attachment and two references in one send");

    // `clean_png` carries no metadata the store strips, so the digest of
    // the bytes the client sent is the name the daemon stored them under.
    let inline_path = attachment_folder(&registry, session_id).join(format!(
        "{}.png",
        crate::attachment_store::sha256_hex(&inline_bytes)
    ));
    assert_eq!(
        written_prompt(&received),
        format!(
            "two files\n\n[Image available at: {}]\n\n[Image available at: {}]\n[Image available at: {}]",
            inline_path.display(),
            stored_path(&registry, session_id, &first.digest).display(),
            stored_path(&registry, session_id, &second.digest).display()
        ),
        "the inline attachment's line comes first and the references follow in the client's order"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

// --- the static route (Claude, Codex, Pi) -----------------------------
//
// These pin the send path's half of the three static providers: a session
// that carries a `static_image_sink` takes the plan's text and never the
// legacy walk, and a session whose route declines (or which carries no
// route at all) writes exactly the bytes it always wrote. A double stands
// in for the provider's own frame owner so neither test needs a child.

/// A route double: records that it was consulted and that its plan was the
/// one sent, and answers with a plan carrying the text the caller must
/// journal — or declines, which is what a provider not authorised for
/// inline bytes answers.
struct RecordingStaticSink {
    calls: Arc<AtomicU64>,
    sent: Arc<AtomicU64>,
    answer: Option<&'static str>,
}

impl StaticImageSink for RecordingStaticSink {
    fn plan_prompt(
        &self,
        _store: &AttachmentStore,
        _session_id: &str,
        _text: &str,
        _attachments: &[PromptAttachment],
    ) -> Result<Option<Box<dyn PlannedStaticPrompt>>, WireError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        Ok(self.answer.map(|text| {
            Box::new(RecordingStaticPlan {
                text: text.to_string(),
                sent: Arc::clone(&self.sent),
            }) as Box<dyn PlannedStaticPrompt>
        }))
    }
}

/// The plan half of the double: the text it carries, and the record that it
/// was the one sent. Modelled rather than framed, so neither test below
/// needs a child.
struct RecordingStaticPlan {
    text: String,
    sent: Arc<AtomicU64>,
}

impl PlannedStaticPrompt for RecordingStaticPlan {
    fn text(&self) -> &str {
        &self.text
    }

    /// The same append the three real plans make, so a references test on
    /// this route sees the text a provider would build rather than a
    /// separate composition the double invented.
    fn append_reference_path_lines(&mut self, reference_paths: &[PathBuf]) {
        push_reference_path_lines(&mut self.text, reference_paths);
    }

    fn send(&self) -> Result<(), WireError> {
        self.sent.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

fn test_static_sink(
    answer: Option<&'static str>,
) -> (Arc<RecordingStaticSink>, Arc<AtomicU64>, Arc<AtomicU64>) {
    let calls = Arc::new(AtomicU64::new(0));
    let sent = Arc::new(AtomicU64::new(0));
    let sink = Arc::new(RecordingStaticSink {
        calls: Arc::clone(&calls),
        sent: Arc::clone(&sent),
        answer,
    });
    (sink, calls, sent)
}

#[test]
fn the_static_route_sends_its_own_frame_and_leaves_the_writer_alone() {
    // The route owns the send: on this branch the plain-text writer is not
    // typed into at all, and the text the journal records is the plan's.
    // That is also what holds a send to one materialization per attachment
    // — `with_attachment_paths`, the legacy walk, is reached only when the
    // route answered nothing.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-static-route", "process-static-route");
    let session_id = "static-route";
    let received = Arc::new(Mutex::new(Vec::new()));
    let (sink, calls, sent) = test_static_sink(Some("the plan's own text"));
    let runtime = insert_live_agent_with_kind_writer_and_sink(
        &registry,
        session_id,
        owner.clone(),
        SessionKind::Claude,
        Box::new(RecordingWriter(Arc::clone(&received))),
        None,
        Some(sink),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 71);
    let image = clean_png(0x21);
    registry
        .send_with_subscription(
            session_id,
            71,
            "describe this",
            &[attachment("photo.png", "image/png", &image)],
            &[],
            &owner,
            &conn,
        )
        .expect("send");
    assert!(
        received.lock().expect("writer").is_empty(),
        "the route's frame went out, not a plain-text write"
    );
    assert_eq!(sent.load(Ordering::Acquire), 1, "the plan was sent once");
    assert_eq!(
        calls.load(Ordering::Acquire),
        1,
        "the route is consulted once per send"
    );
    let recorded = conn
        .pull_events()
        .into_iter()
        .find_map(|event| match event.envelope.event {
            SessionEvent::AgentUserMessage { text, .. } => Some(text),
            _ => None,
        })
        .expect("the plan's text is what the journal records");
    assert_eq!(recorded, "the plan's own text");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The same append on a plan route: the references go into the plan's own
/// text, which is the string the provider's frame carries *and* the string
/// the journal records, so the two cannot disagree about which files the
/// prompt named.
#[test]
fn the_static_routes_plan_text_carries_the_reference_lines_too() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-ref-static", "process-ref-static");
    let session_id = "ref-static";
    let received = Arc::new(Mutex::new(Vec::new()));
    let (sink, calls, sent) = test_static_sink(Some("the plan's own text"));
    let runtime = insert_live_agent_with_kind_writer_and_sink(
        &registry,
        session_id,
        owner.clone(),
        SessionKind::Claude,
        Box::new(RecordingWriter(Arc::clone(&received))),
        None,
        Some(sink),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 73);
    let reference = registry
        .deposit(
            session_id,
            &owner,
            &conn,
            &attachment("deck.png", "image/png", &clean_png(0x37)),
        )
        .expect("deposit");

    registry
        .send_with_subscription(
            session_id,
            73,
            "read the deck",
            &[],
            std::slice::from_ref(&reference),
            &owner,
            &conn,
        )
        .expect("send");

    assert_eq!(
        calls.load(Ordering::Acquire),
        1,
        "the route is consulted once per send"
    );
    assert_eq!(sent.load(Ordering::Acquire), 1, "the plan was the one sent");
    assert!(
        received.lock().expect("writer").is_empty(),
        "the route's frame went out, not a plain-text write"
    );
    let recorded = conn
        .pull_events()
        .into_iter()
        .find_map(|event| match event.envelope.event {
            SessionEvent::AgentUserMessage { text, .. } => Some(text),
            _ => None,
        })
        .expect("the plan's text is what the journal records");
    assert_eq!(
        recorded,
        format!(
            "the plan's own text\n\n[Image available at: {}]",
            stored_path(&registry, session_id, &reference.digest).display()
        ),
        "the reference line is part of the plan's text, not a block beside it"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_static_route_that_declines_keeps_the_legacy_write_byte_for_byte() {
    // `None` is the provider saying nothing travels inline — a Pi model
    // that declared no image, or no attachments at all. The send must then
    // write exactly the text it has always written.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-static-declined", "process-static-declined");
    let session_id = "static-declined";
    let received = Arc::new(Mutex::new(Vec::new()));
    let (sink, calls, sent) = test_static_sink(None);
    let runtime = insert_live_agent_with_kind_writer_and_sink(
        &registry,
        session_id,
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::clone(&received))),
        None,
        Some(sink),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 72);
    let image = clean_png(0x22);
    registry
        .send_with_subscription(
            session_id,
            72,
            "describe this",
            &[attachment("photo.png", "image/png", &image)],
            &[],
            &owner,
            &conn,
        )
        .expect("send");
    let written = String::from_utf8(received.lock().expect("writer").clone()).expect("utf8");
    let legacy = with_attachment_paths(
        &registry.attachments,
        session_id,
        "describe this",
        &[attachment("photo.png", "image/png", &image)],
    )
    .expect("legacy text");
    assert_eq!(written, legacy, "the declined route changes no byte");
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert_eq!(
        sent.load(Ordering::Acquire),
        0,
        "a declined route sends nothing"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn multiple_observers_can_send_complete_inputs_concurrently() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-multi-writer", "process-multi-writer");
    let written = Arc::new(Mutex::new(Vec::new()));
    let session_id = "s.multi-writer.1";
    let first_text = "first observer input\n".repeat(32);
    let second_text = "second observer input\n".repeat(32);
    let start = Arc::new(Barrier::new(3));
    let first_write = Arc::new(Barrier::new(2));
    insert_live_agent_with_writer(
        &registry,
        session_id,
        owner.clone(),
        Box::new(BytewiseRecordingWriter {
            bytes: Arc::clone(&written),
            first_write: Arc::clone(&first_write),
            first_write_seen: AtomicBool::new(false),
        }),
    );
    let first = ConnHandle::new(1);
    let second = ConnHandle::new(2);
    registry
        .attach_with_subscription(session_id, 101, None, &first, &owner, true)
        .expect("first observer attaches");
    registry
        .attach_with_subscription(session_id, 202, None, &second, &owner, true)
        .expect("second observer attaches");

    let first_registry = registry.clone();
    let first_start = Arc::clone(&start);
    let first_owner = owner.clone();
    let first_session_id = session_id.to_string();
    let first_handle = std::thread::spawn(move || {
        first_start.wait();
        first_registry
            .send_with_subscription(
                &first_session_id,
                101,
                &first_text,
                &[],
                &[],
                &first_owner,
                &first,
            )
            .expect("first input");
        first_text
    });
    let second_registry = registry.clone();
    let second_start = Arc::clone(&start);
    let second_owner = owner.clone();
    let second_session_id = session_id.to_string();
    let second_handle = std::thread::spawn(move || {
        second_start.wait();
        second_registry
            .send_with_subscription(
                &second_session_id,
                202,
                &second_text,
                &[],
                &[],
                &second_owner,
                &second,
            )
            .expect("second input");
        second_text
    });
    start.wait();
    first_write.wait();
    let first_text = first_handle.join().expect("first sender joins");
    let second_text = second_handle.join().expect("second sender joins");
    let received = written.lock().expect("writer").clone();
    let first_then_second = [first_text.as_bytes(), second_text.as_bytes()].concat();
    let second_then_first = [second_text.as_bytes(), first_text.as_bytes()].concat();
    assert!(
        received == first_then_second || received == second_then_first,
        "concurrent inputs were interleaved"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_resize_owner_can_resize_terminal() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-resize-owner", "process-resize-owner");
    let session_id = "s.resize-owner.1";
    insert_live(&registry, session_id, owner.clone());
    let first = ConnHandle::new(1);
    let second = ConnHandle::new(2);
    registry
        .attach_with_subscription(session_id, 101, None, &first, &owner, false)
        .expect("first observer attaches");
    registry
        .attach_with_subscription(session_id, 202, None, &second, &owner, false)
        .expect("second observer attaches");
    registry
        .claim_resize_with_subscription(session_id, 101, &owner, &first)
        .expect("first observer claims resize control");

    let error = registry
        .resize_with_subscription(session_id, 202, 100, 30, &owner, &second)
        .expect_err("non-owner resize must be rejected");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("resize control"));
    registry
        .resize_with_subscription(session_id, 101, 100, 30, &owner, &first)
        .expect("resize owner can resize");
    let runtime = registry.runtime(session_id).expect("runtime");
    assert_eq!(
        runtime
            .stream
            .lock()
            .expect("stream")
            .screen
            .as_ref()
            .expect("screen")
            .dimensions(),
        (100, 30)
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

pub(super) fn insert_live(registry: &SessionRegistry, id: &str, owner: OwnerId) {
    insert_live_with_writer(registry, id, owner, Box::new(std::io::sink()));
}

pub(super) fn insert_live_with_writer(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
    writer: Box<dyn Write + Send>,
) {
    let metadata = Session {
        id: id.to_string(),
        workspace_id: None,
        cwd: None,
        kind: SessionKind::Terminal,
        title: "Terminal".to_string(),
        state: SessionState::Live { generation: 1 },
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
    let runtime = Arc::new(SessionRuntime::with_journal(
        id.to_string(),
        registry.journal.clone(),
    ));
    registry.configure_runtime_attention(&runtime, &owner);
    let session = PtySession {
        metadata,
        owner,
        process_job: Arc::new(JobObject::new().expect("job")),
        master: None,
        killer: Box::new(NoopKiller),
        steerer: Box::new(UnsupportedSteerer),
        switcher: None,
        stderr_handle: None,
        child_wait: None,
        writer: Arc::new(Mutex::new(writer)),
        // A terminal has no structured prompt route.
        image_sink: None,
        static_image_sink: None,
        reader_handle: None,
        coalesce_handle: None,
        runtime,
        mcp_session: None,
        exited: Arc::new(AtomicBool::new(false)),
        preserve_on_exit: Arc::new(AtomicBool::new(false)),
    };
    registry
        .inner
        .lock()
        .expect("registry")
        .insert(id.to_string(), RegistryEntry::Live(Box::new(session)));
}

#[test]
fn learned_peer_session_id_is_durable_and_restored_on_hydration() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-peer", "process-1111");
    let caller = test_owner("S-1-5-21-peer", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "peer01").expect("id");
    let mut record = ended_record(&session_id, &original.user);
    record.kind = SessionKind::Acp;
    journal.upsert_blocking(record).expect("row");

    let runtime = SessionRuntime::with_journal(session_id.clone(), Some(Arc::clone(&journal)));
    runtime.set_peer_session_id("peer-session-1".to_string());
    journal.flush().expect("peer id");
    let row = journal
        .list()
        .expect("list")
        .into_iter()
        .find(|row| row.id == session_id)
        .expect("row");
    assert_eq!(row.peer_session_id.as_deref(), Some("peer-session-1"));

    let conn = ConnHandle::new(1);
    registry
        .attach(&session_id, None, &conn, &caller, true)
        .expect("same-user hydration");
    let hydrated = registry
        .inner
        .lock()
        .expect("registry")
        .get(&session_id)
        .expect("hydrated entry")
        .runtime()
        .peer_session_id();
    assert_eq!(hydrated.as_deref(), Some("peer-session-1"));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_attach_allows_a_previous_run_session_for_the_same_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-attach", "process-1111");
    let caller = test_owner("S-1-5-21-attach", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "attach01").expect("id");
    journal
        .upsert_blocking(ended_record(&session_id, &original.user))
        .expect("row");
    registry
        .attach(&session_id, None, &ConnHandle::new(1), &caller, false)
        .expect("same user, different client must attach");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_attach_allows_a_live_registry_session_from_a_dead_client_same_user() {
    // The most common restart shape: the daemon survives, the app does
    // not. The registry still holds the LIVE entry under the old client
    // token; the new client (same user) must attach through
    // runtime_for_user, not through journal hydration.
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-attach-live", "process-1111");
    let caller = test_owner("S-1-5-21-attach-live", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "attach03").expect("id");
    insert_live(&registry, &session_id, original);
    registry
        .attach(&session_id, None, &ConnHandle::new(1), &caller, false)
        .expect("same user, different client must attach to the live entry");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_attach_rejects_a_live_registry_session_from_another_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-attach-live-owner", "process-1111");
    let stranger = test_owner("S-1-5-21-attach-live-stranger", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "attach04").expect("id");
    insert_live(&registry, &session_id, original);
    let error = registry
        .attach(&session_id, None, &ConnHandle::new(1), &stranger, false)
        .expect_err("different user must stay unauthorized on the live entry");
    assert_eq!(error.code, ErrorCode::Unauthorized);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_attach_rejects_a_previous_run_session_from_another_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-attach-owner", "process-1111");
    let stranger = test_owner("S-1-5-21-attach-stranger", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "attach02").expect("id");
    journal
        .upsert_blocking(ended_record(&session_id, &original.user))
        .expect("row");
    let error = registry
        .attach(&session_id, None, &ConnHandle::new(1), &stranger, false)
        .expect_err("different user must stay unauthorized");
    assert_eq!(error.code, ErrorCode::Unauthorized);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_close_allows_a_previous_run_session_for_the_same_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-close", "process-1111");
    let caller = test_owner("S-1-5-21-close", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "close01").expect("id");
    journal
        .upsert_blocking(ended_record(&session_id, &original.user))
        .expect("row");
    assert!(!registry
        .close(&session_id, &caller, &None)
        .expect("same user, different client must close"));
    assert!(journal
        .list()
        .expect("list")
        .iter()
        .all(|row| row.id != session_id));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_close_rejects_a_previous_run_session_from_another_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-close-owner", "process-1111");
    let stranger = test_owner("S-1-5-21-close-stranger", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "close02").expect("id");
    journal
        .upsert_blocking(ended_record(&session_id, &original.user))
        .expect("row");
    let error = registry
        .close(&session_id, &stranger, &None)
        .expect_err("different user must stay unauthorized");
    assert_eq!(error.code, ErrorCode::Unauthorized);
    assert!(journal
        .list()
        .expect("list")
        .iter()
        .any(|row| row.id == session_id));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_stop_allows_a_previous_run_live_session_for_the_same_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-stop", "process-1111");
    let caller = test_owner("S-1-5-21-stop", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "stop01").expect("id");
    insert_live(&registry, &session_id, original);
    registry
        .stop(&session_id, &caller)
        .expect("same user, different client must stop");
    assert!(registry
        .inner
        .lock()
        .expect("registry")
        .get(&session_id)
        .and_then(RegistryEntry::as_peer_visible)
        .is_some_and(|session| session.preserve_on_exit.load(Ordering::Acquire)));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_stop_rejects_a_previous_run_live_session_from_another_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-stop-owner", "process-1111");
    let stranger = test_owner("S-1-5-21-stop-stranger", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "stop02").expect("id");
    insert_live(&registry, &session_id, original);
    let error = registry
        .stop(&session_id, &stranger)
        .expect_err("different user must stay unauthorized");
    assert_eq!(error.code, ErrorCode::Unauthorized);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn roster_and_history_are_user_scoped_and_include_previous_run_sessions() {
    let (dir, registry, journal) = tmp_delete_registry();
    let previous_run = test_owner("S-1-5-21-roster", "process-1111");
    let caller = test_owner("S-1-5-21-roster", "process-2222");
    let stranger = test_owner("S-1-5-21-other", "process-3333");
    let previous_id = compose_session_id(&previous_run.session_token(), "roster01").expect("id");
    let stranger_id = compose_session_id(&stranger.session_token(), "roster02").expect("id");
    journal
        .upsert_blocking(ended_record(&previous_id, &previous_run.user))
        .expect("previous row");
    journal
        .upsert_blocking(ended_record(&stranger_id, &stranger.user))
        .expect("stranger row");

    let roster = registry.state_snapshots(&caller);
    assert!(roster.iter().any(|session| session.id == previous_id));
    assert!(roster.iter().all(|session| session.id != stranger_id));
    let history = registry.list(&caller).expect("history");
    assert!(history.iter().any(|session| session.id == previous_id));
    assert!(history.iter().all(|session| session.id != stranger_id));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn live_transition_does_not_requery_the_journal_roster() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-roster-cache", "process-roster-cache");
    let runtime = insert_live_agent(&registry, "s.roster-cache.1", owner.clone());
    let journal_id =
        compose_session_id(&owner.session_token(), "roster-cache-history").expect("journal id");
    journal
        .upsert_blocking(ended_record(&journal_id, &owner.user))
        .expect("journal row");

    let _ = registry.state_snapshots(&owner);
    assert_eq!(registry.journal_list_call_count(), 1);

    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    let _ = registry.state_snapshots(&owner);

    assert_eq!(
        registry.journal_list_call_count(),
        1,
        "a live transition must reuse the cached journal roster"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn journal_roster_does_not_cache_rows_under_revision_that_changed_after_list() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-roster-race", "process-roster-race");
    let initial_id = compose_session_id(&owner.session_token(), "roster-race-initial")
        .expect("initial journal id");
    let added_after_list_id = compose_session_id(&owner.session_token(), "roster-race-after-list")
        .expect("post-list journal id");
    journal
        .upsert_blocking(ended_record(&initial_id, &owner.user))
        .expect("initial journal row");

    let hook_journal = Arc::clone(&journal);
    let hook_owner = owner.clone();
    let hook_id = added_after_list_id.clone();
    registry.set_journal_roster_after_list_hook(Arc::new(move || {
        hook_journal
            .upsert_blocking(ended_record(&hook_id, &hook_owner.user))
            .expect("post-list journal row");
    }));

    // The hook queues a real roster mutation after list() has returned,
    // deterministically reproducing the revision/data mismatch without
    // depending on sleeps or scheduler timing.
    let first = registry.state_snapshots(&owner);
    assert!(first
        .iter()
        .all(|session| session.id != added_after_list_id));

    let second = registry.state_snapshots(&owner);
    assert!(
        second
            .iter()
            .any(|session| session.id == added_after_list_id),
        "a row added after list() must not be hidden by a stale cache"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S5-09 and S5-04 on the path that matters to a running app: a child
/// created while the client is already attached arrives as a *push*-only
/// row, so the snapshot that push carries must name the child and say which
/// session created it. The row the next full roster build produces must say
/// the same thing, or the two paths disagree about the same session.
#[test]
fn a_push_only_row_carries_the_childs_name_and_creator() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-snapshot-names", "process-snapshot-names");
    let creator = "s.snapshot-names.parent";
    let child = "s.snapshot-names.child";
    // The app is open and holds this roster already: the next state change
    // is served from the cache, which is what makes it a push.
    let _ = registry.state_snapshots(&owner);
    insert_live_agent(&registry, child, owner.clone());
    {
        let mut map = registry.inner.lock().expect("map");
        let live = map
            .get_mut(child)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("the live child");
        live.metadata.display_name = Some("worker".to_string());
        live.metadata.created_by = Some(creator.to_string());
    }

    registry.notify_session_transition(&owner, child);
    let pushed = registry.state_snapshots(&owner);
    assert_eq!(
        registry.full_roster_build_count(),
        1,
        "the row came from the push, not from a rebuild"
    );
    let row = pushed
        .iter()
        .find(|session| session.id == child)
        .expect("the pushed row");
    assert_eq!(
        row.display_name.as_deref(),
        Some("worker"),
        "the push names the child"
    );
    assert_eq!(
        row.created_by.as_deref(),
        Some(creator),
        "the push names the session that created it"
    );

    // The same session through a full build: the two paths agree.
    registry.state_roster_cache.lock().expect("cache").clear();
    let rebuilt = registry.state_snapshots(&owner);
    assert_eq!(registry.full_roster_build_count(), 2, "the cache was gone");
    let row = rebuilt
        .iter()
        .find(|session| session.id == child)
        .expect("the rebuilt row");
    assert_eq!(row.display_name.as_deref(), Some("worker"));
    assert_eq!(row.created_by.as_deref(), Some(creator));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn live_transition_does_not_rebuild_a_large_roster() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-large-roster", "process-large-roster");
    for index in 0..64 {
        let id = compose_session_id(
            &owner.session_token(),
            &format!("roster-history-{index:02}"),
        )
        .expect("journal id");
        journal
            .upsert_blocking(ended_record(&id, &owner.user))
            .expect("journal row");
    }
    let runtimes = (0..8)
        .map(|index| {
            insert_live_agent(&registry, &format!("s.large-roster-{index}"), owner.clone())
        })
        .collect::<Vec<_>>();

    let roster = registry.state_snapshots(&owner);
    assert_eq!(roster.len(), 72);
    assert_eq!(registry.full_roster_build_count(), 1);
    assert_eq!(registry.journal_list_call_count(), 1);

    runtimes[0].publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    let updated = registry.state_snapshots(&owner);

    assert_eq!(updated.len(), 72);
    assert_eq!(registry.full_roster_build_count(), 1);
    assert_eq!(
        registry.journal_list_call_count(),
        1,
        "the transition must not make work proportional to journal-only sessions"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resume_refuses_a_session_without_a_persisted_provider() {
    let owner = test_owner("S-1-5-21-resume-provider", "process-1111");
    let session_id = compose_session_id(&owner.session_token(), "resume01").expect("id");
    let mut record = ended_record(&session_id, &owner.user);
    record.kind = SessionKind::Acp;
    record.peer_session_id = Some("peer-session".to_string());
    let error = resume_handle(&record, &owner).expect_err("missing provider must refuse");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("provider was not persisted"));
}

#[test]
fn resume_refuses_a_session_without_a_persisted_peer_id() {
    let owner = test_owner("S-1-5-21-resume-peer", "process-1111");
    let session_id = compose_session_id(&owner.session_token(), "resume02").expect("id");
    let mut record = ended_record(&session_id, &owner.user);
    record.kind = SessionKind::Acp;
    record.provider = Some("grok".to_string());
    let error = resume_handle(&record, &owner).expect_err("missing peer id must refuse");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    // True for a row that never had a handle and for one whose handle a
    // disown retracted: there is nothing here to resume from.
    assert!(error
        .message
        .contains("the row has no provider handle to resume from"));
}

#[test]
fn resume_refuses_a_session_from_another_user() {
    let owner = test_owner("S-1-5-21-resume-owner", "process-1111");
    let stranger = test_owner("S-1-5-21-resume-stranger", "process-2222");
    let session_id = compose_session_id(&owner.session_token(), "resume03").expect("id");
    let mut record = ended_record(&session_id, &owner.user);
    record.kind = SessionKind::Acp;
    record.provider = Some("grok".to_string());
    record.peer_session_id = Some("peer-session".to_string());
    let error = resume_handle(&record, &stranger).expect_err("wrong user must refuse");
    assert_eq!(error.code, ErrorCode::Unauthorized);
}

#[test]
fn resume_owner_transfer_allows_the_resumer_and_rejects_a_third_client() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-resume-transfer", "process-1111");
    let resumer = test_owner("S-1-5-21-resume-transfer", "process-2222");
    let third = test_owner("S-1-5-21-resume-transfer", "process-3333");
    let session_id = compose_session_id(&original.session_token(), "resume04").expect("id");
    insert_live(&registry, &session_id, original);
    {
        let mut map = registry.inner.lock().expect("registry");
        let entry = map.get_mut(&session_id).expect("live entry");
        entry.as_peer_visible_mut().expect("live session").owner = resumer.clone();
    }
    let map = registry.inner.lock().expect("registry");
    let entry = map.get(&session_id).expect("transferred entry");
    assert!(check_owner(entry, &resumer).is_ok());
    assert_eq!(
        check_owner(entry, &third)
            .expect_err("third client must not drive resumed session")
            .code,
        ErrorCode::Unauthorized
    );
    drop(map);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn explicit_provider_is_not_hijacked_by_claude_env_override() {
    let (kind, provider, provenance) = SessionRegistry::resolve_session_provider(
        SessionKind::Acp,
        Some("grok".to_string()),
        Some("claude"),
    );
    assert_eq!(kind, SessionKind::Acp);
    assert_eq!(provider.as_deref(), Some("grok"));
    assert_eq!(provenance, Some(ProviderProvenance::Request));
}

#[test]
fn claude_env_override_applies_when_the_request_has_no_provider() {
    let (kind, provider, provenance) =
        SessionRegistry::resolve_session_provider(SessionKind::Acp, None, Some("claude"));
    assert_eq!(kind, SessionKind::Claude);
    assert_eq!(provider, None);
    assert_eq!(provenance, None);
}

#[test]
fn pi_provider_selection_uses_the_first_class_rpc_kind() {
    let (kind, provider, provenance) = SessionRegistry::resolve_session_provider(
        SessionKind::Acp,
        Some("pi".to_string()),
        Some("claude"),
    );
    assert_eq!(kind, SessionKind::Pi);
    assert_eq!(provider, None);
    assert_eq!(provenance, None);

    let (kind, provider, provenance) =
        SessionRegistry::resolve_session_provider(SessionKind::Acp, None, Some("pi"));
    assert_eq!(kind, SessionKind::Pi);
    assert_eq!(provider, None);
    assert_eq!(provenance, None);
}

#[test]
fn env_named_provider_is_marked_as_env_provenance() {
    let (kind, provider, provenance) =
        SessionRegistry::resolve_session_provider(SessionKind::Acp, None, Some("codex-acp"));
    assert_eq!(kind, SessionKind::Acp);
    assert_eq!(provider.as_deref(), Some("codex-acp"));
    assert_eq!(provenance, Some(ProviderProvenance::Env));
}

#[test]
fn env_override_cannot_launch_npx_wrapper() {
    let error = SessionRegistry::env_override_cannot_launch_npx(
        "codex-acp",
        Some(ProviderProvenance::Env),
        Some(crate::provider_catalog::ProviderOrigin::NpxWrapper),
    )
    .expect_err("env npx must be denied");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(
        error.message,
        "provider 'codex-acp' is an npx wrapper; npx wrappers require explicit selection, the env override cannot launch them"
    );
}

#[test]
fn env_override_still_allows_native_user_binary() {
    SessionRegistry::env_override_cannot_launch_npx(
        "grok",
        Some(ProviderProvenance::Env),
        Some(crate::provider_catalog::ProviderOrigin::UserBinary),
    )
    .expect("env native must still resolve");
}

#[test]
fn request_provided_npx_wrapper_is_not_blocked_by_env_policy() {
    SessionRegistry::env_override_cannot_launch_npx(
        "codex-acp",
        Some(ProviderProvenance::Request),
        Some(crate::provider_catalog::ProviderOrigin::NpxWrapper),
    )
    .expect("explicit npx is the consent path");
}

#[test]
fn invalid_claude_effort_is_rejected_before_switcher() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-set-model", "process-set-model");
    let session_id = "claude-set-model";
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        registry.journal.clone(),
    ));
    runtime.store_claude_manifest(
        crate::claude_catalog::initial_manifest(crate::claude_catalog::fallback_models()),
        crate::claude_catalog::ClaudeCatalogState::Provisional,
    );
    let calls = Arc::new(AtomicU64::new(0));
    let metadata = Session {
        id: session_id.to_string(),
        workspace_id: None,
        cwd: None,
        kind: SessionKind::Claude,
        title: "Claude".to_string(),
        state: SessionState::Live { generation: 1 },
        elapsed_ms: Some(0),
        provider: Some("claude".to_string()),
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
    let session = PtySession {
        metadata,
        owner: owner.clone(),
        process_job: Arc::new(JobObject::new().expect("job")),
        master: None,
        killer: Box::new(NoopKiller),
        steerer: Box::new(UnsupportedSteerer),
        switcher: Some(Box::new(RecordingSwitcher(Arc::clone(&calls)))),
        stderr_handle: None,
        child_wait: None,
        writer: Arc::new(Mutex::new(Box::new(std::io::sink()))),
        // Not an ACP session under test: no structured prompt route.
        image_sink: None,
        static_image_sink: None,
        reader_handle: None,
        coalesce_handle: None,
        runtime: Arc::clone(&runtime),
        mcp_session: None,
        exited: Arc::new(AtomicBool::new(false)),
        preserve_on_exit: Arc::new(AtomicBool::new(false)),
    };
    registry.inner.lock().expect("registry").insert(
        session_id.to_string(),
        RegistryEntry::Live(Box::new(session)),
    );

    registry
        .set_model(session_id, &owner, Some("claude-opus-5"), None)
        .expect("a provisional catalog must not reject a model");
    assert_eq!(calls.load(Ordering::Acquire), 1);

    runtime.store_claude_catalog(SessionEvent::SessionManifest {
        provider_id: Some("claude".to_string()),
        current_model_id: Some("claude-sonnet-5".to_string()),
        models: vec![devboule_protocol::SessionModel {
            model_id: "claude-sonnet-5".to_string(),
            name: "Claude Sonnet 5".to_string(),
            description: None,
            context_tokens: None,
            current_effort: Some("high".to_string()),
            efforts: Some(vec![devboule_protocol::SessionModelEffort {
                id: "high".to_string(),
                label: "High".to_string(),
                description: None,
                default: Some(true),
            }]),
        }],
        modes: None,
    });

    let error = registry
        .set_model(session_id, &owner, Some("claude-bogus-999"), None)
        .expect_err("unknown model must be rejected before the switcher");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("not in the current catalog"));
    assert_eq!(calls.load(Ordering::Acquire), 1);

    let error = registry
        .set_model(session_id, &owner, None, Some("bogus"))
        .expect_err("unknown effort must be rejected before the switcher");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("not supported"));
    assert_eq!(calls.load(Ordering::Acquire), 1);

    registry
        .set_model(
            session_id,
            &owner,
            Some("claude-sonnet-5[1m]"),
            Some("high"),
        )
        .expect("model variants must use the base model catalog");
    assert_eq!(calls.load(Ordering::Acquire), 2);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn invalid_session_mode_is_rejected_without_changing_the_manifest() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-set-mode", "process-set-mode");
    let session_id = "acp-set-mode";
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        registry.journal.clone(),
    ));
    runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: Some("test-agent".to_string()),
        current_model_id: None,
        models: Vec::new(),
        modes: Some(devboule_protocol::SessionModeStateView {
            current_mode_id: "ask".to_string(),
            available_modes: vec![devboule_protocol::SessionModeView {
                id: "ask".to_string(),
                name: "Always ask".to_string(),
                description: None,
            }],
        }),
    });
    let calls = Arc::new(AtomicU64::new(0));
    let metadata = Session {
        id: session_id.to_string(),
        workspace_id: None,
        cwd: None,
        kind: SessionKind::Acp,
        title: "Agent".to_string(),
        state: SessionState::Live { generation: 1 },
        elapsed_ms: Some(0),
        provider: Some("test-agent".to_string()),
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
    let session = PtySession {
        metadata,
        owner: owner.clone(),
        process_job: Arc::new(JobObject::new().expect("job")),
        master: None,
        killer: Box::new(NoopKiller),
        steerer: Box::new(UnsupportedSteerer),
        switcher: Some(Box::new(RecordingSwitcher(Arc::clone(&calls)))),
        stderr_handle: None,
        child_wait: None,
        writer: Arc::new(Mutex::new(Box::new(std::io::sink()))),
        // Fallback world: no structured route, so the mode rejection below
        // exercises the plain-text session, not the sink.
        image_sink: None,
        static_image_sink: None,
        reader_handle: None,
        coalesce_handle: None,
        runtime: Arc::clone(&runtime),
        mcp_session: None,
        exited: Arc::new(AtomicBool::new(false)),
        preserve_on_exit: Arc::new(AtomicBool::new(false)),
    };
    registry.inner.lock().expect("registry").insert(
        session_id.to_string(),
        RegistryEntry::Live(Box::new(session)),
    );

    let before = runtime.session_manifest();
    let error = registry
        .set_mode(session_id, &owner, "missing", &ConnHandle::new(1))
        .expect_err("unknown mode must be rejected before the switcher");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(runtime.session_manifest(), before);
    assert_eq!(calls.load(Ordering::Acquire), 0);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

fn tmp_registry_cache() -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let process_id = std::process::id();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("devboule-env-npx-{process_id}-{stamp}-{counter}"));
    std::fs::create_dir(&dir).expect("tmp dir");
    crate::registry::write_cache(&dir, crate::registry::TEST_REGISTRY_FIXTURE);
    dir
}

#[test]
fn env_provided_npx_wrapper_is_denied_on_session_create() {
    let dir = tmp_registry_cache();
    let state = ServerState::with_paths(
        "test-instance".to_string(),
        RuntimePaths::from_dir(dir.clone()),
    )
    .expect("state");
    let owner = test_owner("S-1-5-21-env-npx", "process-env-npx");
    let error = state
        .sessions
        .create_with_provider_env(
            &state,
            &owner,
            None,
            SessionKind::Acp,
            None,
            crate::profile_delivery::ProfileDelivery::none(),
            None,
            &None,
            Some("codex-acp"),
            &SessionCreateMeta::default(),
        )
        .expect_err("env npx create must fail");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(
        error.message,
        "provider 'codex-acp' is an npx wrapper; npx wrappers require explicit selection, the env override cannot launch them"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn env_provided_native_id_passes_env_reject_gate() {
    let dir = tmp_registry_cache();
    let paths = RuntimePaths::from_dir(&dir);
    SessionRegistry::reject_env_npx_wrapper("grok", Some(ProviderProvenance::Env), &paths)
        .expect("env native must pass the env gate");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn request_provided_npx_id_still_resolves_past_env_gate() {
    let dir = tmp_registry_cache();
    let paths = RuntimePaths::from_dir(&dir);
    SessionRegistry::reject_env_npx_wrapper("codex-acp", Some(ProviderProvenance::Request), &paths)
        .expect("request npx must pass the env gate");
    let agent = crate::provider_catalog::find_in_catalog(
        "codex-acp",
        &crate::registry::CdnRegistryFetch,
        &dir,
    )
    .expect("explicit npx id must still resolve in the catalog");
    assert_eq!(
        agent.origin,
        crate::provider_catalog::ProviderOrigin::NpxWrapper
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The ownership paths whose call site passes no requestor identity
/// (`&None`), and which therefore answer with the owner comparison alone.
///
/// It is not a hand-written claim: the test below derives it from
/// `peer_policy::peer_allows`, so a path may only be identity-free while
/// **no** role holding **any** capability set can reach the act it serves.
/// `set_mode` left this list in the slice-3 fix pass: `SessionSetMode` is
/// under `CAP_SEND`, so a paired device can reach it and the call site has
/// to carry the requestor's identity (§8b A3/A4/A5, H5).
const IDENTITY_FREE_PATHS: [&str; 2] = ["stop", "set_model"];

/// A connection that speaks for a paired device, as `server.rs` builds one.
pub(super) fn remote_conn(role: PeerRole, paired_by_user: Option<&str>) -> Arc<ConnHandle> {
    ConnHandle::with_conn_peer(
        7,
        None,
        Some(ConnPeer::Remote {
            device_id: "dev-phone".to_string(),
            role,
            paired_by_user: paired_by_user.map(str::to_string),
            binding: crate::peer_policy::TransportBinding::tailnet(
                "nstable",
                "node.tailnet.ts.net.",
                "user@example.com",
            ),
        }),
    )
}

/// One recovered entry owned by `owner_user` whose row carries `origin`.
/// The ownership check reads exactly these two facts.
fn transcript_entry(owner_user: &str, origin: SessionOrigin) -> RegistryEntry {
    let metadata = Session {
        id: "s.x.1".to_string(),
        workspace_id: None,
        cwd: None,
        kind: SessionKind::Acp,
        title: "Agent".to_string(),
        state: SessionState::Live { generation: 1 },
        elapsed_ms: None,
        provider: None,
        peer_session_id: None,
        created_at_ms: 1,
        origin,
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: devboule_protocol::UnattendedState::No,
        labels: Default::default(),
        resumable: false,
    };
    RegistryEntry::Transcript(Box::new(TranscriptSession {
        metadata,
        owner: test_owner(owner_user, "process-1"),
        runtime: Arc::new(SessionRuntime::new()),
    }))
}

/// Rewrite one live entry's stored origin, the way the create that made it
/// would have.
pub(super) fn set_entry_origin(registry: &SessionRegistry, id: &str, origin: SessionOrigin) {
    let mut map = registry.inner.lock().expect("registry");
    let entry = map.get_mut(id).expect("entry");
    entry.as_peer_visible_mut().expect("live").metadata.origin = origin;
}

/// Every ownership path this registry exposes, called for `id` by `owner`
/// over `conn`.
///
/// `stop` and `set_model` take no connection: `IDENTITY_FREE_PATHS` names
/// exactly those two, and the test below proves the capability gate denies
/// them to every role and capability set. Every other path is called with
/// the real connection, so the requestor's identity reaches
/// `check_user_owner` (§8b A3).
fn ownership_paths(
    registry: &SessionRegistry,
    id: &str,
    owner: &OwnerId,
    conn: &Arc<ConnHandle>,
) -> Vec<(&'static str, Result<(), WireError>)> {
    vec![
        (
            "send",
            registry.send_with_subscription(id, 1, "hi", &[], &[], owner, conn),
        ),
        ("stop", registry.stop(id, owner)),
        (
            "stop_with_subscription",
            registry.stop_with_subscription(id, 1, owner, conn),
        ),
        (
            "interrupt",
            registry.interrupt_with_subscription(id, 1, owner, conn),
        ),
        (
            "set_model",
            registry.set_model(id, owner, Some("model-x"), None),
        ),
        (
            "set_mode",
            registry.set_mode(id, owner, "acceptEdits", conn),
        ),
        (
            "resize",
            registry.resize_with_subscription(id, 1, 80, 24, owner, conn),
        ),
        (
            "attach",
            registry.attach_with_subscription(id, 1, None, conn, owner, false),
        ),
        (
            "claim",
            registry.claim_resize_with_subscription(id, 1, owner, conn),
        ),
        (
            "permission_respond",
            registry.permission_respond_with_subscription(
                PermissionResponse {
                    session_id: id,
                    request_id: "req-1",
                    outcome: PermissionOutcome::Deny,
                    option_id: None,
                },
                1,
                conn,
                owner,
            ),
        ),
        // This table models a wire frame for a remote caller: the sender is
        // deliberately absent here, so the row exercises the target's
        // ownership/origin decision without resolving the far namespace.
        (
            "agent_message_send",
            registry.agent_message_send_from_peer("s.far.source", id, "hi", owner, conn),
        ),
        // The one path that writes bytes rather than reading state: the
        // attachment is built by the same helper and the same PNG the send
        // tests use, so this row proves the ownership check and nothing
        // about attachment handling.
        (
            "deposit",
            registry
                .deposit(
                    id,
                    owner,
                    conn,
                    &attachment("photo.png", "image/png", &clean_png(0x0b)),
                )
                .map(|_| ()),
        ),
        // The read half of the deposit: a well-formed digest naming no
        // file, so this row proves the ownership check and answers about
        // the session rather than its absence.
        (
            "read_attachment",
            registry
                .read_attachment(
                    &devboule_protocol::AttachmentReference {
                        session_id: id.to_string(),
                        digest: "a".repeat(64),
                        stored_bytes: 0,
                    },
                    owner,
                    conn,
                )
                .map(|_| ()),
        ),
        // `close` is destructive, and this vector is evaluated eagerly and in
        // order: it goes last, or every row behind it would run against the
        // session it just removed, answer `SessionNotFound`, and satisfy the
        // positive loops' "not `Unauthorized`" for the wrong reason (HND-03).
        (
            "close",
            registry.close(id, owner, &conn.conn_peer).map(|_| ()),
        ),
    ]
}

/// §8b A3, one arm at a time: the local pipe is the owner's SID, a `Client`
/// peer is the person who paired it, and a `Daemon` peer is the origin
/// device. Every path into a session goes through this check.
#[test]
fn the_ownership_check_branches_on_role_and_origin() {
    let mine = test_owner("S-1-5-21-mine", "process-1");
    let local = ConnHandle::new(9);
    let owned = transcript_entry("S-1-5-21-mine", SessionOrigin::local());
    assert!(check_user_owner(&owned, &mine, &local.conn_peer).is_ok());

    let stranger = test_owner("S-1-5-21-other", "process-1");
    assert_eq!(
        check_user_owner(&owned, &stranger, &local.conn_peer)
            .err()
            .map(|error| error.code),
        Some(ErrorCode::Unauthorized)
    );

    // A `Client` peer speaks for the user who paired it, and only for that
    // user: its own answer is the paired SID, never another account.
    let client = remote_conn(PeerRole::Client, Some("S-1-5-21-mine"));
    assert!(check_user_owner(&owned, &mine, &client.conn_peer).is_ok());
    let other_client = remote_conn(PeerRole::Client, Some("S-1-5-21-other"));
    assert_eq!(
        check_user_owner(&owned, &mine, &other_client.conn_peer)
            .err()
            .map(|error| error.code),
        Some(ErrorCode::Unauthorized)
    );
    // A pairing row with no recorded user grants nothing.
    let unlabelled = remote_conn(PeerRole::Client, None);
    assert!(check_user_owner(&owned, &mine, &unlabelled.conn_peer).is_err());

    // A `Daemon` peer is scoped by the origin, not by the owner name.
    let daemon = remote_conn(PeerRole::Daemon, None);
    let own = test_owner("peer_dev-phone", "daemon");
    let own_origin = SessionOrigin::peer("dev-phone", PeerRole::Daemon);
    let its_own = transcript_entry("peer_dev-phone", own_origin.clone());
    assert!(check_user_owner(&its_own, &own, &daemon.conn_peer).is_ok());
    // Same owner, another origin device: refused.
    let another = transcript_entry(
        "peer_dev-phone",
        SessionOrigin::peer("dev-tablet", PeerRole::Daemon),
    );
    assert!(check_user_owner(&another, &own, &daemon.conn_peer).is_err());
    // A local session at this machine: refused whatever the owner says.
    let local_session = transcript_entry("peer_dev-phone", SessionOrigin::local());
    assert!(check_user_owner(&local_session, &own, &daemon.conn_peer).is_err());
    // And the owner comparison still holds: another user's session is out
    // even when the origin names this device.
    let someone_elses = transcript_entry("S-1-5-21-other", own_origin);
    assert!(check_user_owner(&someone_elses, &own, &daemon.conn_peer).is_err());
}

#[test]
fn agent_message_brakes_limit_rate_and_distinct_recipients() {
    let brakes: Arc<Mutex<MessageBrakeTable>> = Arc::new(Mutex::new(MessageBrakeTable::default()));
    let now = Instant::now();
    for recipient in ["agent-b", "agent-c", "agent-d"] {
        assert!(reserve_message_brake(&brakes, "agent-a", recipient, None, now).is_ok());
    }
    assert_eq!(
        reserve_message_brake(&brakes, "agent-a", "agent-e", None, now)
            .expect_err("fan-out brake")
            .code,
        ErrorCode::CapabilityNotSupported
    );
    assert!(
        reserve_message_brake(&brakes, "agent-a", "agent-b", None, now).is_ok(),
        "an existing recipient stays available until the source rate is exhausted"
    );
    // Five in flight is the brief's `max_outstanding_per_sender`: the fifth
    // is admitted, and the sixth is the one that is refused.
    assert!(reserve_message_brake(&brakes, "agent-a", "agent-b", None, now).is_ok());
    assert_eq!(
        reserve_message_brake(&brakes, "agent-a", "agent-b", None, now)
            .expect_err("rate brake")
            .code,
        ErrorCode::CapabilityNotSupported
    );
}

/// S4-03: both windows must let go. A slot a target never answered expires,
/// and the recipient set is a sliding window rather than a permanent one.
#[test]
fn an_expired_slot_is_released_and_a_recipient_leaves_the_window() {
    let brakes: Arc<Mutex<MessageBrakeTable>> = Arc::new(Mutex::new(MessageBrakeTable::default()));
    let now = Instant::now();
    for recipient in ["agent-b", "agent-c", "agent-d"] {
        reserve_message_brake(&brakes, "agent-a", recipient, None, now).expect("admitted");
    }
    assert!(
        reserve_message_brake(&brakes, "agent-a", "agent-e", None, now).is_err(),
        "three distinct recipients inside the window"
    );

    let later = now + Duration::from_secs(61);
    assert!(
        reserve_message_brake(&brakes, "agent-a", "agent-e", None, later).is_ok(),
        "once the window slides, a fourth recipient is admitted"
    );
    assert_eq!(
        agent_message_slots(&brakes, "agent-a"),
        1,
        "the three expired slots were dropped; only the new one is in flight"
    );
    assert_eq!(
        brakes.lock().expect("brakes")["agent-a"].sent_in_window,
        1,
        "the rate window is its own, and it restarted"
    );
}

/// A paired `Client` reaches the sessions of the person who paired it, and
/// no others, through every ownership path the registry exposes.
#[test]
fn a_client_peer_reaches_only_the_paired_users_sessions() {
    let (dir, registry, journal) = tmp_delete_registry();
    let mine = test_owner("S-1-5-21-mine", "process-1");
    let theirs = test_owner("S-1-5-21-theirs", "process-2");
    let mine_id = compose_session_id(&mine.session_token(), "mine01").expect("id");
    let theirs_id = compose_session_id(&theirs.session_token(), "theirs01").expect("id");
    insert_live(&registry, &mine_id, mine.clone());
    insert_live(&registry, &theirs_id, theirs.clone());
    let conn = remote_conn(PeerRole::Client, Some("S-1-5-21-mine"));

    for (path, result) in ownership_paths(&registry, &theirs_id, &mine, &conn) {
        assert_eq!(
            result.err().map(|error| error.code),
            Some(ErrorCode::Unauthorized),
            "{path} must refuse another user's session to a paired device"
        );
    }
    for (path, result) in ownership_paths(&registry, &mine_id, &mine, &conn) {
        assert_ne!(
            result.err().map(|error| error.code),
            Some(ErrorCode::Unauthorized),
            "{path} must let the paired user reach their own session"
        );
    }
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// HND-03: `ownership_paths` builds a `vec![...]`, so its rows are evaluated
/// eagerly and in order. `close` removes the session, so a `close` row that
/// is not last makes every row behind it answer `SessionNotFound` — which the
/// positive loops read as "not `Unauthorized`" and which therefore proves
/// nothing about ownership. This is the measurement that keeps `close` last:
/// every other row has to answer about the session itself.
#[test]
fn every_ownership_path_before_close_runs_on_a_live_session() {
    let (dir, registry, journal) = tmp_delete_registry();
    let mine = test_owner("S-1-5-21-mine", "process-1");
    let id = compose_session_id(&mine.session_token(), "live01").expect("id");
    insert_live(&registry, &id, mine.clone());
    let conn = remote_conn(PeerRole::Client, Some("S-1-5-21-mine"));

    let mut all: Vec<(&'static str, Option<ErrorCode>)> = Vec::new();
    let mut missing: Vec<&'static str> = Vec::new();
    for (path, result) in ownership_paths(&registry, &id, &mine, &conn) {
        let code = result.err().map(|error| error.code);
        all.push((path, code));
        // `close` is the row that removes the session, and
        // The agent-message row names a far source on purpose: the remote
        // caller owns that namespace, while this row decides whether the live
        // target is reachable. Nothing else is allowed to talk about a
        // session that is not there.
        if path != "close" && code == Some(ErrorCode::SessionNotFound) {
            missing.push(path);
        }
    }
    assert!(
        missing.is_empty(),
        "these rows answered `SessionNotFound` for a live session, so they prove \
         nothing about ownership: {missing:?} (all rows: {all:?})"
    );
    assert!(
        all.len() >= 10,
        "the loop has to walk the table, not a subset: {all:?}"
    );
    // Measured, not assumed (HND-03): with `close` last, the rows that used
    // to sit behind it answer about the session rather than about its
    // absence. `interrupt` and `set_mode` say the kind cannot do that, and
    // `deposit` succeeds — three answers that were all `SessionNotFound`
    // while the destructive row sat in the middle.
    let answer = |path: &str| {
        all.iter()
            .find(|(name, _)| *name == path)
            .expect("each row this test names is in the table")
            .1
    };
    assert_eq!(answer("interrupt"), Some(ErrorCode::InvalidRequest));
    assert_eq!(answer("set_mode"), Some(ErrorCode::InvalidRequest));
    assert_eq!(answer("deposit"), None);
    assert_ne!(
        answer("agent_message_send"),
        Some(ErrorCode::SessionNotFound),
        "the live target must be reached without resolving the far source"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// §8 R2: a `Daemon` peer reaches the sessions its own device created, and
/// `AgentMessageSend` now delivers into that peer-origin session from the
/// caller's far namespace. A different peer-origin session remains denied.
#[test]
fn a_daemon_peer_is_scoped_by_origin_and_delivers_agent_messages() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("peer_dev-phone", "daemon");
    let own_id = compose_session_id(&owner.session_token(), "peer01").expect("id");
    let other_id = compose_session_id(&owner.session_token(), "peer02").expect("id");
    let own_received = Arc::new(Mutex::new(Vec::new()));
    insert_live_agent_with_kind_and_writer(
        &registry,
        &own_id,
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::clone(&own_received))),
    );
    insert_live(&registry, &other_id, owner.clone());
    set_entry_origin(
        &registry,
        &own_id,
        SessionOrigin::peer("dev-phone", PeerRole::Daemon),
    );
    set_entry_origin(
        &registry,
        &other_id,
        SessionOrigin::peer("dev-tablet", PeerRole::Daemon),
    );
    let conn = remote_conn(PeerRole::Daemon, Some("peer_dev-phone"));

    for (path, result) in ownership_paths(&registry, &other_id, &owner, &conn) {
        if IDENTITY_FREE_PATHS.contains(&path) {
            // `stop` and `set_model` take no requestor identity, so the
            // origin cannot answer for them; a peer never reaches them
            // anyway (`peer_allows` denies both to both roles, pinned by
            // `every_identity_free_path_is_denied_to_a_peer`). What they
            // enforce is the owner comparison, which the Client test above
            // exercises with two real users.
            continue;
        }
        assert_eq!(
            result.err().map(|error| error.code),
            Some(ErrorCode::Unauthorized),
            "{path} must refuse a session whose origin is another device"
        );
    }
    for (path, result) in ownership_paths(&registry, &own_id, &owner, &conn) {
        if path == "agent_message_send" {
            assert!(
                result.is_ok(),
                "the origin device's agent message must still deliver: {result:?}"
            );
        }
        assert_ne!(
            result.err().map(|error| error.code),
            Some(ErrorCode::Unauthorized),
            "{path} must let the origin device reach its own session"
        );
    }
    let envelope = String::from_utf8(own_received.lock().expect("received").clone())
        .expect("the envelope is utf8");
    assert!(
        envelope.contains("from_agent: peer:dev-phone/s.far.source"),
        "the accepted delivery keeps the far sender in the peer namespace: {envelope}"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The ownership paths a frame reaches, or `None` when this harness serves
/// no path for it.
///
/// One arm per `ClientMessage` variant, **no `_` arm**: the same shape as
/// `peer_policy::matrix_row`, and the compiler is the proof — a new variant
/// does not build until it says whether it names a session. `path_requests`
/// below is derived from these answers, so the pairing is no longer written
/// by hand and a new frame cannot be dropped silently.
///
/// `SessionStop` answers with two paths because one frame has two registry
/// entry points (`stop`, `stop_with_subscription`) and the harness walks
/// both. `AgentMessageSend` answers with the path that serves it: the
/// registry's own `agent_message_send`.
///
/// Frames that name a session but reach no row here — `SessionDetach`,
/// `SessionDelete`, `SessionReportAgent`, `SessionResume`,
/// `SessionsPresence` — are `None` on purpose: this harness calls the
/// registry directly, and those five cannot be entered from it without the
/// daemon's `ServerState` or a live process. Their ownership checks are
/// covered where they live.
fn session_paths_of(request: &ClientMessage) -> Option<&'static [&'static str]> {
    match request {
        ClientMessage::SessionSend { .. } => Some(&["send"]),
        ClientMessage::SessionDeposit { .. } => Some(&["deposit"]),
        ClientMessage::SessionAttachmentRead { .. } => Some(&["read_attachment"]),
        ClientMessage::AgentMessageSend { .. } => Some(&["agent_message_send"]),
        ClientMessage::SessionStop { .. } => Some(&["stop", "stop_with_subscription"]),
        ClientMessage::SessionClose { .. } => Some(&["close"]),
        ClientMessage::SessionInterrupt { .. } => Some(&["interrupt"]),
        ClientMessage::SessionSetModel { .. } => Some(&["set_model"]),
        ClientMessage::SessionSetMode { .. } => Some(&["set_mode"]),
        ClientMessage::SessionResize { .. } => Some(&["resize"]),
        ClientMessage::SessionAttach { .. } => Some(&["attach"]),
        ClientMessage::SessionClaim { .. } => Some(&["claim"]),
        ClientMessage::SessionPermissionRespond { .. } => Some(&["permission_respond"]),
        ClientMessage::Hello(_) => None,
        ClientMessage::Ping { .. } => None,
        ClientMessage::Status { .. } => None,
        ClientMessage::DaemonDiagnostics { .. } => None,
        ClientMessage::Shutdown { .. } => None,
        ClientMessage::SessionCreate { .. } => None,
        ClientMessage::SessionDetach { .. } => None,
        ClientMessage::SessionReportAgent { .. } => None,
        ClientMessage::SessionsList { .. } => None,
        // Names no session of this daemon: it asks for the roster itself,
        // and its ownership rule is the pairing-user scope in
        // `peer_roster.rs`, covered there.
        ClientMessage::PeerAgentsList { .. } => None,
        ClientMessage::SessionsWatch { .. } => None,
        ClientMessage::SessionsUnwatch { .. } => None,
        ClientMessage::SessionsPresence { .. } => None,
        ClientMessage::SessionResume { .. } => None,
        ClientMessage::JournalUsage { .. } => None,
        ClientMessage::JournalRetentionGet { .. } => None,
        ClientMessage::JournalRetentionSet { .. } => None,
        ClientMessage::SessionDelete { .. } => None,
        ClientMessage::ProjectsList { .. } => None,
        ClientMessage::ProjectAdd { .. } => None,
        ClientMessage::WorkspacesList { .. } => None,
        ClientMessage::WorkspaceCreate { .. } => None,
        ClientMessage::WorkspaceDelete { .. } => None,
        ClientMessage::ProvidersList { .. } => None,
        ClientMessage::ProvidersRefresh { .. } => None,
        ClientMessage::ProviderUpdate { .. } => None,
        ClientMessage::Invoke { .. } => None,
        ClientMessage::DevicesList { .. } => None,
        ClientMessage::PairingStart { .. } => None,
        ClientMessage::PairingComplete { .. } => None,
        ClientMessage::PairingConfirm { .. } => None,
        ClientMessage::PeerRevoke { .. } => None,
        ClientMessage::PeerSetCaps { .. } => None,
        ClientMessage::ToolPolicyGet { .. } => None,
        ClientMessage::ToolPolicySet { .. } => None,
        ClientMessage::AgentProfilesGet { .. } => None,
        ClientMessage::AgentProfilesSet { .. } => None,
        ClientMessage::ProviderVocabularyGet { .. } => None,
        ClientMessage::DelegationGet { .. } => None,
        ClientMessage::DelegationSet { .. } => None,
    }
}

/// The frame each ownership path serves, derived from the closed
/// classification above and `peer_policy`'s pinned frame list: one row per
/// path, and every row is a frame that reaches it.
///
/// The order is the matrix's (`ClientMessage::name()` order), not
/// `ownership_paths` order — every consumer of this table filters or sorts,
/// and deriving the rows makes the order a property of the frame list
/// rather than a promise this table has to keep.
fn path_requests() -> Vec<(&'static str, ClientMessage)> {
    let mut rows = Vec::new();
    for frame in crate::peer_policy::tests::matrix_samples() {
        if let Some(paths) = session_paths_of(&frame) {
            for path in paths {
                rows.push((*path, frame.clone()));
            }
        }
    }
    rows
}

/// §8b A3/A4/A5, H5: the table and the closed classification cannot drift.
///
/// `session_paths_of` is a closed match over `ClientMessage` with no `_`
/// arm, so every variant has an explicit answer and the compiler is the
/// proof that none was omitted. The frame list is pinned next door, the way
/// `peer_policy` pins its matrix: one sample per variant, asserted against
/// `VARIANT_COUNT`. Walking that list through both halves is what this test
/// adds — for every variant, the rows in the table are exactly the paths the
/// classification names, or there are none at all.
#[test]
fn every_ownership_path_comes_from_the_frame_that_serves_it() {
    let samples = crate::peer_policy::tests::matrix_samples();
    assert_eq!(
        samples.len(),
        crate::peer_policy::tests::VARIANT_COUNT,
        "one sample per ClientMessage variant"
    );
    let rows = path_requests();
    for (path, request) in &rows {
        assert!(
            samples.iter().any(|frame| frame.name() == request.name()),
            "{path} serves a frame the matrix does not carry: {}",
            request.name()
        );
    }
    for frame in &samples {
        let served: Vec<&'static str> = rows
            .iter()
            .filter(|(_, request)| request.name() == frame.name())
            .map(|(path, _)| *path)
            .collect();
        match session_paths_of(frame) {
            Some(paths) => assert_eq!(
                served,
                paths.to_vec(),
                "{}: the table and the closed match must name the same paths",
                frame.name()
            ),
            None => assert!(
                served.is_empty(),
                "{} reaches no path here, so it must have no row: {served:?}",
                frame.name()
            ),
        }
    }
    let mut names: Vec<&'static str> = rows.iter().map(|(path, _)| *path).collect();
    names.sort_unstable();
    for skipped in IDENTITY_FREE_PATHS {
        assert!(
            names.contains(&skipped),
            "{skipped} is on the skip list but no ownership path serves it"
        );
    }
}

/// §8b A3/A4/A5, H5: the identity-free list is *derived*, not asserted.
///
/// For every ownership path, `peer_allows` answers whether a paired device
/// can reach the act at all — over both roles and the capability sets that
/// bracket the space (nothing, each single capability, all four). Two rules
/// follow from that pairing: a path a peer *can* reach must hand
/// `check_user_owner` the connection's identity (which `ownership_paths`
/// does for every path not listed as identity-free), and a path that
/// passes `&None` must be denied to every role holding anything. `set_mode`
/// sat on that list while `SessionSetMode` was under `CAP_SEND`, which is
/// exactly the drift this test refuses.
#[test]
fn every_identity_free_path_is_denied_to_a_peer() {
    use crate::peer_policy::{
        peer_allows, PeerDecision, CAP_ANSWER_PERMISSIONS, CAP_CREATE_SESSIONS, CAP_SEND, CAP_VIEW,
    };
    let cap = |name: &str| vec![name.to_string()];
    let capability_sets = [
        Vec::new(),
        cap(CAP_VIEW),
        cap(CAP_SEND),
        cap(CAP_ANSWER_PERMISSIONS),
        cap(CAP_CREATE_SESSIONS),
        vec![
            CAP_VIEW.to_string(),
            CAP_SEND.to_string(),
            CAP_ANSWER_PERMISSIONS.to_string(),
            CAP_CREATE_SESSIONS.to_string(),
        ],
    ];
    let reachable_by_a_peer = |request: &ClientMessage| {
        [PeerRole::Client, PeerRole::Daemon].iter().any(|role| {
            capability_sets
                .iter()
                .any(|caps| peer_allows(*role, caps, request) == PeerDecision::Allow)
        })
    };
    let mut reachable_variants: Vec<(&'static str, &'static str)> = Vec::new();
    for (path, request) in path_requests() {
        // Requirement one: an act a peer may perform is served by a path
        // that threads the connection. `stop_with_subscription` serves
        // `SessionStop`, which no capability opens — a path may take the
        // connection for an act no peer can reach, and that is what the
        // harness does. What must never happen is the opposite: an act a
        // peer *can* reach answered by a call site that passes `&None`.
        if reachable_by_a_peer(&request) {
            reachable_variants.push((path, request.name()));
            assert!(
                !IDENTITY_FREE_PATHS.contains(&path),
                "{path} serves {}, which a paired device can reach, and must pass \
                 `conn.conn_peer` into `check_user_owner`",
                request.name()
            );
        }
        // Requirement two: every path on the skip list is denied to every
        // role and every capability set, so `&None` is the whole truth
        // there. `stop` and `set_model` are the two that qualify.
        if IDENTITY_FREE_PATHS.contains(&path) {
            assert!(
                !reachable_by_a_peer(&request),
                "{path} takes `&None`, but a paired device can reach {}: the call site \
                 must carry the requestor's identity",
                request.name()
            );
        }
    }
    assert!(
        reachable_variants.len() >= IDENTITY_FREE_PATHS.len(),
        "the peer surface is larger than the skip list: {reachable_variants:?}"
    );
    // No path is missing from the table and none is on the skip list
    // without serving a path the harness knows.
    let names = ownership_paths_for_names();
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), names.len(), "one row per ownership path");
    for skipped in IDENTITY_FREE_PATHS {
        assert!(
            names.contains(&skipped),
            "{skipped} is on the skip list but no ownership path serves it"
        );
    }
}

/// The path names `ownership_paths` returns, without needing a registry:
/// read from the same table, so the two cannot drift apart.
fn ownership_paths_for_names() -> Vec<&'static str> {
    path_requests().into_iter().map(|(path, _)| path).collect()
}

/// §8 R2, item 7: an origin the journal could not read is `Unknown`, and a
/// `Daemon` peer is refused it exactly like a local session. The ownership
/// arm reads `kind == Peer` plus a device id, so "not known" names no
/// device and therefore grants nothing.
#[test]
fn a_daemon_peer_is_refused_an_unknown_origin_like_a_local_one() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("peer_dev-phone", "daemon");
    let local_id = compose_session_id(&owner.session_token(), "unkn01").expect("id");
    let unknown_id = compose_session_id(&owner.session_token(), "unkn02").expect("id");
    insert_live(&registry, &local_id, owner.clone());
    insert_live(&registry, &unknown_id, owner.clone());
    set_entry_origin(&registry, &local_id, SessionOrigin::local());
    set_entry_origin(
        &registry,
        &unknown_id,
        SessionOrigin {
            kind: SessionOriginKind::Unknown,
            device_id: None,
            role: None,
        },
    );
    let conn = remote_conn(PeerRole::Daemon, Some("peer_dev-phone"));
    for id in [&local_id, &unknown_id] {
        for (path, result) in ownership_paths(&registry, id, &owner, &conn) {
            if IDENTITY_FREE_PATHS.contains(&path) {
                continue;
            }
            if id == &local_id && path == "agent_message_send" {
                // Agent messages are the deliberate exception: a daemon peer
                // may write into a session local to this daemon, while every
                // other operation still follows the origin-scoped door.
                assert_ne!(
                    result.err().map(|error| error.code),
                    Some(ErrorCode::Unauthorized),
                    "agent messaging may reach a local target"
                );
                continue;
            }
            assert_eq!(
                result.err().map(|error| error.code),
                Some(ErrorCode::Unauthorized),
                "{path} must refuse session {id} to a daemon peer"
            );
        }
    }
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A live session of another provider kind, which is what the A4/A5 list
/// is keyed on: the guard reads the kind off the metadata.
fn set_entry_kind(registry: &SessionRegistry, id: &str, kind: SessionKind) {
    let mut map = registry.inner.lock().expect("registry");
    let entry = map.get_mut(id).expect("entry");
    entry.as_peer_visible_mut().expect("live").metadata.kind = kind;
}

/// §8b A4/A5 need two facts about a session a peer names: its provider kind
/// and the mode it is in *now*. This is the registry's answer to both, and
/// the case the whole rule turns on — a session sitting in a mode that
/// skips the permission prompt.
#[test]
fn a_session_advertising_a_prompt_skipping_mode_is_reported_by_the_guard() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-mine", "process-1");
    let id = compose_session_id(&owner.session_token(), "mode01").expect("id");
    insert_live(&registry, &id, owner.clone());

    // No manifest yet: the daemon cannot say which mode the session is in,
    // so nothing is refused on this ground (the request is still refused on
    // any other ground that applies).
    assert_eq!(
        registry.session_mode_guard(&id),
        Some((SessionKind::Terminal, None))
    );
    assert_eq!(registry.session_mode_guard("s.nobody.1"), None);

    set_entry_kind(&registry, &id, SessionKind::Claude);
    let runtime = registry.runtime(&id).expect("runtime");
    runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: Some("claude".to_string()),
        current_model_id: None,
        models: Vec::new(),
        modes: Some(devboule_protocol::SessionModeStateView {
            current_mode_id: "bypassPermissions".to_string(),
            available_modes: Vec::new(),
        }),
    });
    assert_eq!(
        registry.session_mode_guard(&id),
        Some((SessionKind::Claude, Some("bypassPermissions".to_string())))
    );

    // The composed decision: this session would run without asking, so a
    // paired device's request must not reach it.
    let Some((kind, mode)) = registry.session_mode_guard(&id) else {
        panic!("the guard must know the session");
    };
    assert!(crate::peer_policy::prompt_skipping_mode(
        kind,
        &mode.expect("a mode")
    ));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// What one scripted steer answers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SteerAnswer {
    /// The provider took the text.
    Steered,
    /// The provider cannot take a steer for this turn.
    Unavailable,
    /// The transport failed.
    Failed,
}

/// A steerer whose answer the test decides.
///
/// `on_steer` runs inside `steer_active_turn` — where a provider's write
/// happens — so a test can observe the turn-hold from within the admission.
pub(super) struct ScriptedSteerer {
    answer: SteerAnswer,
    calls: Arc<AtomicU64>,
    on_steer: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl ScriptedSteerer {
    pub(super) fn new(answer: SteerAnswer, calls: Arc<AtomicU64>) -> Self {
        Self {
            answer,
            calls,
            on_steer: None,
        }
    }

    fn observing(
        answer: SteerAnswer,
        calls: Arc<AtomicU64>,
        on_steer: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            answer,
            calls,
            on_steer: Some(on_steer),
        }
    }
}

impl SessionSteerer for ScriptedSteerer {
    fn steer_active_turn(
        &mut self,
        _text: &str,
        _turn: &mut TurnToken<'_>,
    ) -> Result<bool, WireError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        if let Some(on_steer) = &self.on_steer {
            on_steer();
        }
        match self.answer {
            SteerAnswer::Steered => Ok(true),
            SteerAnswer::Unavailable => Ok(false),
            SteerAnswer::Failed => Err(WireError::new(ErrorCode::Io, "synthetic steer failure")),
        }
    }

    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(Self {
            answer: self.answer,
            calls: Arc::clone(&self.calls),
            on_steer: self.on_steer.clone(),
        })
    }
}

/// The shape Pi's steerer has: the write happens under the caller's hold and
/// releases it, and the provider's answer only comes back afterwards, so the
/// turn can end in that window. `at_write` runs inside the hold (the write),
/// `at_reply` after it was released (the wait for the answer), which is how a
/// test puts an event between the two — the property the whole split exists
/// for.
struct RoundTripSteerer {
    calls: Arc<AtomicU64>,
    at_write: Arc<dyn Fn() + Send + Sync>,
    at_reply: Arc<dyn Fn() + Send + Sync>,
}

impl SessionSteerer for RoundTripSteerer {
    fn steer_active_turn(
        &mut self,
        _text: &str,
        turn: &mut TurnToken<'_>,
    ) -> Result<bool, WireError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        turn.write_then_release(|| (self.at_write)());
        (self.at_reply)();
        Ok(true)
    }

    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(Self {
            calls: Arc::clone(&self.calls),
            at_write: Arc::clone(&self.at_write),
            at_reply: Arc::clone(&self.at_reply),
        })
    }
}

/// A killer that records whether a refused steer fell back to an interrupt.
struct RecordingKiller(Arc<AtomicBool>);

impl RecordingKiller {
    fn new() -> (Self, Arc<AtomicBool>) {
        let interrupted = Arc::new(AtomicBool::new(false));
        (Self(Arc::clone(&interrupted)), interrupted)
    }
}

impl SessionKiller for RecordingKiller {
    fn kill(&mut self) {}

    fn interrupt(&mut self) {
        self.0.store(true, Ordering::Release);
    }

    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self(Arc::clone(&self.0)))
    }
}

/// One pending permission card, as a provider publishes it.
fn permission_card(tool_call_id: &str) -> SessionEvent {
    SessionEvent::PermissionRequest {
        tool_call_id: tool_call_id.to_string(),
        title: "Run command".to_string(),
        description: None,
        command: Some("cargo test".to_string()),
        args: None,
        cwd: None,
        env: None,
        options: vec![devboule_protocol::PermissionOption {
            option_id: "allow".to_string(),
            name: "Allow once".to_string(),
            kind: "allow_once".to_string(),
        }],
        origin: SessionOrigin::local(),
        create_agent: None,
    }
}

/// Attach an existing connection to a session, the way every attach does.
fn attach_conn_for_test(runtime: &Arc<SessionRuntime>, session_id: &str, conn: &ConnHandle) {
    let outcome = runtime
        .try_attach_with_replay(None, conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
}

/// One live agent session with the steer collaborators the test names, its
/// runtime, and (optionally) an attached observer.
fn steer_session(
    registry: &SessionRegistry,
    id: &str,
    owner: &OwnerId,
    kind: SessionKind,
    answer: SteerAnswer,
    calls: Arc<AtomicU64>,
    observer: Option<u64>,
) -> (Arc<SessionRuntime>, Arc<AtomicBool>, Arc<ConnHandle>) {
    steer_session_with_steerer(
        registry,
        id,
        owner,
        kind,
        Box::new(ScriptedSteerer::new(answer, calls)),
        observer,
    )
}

fn steer_session_with_steerer(
    registry: &SessionRegistry,
    id: &str,
    owner: &OwnerId,
    kind: SessionKind,
    steerer: Box<dyn SessionSteerer>,
    observer: Option<u64>,
) -> (Arc<SessionRuntime>, Arc<AtomicBool>, Arc<ConnHandle>) {
    let (killer, interrupted) = RecordingKiller::new();
    let runtime = insert_live_agent_with_turn_control(
        registry,
        id,
        owner.clone(),
        kind,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
        None,
        None,
        Box::new(killer),
        steerer,
    );
    let conn = match observer {
        Some(conn_id) => attach_live_agent_for_test(&runtime, id, conn_id),
        None => {
            let conn = ConnHandle::new(0);
            attach_conn_for_test(&runtime, id, &conn);
            conn
        }
    };
    (runtime, interrupted, conn)
}

/// The permission events one observer has been sent since it last drained.
fn resolved_cards(conn: &ConnHandle) -> Vec<String> {
    drain(conn)
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::PermissionResolved { tool_call_id, .. } => Some(tool_call_id),
            _ => None,
        })
        .collect()
}

/// S4-02, the race this fix is about: a turn ends while a steer is being
/// admitted. The runtime hands the steerer its token under the same lock the
/// `AgentFinished` transition takes, so the end of the turn cannot land
/// between the check and the write — the finish is blocked until the write
/// is done, and the text lands in the turn it was admitted for.
#[test]
fn a_finish_cannot_end_the_turn_between_a_steer_s_check_and_its_write() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-steer", "process-steer");
    let calls = Arc::new(AtomicU64::new(0));
    // `Barrier` and `AtomicBool` rather than channels: the steerer's hook is
    // stored as `Arc<dyn Fn() + Send + Sync>`, and a channel end is not
    // `Sync`.
    let met = Arc::new(Barrier::new(2));
    let attempting = Arc::new(AtomicBool::new(false));
    let runtime_slot: Arc<Mutex<Option<Arc<SessionRuntime>>>> = Arc::new(Mutex::new(None));
    let runtime_for_steer = Arc::clone(&runtime_slot);
    let met_inside = Arc::clone(&met);
    let attempting_inside = Arc::clone(&attempting);
    let on_steer: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        // Inside the admission: meet the finisher, let it get to its
        // publish, and then look at the runtime it is trying to move.
        met_inside.wait();
        for _ in 0..1000 {
            if attempting_inside.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            attempting_inside.load(Ordering::Acquire),
            "the finisher never reached its publish"
        );
        let runtime = runtime_for_steer
            .lock()
            .expect("runtime slot")
            .clone()
            .expect("the session is installed before the send");
        let turn = runtime.turn_counter();
        for _ in 0..20 {
            assert!(
                runtime.is_turn_active(turn),
                "the finish took the turn while the steer was writing"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            runtime.turn_counter(),
            turn,
            "the turn counter moved under an admitted steer"
        );
    });
    let (killer, interrupted) = RecordingKiller::new();
    let runtime = insert_live_agent_with_turn_control(
        &registry,
        "s.steer.race",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
        None,
        None,
        Box::new(killer),
        Box::new(ScriptedSteerer::observing(
            SteerAnswer::Steered,
            Arc::clone(&calls),
            on_steer,
        )),
    );
    runtime_slot
        .lock()
        .expect("runtime slot")
        .replace(Arc::clone(&runtime));
    let conn = attach_live_agent_for_test(&runtime, "s.steer.race", 71);
    runtime.begin_turn();
    let admitted_turn = runtime.turn_counter();

    let finisher_runtime = Arc::clone(&runtime);
    let met_outside = Arc::clone(&met);
    let attempting_outside = Arc::clone(&attempting);
    let finisher = std::thread::spawn(move || {
        met_outside.wait();
        // About to publish: from here on the only thing between this thread
        // and the turn transition is the turn-hold the steer is holding.
        attempting_outside.store(true, Ordering::Release);
        finisher_runtime.publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
    });

    registry
        .send_with_subscription_behavior(
            "s.steer.race",
            conn.id,
            "turn left instead",
            &[],
            &[],
            &owner,
            &conn,
            Some(ActiveTurnBehavior::Steer),
        )
        .expect("the steer is admitted for the running turn");
    finisher.join().expect("the finisher publishes");

    assert_eq!(calls.load(Ordering::Acquire), 1, "the provider was asked");
    assert!(
        !interrupted.load(Ordering::Acquire),
        "an accepted steer does not interrupt its own turn"
    );
    assert_eq!(
        runtime.turn_counter(),
        admitted_turn + 1,
        "the finish lands after the write, on the next turn"
    );
    assert!(!runtime.is_turn_active(admitted_turn));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The other half of the rule above, and the one a refactor is most likely
/// to break: the hold is released as soon as the provider's bytes are written,
/// and the provider's *answer* comes back later — so the turn the steer was
/// written into can end in that window.
///
/// The design says which event decides what, and this pins it: the *write*
/// decides the turn (it happens while the admitted turn is the running one,
/// under the hold, so it goes into turn N), and the *answer* decides
/// acceptance (`Ok(true)` is what records the steer). A finish that lands
/// between them therefore must not drop the steer and must not re-attribute
/// it: the text is already in the provider's turn N.
///
/// It fails if the write moves outside the hold (the finisher's transition
/// would land before the write, so the write would no longer be for the
/// running turn), if the hold is never released (the finisher could not
/// complete and `at_reply` would never see the end of the turn), or if an
/// accepted steer is dropped because a finish arrived in the window.
#[test]
fn a_finish_that_lands_after_the_write_and_before_the_reply_still_records_the_steer() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-window", "process-window");
    let calls = Arc::new(AtomicU64::new(0));
    // `Barrier`/`AtomicBool`/`AtomicU64`, not channels: the hooks are stored
    // as `Arc<dyn Fn() + Send + Sync>` and a channel end is not `Sync`.
    let met = Arc::new(Barrier::new(2));
    let attempting = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let admitted = Arc::new(AtomicU64::new(0));
    let written_under_hold = Arc::new(AtomicBool::new(false));
    let ended_before_reply = Arc::new(AtomicBool::new(false));
    let runtime_slot: Arc<Mutex<Option<Arc<SessionRuntime>>>> = Arc::new(Mutex::new(None));

    let at_write: Arc<dyn Fn() + Send + Sync> = {
        let met = Arc::clone(&met);
        let attempting = Arc::clone(&attempting);
        let admitted = Arc::clone(&admitted);
        let written_under_hold = Arc::clone(&written_under_hold);
        let runtime_slot = Arc::clone(&runtime_slot);
        Arc::new(move || {
            let runtime = runtime_slot
                .lock()
                .expect("runtime slot")
                .clone()
                .expect("the session is installed before the send");
            // The finisher is up and on its way to the transition.
            met.wait();
            for _ in 0..1000 {
                if attempting.load(Ordering::Acquire) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(
                attempting.load(Ordering::Acquire),
                "the finisher never reached its publish"
            );
            // This is the write, and it runs under the hold. The finisher is
            // already on its way to the transition, so every read below is a
            // chance for it to land: it cannot, because the hold is ours —
            // 20 reads over ~100 ms, which is what makes the claim about the
            // write's placement testable rather than assumed.
            let admitted = admitted.load(Ordering::Acquire);
            for _ in 0..20 {
                assert!(
                    runtime.is_turn_active(admitted),
                    "the finish landed before the write: the write is not under the hold"
                );
                assert_eq!(
                    runtime.turn_counter(),
                    admitted,
                    "the turn counter moved before the write"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            written_under_hold.store(true, Ordering::Release);
        })
    };
    let at_reply: Arc<dyn Fn() + Send + Sync> = {
        let finished = Arc::clone(&finished);
        let admitted = Arc::clone(&admitted);
        let ended_before_reply = Arc::clone(&ended_before_reply);
        let runtime_slot = Arc::clone(&runtime_slot);
        Arc::new(move || {
            let runtime = runtime_slot
                .lock()
                .expect("runtime slot")
                .clone()
                .expect("the session is installed before the send");
            // The write is done and the hold is released, so the finish that
            // was waiting on it lands now — before this answer.
            for _ in 0..2000 {
                if finished.load(Ordering::Acquire) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(
                finished.load(Ordering::Acquire),
                "the finish never completed: the hold was not released after the write"
            );
            let admitted = admitted.load(Ordering::Acquire);
            let ended = runtime.turn_counter() == admitted + 1 && !runtime.is_turn_active(admitted);
            ended_before_reply.store(ended, Ordering::Release);
            assert!(
                ended,
                "the finish did not end the turn the steer was written into"
            );
        })
    };

    let (killer, _interrupted) = RecordingKiller::new();
    let runtime = insert_live_agent_with_turn_control(
        &registry,
        "s.steer.window",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
        None,
        None,
        Box::new(killer),
        Box::new(RoundTripSteerer {
            calls: Arc::clone(&calls),
            at_write,
            at_reply,
        }),
    );
    runtime_slot
        .lock()
        .expect("runtime slot")
        .replace(Arc::clone(&runtime));
    let conn = attach_live_agent_for_test(&runtime, "s.steer.window", 81);
    journal
        .upsert_blocking(new_session_record(
            "s.steer.window",
            "S-1-5-21-window",
            None,
            SessionKind::Pi,
            "Agent",
        ))
        .expect("the journal knows the session");
    runtime.begin_turn();
    admitted.store(runtime.turn_counter(), Ordering::Release);

    // The finish that lands in the window between the write and the answer.
    let finisher_runtime = Arc::clone(&runtime);
    let met_outside = Arc::clone(&met);
    let attempting_outside = Arc::clone(&attempting);
    let finished_outside = Arc::clone(&finished);
    let finisher = std::thread::spawn(move || {
        met_outside.wait();
        attempting_outside.store(true, Ordering::Release);
        finisher_runtime.publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
        finished_outside.store(true, Ordering::Release);
    });

    registry
        .send_with_subscription_behavior(
            "s.steer.window",
            conn.id,
            "turn left instead",
            &[],
            &[],
            &owner,
            &conn,
            Some(ActiveTurnBehavior::Steer),
        )
        .expect("the write went into the running turn, so its answer is accepted for that turn");
    finisher.join().expect("the finisher publishes");

    assert!(written_under_hold.load(Ordering::Acquire));
    assert!(
        ended_before_reply.load(Ordering::Acquire),
        "the test must have put the finish between the write and the answer"
    );
    assert_eq!(
        calls.load(Ordering::Acquire),
        1,
        "the provider was asked once"
    );
    assert_eq!(
        runtime.turn_counter(),
        admitted.load(Ordering::Acquire) + 1,
        "exactly one turn ended: the one the steer was written into"
    );
    let echoes: Vec<String> = drain(&conn)
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::AgentUserMessage { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(
        echoes,
        vec!["turn left instead".to_string()],
        "the accepted steer is still recorded, against the turn it went into"
    );
    journal.flush().expect("flush the journal");
    let steered = journal
        .replay("s.steer.window")
        .expect("replay")
        .events
        .into_iter()
        .filter(|event| matches!(event, SessionEvent::Steered { .. }))
        .count();
    assert_eq!(steered, 1, "and journaled once, after that finish");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_turn_that_ended_before_admission_is_sent_as_a_plain_message() {
    // The other side of the same rule: with no turn to join, admission
    // refuses and the text goes the ordinary way — no steer, and no
    // interrupt either, because nothing is running to replace.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-steer", "process-steer");
    let calls = Arc::new(AtomicU64::new(0));
    let received = Arc::new(Mutex::new(Vec::new()));
    let (killer, interrupted) = RecordingKiller::new();
    let runtime = insert_live_agent_with_turn_control(
        &registry,
        "s.steer.idle",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::clone(&received))),
        None,
        None,
        Box::new(killer),
        Box::new(ScriptedSteerer::new(
            SteerAnswer::Steered,
            Arc::clone(&calls),
        )),
    );
    let conn = attach_live_agent_for_test(&runtime, "s.steer.idle", 72);
    runtime.begin_turn();
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );

    registry
        .send_with_subscription_behavior(
            "s.steer.idle",
            72,
            "a fresh task",
            &[],
            &[],
            &owner,
            &conn,
            Some(ActiveTurnBehavior::Steer),
        )
        .expect("the text is delivered as a plain send");
    assert_eq!(calls.load(Ordering::Acquire), 0, "nothing was steered");
    assert!(!interrupted.load(Ordering::Acquire));
    assert_eq!(
        &*received.lock().expect("received"),
        b"a fresh task",
        "the ordinary write happened"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_steer_the_provider_cannot_take_is_refused_for_a_paired_device() {
    // S4-01: a local caller keeps the interrupt-and-replace fallback. A
    // paired device does not get it, because interrupting the turn is the
    // act `SessionInterrupt` decides and no capability opens that to a peer.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-peer", "process-peer");
    let calls = Arc::new(AtomicU64::new(0));
    let (runtime, interrupted, local) = steer_session(
        &registry,
        "s.steer.fallback",
        &owner,
        SessionKind::Pi,
        SteerAnswer::Unavailable,
        Arc::clone(&calls),
        Some(73),
    );
    runtime.begin_turn();
    registry
        .send_with_subscription_behavior(
            "s.steer.fallback",
            73,
            "replace the turn",
            &[],
            &[],
            &owner,
            &local,
            Some(ActiveTurnBehavior::Steer),
        )
        .expect("a local caller falls back to interrupt-and-replace");
    assert!(
        interrupted.load(Ordering::Acquire),
        "the local fallback interrupts the running turn"
    );

    // The same request from a device paired to that user.
    let peer = remote_conn(PeerRole::Client, Some("S-1-5-21-peer"));
    attach_conn_for_test(&runtime, "s.steer.fallback", &peer);
    let error = registry
        .send_with_subscription_behavior(
            "s.steer.fallback",
            peer.id,
            "peer steer",
            &[],
            &[],
            &owner,
            &peer,
            Some(ActiveTurnBehavior::Steer),
        )
        .expect_err("a paired device's refused steer is an error");
    assert_eq!(error.code, ErrorCode::Unauthorized);
    assert_eq!(
        error.message,
        "this agent cannot take a steer and interrupting is not permitted for a paired device"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S4-06: cards are cancelled only once the provider has taken the text.
/// A steer that failed leaves the turn — and its cards — exactly as they
/// were, and the caller sees the failure.
#[test]
fn a_failed_steer_leaves_the_permission_cards_where_they_were() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-cards", "process-cards");
    let calls = Arc::new(AtomicU64::new(0));
    let (runtime, _interrupted, conn) = steer_session(
        &registry,
        "s.steer.cards",
        &owner,
        SessionKind::Pi,
        SteerAnswer::Failed,
        calls,
        Some(74),
    );
    let broker = runtime
        .permission_broker()
        .expect("the session has a broker");
    broker
        .register(51, permission_card("call-51"), &runtime)
        .expect("a card is pending");
    runtime.begin_turn();

    let error = registry
        .send_with_subscription_behavior(
            "s.steer.cards",
            74,
            "turn left",
            &[],
            &[],
            &owner,
            &conn,
            Some(ActiveTurnBehavior::Steer),
        )
        .expect_err("a failed steer is an error");
    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(
        broker.pending_len(),
        1,
        "the card is still the user's to answer"
    );
    assert!(resolved_cards(&conn).is_empty(), "nothing was cancelled");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_accepted_steer_cancels_the_pending_cards_once() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-cards", "process-cards");
    let calls = Arc::new(AtomicU64::new(0));
    let (runtime, _interrupted, conn) = steer_session(
        &registry,
        "s.steer.cards-ok",
        &owner,
        SessionKind::Pi,
        SteerAnswer::Steered,
        calls,
        Some(75),
    );
    let broker = runtime
        .permission_broker()
        .expect("the session has a broker");
    broker
        .register(52, permission_card("call-52"), &runtime)
        .expect("a card is pending");
    runtime.begin_turn();

    registry
        .send_with_subscription_behavior(
            "s.steer.cards-ok",
            75,
            "turn left",
            &[],
            &[],
            &owner,
            &conn,
            Some(ActiveTurnBehavior::Steer),
        )
        .expect("the provider took the steer");
    assert_eq!(broker.pending_len(), 0, "the card was cancelled");
    assert_eq!(
        resolved_cards(&conn),
        vec!["call-52".to_string()],
        "exactly one resolved card, and it is the one that was pending"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The other side of the cancel rule above: a steer the provider cannot take
/// must not take a permission card with it on the way out.
///
/// The cards belong to the turn that is still running, and a steer that never
/// reached the provider changes nothing about that turn. A local caller's
/// fallback may still interrupt — and cancelling then is the killer's
/// business, not the steer's — but a paired device's refusal has no interrupt
/// at all, so its card has to be there afterwards too.
///
/// This fails if `cancel_pending()` moves back in front of the steer: the card
/// would be gone, with a `PermissionResolved` emitted, in both halves.
#[test]
fn a_refused_steer_leaves_the_pending_cards_to_the_turn_that_is_still_running() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-cards-refused", "process-cards-refused");
    let calls = Arc::new(AtomicU64::new(0));
    let received = Arc::new(Mutex::new(Vec::new()));
    let (killer, interrupted) = RecordingKiller::new();
    let runtime = insert_live_agent_with_turn_control(
        &registry,
        "s.steer.cards-refused",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::clone(&received))),
        None,
        None,
        Box::new(killer),
        Box::new(ScriptedSteerer::new(
            SteerAnswer::Unavailable,
            Arc::clone(&calls),
        )),
    );
    let conn = attach_live_agent_for_test(&runtime, "s.steer.cards-refused", 82);
    let broker = runtime
        .permission_broker()
        .expect("the session has a broker");
    broker
        .register(62, permission_card("call-62"), &runtime)
        .expect("a card is pending");
    runtime.begin_turn();

    // The person at this machine: the steer is refused, so the fallback
    // interrupts — which is what cancels cards — and re-sends the text. The
    // refused steer cancelled nothing on its way out.
    registry
        .send_with_subscription_behavior(
            "s.steer.cards-refused",
            82,
            "replace the turn",
            &[],
            &[],
            &owner,
            &conn,
            Some(ActiveTurnBehavior::Steer),
        )
        .expect("a local caller falls back to interrupt-and-replace");
    assert!(
        interrupted.load(Ordering::Acquire),
        "the local fallback interrupts the running turn"
    );
    assert_eq!(
        &*received.lock().expect("received"),
        b"replace the turn",
        "the fallback re-sent the text to the provider"
    );
    assert_eq!(
        broker.pending_len(),
        1,
        "the refused steer cancelled no card; only the fallback's interrupt cancels"
    );
    assert!(
        resolved_cards(&conn).is_empty(),
        "no PermissionResolved was emitted for the refused steer"
    );

    // The same text from a device paired to that user: refused, with no
    // interrupt, and again with the card still pending afterwards.
    interrupted.store(false, Ordering::Release);
    let peer = remote_conn(PeerRole::Client, Some("S-1-5-21-cards-refused"));
    attach_conn_for_test(&runtime, "s.steer.cards-refused", &peer);
    let error = registry
        .send_with_subscription_behavior(
            "s.steer.cards-refused",
            peer.id,
            "peer steer",
            &[],
            &[],
            &owner,
            &peer,
            Some(ActiveTurnBehavior::Steer),
        )
        .expect_err("a paired device's refused steer is an error");
    assert_eq!(error.code, ErrorCode::Unauthorized);
    assert!(
        !interrupted.load(Ordering::Acquire),
        "the refusal does not fall back to interrupting"
    );
    assert_eq!(broker.pending_len(), 1, "the refusal cancelled no card");
    assert!(
        resolved_cards(&peer).is_empty(),
        "no PermissionResolved reached the paired device"
    );
    assert_eq!(
        calls.load(Ordering::Acquire),
        2,
        "both attempts asked the provider before giving up"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S4-07/S4-12: an accepted steer is echoed into the session's transcript
/// as the event every accepted input publishes, and journaled as `Steered`.
#[test]
fn an_accepted_steer_echoes_one_user_message_and_journals_one_steered_row() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-echo", "process-echo");
    // The journal is the audit trail: it has to know the session before it
    // can record anything for it, the same row a create writes.
    journal
        .upsert_blocking(new_session_record(
            "s.steer.echo",
            "S-1-5-21-echo",
            None,
            SessionKind::Pi,
            "Agent",
        ))
        .expect("the journal knows the session");
    let calls = Arc::new(AtomicU64::new(0));
    let (runtime, _interrupted, conn) = steer_session(
        &registry,
        "s.steer.echo",
        &owner,
        SessionKind::Pi,
        SteerAnswer::Steered,
        calls,
        Some(76),
    );
    runtime.begin_turn();
    registry
        .send_with_subscription_behavior(
            "s.steer.echo",
            76,
            "turn left instead",
            &[],
            &[],
            &owner,
            &conn,
            Some(ActiveTurnBehavior::Steer),
        )
        .expect("the steer is accepted");

    let echoes: Vec<(Option<String>, String, devboule_protocol::UserMessageKind)> = drain(&conn)
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::AgentUserMessage {
                message_id,
                text,
                message_kind,
                ..
            } => Some((message_id, text, message_kind)),
            _ => None,
        })
        .collect();
    assert_eq!(echoes.len(), 1, "one echo for the accepted steer");
    assert_eq!(
        echoes[0].1, "turn left instead",
        "and it is the steered text"
    );
    assert_eq!(
        echoes[0].2,
        devboule_protocol::UserMessageKind::Composer,
        "a steer is still the person's composer message"
    );
    let echo_message_id = echoes[0]
        .0
        .clone()
        .expect("the echo names the message it published");
    // A2-10: the journal row carries the *same* id as the echo, so the row
    // and the transcript message are one message rather than two that a
    // reader has to guess between.
    journal.flush().expect("flush the journal");
    let steered: Vec<Option<String>> = journal
        .replay("s.steer.echo")
        .expect("replay")
        .events
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::Steered { message_id, .. } => Some(message_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        steered,
        vec![Some(echo_message_id)],
        "one Steered row, carrying the echo's own message id"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S4-10: a steer is text only, and the refusal comes before any attachment
/// byte is planned, decoded, materialized or written.
#[test]
fn a_steer_with_an_attachment_is_refused_before_anything_decodes_it() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attach", "process-attach");
    let calls = Arc::new(AtomicU64::new(0));
    let (runtime, _interrupted, conn) = steer_session(
        &registry,
        "s.steer.attach",
        &owner,
        SessionKind::Pi,
        SteerAnswer::Steered,
        Arc::clone(&calls),
        Some(77),
    );
    runtime.begin_turn();
    let attachments = vec![attachment("photo.png", "image/png", b"not a real png")];
    let error = registry
        .send_with_subscription_behavior(
            "s.steer.attach",
            77,
            "look at this",
            &attachments,
            &[],
            &owner,
            &conn,
            Some(ActiveTurnBehavior::Steer),
        )
        .expect_err("a steer carries text only");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(
        error.message,
        "a steer carries text only; send attachments as a new message"
    );
    assert_eq!(
        calls.load(Ordering::Acquire),
        0,
        "nothing reached the provider"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S4-06/S4-09: after the provider has taken the text, a recording failure
/// is a degraded session — never an error the caller could retry into a
/// second steer.
#[test]
fn a_steer_the_provider_took_is_ok_even_when_its_echo_can_no_longer_be_recorded() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-degrade", "process-degrade");
    let calls = Arc::new(AtomicU64::new(0));
    let runtime_slot: Arc<Mutex<Option<Arc<SessionRuntime>>>> = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&runtime_slot);
    let on_steer: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        // The stream closes while the provider is taking the text, so the
        // echo and the audit row can no longer be recorded.
        if let Some(runtime) = slot.lock().expect("runtime slot").clone() {
            runtime.close_output();
        }
    });
    let (killer, _interrupted) = RecordingKiller::new();
    let runtime = insert_live_agent_with_turn_control(
        &registry,
        "s.steer.degrade",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
        None,
        None,
        Box::new(killer),
        Box::new(ScriptedSteerer::observing(
            SteerAnswer::Steered,
            Arc::clone(&calls),
            on_steer,
        )),
    );
    runtime_slot
        .lock()
        .expect("runtime slot")
        .replace(Arc::clone(&runtime));
    let conn = attach_live_agent_for_test(&runtime, "s.steer.degrade", 78);
    runtime.begin_turn();

    registry
        .send_with_subscription_behavior(
            "s.steer.degrade",
            78,
            "turn left",
            &[],
            &[],
            &owner,
            &conn,
            Some(ActiveTurnBehavior::Steer),
        )
        .expect("the provider took the text, so this is Ok whatever the journal says");
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert!(
        journal.is_session_degraded("s.steer.degrade"),
        "the unrecorded steer is surfaced as a degraded session"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The number of in-flight slots one sender is holding.
fn agent_message_slots(brakes: &Arc<Mutex<MessageBrakeTable>>, from_session: &str) -> usize {
    brakes
        .lock()
        .expect("brakes")
        .get(from_session)
        .map(|brake| brake.outstanding.len())
        .unwrap_or(0)
}

/// The number of recipients one sender's window is holding (A2-06).
fn agent_message_recipients(brakes: &Arc<Mutex<MessageBrakeTable>>, from_session: &str) -> usize {
    brakes
        .lock()
        .expect("brakes")
        .get(from_session)
        .map(|brake| brake.recipients.len())
        .unwrap_or(0)
}

/// How many senders the brake table still has an entry for. A sender with
/// nothing in flight and no recipient left must not keep one (A2-06).
fn agent_message_brake_entries(brakes: &Arc<Mutex<MessageBrakeTable>>) -> usize {
    brakes.lock().expect("brakes").len()
}

/// The hook id one slot currently holds armed, if any (S4-15).
fn agent_message_release_hook(
    brakes: &Arc<Mutex<MessageBrakeTable>>,
    from_session: &str,
    slot: u64,
) -> Option<u64> {
    brakes
        .lock()
        .expect("brakes")
        .get(from_session)
        .and_then(|brake| {
            brake
                .outstanding
                .iter()
                .find(|entry| entry.slot == slot)
                .and_then(|entry| entry.release.as_ref().map(|(_, hook)| *hook))
        })
}

/// Whether one slot is waiting on a boundary that has already arrived
/// (S4-10/S4-14).
fn agent_message_boundary_reached(
    brakes: &Arc<Mutex<MessageBrakeTable>>,
    from_session: &str,
    slot: u64,
) -> bool {
    brakes
        .lock()
        .expect("brakes")
        .get(from_session)
        .and_then(|brake| brake.outstanding.iter().find(|entry| entry.slot == slot))
        .is_some_and(|entry| entry.boundary_reached)
}

/// How many global sweeps the table has run (S4-16).
fn agent_message_sweep_count(brakes: &Arc<Mutex<MessageBrakeTable>>) -> u64 {
    brakes.lock().expect("brakes").sweeps
}

/// S4-03: the slot a message holds ends with the turn that message went into,
/// so a sender whose messages have been answered can send again.
///
/// Here the turn is already running, so that is the turn the two messages
/// join and the boundary their slots are keyed on. A message to an *idle*
/// target takes the other arm — a plain prompt, with the hook armed for the
/// turn that prompt starts — and
/// `a_finish_before_the_registration_sends_a_prompt_whose_turn_ends_the_slot`
/// pins that side, including the release.
#[test]
fn a_target_s_finished_turn_releases_the_sender_s_slots() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-brake", "process-brake");
    insert_live_agent_with_kind_and_writer(
        &registry,
        "s.msg.a",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    let target = insert_live_agent_with_kind_and_writer(
        &registry,
        "s.msg.b",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    // The turn the two messages join.
    target.begin_turn();
    let conn = ConnHandle::new(0);
    for _ in 0..2 {
        registry
            .agent_message_send("s.msg.a", "s.msg.b", "hello", &owner, &conn)
            .expect("delivered");
    }
    assert_eq!(agent_message_slots(&registry.message_brakes, "s.msg.a"), 2);

    // The joined turn ends: that is the boundary both slots are keyed on.
    target.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert_eq!(
        agent_message_slots(&registry.message_brakes, "s.msg.a"),
        0,
        "the turn end released the in-flight messages"
    );

    registry
        .agent_message_send("s.msg.a", "s.msg.b", "again", &owner, &conn)
        .expect("reuse after completion");
    assert_eq!(agent_message_slots(&registry.message_brakes, "s.msg.a"), 1);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_delivery_gives_the_sender_s_slot_back() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-brake", "process-brake");
    insert_live_agent_with_kind_and_writer(
        &registry,
        "s.msg.a",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    // The target's writer refuses the write: the message is in flight
    // nowhere, so it must not hold a slot until a turn end that will never
    // come for it.
    insert_live_agent_with_kind_and_writer(
        &registry,
        "s.msg.b",
        owner.clone(),
        SessionKind::Pi,
        Box::new(FailingWriter),
    );
    let conn = ConnHandle::new(0);
    let error = registry
        .agent_message_send("s.msg.a", "s.msg.b", "hello", &owner, &conn)
        .expect_err("the target refuses the write");
    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(
        agent_message_slots(&registry.message_brakes, "s.msg.a"),
        0,
        "a failed delivery holds no slot"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A2-05: the boundary (the target's turn ending) and the delivery returning
/// are two different moments, and a slot is over only when both have passed.
///
/// Releasing it at the boundary hands the sender back a place it has not
/// given up yet: the message is still being written, and the next send then
/// leaves on top of the cap this count exists to keep. The two functions the
/// send path calls are the ones driven here.
#[test]
fn an_admitted_message_still_counts_until_its_delivery_returns() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-inflight", "process-inflight");
    for id in ["s.msg.a", "s.msg.b"] {
        insert_live_agent_with_kind_and_writer(
            &registry,
            id,
            owner.clone(),
            SessionKind::Pi,
            Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
        );
    }
    let target = registry.runtime("s.msg.b").expect("the target runtime");
    // The turn is running, and it is the turn this admission is for (S4-03).
    target.begin_turn();
    let admission = reserve_message_brake(
        &registry.message_brakes,
        "s.msg.a",
        "s.msg.b",
        Some((&target, target.turn_counter())),
        Instant::now(),
    )
    .expect("admitted");
    assert!(
        admission.steered_into_turn,
        "the running turn is the one this message joined"
    );
    assert_eq!(agent_message_slots(&registry.message_brakes, "s.msg.a"), 1);

    // The turn ends while the delivery is still in flight.
    target.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert_eq!(
        agent_message_slots(&registry.message_brakes, "s.msg.a"),
        1,
        "the boundary alone does not give the slot back: the delivery has not returned"
    );

    // The delivery returns, and only now is the slot over.
    finish_message_delivery(&registry.message_brakes, "s.msg.a", admission.slot, true);
    assert_eq!(
        agent_message_slots(&registry.message_brakes, "s.msg.a"),
        0,
        "the slot ends when the boundary and the delivery have both passed"
    );
    assert_eq!(
        agent_message_recipients(&registry.message_brakes, "s.msg.a"),
        1,
        "and the recipient stays in its window: that brake is time-based (S4-01)"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S4-01: the three-recipient window is the fan-out brake, and it is *time*
/// based. Releasing every slot — each one by the turn it joined ending — must
/// not hand the sender a fresh place to reach a fourth agent inside the
/// window; only the window ageing out does that.
#[test]
fn the_recipient_window_survives_its_slots_ending() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-window", "process-window");
    insert_live_agent_with_kind_and_writer(
        &registry,
        "s.msg.a",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    let recipients = ["s.msg.b", "s.msg.c", "s.msg.d"];
    let mut targets = Vec::new();
    for recipient in recipients {
        let target = insert_live_agent_with_kind_and_writer(
            &registry,
            recipient,
            owner.clone(),
            SessionKind::Pi,
            Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
        );
        // Every message joins a running turn, so every slot has a boundary.
        target.begin_turn();
        targets.push(target);
    }
    let now = Instant::now();
    for (target, recipient) in targets.iter().zip(recipients) {
        let admission = reserve_message_brake(
            &registry.message_brakes,
            "s.msg.a",
            recipient,
            Some((target, target.turn_counter())),
            now,
        )
        .expect("admitted");
        assert!(admission.steered_into_turn);
        finish_message_delivery(&registry.message_brakes, "s.msg.a", admission.slot, true);
    }
    assert_eq!(agent_message_slots(&registry.message_brakes, "s.msg.a"), 3);
    assert_eq!(
        agent_message_recipients(&registry.message_brakes, "s.msg.a"),
        3
    );

    // Every turn ends: every slot goes, and the window does not move with it.
    for target in &targets {
        target.publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
    }
    assert_eq!(
        agent_message_slots(&registry.message_brakes, "s.msg.a"),
        0,
        "the joins ended: no slot is in flight any more"
    );
    assert_eq!(
        agent_message_recipients(&registry.message_brakes, "s.msg.a"),
        3,
        "and the three recipients are still inside their window (S4-01)"
    );

    // A fourth recipient inside the window is refused, by that brake.
    let error = reserve_message_brake(
        &registry.message_brakes,
        "s.msg.a",
        "s.msg.e",
        None,
        now + Duration::from_secs(1),
    )
    .expect_err("the fan-out brake holds");
    assert!(
        error.message.contains("recipient limit"),
        "the refusal names the recipient window: {}",
        error.message
    );

    // Once the window ages out, the same send is admitted — as a plain
    // prompt, because there is no turn to join.
    let admission = reserve_message_brake(
        &registry.message_brakes,
        "s.msg.a",
        "s.msg.e",
        None,
        now + Duration::from_secs(62),
    )
    .expect("the window slid");
    assert!(
        !admission.steered_into_turn,
        "an idle target with no turn to join gets a prompt, not a steer"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A2-06: a target that closes takes every entry that names it with it.
#[test]
fn closing_a_target_forgets_the_message_brake_entries_that_name_it() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-closed-target", "process-closed-target");
    for id in ["s.msg.a", "s.msg.b"] {
        insert_live_agent_with_kind_and_writer(
            &registry,
            id,
            owner.clone(),
            SessionKind::Pi,
            Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
        );
    }
    let conn = ConnHandle::new(0);
    registry
        .agent_message_send("s.msg.a", "s.msg.b", "hello", &owner, &conn)
        .expect("delivered");
    assert_eq!(agent_message_slots(&registry.message_brakes, "s.msg.a"), 1);
    assert_eq!(
        agent_message_recipients(&registry.message_brakes, "s.msg.a"),
        1
    );

    // The target is a sender too, so closing it must take its own budget with
    // it: a closed session can never write again, and with a time-based
    // window nothing else would ever age that entry out (A2-06).
    reserve_message_brake(
        &registry.message_brakes,
        "s.msg.b",
        "s.msg.a",
        None,
        Instant::now(),
    )
    .expect("the target's own send is admitted");
    assert_eq!(agent_message_brake_entries(&registry.message_brakes), 2);

    // The target closes. Its turn can never end now, so its slots would
    // otherwise sit out the whole expiry holding the sender's budget.
    registry
        .close("s.msg.b", &owner, &None)
        .expect("the target closes");

    assert_eq!(
        agent_message_slots(&registry.message_brakes, "s.msg.a"),
        0,
        "the closed target's slot is gone"
    );
    assert_eq!(
        agent_message_recipients(&registry.message_brakes, "s.msg.a"),
        1,
        "but its entry in the window stays while it is young: closing a target is not a way to reach a fresh one (S4-01)"
    );
    assert_eq!(
        agent_message_brake_entries(&registry.message_brakes),
        2,
        "both windows are still remembered: the sender's, and the closed session's own (S4-12)"
    );
    // The entry is the window's, so it goes when the window ages out — the
    // path `prune` owns, tested with a moved clock in
    // `the_recipient_window_survives_its_slots_ending`.
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A2-05: the target check and the slot reservation are one critical section.
///
/// With the brake table held by the test, a send that has found its target
/// must still be holding the session map while it waits for its slot — the
/// two answers cannot be given at different times. A refactor that releases
/// the map before reserving (the check-then-reserve shape this replaces)
/// lets this lock go, and the test sees it.
#[test]
fn the_target_check_and_the_slot_reservation_are_one_critical_section() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-atomic", "process-atomic");
    for id in ["s.msg.a", "s.msg.b"] {
        insert_live_agent_with_kind_and_writer(
            &registry,
            id,
            owner.clone(),
            SessionKind::Pi,
            Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
        );
    }
    let conn = ConnHandle::new(0);
    let started = Arc::new(AtomicBool::new(false));

    // Hold the brake table: the send below can pass every check the session
    // map guards and still not have its slot.
    let held = registry.message_brakes.lock().expect("brakes");
    let sender = {
        let registry = registry.clone();
        let owner = owner.clone();
        let started = Arc::clone(&started);
        std::thread::spawn(move || {
            started.store(true, Ordering::Release);
            registry.agent_message_send("s.msg.a", "s.msg.b", "hello", &owner, &conn)
        })
    };

    let mut held_samples = 0;
    for _ in 0..200 {
        if !started.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        if registry.inner.try_lock().is_err() {
            held_samples += 1;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        held_samples >= 190,
        "the admission holds the session map while it waits for its slot \
         ({held_samples}/200 samples)"
    );

    drop(held);
    sender
        .join()
        .expect("the sender thread")
        .expect("the delivery completes once the slot is free");
    assert_eq!(agent_message_slots(&registry.message_brakes, "s.msg.a"), 1);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S4-02: a slot that expires gives its one-shot *hook* back too.
///
/// `prune` answers with the hooks of the slots it ended, and this pass's
/// predecessor dropped that answer on the floor: a target that never ends a
/// turn accumulated one callback per expired message, forever.
#[test]
fn an_expired_slot_unregisters_its_boundary_hook() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-hooks", "process-hooks");
    insert_live_agent_with_kind_and_writer(
        &registry,
        "s.msg.a",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    let target = insert_live_agent_with_kind_and_writer(
        &registry,
        "s.msg.b",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    // A running turn, so the admission arms a boundary hook on the target.
    target.begin_turn();
    let now = Instant::now();
    let admission = reserve_message_brake(
        &registry.message_brakes,
        "s.msg.a",
        "s.msg.b",
        Some((&target, target.turn_counter())),
        now,
    )
    .expect("admitted");
    assert!(admission.steered_into_turn);
    assert_eq!(
        target.turn_end_hook_count(),
        1,
        "the turn this message joined armed one boundary hook"
    );

    // The delivery never returned, so the slot expires; the next admission is
    // what prunes it, and the hook must go with it.
    let later = now + Duration::from_secs(61);
    reserve_message_brake(&registry.message_brakes, "s.msg.a", "s.msg.b", None, later)
        .expect("the second message is admitted once the first expired");
    assert_eq!(
        target.turn_end_hook_count(),
        0,
        "the expired slot's hook was unregistered (S4-02); the idempotent second admission armed none"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S4-03: a finish that lands between the caller's look and the registration
/// is *observed*, so the message goes as a plain prompt — and that prompt's
/// turn is what ends the slot.
///
/// The caller's look and the finish are both explicit here: the look says a
/// turn is running, the finish takes it away, and only then does the message
/// arrive. With the snapshot deciding the *steer*, that message would be a
/// steer; with the runtime deciding — the check and the registration being one
/// step under the lock `finish_turn` takes — the answer is `None`, so the text
/// is a prompt and the one boundary hook is armed for the turn that prompt
/// starts, not for the turn that is gone.
#[test]
fn a_finish_before_the_registration_sends_a_prompt_whose_turn_ends_the_slot() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-race", "process-race");
    let calls = Arc::new(AtomicU64::new(0));
    let received = Arc::new(Mutex::new(Vec::new()));
    insert_live_agent_with_kind_and_writer(
        &registry,
        "s.msg.a",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    let (killer, _interrupted) = RecordingKiller::new();
    let target = insert_live_agent_with_turn_control(
        &registry,
        "s.msg.b",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::clone(&received))),
        None,
        None,
        Box::new(killer),
        Box::new(ScriptedSteerer::new(
            SteerAnswer::Steered,
            Arc::clone(&calls),
        )),
    );
    // The look the old code decided on: a turn is running.
    target.begin_turn();
    let snapshot = target.is_turn_active(target.turn_counter());
    assert!(snapshot, "the caller's look sees the running turn");

    // The finish lands before the registration the admission will make.
    target.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );

    let conn = ConnHandle::new(0);
    registry
        .agent_message_send("s.msg.a", "s.msg.b", "hello", &owner, &conn)
        .expect("the message is delivered");

    assert_eq!(
        calls.load(Ordering::Acquire),
        0,
        "the turn ended before the write: the text goes as a prompt, not a steer"
    );
    assert!(
        !received.lock().expect("received").is_empty(),
        "the plain prompt was delivered"
    );
    assert_eq!(
        target.turn_end_hook_count(),
        1,
        "one boundary hook, armed for the turn the prompt starts (S4-03)"
    );
    assert_eq!(
        agent_message_slots(&registry.message_brakes, "s.msg.a"),
        1,
        "and the slot holds until that boundary"
    );

    // The turn that prompt started ends: that is the boundary the slot was
    // armed for, and it goes there.
    target.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert_eq!(
        agent_message_slots(&registry.message_brakes, "s.msg.a"),
        0,
        "the prompt's turn ending released the slot"
    );
    assert_eq!(target.turn_end_hook_count(), 0, "and the hook is one shot");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S4-03 at the reservation itself: the answer to "is the turn the caller
/// checked still running" is the one the slot is booked with.
///
/// With the snapshot deciding, this reservation reports
/// `steered_into_turn == true` for a turn that is over and arms a boundary for
/// it on top of the prompt's; with the runtime deciding, it reports `false` and
/// arms exactly one hook — the one the plain prompt's turn ends on.
#[test]
fn a_turn_that_ended_before_the_registration_is_not_joined() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-reserve-race", "process-reserve-race");
    let target = insert_live_agent_with_kind_and_writer(
        &registry,
        "s.msg.b",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    target.begin_turn();
    let expected = target.turn_counter();
    assert!(target.is_turn_active(expected), "the caller's look");

    // The finish lands between the caller's look and the registration.
    target.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );

    let admission = reserve_message_brake(
        &registry.message_brakes,
        "s.msg.a",
        "s.msg.b",
        Some((&target, expected)),
        Instant::now(),
    )
    .expect("admitted");

    assert!(
        !admission.steered_into_turn,
        "the turn the caller checked is over: this message is a prompt"
    );
    assert_eq!(
        target.turn_end_hook_count(),
        1,
        "one boundary hook (the prompt's turn), and none for the turn that is gone"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S4-10: the turn can end between the boundary registration and the delivery.
/// The delivery then writes a plain prompt — and that prompt's turn is the
/// boundary the slot has to end on, not the turn that is gone.
///
/// The gap is entered through the test-only hook that runs between the
/// admission and the delivery; the fallback and the bookkeeping the test then
/// asserts on are the production ones.
#[test]
fn a_turn_that_ends_before_the_delivery_keeps_the_slot_until_the_prompt_ends() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-s410", "process-s410");
    let calls = Arc::new(AtomicU64::new(0));
    let received = Arc::new(Mutex::new(Vec::new()));
    insert_live_agent_with_kind_and_writer(
        &registry,
        "s.msg.a",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    let (killer, _interrupted) = RecordingKiller::new();
    let target = insert_live_agent_with_turn_control(
        &registry,
        "s.msg.b",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::clone(&received))),
        None,
        None,
        Box::new(killer),
        Box::new(ScriptedSteerer::new(
            SteerAnswer::Steered,
            Arc::clone(&calls),
        )),
    );
    // The turn the message is admitted into, and whose end the admission's
    // hook fires on.
    target.begin_turn();
    let finishing = Arc::clone(&target);
    registry.set_agent_message_after_admission_hook(Arc::new(move || {
        finishing.publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
    }));

    registry
        .agent_message_send("s.msg.a", "s.msg.b", "hello", &owner, &ConnHandle::new(0))
        .expect("the message is delivered");

    assert_eq!(
        calls.load(Ordering::Acquire),
        0,
        "the turn was over by the time the delivery looked: a prompt, not a steer"
    );
    assert!(
        !received.lock().expect("received").is_empty(),
        "the plain prompt was written"
    );
    assert_eq!(
        agent_message_slots(&registry.message_brakes, "s.msg.a"),
        1,
        "a delivery that succeeded does not retire the slot whose prompt is still running (S4-10)"
    );
    assert_eq!(
        target.turn_end_hook_count(),
        1,
        "exactly one boundary hook: the one that re-keyed the slot onto the prompt's turn"
    );

    // The turn that prompt started ends: now, and only now, the slot is over.
    target.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert_eq!(
        agent_message_slots(&registry.message_brakes, "s.msg.a"),
        0,
        "the prompt's turn ending released the slot"
    );
    assert_eq!(target.turn_end_hook_count(), 0, "and its hook is one shot");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S4-11: a session that closes takes its own outstanding slots with it — and
/// their hooks off the targets they were armed on, including a target this
/// close does not even name.
#[test]
fn closing_a_sender_unregisters_the_hooks_of_its_other_messages() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-s411", "process-s411");
    for id in ["s.msg.x", "s.msg.b"] {
        insert_live_agent_with_kind_and_writer(
            &registry,
            id,
            owner.clone(),
            SessionKind::Pi,
            Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
        );
    }
    let unrelated = registry.runtime("s.msg.b").expect("the unrelated target");
    unrelated.begin_turn();
    let admission = reserve_message_brake(
        &registry.message_brakes,
        "s.msg.x",
        "s.msg.b",
        Some((&unrelated, unrelated.turn_counter())),
        Instant::now(),
    )
    .expect("admitted");
    assert!(
        admission.steered_into_turn,
        "the message joined a running turn"
    );
    assert_eq!(
        unrelated.turn_end_hook_count(),
        1,
        "the message armed one hook on its target"
    );

    registry
        .close("s.msg.x", &owner, &None)
        .expect("the sender closes");

    assert_eq!(
        agent_message_slots(&registry.message_brakes, "s.msg.x"),
        0,
        "the closed sender's slot is gone"
    );
    assert_eq!(
        unrelated.turn_end_hook_count(),
        0,
        "and the hook it had armed on a target this close never names went with it (S4-11)"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S4-12: closing and resuming the same session id does not buy a fresh
/// recipient window.
#[test]
fn a_closed_and_resumed_sender_keeps_its_recipient_window() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-s412", "process-s412");
    insert_live_agent_with_kind_and_writer(
        &registry,
        "s.msg.a",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    let now = Instant::now();
    for recipient in ["s.msg.b", "s.msg.c", "s.msg.d"] {
        reserve_message_brake(&registry.message_brakes, "s.msg.a", recipient, None, now)
            .expect("admitted");
    }
    assert_eq!(
        agent_message_recipients(&registry.message_brakes, "s.msg.a"),
        3
    );

    // The session closes and comes back under the same id, inside the window.
    registry
        .close("s.msg.a", &owner, &None)
        .expect("the sender closes");

    let error = reserve_message_brake(
        &registry.message_brakes,
        "s.msg.a",
        "s.msg.e",
        None,
        now + Duration::from_secs(1),
    )
    .expect_err("the window is not reset by a close and a resume (S4-12)");
    assert!(
        error.message.contains("recipient limit"),
        "the refusal names the recipient window: {}",
        error.message
    );

    // Once the window has aged out, the same send is admitted.
    reserve_message_brake(
        &registry.message_brakes,
        "s.msg.a",
        "s.msg.e",
        None,
        now + Duration::from_secs(62),
    )
    .expect("the window slid");
    assert_eq!(
        agent_message_slots(&registry.message_brakes, "s.msg.a"),
        1,
        "the resumed session has one outstanding message, not a fresh window"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S4-14: the admitted turn can end and another can start before the delivery
/// looks. Steering into the turn that is running is the right delivery; the
/// slot's boundary has to follow the text into it.
#[test]
fn a_slot_that_enters_a_newer_turn_keeps_exactly_one_boundary() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-s414", "process-s414");
    let calls = Arc::new(AtomicU64::new(0));
    insert_live_agent_with_kind_and_writer(
        &registry,
        "s.msg.a",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    let (killer, _interrupted) = RecordingKiller::new();
    let target = insert_live_agent_with_turn_control(
        &registry,
        "s.msg.b",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
        None,
        None,
        Box::new(killer),
        Box::new(ScriptedSteerer::new(
            SteerAnswer::Steered,
            Arc::clone(&calls),
        )),
    );
    // Turn 1 is the turn the admission registers its boundary against.
    target.begin_turn();
    let admitted = target.turn_counter();
    // Between the admission and the delivery: turn 1 ends, turn 2 starts.
    let starting = Arc::clone(&target);
    registry.set_agent_message_after_admission_hook(Arc::new(move || {
        starting.publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
        starting.begin_turn();
    }));

    registry
        .agent_message_send("s.msg.a", "s.msg.b", "hello", &owner, &ConnHandle::new(0))
        .expect("the message is delivered");

    assert_ne!(
        target.turn_counter(),
        admitted,
        "another turn is running by the time the delivery writes"
    );
    assert_eq!(
        calls.load(Ordering::Acquire),
        1,
        "the delivery steered into the turn that is running, as it should"
    );
    assert_eq!(
        agent_message_slots(&registry.message_brakes, "s.msg.a"),
        1,
        "and its slot followed the text into that turn (S4-14)"
    );
    assert_eq!(
        target.turn_end_hook_count(),
        1,
        "with exactly one live boundary"
    );

    // Turn 2 ends: the turn the text entered is what releases the slot.
    target.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert_eq!(
        agent_message_slots(&registry.message_brakes, "s.msg.a"),
        0,
        "the turn it entered released it"
    );
    assert_eq!(target.turn_end_hook_count(), 0, "and the hook is one shot");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S4-15: the callback of a hook that has been replaced is a no-op.
///
/// `fire_turn_end_hooks` invokes a drained callback outside the hook lock, so
/// the old boundary can arrive after the delivery re-keyed the slot. Without the
/// id check it would take the *new* hook, unregister it and mark the slot
/// reached — leaving a slot whose turn is still running with no boundary at all.
#[test]
fn a_replaced_boundary_callback_leaves_the_new_hook_alone() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-s415", "process-s415");
    let target = insert_live_agent_with_kind_and_writer(
        &registry,
        "s.msg.b",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    target.begin_turn();
    let admission = reserve_message_brake(
        &registry.message_brakes,
        "s.msg.a",
        "s.msg.b",
        Some((&target, target.turn_counter())),
        Instant::now(),
    )
    .expect("admitted");
    assert!(admission.steered_into_turn);
    let stale_hook =
        agent_message_release_hook(&registry.message_brakes, "s.msg.a", admission.slot)
            .expect("the admitted boundary is armed");

    // The delivery re-keys the slot: its admitted turn is gone, the text enters
    // another one.
    let slot_ref = MessageSlotRef {
        brakes: &registry.message_brakes,
        brake_key: "s.msg.a",
        slot: admission.slot,
        admitted_turn_id: admission.expected_turn_id,
    };
    rearm_message_slot_boundary(&slot_ref, &target);
    let live_hook = agent_message_release_hook(&registry.message_brakes, "s.msg.a", admission.slot)
        .expect("the re-arm armed a replacement");
    assert_ne!(live_hook, stale_hook, "the boundary is a new hook now");
    assert_eq!(target.turn_end_hook_count(), 1, "one hook, not two");

    // The old callback finally runs, as the drained-hook path allows: its cell
    // still holds the id it was armed as.
    let stale_cell = AtomicU64::new(stale_hook);
    boundary_reached_message_slot(
        &registry.message_brakes,
        "s.msg.a",
        admission.slot,
        &stale_cell,
    );

    assert!(
        !agent_message_boundary_reached(&registry.message_brakes, "s.msg.a", admission.slot),
        "the stale callback did not mark the slot's boundary reached (S4-15)"
    );
    assert_eq!(
        target.turn_end_hook_count(),
        1,
        "and it did not unregister the hook that replaced it"
    );
    assert_eq!(
        agent_message_release_hook(&registry.message_brakes, "s.msg.a", admission.slot),
        Some(live_hook),
        "the slot still holds the live hook"
    );

    // The live boundary still does its job.
    let live_cell = AtomicU64::new(live_hook);
    boundary_reached_message_slot(
        &registry.message_brakes,
        "s.msg.a",
        admission.slot,
        &live_cell,
    );
    assert!(
        agent_message_boundary_reached(&registry.message_brakes, "s.msg.a", admission.slot),
        "the live boundary still marks the slot reached"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S4-16: the global sweep runs at most once per rate window.
///
/// The sweep walks every other sender's entry under the single brakes lock, so
/// it is a per-window cost; the caller's own entry is still pruned on every
/// reserve, which is what its own braking needs.
#[test]
fn the_global_sweep_runs_once_per_window() {
    let brakes: Arc<Mutex<MessageBrakeTable>> = Arc::new(Mutex::new(MessageBrakeTable::default()));
    let now = Instant::now();
    // A sender whose window ran out long ago: only a sweep removes it.
    reserve_message_brake(
        &brakes,
        "s.msg.gone",
        "s.msg.b",
        None,
        now - Duration::from_secs(61),
    )
    .expect("seeded");
    let seeded = agent_message_sweep_count(&brakes);

    reserve_message_brake(&brakes, "s.msg.a", "s.msg.b", None, now).expect("admitted");
    let after_first = agent_message_sweep_count(&brakes);
    assert_eq!(
        after_first,
        seeded + 1,
        "the first reserve of a new window sweeps"
    );
    assert_eq!(
        agent_message_brake_entries(&brakes),
        1,
        "and the sender whose window ran out is gone"
    );

    reserve_message_brake(
        &brakes,
        "s.msg.a",
        "s.msg.c",
        None,
        now + Duration::from_millis(250),
    )
    .expect("admitted");
    assert_eq!(
        agent_message_sweep_count(&brakes),
        after_first,
        "a second reserve inside the window does not sweep again (S4-16)"
    );

    reserve_message_brake(
        &brakes,
        "s.msg.a",
        "s.msg.d",
        None,
        now + Duration::from_secs(62),
    )
    .expect("admitted");
    assert_eq!(
        agent_message_sweep_count(&brakes),
        after_first + 1,
        "and once the window has moved on it sweeps again"
    );
}
