//! Helpers shared by the module test blocks.
//!
//! A test that needs an external program — the `node` scripts the Pi and Codex
//! tests use as fake providers — must *skip* when that program is not on PATH
//! instead of failing (A2-13): a machine without `node` is not a broken build,
//! and a suite that reports red for it teaches readers to ignore red.

/// Why a test that needs `program` must skip here, or `None` when it can run.
///
/// One implementation for every module's test block: the reason it returns is
/// printed by the skipping test, and it names the program and the OS error so a
/// reader of the log knows what was missing.
pub(crate) fn external_program_skip_reason(program: &str) -> Option<String> {
    match std::process::Command::new(program)
        .arg("--version")
        .output()
    {
        Ok(_) => None,
        Err(error) => Some(format!(
            "skipping: `{program}` is not runnable here ({error}; kind={:?})",
            error.kind()
        )),
    }
}

/// Run one steerer through a real turn admission, the way the daemon admits a
/// steer: a turn is begun, and the steerer is handed the token that
/// `with_active_turn` gives out. `None` means the turn was already over at
/// admission, which is a different answer from any the steerer can give.
#[cfg(test)]
pub(crate) fn steer_through_the_turn(
    steerer: &mut dyn crate::session::SessionSteerer,
    text: &str,
) -> Option<Result<bool, devboule_protocol::WireError>> {
    let runtime = std::sync::Arc::new(crate::session::SessionRuntime::new());
    runtime.begin_turn();
    runtime.with_active_turn(runtime.turn_counter(), |turn| {
        steerer.steer_active_turn(text, turn)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The policy the node-backed tests take, pinned with a program that is
    /// certainly not on PATH: an unrunnable program is a *reason to skip*, never
    /// a panic and never `None`. A refactor that turns the failure into an
    /// `expect`, or that reports `None` for a program it could not spawn, fails
    /// this before it can turn a missing `node` into a red suite.
    #[test]
    fn a_program_that_cannot_be_spawned_is_a_skip_not_a_failure() {
        let reason = external_program_skip_reason("devboule-no-such-program-3f1a")
            .expect("an unrunnable program is a skip");
        assert!(reason.contains("skipping"), "{reason}");
        assert!(reason.contains("devboule-no-such-program-3f1a"), "{reason}");
    }
}
