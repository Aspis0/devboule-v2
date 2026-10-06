//! How many watches one session and one repository may hold, so one agent's
//! flood of watches cannot use up the store every other session shares.
//!
//! One phrase: say whether a new watch fits its session and its repository,
//! and tell the caller why not when it does not.
//! Room is only ever made from the asking session's own finished history; a
//! verdict still owed to anyone is never dropped for it.

use crate::ci_gh::{CiError, RepoRef};
use crate::ci_watch_store::{CiWatchRecord, InsertError, Wake};

/// Watches one session keeps, open and finished together.
pub(crate) const MAX_PER_SESSION: usize = 25;
/// Unfinished watches on one repository, whoever asked for them.
pub(crate) const MAX_OPEN_PER_REPO: usize = 100;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    Session,
    Repo,
}

/// Fit `incoming` within its bounds, forgetting the session's own settled
/// history as needed, or say which bound refuses it.
pub(crate) fn make_room(
    records: &mut Vec<CiWatchRecord>,
    incoming: &CiWatchRecord,
) -> Result<(), Refusal> {
    let open_in_repo = records
        .iter()
        .filter(|record| !record.state.is_terminal() && same_repo(record, incoming))
        .count();
    if open_in_repo >= MAX_OPEN_PER_REPO {
        return Err(Refusal::Repo);
    }
    while records
        .iter()
        .filter(|record| record.session_id == incoming.session_id)
        .count()
        >= MAX_PER_SESSION
    {
        let oldest_settled = records
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                record.session_id == incoming.session_id
                    && record.state.is_terminal()
                    && matches!(record.wake, Wake::Delivered | Wake::DeliveredUncertain)
            })
            .min_by_key(|(_, record)| record.created_at_ms)
            .map(|(index, _)| index);
        let Some(index) = oldest_settled else {
            return Err(Refusal::Session);
        };
        records.remove(index);
    }
    Ok(())
}

fn same_repo(left: &CiWatchRecord, right: &CiWatchRecord) -> bool {
    left.host.eq_ignore_ascii_case(&right.host)
        && left.repo_owner.eq_ignore_ascii_case(&right.repo_owner)
        && left.repo.eq_ignore_ascii_case(&right.repo)
}

/// The answer a refused or failed insert gives the caller.
pub(crate) fn refusal_of(error: InsertError, repo: &RepoRef) -> CiError {
    match error {
        InsertError::Quota(Refusal::Session) => CiError::new(
            "too_many_watches",
            format!(
                "This session already holds {MAX_PER_SESSION} CI watches that are unfinished or whose verdict has not reached it; ask again when some have."
            ),
            true,
        ),
        InsertError::Quota(Refusal::Repo) => CiError::new(
            "too_many_watches",
            format!(
                "{} already has {MAX_OPEN_PER_REPO} unfinished CI watches; ask again when some finish.",
                repo.slug()
            ),
            true,
        ),
        InsertError::Full => CiError::new(
            "too_many_watches",
            "The daemon is keeping as many unfinished CI watches as it can; ask again when some finish.",
            true,
        ),
        InsertError::Io(error) => CiError::new(
            "internal",
            format!("The watch could not be saved: {error}"),
            true,
        ),
    }
}

#[cfg(test)]
#[path = "ci_watch_quota_tests.rs"]
mod tests;
