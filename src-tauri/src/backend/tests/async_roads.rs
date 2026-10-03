//! The class of roads the window must not wait on, pinned on the sources.
//!
//! A `#[tauri::command]` that is not `async` runs inline on the window's own
//! thread, so every wait it makes is a frozen window: the measured cases are
//! Reopen (`scout/user-pass/f08b.png`) and the startup queue of the trace
//! (`scout/freeze-followup/REPRO-sliceM.md`, `SessionAttach` at 522 ms with
//! the daemon idle). What is pinned here is the form the framework reads —
//! which commands must be `async`, how many waits go through the helper —
//! and the helper's own promise that the wait leaves the caller's thread.

use std::collections::{BTreeMap, BTreeSet};

use super::super::blocking::off_main_thread;
use super::command_scan;
use devboule_daemon::DaemonError;

/// Bridge commands that take `State<'_, DaemonBridge>` but send the daemon no
/// frame: a local snapshot of what the bridge already holds. The only declared
/// exception to the class rule below, checked both ways: a name here that
/// stops being synchronous fails as stale, and a synchronous bridge command
/// outside this list fails by name.
const BRIDGE_COMMANDS_WITHOUT_A_WAIT: &[&str] = &["daemon_status"];

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

/// Three roads do blocking work without ever touching the bridge — a
/// document write with its digest pass, a folder copy with its own digest
/// pass, and the editor-target scan — so the class pin cannot see them;
/// their names are pinned here.
#[test]
fn the_blocking_roads_outside_the_bridge_stay_async() {
    let scan = command_scan::scan();
    for name in [
        "artifact_write_file",
        "plugin_install",
        "editor_targets_list",
    ] {
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
/// reviewable act, never a silent one. `providers_auth_check` is on the
/// list for a sharper reason: the login check spawns provider CLIs in the
/// host's credential context, so it must never run on the UI thread.
#[test]
fn every_wait_goes_through_the_blocking_helper() {
    let scan = command_scan::scan();
    assert_eq!(
        scan.helper_calls, 72,
        "one helper call per waiting road, plus the thread test below that calls the helper itself"
    );
}

/// The queue forwarders, and the checks each one makes before its frame
/// leaves the process.
///
/// Read from the sources because the checks run against a live bridge registry
/// and a live pipe, neither of which a unit test here can stand up: what is
/// pinned is which guard each command calls. `session_queue_send_now` is a
/// send — it carries the subscription a send needs — so it must confirm this
/// view holds that attach before the frame goes; the other four carry no
/// subscription and have no such guard to make.
///
/// Both sides are compared sorted: which guards a command makes is a fact, the
/// order it happens to write them in is not.
#[test]
fn the_queue_forwarders_make_the_checks_their_frames_need() {
    let scan = command_scan::scan();
    let guards: BTreeMap<&str, Vec<&str>> = scan
        .commands
        .iter()
        .filter(|command| command.name.starts_with("session_queue_"))
        .map(|command| {
            let mut found: Vec<&str> = command.guards.iter().map(String::as_str).collect();
            found.sort_unstable();
            (command.name.as_str(), found)
        })
        .collect();
    let named = |command: &str| {
        guards
            .get(command)
            .map(|found| found.join(" "))
            .unwrap_or_else(|| format!("<{command} is gone>"))
    };
    assert_eq!(
        named("session_queue_add"),
        "require_attachment_limits require_session_id require_write_size",
        "an add is the one queue frame that carries a prompt: it is checked as a prompt is"
    );
    assert_eq!(
        named("session_queue_send_now"),
        "ensure_subscription_attached require_session_id",
        "a send-now is a send, so it waits through the same attachment guard every other send does"
    );
    // The other three name a session and one row: remove and move carry no
    // prompt at all, and an edit carries text but no attachment.
    assert_eq!(
        named("session_queue_edit"),
        "require_session_id require_write_size",
        "an edit carries the row's new text, so it is size-checked like a send"
    );
    for command in ["session_queue_remove", "session_queue_move"] {
        assert_eq!(
            named(command),
            "require_session_id",
            "{command} names a row, not a prompt or a subscription"
        );
    }
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
