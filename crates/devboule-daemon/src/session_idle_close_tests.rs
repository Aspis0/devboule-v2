//! The idle-close timer's decisions: the clock it keeps, the four conditions
//! it weighs, the reset focusing a child gives it, and the scope that keeps
//! human-started sessions out of all of it. Every instant is injected — the
//! sweep's own `now` argument is the clock — so nothing here sleeps. The
//! profile's own switch and minutes are `session_idle_close_profile_tests.rs`'s
//! topic, and the close's outputs (the notice, the creator's envelope, the
//! sentence a closed child answers a send with) are
//! `session_idle_close_delivery_tests.rs`'s.

use super::tests::{insert_child, insert_live_agent, park_card, test_owner};
use super::*;

/// A state of one's own: the sweep releases the slot of every child it closes
/// on the state its registry belongs to, so a bare registry cannot drive it.
/// `pub(super)` because the close's own outputs are tested in the sibling
/// module and need the same state.
pub(super) fn idle_state(label: &str) -> (Arc<ServerState>, std::path::PathBuf) {
    let state = ServerState::new(format!("idle-{label}"));
    let dir = state.sessions.runtime_dir().to_path_buf();
    (state, dir)
}

pub(super) fn shut_down(state: &Arc<ServerState>, dir: &std::path::Path) {
    if let Some(journal) = state.sessions.journal.clone() {
        journal.shutdown();
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// The row a spawn writes for a session: the notice appends to it, the close
/// marks it, and the send's refusal reads it back. A fixture without one
/// makes every journal write in the path under test fail on "no session with
/// that id", which is a fixture gap, not a finding.
pub(super) fn birth_row(
    registry: &SessionRegistry,
    id: &str,
    owner: &OwnerId,
    created_by: Option<&str>,
    display_name: Option<&str>,
) {
    let journal = registry.journal.clone().expect("journal");
    let mut record =
        crate::journal::new_session_record(id, &owner.user, None, SessionKind::Acp, "Agent");
    record.created_by = created_by.map(str::to_string);
    record.display_name = display_name.map(str::to_string);
    journal.create_session(record).expect("birth row");
}

/// A commissioned child in the shape production writes one: linked to its
/// creator, born with a journal row, carrying the display name its creator
/// and the send's answers see.
pub(super) fn linked_child(
    registry: &SessionRegistry,
    id: &str,
    owner: &OwnerId,
    creator: &str,
) -> String {
    insert_child(registry, id, owner.clone(), creator);
    registry.commit_agent_child_for_test(creator, id, true);
    birth_row(registry, id, owner, Some(creator), Some("child"));
    id.to_string()
}

/// A creator worth telling: live, with the row its transcript writes go to.
pub(super) fn linked_creator(registry: &SessionRegistry, creator: &str, owner: &OwnerId) {
    insert_live_agent(registry, creator, owner.clone());
    birth_row(registry, creator, owner, None, None);
}

/// One child's armed instant: the whole memory of its idle spell.
pub(super) fn armed(registry: &SessionRegistry, child: &str) -> Option<Instant> {
    registry
        .creations
        .lock()
        .expect("creations")
        .children
        .get(child)?
        .idle_close_since
}

pub(super) fn minutes(count: u64) -> Duration {
    Duration::from_secs(count * 60)
}

/// A linked child's runtime, without keeping the insert's handle around.
fn runtime_of(registry: &SessionRegistry, child: &str) -> Arc<SessionRuntime> {
    registry.child_view(child).expect("live child").1
}

// ------------------------------------------------------------------
// The clock: thirty minutes means thirty.
// ------------------------------------------------------------------

#[test]
fn a_child_closes_at_thirty_idle_minutes_and_not_at_twenty_nine() {
    let (state, dir) = idle_state("clock");
    let registry = &state.sessions;
    let owner = test_owner("idle-clock-user", "idle-clock-client");
    let creator = "idle-clock-creator";
    linked_creator(registry, creator, &owner);
    linked_child(registry, "idle-clock-child", &owner, creator);

    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(
        armed(registry, "idle-clock-child"),
        Some(start),
        "the first idle sweep starts the spell"
    );

    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(29)),
        0
    );
    assert_eq!(
        armed(registry, "idle-clock-child"),
        Some(start),
        "twenty-nine minutes is not thirty: the spell is untouched"
    );
    assert!(
        registry
            .inner
            .lock()
            .expect("registry")
            .contains_key("idle-clock-child"),
        "the child is still there at twenty-nine"
    );

    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(30)),
        1,
        "one child closed, and only one"
    );
    assert!(
        !registry
            .inner
            .lock()
            .expect("registry")
            .contains_key("idle-clock-child"),
        "the child is gone at thirty"
    );
    shut_down(&state, &dir);
}

// ------------------------------------------------------------------
// The four conditions: each one on its own holds the timer.
// ------------------------------------------------------------------

