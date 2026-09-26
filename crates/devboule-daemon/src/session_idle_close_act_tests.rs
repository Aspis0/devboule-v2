//! The act's own guards: what the sweep's re-read just before the close
//! catches. Each case lands its event at the expiry instant through the
//! registry's one-shot hook — the moment between the first weigh of the four
//! conditions and the re-read that precedes the act — so nothing here sleeps
//! or races the sweep it is testing.

use super::session_idle_close_profile_tests::{profile_child, set_profile_minutes};
use super::session_idle_close_tests::{
    armed, idle_state, linked_child, linked_creator, minutes, shut_down,
};
use super::tests::test_owner;
use super::*;

/// How many notices a child's own transcript has carried. The activity feed
/// is the publish path's own record of what it published, and it outlives the
/// close the way a held runtime handle does.
fn notices(runtime: &Arc<SessionRuntime>) -> usize {
    runtime
        .recent_activity(50)
        .iter()
        .filter(|mark| mark.kind == "session_notice")
        .count()
}

#[test]
fn a_message_admitted_at_the_expiry_instant_is_never_lost() {
    let (state, dir) = idle_state("recheck-send");
    let registry = &state.sessions;
    let owner = test_owner("idle-recheck-user", "idle-recheck-client");
    let creator = "idle-recheck-creator";
    linked_creator(registry, creator, &owner);
    let child = linked_child(registry, "idle-recheck-child", &owner, creator);

    // The admission lands exactly where the race lives: after the sweep's
    // first weigh of the conditions, before the re-read that precedes the act.
    let admitted = Arc::new(Mutex::new(false));
    let target = child.clone();
    let flag = Arc::clone(&admitted);
    registry.set_idle_close_before_act_hook(Box::new(move |sender: &SessionRegistry| {
        reserve_message_brake(
            &sender.message_brakes,
            "idle-recheck-sender",
            &target,
            None,
            Instant::now(),
        )
        .expect("the message is admitted");
        *flag.lock().expect("flag") = true;
    }));

    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(30)),
        0,
        "the re-read under the map lock sees the admission"
    );
    assert!(
        *admitted.lock().expect("flag"),
        "the fixture admitted the message at the instant it set out to"
    );
    assert!(
        registry
            .inner
            .lock()
            .expect("registry")
            .contains_key(&child),
        "the child stays: an accepted message is never torn down"
    );
    assert!(
        registry
            .message_brakes
            .lock()
            .expect("brakes")
            .values()
            .any(|brake| brake
                .outstanding
                .iter()
                .any(|message| message.to_session == child)),
        "the sender's slot still counts the message the sweep did not close over"
    );
    shut_down(&state, &dir);
}

#[test]
fn turning_the_timer_off_at_the_expiry_instant_wins() {
    let (state, dir) = idle_state("recheck-off");
    let registry = &state.sessions;
    let owner = test_owner("idle-off-user", "idle-off-client");
    let creator = "idle-off-creator";
    linked_creator(registry, creator, &owner);
    let child = profile_child(&state, "recheck", &owner, creator, Some(30));

    // The human ticks "never" in settings at the very instant the spell runs
    // out — after the sweep has already read thirty minutes once.
    let editor = Arc::clone(&state);
    registry.set_idle_close_before_act_hook(Box::new(move |_registry: &SessionRegistry| {
        set_profile_minutes(&editor, "p-recheck", Some(0));
    }));

    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(armed(registry, &child), Some(start));
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(30)),
        0,
        "the switch off wins over a spell that has already run out"
    );
    assert!(
        registry
            .inner
            .lock()
            .expect("registry")
            .contains_key(&child),
        "the child the human just spared is still there"
    );
    assert_eq!(
        armed(registry, &child),
        None,
        "and the spell is cleared, not merely paused"
    );
    shut_down(&state, &dir);
}

#[test]
fn a_close_that_refuses_publishes_the_notice_once() {
    let (state, dir) = idle_state("recheck-latch");
    let registry = &state.sessions;
    let owner = test_owner("idle-latch-user", "idle-latch-client");
    let creator = "idle-latch-creator";
    linked_creator(registry, creator, &owner);
    let child = linked_child(registry, "idle-latch-child", &owner, creator);
    let child_runtime = registry.child_view(&child).expect("live child").1;

    // The close is made to refuse: the entry's owner changes at the instant
    // the act runs, so `close`'s own owner check fails — the notice and the
    // creator's envelope are already out by then.
    let target = child.clone();
    registry.set_idle_close_before_act_hook(Box::new(move |entry: &SessionRegistry| {
        let mut map = entry.inner.lock().expect("registry");
        let live = map
            .get_mut(&target)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("live entry");
        live.owner = test_owner("idle-latch-other", "idle-latch-other-client");
    }));

    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(30)),
        0,
        "the close refused, so nothing was closed"
    );
    assert!(
        registry
            .inner
            .lock()
            .expect("registry")
            .contains_key(&child),
        "the refused close left the child in place"
    );
    assert_eq!(
        notices(&child_runtime),
        1,
        "the notice went out with the attempt that made it"
    );

    // The next sweep retries the close — and must not publish the notice a
    // second time for the same spell.
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(60)),
        1,
        "the second attempt closes the child"
    );
    assert!(!registry
        .inner
        .lock()
        .expect("registry")
        .contains_key(&child));
    assert_eq!(
        notices(&child_runtime),
        1,
        "the latch keeps the retry silent: one \"closed: idle\" line, ever"
    );
    shut_down(&state, &dir);
}
