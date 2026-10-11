//! How a finished command reads. A provider says `failed` for a real failure,
//! and Codex (and ACP shells) also say it when the process ran and exited
//! non-zero: that is a finished command with an exit code, not a failure. The
//! rule below changes only that case; everything else keeps the provider's word.

/// The status a finished command row carries, given the provider's status and
/// the exit code it reported.
pub(crate) fn settled_command_status(status: &str, exit_code: Option<i32>) -> String {
    match (status, exit_code) {
        ("failed", Some(code)) if code != 0 => "completed".to_string(),
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