#[test]
fn a_turn_a_card_a_message_in_flight_and_a_viewer_each_hold_the_timer() {
    let (state, dir) = idle_state("conditions");
    let registry = &state.sessions;
    let owner = test_owner("idle-cond-user", "idle-cond-client");
    let creator = "idle-cond-creator";
    linked_creator(registry, creator, &owner);

    linked_child(registry, "idle-cond-control", &owner, creator);
    let working = linked_child(registry, "idle-cond-working", &owner, creator);
    runtime_of(registry, &working).begin_turn();
    let blocked = linked_child(registry, "idle-cond-blocked", &owner, creator);
    park_card(registry, &runtime_of(registry, &blocked), "idle-cond-card");
    linked_child(registry, "idle-cond-sending", &owner, creator);
    reserve_message_brake(
        &registry.message_brakes,
        "idle-cond-sender",
        "idle-cond-sending",
        None,
        Instant::now(),
    )
    .expect("the message is admitted");
    linked_child(registry, "idle-cond-watched", &owner, creator);
    registry
        .set_presence(7, &owner, Some("idle-cond-watched".to_string()), true)
        .expect("presence is recorded");

    let held = [
        "idle-cond-working",
        "idle-cond-blocked",
        "idle-cond-sending",
        "idle-cond-watched",
    ];
    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    for child in held {
        assert_eq!(armed(registry, child), None, "{child} never starts a spell");
    }
    assert!(
        runtime_of(registry, &working).is_running_turn(),
        "the turn is what holds the timer, and it still runs"
    );

    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(30)),
        1,
        "only the control child closes"
    );
    {
        let map = registry.inner.lock().expect("registry");
        for child in held {
            assert!(map.contains_key(child), "{child} survives its condition");
        }
        assert!(!map.contains_key("idle-cond-control"));
    }

    // One condition lifted — the card answered — and its spell starts now.
    runtime_of(registry, &blocked)
        .permission_broker()
        .expect("broker")
        .respond("idle-cond-card", PermissionOutcome::AllowOnce)
        .expect("the answer lands");
    let answered = start + minutes(31);
    assert_eq!(registry.sweep_idle_close_children(&state, answered), 0);
    assert_eq!(
        armed(registry, "idle-cond-blocked"),
        Some(answered),
        "the spell starts when the card that held it is answered"
    );
    assert_eq!(
        registry.sweep_idle_close_children(&state, answered + minutes(30)),
        1,
        "and closes thirty minutes later"
    );
    assert!(!registry
        .inner
        .lock()
        .expect("registry")
        .contains_key("idle-cond-blocked"));
    shut_down(&state, &dir);
}

// ------------------------------------------------------------------
// Presence: looking at a child starts its spell over.
// ------------------------------------------------------------------

#[test]
fn focusing_a_child_starts_its_idle_spell_over() {
    let (state, dir) = idle_state("presence");
    let registry = &state.sessions;
    let owner = test_owner("idle-focus-user", "idle-focus-client");
    let creator = "idle-focus-creator";
    linked_creator(registry, creator, &owner);
    linked_child(registry, "idle-focus-child", &owner, creator);

    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(armed(registry, "idle-focus-child"), Some(start));

    registry
        .set_presence(3, &owner, Some("idle-focus-child".to_string()), true)
        .expect("presence is recorded");
    assert_eq!(
        armed(registry, "idle-focus-child"),
        None,
        "the viewer's focus clears the timer"
    );
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(30)),
        0,
        "and the sweep keeps it cleared while they look"
    );
    assert_eq!(armed(registry, "idle-focus-child"), None);

    // They look away: the spell is new, so the thirty minutes behind it are
    // not owed to the new one.
    registry.clear_presence(3);
    let away = start + minutes(31);
    assert_eq!(registry.sweep_idle_close_children(&state, away), 0);
    assert_eq!(armed(registry, "idle-focus-child"), Some(away));
    assert_eq!(
        registry.sweep_idle_close_children(&state, away + minutes(29)),
        0
    );
    assert!(
        registry
            .inner
            .lock()
            .expect("registry")
            .contains_key("idle-focus-child"),
        "the old spell's minutes do not count against the new one"
    );
    assert_eq!(
        registry.sweep_idle_close_children(&state, away + minutes(30)),
        1
    );
    shut_down(&state, &dir);
}

// ------------------------------------------------------------------
// Scope: a session nobody commissioned from here is not a candidate.
// ------------------------------------------------------------------

#[test]
fn a_human_started_session_is_never_visited() {
    let (state, dir) = idle_state("human");
    let registry = &state.sessions;
    let owner = test_owner("idle-human-user", "idle-human-client");
    let creator = "idle-human-creator";
    linked_creator(registry, creator, &owner);
    let human = insert_live_agent(registry, "idle-human-solo", owner.clone());
    birth_row(registry, "idle-human-solo", &owner, None, None);
    linked_child(registry, "idle-human-child", &owner, creator);

    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(30)),
        1,
        "the commissioned child closes"
    );
    {
        let map = registry.inner.lock().expect("registry");
        assert!(map.contains_key("idle-human-solo"), "the human's session");
        assert!(!map.contains_key("idle-human-child"));
    }
    assert!(
        human.recent_activity(10).is_empty(),
        "no notice was published on a session the sweep never looks at"
    );
    shut_down(&state, &dir);
}

// ------------------------------------------------------------------
// The slot: a close that holds one gives it back.
// ------------------------------------------------------------------

/// The sweep closes through the `close(..., &None)` road the app's own close
/// uses, and the release rides the same act — a missing one would leave the
/// idle-shutdown count above zero for a session that is already gone, and the
/// daemon would never arm its idle exit again.
#[test]
fn closing_a_child_for_idleness_gives_its_slot_back() {
    let (state, dir) = idle_state("slot");
    let registry = &state.sessions;
    let owner = test_owner("idle-slot-user", "idle-slot-client");
    let creator = "idle-slot-creator";
    linked_creator(registry, creator, &owner);
    linked_child(registry, "idle-slot-child", &owner, creator);
    assert!(state.session_started(), "the child's creation took a slot");
    assert_eq!(state.live_session_count(), 1);

    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(30)),
        1
    );
    assert_eq!(
        state.live_session_count(),
        0,
        "the closed child's slot came back with its close"
    );
    shut_down(&state, &dir);
}
