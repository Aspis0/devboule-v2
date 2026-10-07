//! The check runs of a commit, or the jobs of one workflow-run attempt, as
//! `gh api --paginate --slurp` hands them over: one page per array entry.
//! One phrase: join the pages into one list, or refuse. A verdict is read off
//! the whole list or not at all, because a missing page can hide the one
//! failed check.

use serde_json::Value;

use crate::ci_gh::CiError;
use crate::ci_summary::{parse_check_runs, parse_jobs, CheckRun};

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

/// The jobs of one workflow-run attempt, joined the way the check runs are:
/// a page that did not arrive is not a short list, it is no list at all, and
/// a verdict read off half an attempt could call a red job green.
pub(crate) fn join_job_pages(pages: &[Value]) -> Result<Vec<CheckRun>, CiError> {
    if pages.is_empty() {
        return Err(unreadable("GitHub returned no page of jobs"));
    }
    let mut jobs = Vec::new();
    let mut announced = 0u64;
    for page in pages {
        let Some(page_jobs) = page.get("jobs").and_then(Value::as_array) else {
            return Err(unreadable("a page of jobs had no list"));
        };
        let parsed = parse_jobs(page);
        if parsed.len() != page_jobs.len() {
            return Err(unreadable("a job on a page could not be read"));
        }
        announced = announced.max(page.get("total_count").and_then(Value::as_u64).unwrap_or(0));
        jobs.extend(parsed);
    }
    if (jobs.len() as u64) < announced {
        return Err(unreadable("fewer jobs arrived than GitHub counted"));
    }
    Ok(jobs)
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
