//! The signal one page's events bump, and the promise that it is dropped with
//! the tab. No page is opened here: what is tested is the bookkeeping that
//! decides whether a settle waits on a signal that will ever move.

use super::*;

#[test]
fn a_page_that_has_never_been_watched_has_not_moved() {
    assert_eq!(moved("tab-never-seen"), 0);
}

#[test]
fn a_watched_page_has_a_signal_and_forgetting_it_takes_the_signal_with_it() {
    signal_for("tab-1");
    assert_eq!(moved("tab-1"), 0, "a new page starts where it was");

    signal_for("tab-1");
    assert_eq!(moved("tab-1"), 0, "watching a page twice does not move it");

    forget("tab-1");
    assert_eq!(moved("tab-1"), 0, "a closed tab leaves nothing behind");
    forget("tab-never-existed");
}

#[test]
fn a_settle_never_outlasts_the_time_its_command_has_left() {
    // One quiet is 300 ms, so a settle that ignored the command's deadline
    // would take at least that however little was left.
    let started = std::time::Instant::now();
    tauri::async_runtime::block_on(settle(
        "tab-settle",
        Deadline::in_(Duration::from_millis(40)),
    ));
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "settled in {:?}",
        started.elapsed()
    );

    let spent = Deadline::in_(Duration::from_millis(1));
    std::thread::sleep(Duration::from_millis(10));
    let started = std::time::Instant::now();
    tauri::async_runtime::block_on(settle("tab-settle", spent));
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "a spent command does not wait at all: {:?}",
        started.elapsed()
    );
}

#[test]
fn the_quiet_a_settle_waits_for_is_shorter_than_the_cap_it_gives_up_at() {
    assert!(QUIET < SETTLE_CAP);
    assert_eq!(
        QUIET,
        std::time::Duration::from_millis(300),
        "a page that has not moved for a third of a second has settled"
    );
}

#[test]
fn a_dropped_subscription_aborts_the_task_it_holds() {
    tauri::async_runtime::block_on(async {
        // The probe is dropped with the task's own future, so a probe that
        // outlives the guard is a drain task nobody stopped.
        let (held, released) = tokio::sync::oneshot::channel::<()>();
        let drain = tauri::async_runtime::spawn(async move {
            let _held = held;
            std::future::pending::<()>().await;
        });
        let watch = WsWatch { drain };

        drop(watch);

        let outcome = tokio::time::timeout(Duration::from_secs(2), released).await;
        assert!(
            outcome
                .expect("a dropped subscription aborts the task it holds")
                .is_err(),
            "the task outlived the guard that was supposed to stop it"
        );
    });
}
