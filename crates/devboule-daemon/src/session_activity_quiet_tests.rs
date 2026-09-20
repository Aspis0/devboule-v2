//! The agent-activity and quiet-notice tests, moved whole out of
//! `session_tests.rs` lines 6813-7208 (at `f1c42d0`), the banner at their head
//! included: the derived headline telling working, blocked and idle apart while
//! a live hook row wins, the derived state and the published hook sharing one
//! session without fighting, the quiet notice firing once per spell and leaving
//! the child alone, a refused notice never steering and leaving the creator's
//! turn and cards alone, a failed delivery keeping the spell owed, a stranger's
//! session refused without saying which, and a resolved card re-arming the quiet
//! clock. Every line below is byte-identical to its text there apart from this
//! header; `SteerAnswer`, `ScriptedSteerer` and `ScriptedSteerer::new` are
//! promoted to `pub(super)` for this move, and the other fixtures come from the
//! provider's own imports.

use super::tests::{
    insert_child, insert_live_agent, insert_live_agent_with_turn_control, park_card, test_owner,
    tmp_delete_registry, NoopKiller, RecordingWriter, ScriptedSteerer, SteerAnswer,
};
use super::*;

// ------------------------------------------------------------------
// Agent activity: derived headline, hook coexistence, quiet notice.
// ------------------------------------------------------------------

fn activity_thought() -> SessionEvent {
    SessionEvent::AgentThought {
        message_id: None,
        text: "thinking".to_string(),
        parent_tool_use_id: None,
        spawn_depth: None,
    }
}

fn activity_report(
    seq: Option<u64>,
    state: AgentActivityState,
) -> crate::agent_report::AgentReport {
    crate::agent_report::AgentReport {
        source: "devboule:stub".to_string(),
        agent: "stub".to_string(),
        state,
        message: None,
        seq,
        agent_session_id: None,
        agent_session_path: None,
        session_start_source: None,
    }
}

#[test]
fn agent_activity_tells_working_blocked_and_idle_apart() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("activity-user", "activity-client");
    let creator = "activity-creator";
    insert_live_agent(&registry, creator, owner.clone());
    let working = insert_child(&registry, "activity-working", owner.clone(), creator);
    let blocked = insert_child(&registry, "activity-blocked", owner.clone(), creator);
    let idle = insert_child(&registry, "activity-idle", owner.clone(), creator);
    registry.commit_agent_child_for_test(creator, "activity-working", true);
    registry.commit_agent_child_for_test(creator, "activity-blocked", true);
    registry.commit_agent_child_for_test(creator, "activity-idle", true);
    working.begin_turn();
    working.publish_agent_event(activity_thought(), None);
    blocked.publish_agent_event(activity_thought(), None);
    blocked.begin_turn();
    park_card(&registry, &blocked, "card-activity");
    idle.publish_agent_event(activity_thought(), None);
    let working_answer = registry
        .agent_activity("activity-working", &owner, 10)
        .expect("working child reads");
    assert_eq!(working_answer["activity"], "working");
    assert!(working_answer["idleMs"].as_u64().is_some());
    assert!(working_answer["recent"]
        .as_array()
        .is_some_and(|recent| recent.iter().any(|mark| mark["kind"] == "agent_thought")));
    let blocked_answer = registry
        .agent_activity("activity-blocked", &owner, 10)
        .expect("blocked child reads");
    assert_eq!(blocked_answer["activity"], "blocked");
    let idle_answer = registry
        .agent_activity("activity-idle", &owner, 10)
        .expect("idle child reads");
    assert_eq!(idle_answer["activity"], "idle");
    let one_line = registry
        .agent_activity("activity-working", &owner, 1)
        .expect("bounded read");
    assert!(one_line["recent"]
        .as_array()
        .is_some_and(|recent| recent.len() <= 1));
    assert!(registry
        .agent_activity("activity-missing", &owner, 10)
        .is_err());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

