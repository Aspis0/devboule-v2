//! The audit row's sentence for one cleanup: who approved it and what the
//! two phases left, down to the pids and the reason for each one spared.

use crate::mcp_broker::tools::first_use::Approval;
use crate::process_terminate::Termination;

/// Pids named per list; the rest are counted, so one row stays one line.
const LISTED: usize = 16;

fn list<T: std::fmt::Display>(items: &[T]) -> String {
    let shown: Vec<String> = items.iter().take(LISTED).map(ToString::to_string).collect();
    let more = items.len().saturating_sub(LISTED);
    if more == 0 {
        format!("[{}]", shown.join(", "))
    } else {
        format!("[{}, +{more} more]", shown.join(", "))
    }
}

fn approver(approval: Approval) -> &'static str {
    match approval {
        Approval::Mode => "approved by automatic mode",
        Approval::Person => "approved by person",
    }
}

/// What a finished cleanup left. `ok` when every planned process is gone and
/// nothing was spared or unproven; `partial` otherwise. `late` holds the
/// members the approval did not cover or that became protected before their
/// signal, and each list is named only when it holds something.
pub(super) fn finished(
    approval: Approval,
    planned: usize,
    targets: &str,
    termination: &Termination,
    unproven: &[u32],
    late: &[(u32, &'static str)],
) -> String {
    let mut spared: Vec<(u32, &'static str)> = termination
        .skipped
        .iter()
        .copied()
        .chain(late.iter().copied())
        .collect();
    spared.sort_by_key(|(pid, _)| *pid);
    let partial =
        !termination.still_running.is_empty() || !spared.is_empty() || !unproven.is_empty();
    let mut row = format!(
        "{}; {}: {planned} planned ({targets}); terminated {}",
        if partial { "partial" } else { "ok" },
        approver(approval),
        list(&termination.terminated),
    );
    if !termination.forced.is_empty() {
        row.push_str(&format!(" (forced {})", list(&termination.forced)));
    }
    if !termination.still_running.is_empty() {
        row.push_str(&format!(
            "; still running {}",
            list(&termination.still_running)
        ));
    }
    if !spared.is_empty() {
        let named = spared
            .iter()
            .map(|(pid, reason)| format!("{pid}:{reason}"))
            .collect::<Vec<_>>();
        row.push_str(&format!("; skipped {}", list(&named)));
    }
    if !unproven.is_empty() {
        row.push_str(&format!("; unproven {}", list(unproven)));
    }
    row
}

/// An approved cleanup that could not run to its end.
pub(super) fn failed(approval: Approval, planned: usize, reason: &str) -> String {
    format!(
        "failed: {reason}; {}: {planned} planned",
        approver(approval)
    )
}

/// A cleanup whose plan held nothing to stop: no card was needed.
pub(super) fn nothing_to_stop(skipped: &[(u32, &'static str)], unproven: &[u32]) -> String {
    let mut row = "ok; nothing to stop".to_string();
    let spared = skipped
        .iter()
        .map(|(pid, reason)| format!("{pid}:{reason}"))
        .collect::<Vec<_>>();
    if !spared.is_empty() {
        row.push_str(&format!("; skipped {}", list(&spared)));
    }
    if !unproven.is_empty() {
        row.push_str(&format!("; unproven {}", list(unproven)));
    }
    row
}

/// A cleanup refused before any card: the session's own tree is not proven.
pub(super) fn refused(reason: &str) -> String {
    format!("refused: {reason}; nothing stopped")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn termination() -> Termination {
        Termination {
            terminated: vec![10, 11],
            forced: vec![11],
            still_running: vec![12],
            skipped: vec![(13, "no_longer_in_session")],
        }
    }

    /// A partial cleanup names which pids stopped, which needed force, which
    /// survived and which were spared with their reason.
    #[test]
    fn a_partial_row_says_what_stopped_what_was_forced_and_what_failed() {
        let row = finished(Approval::Mode, 4, "node", &termination(), &[14], &[]);
        assert_eq!(
            row,
            "partial; approved by automatic mode: 4 planned (node); terminated [10, 11] \
             (forced [11]); still running [12]; skipped [13:no_longer_in_session]; unproven [14]"
        );
    }

    /// A member that became protected, or fell outside the approved plan, after
    /// the approval is named in the row: it never reads ok while something was
    /// spared.
    #[test]
    fn a_skip_after_approval_makes_the_row_partial_and_names_it() {
        let clean = Termination {
            terminated: vec![10],
            forced: Vec::new(),
            still_running: Vec::new(),
            skipped: Vec::new(),
        };
        assert_eq!(
            finished(
                Approval::Mode,
                1,
                "node",
                &clean,
                &[],
                &[(7, "protected_at_execution")]
            ),
            "partial; approved by automatic mode: 1 planned (node); terminated [10]; \
             skipped [7:protected_at_execution]"
        );
    }

    /// An unproven member makes the row partial, even when nothing was spared.
    #[test]
    fn an_unproven_member_makes_the_row_partial() {
        let clean = Termination {
            terminated: vec![10],
            forced: Vec::new(),
            still_running: Vec::new(),
            skipped: Vec::new(),
        };
        let row = finished(Approval::Person, 1, "node", &clean, &[14], &[]);
        assert!(row.starts_with("partial;"), "{row}");
    }

    #[test]
    fn a_clean_row_is_ok_and_names_only_what_happened() {
        let clean = Termination {
            terminated: vec![10],
            forced: Vec::new(),
            still_running: Vec::new(),
            skipped: Vec::new(),
        };
        assert_eq!(
            finished(Approval::Person, 1, "node", &clean, &[], &[]),
            "ok; approved by person: 1 planned (node); terminated [10]"
        );
    }

    #[test]
    fn the_failed_and_empty_rows_say_why() {
        assert_eq!(
            failed(Approval::Mode, 2, "platform_unavailable"),
            "failed: platform_unavailable; approved by automatic mode: 2 planned"
        );
        assert_eq!(
            nothing_to_stop(&[(7, "agent_root")], &[]),
            "ok; nothing to stop; skipped [7:agent_root]"
        );
    }

    #[test]
    fn a_long_list_is_counted_not_dumped() {
        let pids: Vec<u32> = (0..20).collect();
        assert!(list(&pids).ends_with(", +4 more]"));
    }
}
