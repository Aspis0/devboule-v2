use super::*;
use crate::browser::test_support::registry_with;
use devboule_protocol::BrowserErrorCode;

fn held_by_someone(registry: &BrowserRegistry) -> impl Send {
    registry
        .guard_of("tab-1")
        .expect("the tab has a guard")
        .try_lock_owned()
        .expect("nobody holds it yet")
}

#[test]
fn a_command_waits_for_a_tab_another_holds_and_then_takes_it() {
    let registry = registry_with("tab-1");
    let held = held_by_someone(&registry);
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        drop(held);
    });

    let started = Instant::now();
    let taken = tauri::async_runtime::block_on(hold(
        &registry,
        "tab-1",
        Deadline::in_(Duration::from_secs(5)),
    ));

    assert!(taken.is_ok(), "the holder let go inside the deadline");
    assert!(
        started.elapsed() >= Duration::from_millis(100),
        "and it was waited for, not skipped: {:?}",
        started.elapsed()
    );
    release.join().expect("the holder ends");
}

#[test]
fn a_command_that_cannot_have_the_tab_in_time_says_it_is_busy() {
    let registry = registry_with("tab-1");
    let _held = held_by_someone(&registry);

    let started = Instant::now();
    let error = tauri::async_runtime::block_on(hold(
        &registry,
        "tab-1",
        Deadline::in_(Duration::from_millis(120)),
    ))
    .err()
    .expect("the tab never came free");

    assert_eq!(error.code, BrowserErrorCode::HostError);
    assert!(error.message.contains("busy"), "{}", error.message);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the wait ended with the deadline: {:?}",
        started.elapsed()
    );
}

#[test]
fn a_tab_that_is_gone_is_not_waited_for() {
    let registry = registry_with("tab-1");

    let error = tauri::async_runtime::block_on(hold(
        &registry,
        "tab-404",
        Deadline::in_(Duration::from_secs(5)),
    ))
    .err()
    .expect("no such tab");
    assert_eq!(error.code, BrowserErrorCode::TabNotFound);

    let pane =
        tauri::async_runtime::block_on(hold_for_pane(&registry, "tab-404", Duration::from_secs(5)));
    assert!(pane.is_none());
}

#[test]
fn the_pane_waits_a_moment_for_an_action_and_then_goes_ahead_without_it() {
    let registry = registry_with("tab-1");
    let _held = held_by_someone(&registry);

    let started = Instant::now();
    let taken = tauri::async_runtime::block_on(hold_for_pane(
        &registry,
        "tab-1",
        Duration::from_millis(120),
    ));

    assert!(
        taken.is_none(),
        "an action that does not finish is not waited out"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(100)
            && started.elapsed() < Duration::from_secs(2),
        "it waited its moment and no longer: {:?}",
        started.elapsed()
    );
}

#[test]
fn the_pane_takes_the_tab_the_moment_an_action_lets_go() {
    let registry = registry_with("tab-1");
    let held = held_by_someone(&registry);
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        drop(held);
    });

    let taken =
        tauri::async_runtime::block_on(hold_for_pane(&registry, "tab-1", Duration::from_secs(5)));

    assert!(taken.is_some());
    release.join().expect("the holder ends");
}

#[test]
fn the_pane_wait_is_far_shorter_than_an_agent_commands_budget() {
    assert!(PANE_WAIT < Duration::from_secs(3));
    assert!(PANE_WAIT < crate::browser::deadline::COMMAND_BUDGET / 4);
}
