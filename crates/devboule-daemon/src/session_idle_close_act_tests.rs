//! The act's own guards: what the sweep's re-read just before the close
//! catches. Most cases land their event at the expiry instant through the
//! registry's one-shot hook — the moment between the first weigh of the
//! conditions and the section that decides — and the wire-send case gates
//! the provider's own pipe instead, so nothing here sleeps or races the
//! sweep it is testing.

use super::session_idle_close_profile_tests::{profile_child, set_profile_minutes};
use super::session_idle_close_tests::{
    armed, birth_row, idle_state, linked_child, linked_creator, minutes, shut_down,
};
use super::tests::{
    attach_live_agent_for_test, insert_live_agent_with_kind_and_writer, test_owner,
};
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
fn a_removal_that_is_refused_publishes_nothing_and_the_retry_tells_the_spell() {
    let (state, dir) = idle_state("recheck-refused");
    let registry = &state.sessions;
    let owner = test_owner("idle-latch-user", "idle-latch-client");
    let creator = "idle-latch-creator";
    linked_creator(registry, creator, &owner);
    let child = linked_child(registry, "idle-latch-child", &owner, creator);
    let child_runtime = registry.child_view(&child).expect("live child").1;

    // The removal is made to refuse: the entry's owner changes at the
    // instant the section runs, so its own owner check fails — and nothing
    // is published for a child this sweep did not take.
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
        "the removal was refused, so nothing was closed"
    );
    assert!(
        registry
            .inner
            .lock()
            .expect("registry")
            .contains_key(&child),
        "the refused removal left the child in place"
    );
    assert_eq!(
        notices(&child_runtime),
        0,
        "nothing is said about a child this sweep did not take"
    );

    // The next sweep retries — the owner now reads as the entry's own — and
    // that attempt is the one that tells the spell.
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(60)),
        1,
        "the retry removes the child"
    );
    assert!(!registry
        .inner
        .lock()
        .expect("registry")
        .contains_key(&child));
    assert_eq!(
        notices(&child_runtime),
        1,
        "the retry says it once: publish only after the removal"
    );
    shut_down(&state, &dir);
}

/// The interleaving the pairing exists for: another road takes the child
/// between this sweep's weigh and its act. The section finds nothing to take,
/// so the sweep must say nothing — the road that took the child reports its
/// own end, and a notice here would claim a close that never happened.
#[test]
fn a_child_taken_between_the_weigh_and_the_act_is_not_narrated() {
    let (state, dir) = idle_state("recheck-taken");
    let registry = &state.sessions;
    let owner = test_owner("idle-taken-user", "idle-taken-client");
    let creator = "idle-taken-creator";
    linked_creator(registry, creator, &owner);
    let child = linked_child(registry, "idle-taken-child", &owner, creator);
    let child_runtime = registry.child_view(&child).expect("live child").1;

    let target = child.clone();
    registry.set_idle_close_before_act_hook(Box::new(move |entry: &SessionRegistry| {
        let mut map = entry.inner.lock().expect("registry");
        map.remove(&target);
    }));

    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(30)),
        0,
        "there was nothing left to take"
    );
    assert!(!registry
        .inner
        .lock()
        .expect("registry")
        .contains_key(&child));
    assert_eq!(
        notices(&child_runtime),
        0,
        "the sweep publishes only for a child it took itself"
    );
    shut_down(&state, &dir);
}

/// A writer that reports the moment the daemon starts writing a prompt into
/// the provider's pipe, and then holds it open until the test lets go: the
/// window between the writer's resolution and the turn's start, made
/// deterministic.
struct GatedWriter {
    entered: Arc<(Mutex<bool>, std::sync::Condvar)>,
    release: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

impl Write for GatedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        {
            let (flag, cvar) = &*self.entered;
            let mut guard = flag.lock().expect("gate");
            *guard = true;
            cvar.notify_all();
        }
        {
            let (flag, cvar) = &*self.release;
            let mut guard = flag.lock().expect("gate");
            while !*guard {
                guard = cvar.wait(guard).expect("gate");
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The road that arms no brake: a human's prompt resolves its writer under
/// the map lock and then writes with no lock held, so the delivery marks
/// itself there instead. The section either sees the mark — the child stays —
/// or the send finds no child at all; never a prompt written into a session
/// that is already going.
#[test]
fn a_prompt_being_written_holds_the_child() {
    let (state, dir) = idle_state("wire-flight");
    let registry = &state.sessions;
    let owner = test_owner("idle-flight-user", "idle-flight-client");
    let creator = "idle-flight-creator";
    linked_creator(registry, creator, &owner);

    let entered = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let child = "idle-flight-child";
    let child_runtime = insert_live_agent_with_kind_and_writer(
        registry,
        child,
        owner.clone(),
        SessionKind::Acp,
        Box::new(GatedWriter {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }),
    );
    {
        let mut map = registry.inner.lock().expect("registry");
        let live = map
            .get_mut(child)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("live entry");
        live.metadata.created_by = Some(creator.to_string());
        live.metadata.display_name = Some("child".to_string());
    }
    registry.commit_agent_child_for_test(creator, child, true);
    birth_row(registry, child, &owner, Some(creator), Some("child"));

    let conn = attach_live_agent_for_test(&child_runtime, child, 91);
    let send_state = Arc::clone(&state);
    let send_owner = owner.clone();
    let send_conn = Arc::clone(&conn);
    let sender = std::thread::spawn(move || {
        send_state.sessions.send_with_subscription(
            child,
            91,
            "do the thing",
            &[],
            &[],
            &send_owner,
            &send_conn,
        )
    });
    {
        let (flag, cvar) = &*entered;
        let mut guard = flag.lock().expect("gate");
        while !*guard {
            guard = cvar.wait(guard).expect("gate");
        }
    }
    // The send holds its writer and is inside the write. Its subscription is
    // the only thing still attached to the child, and a tab that closes
    // mid-send is a tab gone — which is how the sweep sees this child the way
    // it would see one nobody is watching.
    registry.detach_conn(&conn);

    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(30)),
        0,
        "a prompt being written holds the child"
    );
    assert!(
        registry.inner.lock().expect("registry").contains_key(child),
        "and the child is still there for the write to finish in"
    );

    {
        let (flag, cvar) = &*release;
        *flag.lock().expect("gate") = true;
        cvar.notify_all();
    }
    let delivered = sender.join().expect("the send thread");
    assert!(
        delivered.is_ok(),
        "the send the sweep stood aside for goes through: {delivered:?}"
    );
    assert!(
        child_runtime.is_running_turn(),
        "the prompt it wrote is the turn that starts"
    );
    shut_down(&state, &dir);
}