#[test]
fn hook_and_derived_share_a_session_without_fighting() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("hook-user", "hook-client");
    let creator = "hook-creator";
    insert_live_agent(&registry, creator, owner.clone());
    let child = insert_child(&registry, "hook-child", owner.clone(), creator);
    registry.commit_agent_child_for_test(creator, "hook-child", true);
    assert!(child
        .accept_agent_report(activity_report(Some(5), AgentActivityState::Working))
        .expect("seq 5 applies"));
    assert!(!child
        .accept_agent_report(activity_report(Some(3), AgentActivityState::Idle))
        .expect("stale applies as false"));
    assert!(!child
        .accept_agent_report(activity_report(Some(5), AgentActivityState::Blocked))
        .expect("duplicate applies as false"));
    assert_eq!(
        child.hook_activity(),
        Some((AgentActivityState::Working, Some(5))),
        "the seq rule holds: a late hook cannot regress the hook state"
    );
    let answer = registry
        .agent_activity("hook-child", &owner, 10)
        .expect("reads");
    assert_eq!(
        answer["activity"], "idle",
        "no turn and no card derives idle"
    );
    assert_eq!(answer["hookActivity"], "working");
    assert_eq!(answer["hookSeq"], 5);
    child.begin_turn();
    let answer = registry
        .agent_activity("hook-child", &owner, 10)
        .expect("reads");
    assert_eq!(answer["activity"], "working", "the derived headline wins");
    assert_eq!(
        answer["hookActivity"], "working",
        "the hook rides beside it"
    );
    assert_eq!(
        child.hook_activity(),
        Some((AgentActivityState::Working, Some(5))),
        "reading the derived state never touches the hook map"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

#[test]
fn quiet_notice_fires_once_per_spell_and_leaves_the_child_alone() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("quiet-user", "quiet-client");
    let creator = "quiet-creator";
    insert_live_agent(&registry, creator, owner.clone());
    recording_writer_for(&registry, creator);
    let quiet = insert_child(&registry, "quiet-child", owner.clone(), creator);
    let blocked = insert_child(&registry, "quiet-blocked", owner.clone(), creator);
    let silent = insert_child(&registry, "quiet-silent", owner.clone(), creator);
    registry.commit_agent_child_for_test(creator, "quiet-child", true);
    registry.commit_agent_child_for_test(creator, "quiet-blocked", true);
    registry.commit_agent_child_for_test(creator, "quiet-silent", false);
    for runtime in [&quiet, &blocked, &silent] {
        runtime.begin_turn();
        runtime.publish_agent_event(activity_thought(), None);
        runtime.stream.lock().expect("stream").last_publish = Some(
            Instant::now() - crate::agent_activity::CHILD_QUIET_AFTER - Duration::from_secs(60),
        );
    }
    park_card(&registry, &blocked, "card-quiet");
    let feed_len = quiet.recent_activity(50).len();
    let now = Instant::now();
    assert!(
        registry.notify_quiet_child("quiet-child", now),
        "one notice for one spell"
    );
    assert!(
        !registry.notify_quiet_child("quiet-child", now),
        "never twice per spell"
    );
    assert_eq!(
        registry.sweep_quiet_children(now),
        0,
        "the sweep finds nothing new"
    );
    assert!(
        quiet.is_running_turn(),
        "the notice steers nothing: the turn runs on"
    );
    assert!(!quiet.permission_pending());
    assert!(quiet
        .stream
        .lock()
        .map(|stream| matches!(stream.disposition, Disposition::Running))
        .unwrap_or(false));
    assert_eq!(
        quiet.recent_activity(50).len(),
        feed_len,
        "no event is published by the notice"
    );
    assert!(
        !registry.notify_quiet_child("quiet-blocked", now),
        "a parked card has its own envelope"
    );
    assert_eq!(
        blocked.permission_broker().expect("broker").pending_len(),
        1
    );
    assert!(blocked.is_running_turn());
    assert!(
        !registry.notify_quiet_child("quiet-silent", now),
        "notifyOnFinish false stays silent"
    );
    quiet.publish_agent_event(activity_thought(), None);
    assert!(
        !registry.notify_quiet_child("quiet-child", Instant::now()),
        "movement re-arms to silence"
    );
    quiet.stream.lock().expect("stream").last_publish =
        Some(Instant::now() - crate::agent_activity::CHILD_QUIET_AFTER - Duration::from_secs(60));
    assert!(
        registry.notify_quiet_child("quiet-child", Instant::now()),
        "the next spell notifies again"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

/// A killer with ACP's exact interrupt contract: flag the call and release
/// the session's pending permission prompts, so a test observes the damage
/// a refused steer inflicts on the interrupted session's cards.
struct CancelBrokerKiller {
    broker: Arc<super::permission_broker::PermissionBroker>,
    interrupted: Arc<AtomicBool>,
}

impl SessionKiller for CancelBrokerKiller {
    fn kill(&mut self) {}
    fn interrupt(&mut self) {
        self.interrupted.store(true, Ordering::Release);
        self.broker.cancel_pending();
    }
    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self {
            broker: Arc::clone(&self.broker),
            interrupted: Arc::clone(&self.interrupted),
        })
    }
}

