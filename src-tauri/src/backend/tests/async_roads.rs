//! The class of roads the window must not wait on, pinned on the sources.
//!
//! A `#[tauri::command]` that is not `async` runs inline on the window's own
//! thread, so every wait it makes is a frozen window: the measured cases are
//! Reopen (`scout/user-pass/f08b.png`) and the startup queue of the trace
//! (`scout/freeze-followup/REPRO-sliceM.md`, `SessionAttach` at 522 ms with
//! the daemon idle). What is pinned here is the form the framework reads —
//! which commands must be `async`, how many waits go through the helper —
//! and the helper's own promise that the wait leaves the caller's thread.

use std::collections::BTreeSet;

use super::super::blocking::off_main_thread;
use super::command_scan;
use devboule_daemon::DaemonError;

/// Bridge commands that take `State<'_, DaemonBridge>` but send the daemon no
/// frame: a local snapshot and a `TerminateProcess` by handle. They are the
/// only declared exception to the class rule below, checked both ways: a
/// name here that stops being synchronous fails as stale, and a synchronous
/// bridge command outside this list fails by name.
const BRIDGE_COMMANDS_WITHOUT_A_WAIT: &[&str] = &["daemon_status", "daemon_restart"];

/// The class pin: every `#[tauri::command]` that holds the daemon bridge is
/// `pub async fn`, except the names the list above declares. A new
/// command that is born synchronous dies here on its own, wherever it lands
/// under `src/`.
#[test]
fn every_daemon_bridge_command_is_an_async_command() {
    let scan = command_scan::scan();
    let declared: BTreeSet<&str> = BRIDGE_COMMANDS_WITHOUT_A_WAIT.iter().copied().collect();

    let mut violations = Vec::new();
    let mut still_synchronous = BTreeSet::new();
    for command in &scan.commands {
        if !command.takes_daemon_bridge {
            continue;
        }
        if !command.is_public {
            violations.push(format!(
                "{}: `{}` holds the daemon bridge but is not `pub`; the class rule is `pub async fn`",
                command.file.display(),
                command.name
            ));
            continue;
        }
        if command.is_async {
            continue;
        }
        if declared.contains(command.name.as_str()) {
            still_synchronous.insert(command.name.as_str());
        } else {
            violations.push(format!(
                "{}: `{}` is a synchronous command holding `State<'_, DaemonBridge>`: its wait \
                 would run on the window's thread. Make it `pub async fn` through \
                 `off_main_thread`, or declare it in the list above if it truly never waits.",
                command.file.display(),
                command.name
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "commands outside the declared exception:\n{}",
        violations.join("\n")
    );

    let stale: Vec<&str> = declared.difference(&still_synchronous).copied().collect();
    assert!(
        stale.is_empty(),
        "declared as a synchronous bridge command, but no synchronous command carries the name \
         (converted or renamed?): {}",
        stale.join(", ")
    );
}

/// Two roads do blocking work without ever touching the bridge — a document
/// write with its digest pass, a folder copy with its own digest pass — so
/// the class pin cannot see them; their names are pinned here.
#[test]
fn the_blocking_roads_outside_the_bridge_stay_async() {
    let scan = command_scan::scan();
    for name in ["artifact_write_file", "plugin_install"] {
        let command = scan
            .commands
            .iter()
            .find(|command| command.name == name)
            .unwrap_or_else(|| panic!("`{name}` is gone: drop it from this list, or restore it"));
        assert!(
            command.is_public && command.is_async,
            "{}: `{}` is blocking work on whatever thread it is called from and must stay \
             `pub async fn`",
            command.file.display(),
            command.name
        );
    }
}

/// Every wait goes through the helper, and the total is a declared number —
/// read from the parsed tree, not from comment-visible text. A road that
/// starts calling its client directly drops the count; raising it is a
/// reviewable act, never a silent one. Raised 50 → 51 with
/// `workspace_file_read` (fetta 1 della sidebar «come Paseo»): the new road
/// waits on the daemon like every other workspace read.
#[test]
fn every_wait_goes_through_the_blocking_helper() {
    let scan = command_scan::scan();
    assert_eq!(
        scan.helper_calls, 51,
        "one helper call per waiting road, plus the thread test below that calls the helper itself"
    );
}

/// The helper's own property: the work runs on a thread other than the
/// caller's, never inline. A helper that ran the closure inline would compile
/// and pass every other test while freezing the window.
///
/// Mutant: `off_main_thread` calling `work()` before awaiting — the two thread
/// ids are equal and this test dies.
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
