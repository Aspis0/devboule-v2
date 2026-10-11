//! How a finished command reads. A provider says `failed` for a real failure,
//! and Codex (and ACP shells) also say it when the process ran and exited
//! non-zero: that is a finished command with an exit code, not a failure. The
//! rule below changes only that case; everything else keeps the provider's word.

/// Whether an exit code is a process that ran and exited on its own with a
/// non-zero code: the shell convention range up to 123. 124 is `timeout`'s own
/// code, 125 to 127 are the shell's ("could not run", "not found"), 128 and up
/// are signals, and negatives are kills; none of those is a clean exit.
pub(crate) fn is_clean_exit_code(code: i32) -> bool {
    (1..=123).contains(&code)
}

/// The status a finished command row carries, given the provider's status and
/// the exit code it reported. Only a clean non-zero exit settles a `failed`
/// command as completed; a timeout, a signal, a crash or no code keeps `failed`.
pub(crate) fn settled_command_status(status: &str, exit_code: Option<i32>) -> String {
    match (status, exit_code) {
        ("failed", Some(code)) if is_clean_exit_code(code) => "completed".to_string(),
        _ => status.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::settled_command_status;

    #[test]
    fn a_non_zero_exit_alone_settles_a_failed_command_as_completed() {
        assert_eq!(settled_command_status("failed", Some(1)), "completed");
        assert_eq!(settled_command_status("failed", Some(2)), "completed");
    }

    #[test]
    fn a_timeout_a_shell_error_or_a_signal_stays_failed() {
        // 124 is `timeout`'s own code, 125 to 127 are the shell's ("timeout", "could
        // not run", "not found"); 128 and up, and negatives, are signals and kills.
        for code in [124, 125, 126, 127, 128, 130, 137, -1, -9] {
            assert_eq!(
                settled_command_status("failed", Some(code)),
                "failed",
                "code {code}"
            );
        }
    }

    #[test]
    fn the_last_clean_exit_code_settles_and_the_first_timeout_code_does_not() {
        assert_eq!(settled_command_status("failed", Some(123)), "completed");
        assert_eq!(settled_command_status("failed", Some(124)), "failed");
    }

    #[test]
    fn a_failure_without_an_exit_code_stays_failed() {
        // A command that could not start, or timed out, never reaches an exit.
        assert_eq!(settled_command_status("failed", None), "failed");
    }

    #[test]
    fn a_failure_that_reports_a_clean_exit_stays_failed() {
        assert_eq!(settled_command_status("failed", Some(0)), "failed");
    }

    #[test]
    fn declined_and_completed_keep_their_own_status() {
        assert_eq!(settled_command_status("declined", None), "declined");
        assert_eq!(settled_command_status("completed", Some(1)), "completed");
    }
}
