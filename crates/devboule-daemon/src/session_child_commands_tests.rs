//! The cancel road at the registry: the internal interrupt reaches the
//! provider adapter, the command resolves the child's parked cards as
//! interrupted, an idle child is untouched (Paseo's `success: false`), and a
//! child that is not the caller's is refused with the sentence that hides
//! whether it exists.

use super::tests::{insert_live_agent, test_owner};
use super::*;

/// A killer that only records the interrupt and never drains cards: a card
/// that disappears after this road ran was the command's own doing, not the
/// adapter's.
struct InterruptRecorder(Arc<AtomicBool>);

impl SessionKiller for InterruptRecorder {
    fn kill(&mut self) {}

    fn interrupt(&mut self) {
        self.0.store(true, Ordering::Release);
    }

    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self(Arc::clone(&self.0)))
    }
}

/// A registry with no journal behind it: the cancel road reads the live map
/// only, and a test that needs rows has its own fixtures.
fn registry() -> (std::path::PathBuf, SessionRegistry) {
    let dir = crate::test_dirs::test_temp_dir("devboule-c1a-cancel");
    let registry = SessionRegistry::new(RuntimePaths::from_dir(&dir), None);
    (dir, registry)
}

/// A live child of `creator` whose interrupt the test can watch: the insert
/// every helper goes through, with the recorder as its killer and the
/// `created_by` link the scope reads.
fn insert_child_with_recorder(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
    creator: &str,
    recorder: &Arc<AtomicBool>,
) -> Arc<SessionRuntime> {
    let runtime = tests::insert_live_agent_with_turn_control(
        registry,
        id,
        owner,
        SessionKind::Acp,
        Box::new(std::io::sink()),
        None,
        None,
        Box::new(InterruptRecorder(Arc::clone(recorder))),
        Box::new(session_items::UnsupportedSteerer),
    );
    {
        let mut map = registry.inner.lock().expect("registry");
        let live = map
            .get_mut(id)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("live entry");
        live.metadata.created_by = Some(creator.to_string());
        live.metadata.display_name = Some(format!("{id} display"));
    }
    runtime
}

#[test]
fn cancel_interrupts_a_running_turn_and_resolves_the_parked_cards() {
    let (dir, registry) = registry();
    let owner = test_owner("c1a-cancel-user", "c1a-cancel-client");
    insert_live_agent(&registry, "c1a-cancel-caller", owner.clone());
    let interrupted = Arc::new(AtomicBool::new(false));
    let child = insert_child_with_recorder(
        &registry,
        "c1a-cancel-child",
        owner.clone(),
        "c1a-cancel-caller",
        &interrupted,
    );
    child.begin_turn();
    let broker = child.permission_broker().expect("broker");
    broker
        .register(1, permission_broker::permission("card-cancel"), &child)
        .expect("the card parks");

    let cancelled = registry
        // By display name, the way the other child tools resolve: the name a
        // creation gave the child, not only its id.
        .interrupt_agent_child("c1a-cancel-caller", "c1a-cancel-child display")
        .expect("the interrupt runs");
    assert!(cancelled, "a running turn is what success: true means");
    assert!(
        interrupted.load(Ordering::Acquire),
        "the internal interrupt road reached the adapter"
    );
    assert_eq!(
        broker.pending_len(),
        0,
        "the child's parked card resolved as interrupted"
    );
    let live = registry
        .live_agent_entries(&owner)
        .expect("entries")
        .into_iter()
        .any(|entry| entry.session.id == "c1a-cancel-child");
    assert!(live, "the child is kept — cancel never kills");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancel_of_an_idle_child_reports_false_and_touches_nothing() {
    let (dir, registry) = registry();
    let owner = test_owner("c1a-idle-user", "c1a-idle-client");
    insert_live_agent(&registry, "c1a-idle-caller", owner.clone());
    let interrupted = Arc::new(AtomicBool::new(false));
    let child = insert_child_with_recorder(
        &registry,
        "c1a-idle-child",
        owner.clone(),
        "c1a-idle-caller",
        &interrupted,
    );
    let broker = child.permission_broker().expect("broker");
    broker
        .register(1, permission_broker::permission("card-idle"), &child)
        .expect("the card parks");

    let cancelled = registry
        .interrupt_agent_child("c1a-idle-caller", "c1a-idle-child")
        .expect("answered");
    assert!(
        !cancelled,
        "no turn running is success: false, not an error"
    );
    assert!(
        !interrupted.load(Ordering::Acquire),
        "nothing was interrupted without a turn"
    );
    assert_eq!(
        broker.pending_len(),
        1,
        "an idle child's cards stay parked for whoever answers them"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancel_refuses_a_child_that_is_not_the_callers() {
    let (dir, registry) = registry();
    let owner = test_owner("c1a-scope-user", "c1a-scope-client");
    let other = test_owner("c1a-scope-other-user", "c1a-scope-other-client");
    insert_live_agent(&registry, "c1a-scope-caller", owner.clone());
    // A stranger's child: the same creator id on another owner's row — the
    // owner scope answers before `created_by` can be read as a claim.
    insert_child_with_recorder(
        &registry,
        "c1a-scope-stranger",
        other,
        "c1a-scope-caller",
        &Arc::new(AtomicBool::new(false)),
    );
    // A same-owner session this caller did not create: the parent case.
    insert_live_agent(&registry, "c1a-scope-parent", owner.clone());

    let refusal = |target: &str| {
        registry
            .interrupt_agent_child("c1a-scope-caller", target)
            .expect_err("refused")
    };
    let stranger = refusal("c1a-scope-stranger");
    assert_eq!(stranger.code, ErrorCode::SessionNotFound);
    assert!(stranger.message.contains("none of your live children"));
    let parent = refusal("c1a-scope-parent");
    assert_eq!(parent.code, ErrorCode::SessionNotFound);
    assert!(
        parent.message.contains("none of your live children"),
        "a parent reads exactly like an invented id: {parent:?}"
    );
    let invented = refusal("c1a-scope-invented");
    assert_eq!(
        invented.message,
        "none of your live children is called 'c1a-scope-invented'"
    );
    assert_eq!(
        parent.message, "none of your live children is called 'c1a-scope-parent'",
        "the refusal's shape never says whether the id exists"
    );
    let itself = refusal("c1a-scope-caller");
    assert_eq!(itself.code, ErrorCode::InvalidRequest);
    assert!(itself.message.contains("not its own child"));
    let _ = std::fs::remove_dir_all(&dir);
}