/// Swap one test session's writer for a recorder so daemon deliveries to it
/// succeed: a `FailingWriter` session cannot be told anything, which is the
/// F1 case, not the fixture default.
fn recording_writer_for(registry: &SessionRegistry, id: &str) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let mut map = registry.inner.lock().expect("registry");
    let live = map
        .get_mut(id)
        .and_then(RegistryEntry::as_peer_visible_mut)
        .expect("live session");
    *live.writer.lock().expect("writer") =
        Box::new(RecordingWriter(Arc::clone(&received))) as Box<dyn Write + Send>;
}

#[test]
fn a_quiet_notice_never_steers_and_leaves_the_creators_turn_and_cards_alone() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("quiet-nosteer-user", "quiet-nosteer-client");
    // Creator mid-turn with an ACP-shaped steerer (steer unavailable) and a
    // killer with ACP's interrupt contract, plus one parked card of its own.
    let steer_calls = Arc::new(AtomicU64::new(0));
    let interrupted = Arc::new(AtomicBool::new(false));
    let creator = insert_live_agent_with_turn_control(
        &registry,
        "quiet-nosteer-creator",
        owner.clone(),
        SessionKind::Acp,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))) as Box<dyn Write + Send>,
        None,
        None,
        Box::new(NoopKiller),
        Box::new(ScriptedSteerer::new(
            SteerAnswer::Unavailable,
            Arc::clone(&steer_calls),
        )),
    );
    {
        let broker = creator.permission_broker().expect("broker");
        let mut map = registry.inner.lock().expect("registry");
        let live = map
            .get_mut("quiet-nosteer-creator")
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("live creator");
        live.killer = Box::new(CancelBrokerKiller {
            broker,
            interrupted: Arc::clone(&interrupted),
        });
    }
    creator.begin_turn();
    park_card(&registry, &creator, "card-creator");
    // Quiet child: working, silent past the threshold.
    let child = insert_child(
        &registry,
        "quiet-nosteer-child",
        owner.clone(),
        "quiet-nosteer-creator",
    );
    registry.commit_agent_child_for_test("quiet-nosteer-creator", "quiet-nosteer-child", true);
    child.begin_turn();
    child.publish_agent_event(activity_thought(), None);
    child.stream.lock().expect("stream").last_publish =
        Some(Instant::now() - crate::agent_activity::CHILD_QUIET_AFTER - Duration::from_secs(60));
    assert!(
        registry.notify_quiet_child("quiet-nosteer-child", Instant::now()),
        "the spell is owed and the creator is reachable"
    );
    assert_eq!(
        steer_calls.load(Ordering::Acquire),
        0,
        "steer never attempted"
    );
    assert!(
        !interrupted.load(Ordering::Acquire),
        "no interrupt reached the creator"
    );
    assert!(creator.is_running_turn(), "the creator's turn still runs");
    assert_eq!(
        creator.permission_broker().expect("broker").pending_len(),
        1,
        "the creator's card is still pending"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

#[test]
fn a_failed_quiet_delivery_keeps_the_spell_owed() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("quiet-retry-user", "quiet-retry-client");
    // The link names a creator that was never inserted: every delivery fails.
    let child = insert_child(&registry, "quiet-retry-child", owner.clone(), "quiet-gone");
    registry.commit_agent_child_for_test("quiet-gone", "quiet-retry-child", true);
    child.begin_turn();
    child.publish_agent_event(activity_thought(), None);
    child.stream.lock().expect("stream").last_publish =
        Some(Instant::now() - crate::agent_activity::CHILD_QUIET_AFTER - Duration::from_secs(60));
    assert!(
        !registry.notify_quiet_child("quiet-retry-child", Instant::now()),
        "a lost notice consumes nothing"
    );
    // The creator appears: the same spell is still owed, not burnt.
    insert_live_agent(&registry, "quiet-gone", owner.clone());
    recording_writer_for(&registry, "quiet-gone");
    assert!(
        registry.notify_quiet_child("quiet-retry-child", Instant::now()),
        "the spell survived the failed delivery"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

#[test]
fn agent_activity_refuses_a_strangers_session_without_saying_which() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("activity-scope-user", "activity-scope-client");
    let stranger = test_owner("activity-stranger-user", "activity-stranger-client");
    insert_live_agent(&registry, "activity-mine", owner.clone());
    insert_live_agent(&registry, "activity-theirs", stranger.clone());
    assert!(registry.agent_activity("activity-mine", &owner, 10).is_ok());
    assert!(registry
        .agent_activity("activity-theirs", &owner, 10)
        .is_err());
    assert!(registry
        .agent_activity("activity-missing", &owner, 10)
        .is_err());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

#[test]
fn a_resolved_card_re_arms_the_quiet_clock() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("quiet-unblock-user", "quiet-unblock-client");
    let creator = "quiet-unblock-creator";
    insert_live_agent(&registry, creator, owner.clone());
    let child = insert_child(&registry, "quiet-unblock-child", owner.clone(), creator);
    registry.commit_agent_child_for_test(creator, "quiet-unblock-child", true);
    child.begin_turn();
    child.publish_agent_event(activity_thought(), None);
    park_card(&registry, &child, "card-unblock");
    // Backdate past the threshold while blocked: the clock reads stale, and
    // the test can see staleness, before anyone answers.
    child.stream.lock().expect("stream").last_publish =
        Some(Instant::now() - crate::agent_activity::CHILD_QUIET_AFTER - Duration::from_secs(60));
    let stale = child
        .activity_idle_at(Instant::now())
        .expect("a clock that reads");
    assert!(stale >= crate::agent_activity::CHILD_QUIET_AFTER);
    // The human answers: the resolution publish is movement, so the clock
    // restarts even though the turn never ended.
    child
        .permission_broker()
        .expect("broker")
        .respond("card-unblock", PermissionOutcome::AllowOnce)
        .expect("the answer lands");
    assert_eq!(child.permission_broker().expect("broker").pending_len(), 0);
    let fresh = child
        .activity_idle_at(Instant::now())
        .expect("a clock that reads");
    assert!(
        fresh < crate::agent_activity::CHILD_QUIET_AFTER,
        "resolution re-armed the clock: {fresh:?}"
    );
    assert!(
        !registry.notify_quiet_child("quiet-unblock-child", Instant::now()),
        "no instant notice after a human just unblocked the child"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}
