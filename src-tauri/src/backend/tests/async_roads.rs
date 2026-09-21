//! The long daemon roads, and the one helper every one of them waits through.

use super::super::blocking::off_main_thread;
use devboule_daemon::DaemonError;

/// The daemon roads that can wait for a whole provider startup — a resume
/// (300 s), a create (the same inline handshake), an install (240 s) — must
/// declare `pub async fn`: Tauri spawns an `async` command's future on its
/// runtime (`tauri::ipc::InvokeResolver::respond_async_serialized`), while a
/// plain `pub fn` is called inline by the IPC dispatcher, which is the
/// window's own thread. That inline call is the measured freeze of Reopen
/// (`scout/user-pass/f08b.png`).
///
/// Mutant: drop the `async` from any of the three — the road is named in the
/// failure.
#[test]
fn the_long_daemon_roads_are_async_commands() {
    let roads = [
        (
            "session.rs",
            include_str!("../session.rs"),
            "session_create",
        ),
        (
            "session.rs",
            include_str!("../session.rs"),
            "session_resume",
        ),
        (
            "providers.rs",
            include_str!("../providers.rs"),
            "provider_update",
        ),
    ];
    for (file, source, command) in roads {
        assert!(
            source.contains(format!("pub async fn {command}(").as_str()),
            "{file}: `{command}` must be `async`, or its wait runs on the window's thread"
        );
        assert!(
            !source.contains(format!("pub fn {command}(").as_str()),
            "{file}: `{command}` still declares a non-`async` command"
        );
    }
}

/// All three roads, and only those three, wait through the helper: an `async`
/// command that called the client directly would park a runtime worker for
/// minutes, which is the thread `spawn_blocking` exists to spare.
///
/// (The needle is concatenated so this test does not match itself.)
///
/// Mutant: one road calling its client directly — the count drops to two and
/// this test dies.
#[test]
fn the_three_long_roads_wait_through_the_blocking_helper() {
    let needle = ["off_main_thread", "("].concat();
    let sources = [
        include_str!("../session.rs"),
        include_str!("../providers.rs"),
    ];
    let calls: usize = sources
        .iter()
        .map(|source| source.matches(needle.as_str()).count())
        .sum();
    assert_eq!(
        calls, 3,
        "one wait per long road: create, resume and update must all go through the helper"
    );
}

/// The helper's own property: the work runs on the runtime's blocking pool,
/// never on the thread that called. A helper that ran the closure inline
/// would compile and pass every other test while freezing the window.
///
/// Mutant: `off_main_thread` calling `work()` before awaiting, or through
/// `block_on` on the caller's thread — the two thread ids are equal and this
/// test dies.
#[test]
fn the_blocking_work_leaves_the_callers_thread() {
    let caller = std::thread::current().id();
    let ran_on = tauri::async_runtime::block_on(off_main_thread(move || {
        Ok::<_, DaemonError>(std::thread::current().id())
    }))
    .expect("the helper answers");
    assert_ne!(
        ran_on, caller,
        "a daemon wait must not run on the thread that called the command"
    );
}
