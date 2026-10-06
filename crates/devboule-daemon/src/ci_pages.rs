//! The check runs of a commit as `gh api --paginate --slurp` hands them over:
//! one page per array entry. One phrase: join the pages into one list, or
//! refuse. A verdict is read off the whole list or not at all, because a
//! missing page can hide the one failed check.

use serde_json::Value;

use crate::ci_gh::CiError;
use crate::ci_summary::{parse_check_runs, CheckRun};

pub(crate) fn join_check_run_pages(pages: &[Value]) -> Result<Vec<CheckRun>, CiError> {
    if pages.is_empty() {
        return Err(unreadable("GitHub returned no page of check runs"));
    }
    let mut runs = Vec::new();
    let mut announced = 0u64;
    for page in pages {
        let Some(page_runs) = page.get("check_runs").and_then(Value::as_array) else {
            return Err(unreadable("a page of check runs had no list"));
        };
        let parsed = parse_check_runs(page);
        if parsed.len() != page_runs.len() {
            return Err(unreadable("a check run on a page could not be read"));
        }
        announced = announced.max(page.get("total_count").and_then(Value::as_u64).unwrap_or(0));
        runs.extend(parsed);
    }
    // GitHub states the whole count on every page, so a list shorter than it
    // is a page that went missing.
    if (runs.len() as u64) < announced {
        return Err(unreadable("fewer check runs arrived than GitHub counted"));
    }
    Ok(runs)
}

fn unreadable(what: &str) -> CiError {
    CiError::new(
        "github_unavailable",
        format!("{what}; the watch tries again."),
        true,
    )
}

#[cfg(test)]
#[path = "ci_pages_tests.rs"]
mod tests;
