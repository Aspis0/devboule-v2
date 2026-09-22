//! Pure parsing of git's own output: `--porcelain=v2` records into rows, and
//! `--numstat -z` records into line counts. No process is spawned here and no
//! repository is opened, so every rule below is provable from a literal
//! string (`workspace_git_status_parse_tests.rs`); the orchestration that
//! runs `git` lives in `workspace_git_status.rs`.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use devboule_protocol::{WorkspaceGitFileStatus, WorkspaceGitRow};

use crate::git::GIT_STDOUT_MAX_BYTES;

/// Bytes of one untracked file read to count its lines. One probe read past
/// it separates "the file ends here" from "the count stopped here", and the
/// row carries that answer as `capped`.
const UNTRACKED_COUNT_MAX_BYTES: usize = 2 * 1024 * 1024;

/// Whether git's output ran past the shared stdout accumulator, which stops
/// at `GIT_STDOUT_MAX_BYTES + 1` and drops whatever came after without saying
/// so. Both dumps that reach the wire use it: a truncated list or a truncated
/// count must never be handed over as a complete one.
pub(super) fn stdout_truncated(stdout: &str) -> bool {
    stdout.len() > GIT_STDOUT_MAX_BYTES
}

/// The branch and the changed paths of a `-z` `--porcelain=v2` dump, in git's
/// own order.
pub(super) fn parse_status(
    stdout: &str,
) -> (Option<String>, Vec<(String, WorkspaceGitFileStatus)>) {
    let mut branch = None;
    let mut rows = Vec::new();
    let mut records = stdout.split('\0').filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        if let Some(head) = record.strip_prefix("# branch.head ") {
            branch = Some(head.to_string());
            continue;
        }
        if record.starts_with("# ") {
            continue;
        }
        if let Some(path) = record.strip_prefix("? ") {
            rows.push((path.to_string(), WorkspaceGitFileStatus::Untracked));
            continue;
        }
        let Some((xy, path)) = entry(record) else {
            continue;
        };
        rows.push((path.to_string(), classify(xy, record.starts_with("u "))));
        if record.starts_with("2 ") {
            // `-z` writes a rename's original path as the very next record,
            // bare. Skipping it by position is the only thing stopping a
            // source path that looks like an entry (`? old`) from becoming a
            // second row — see the killer test in the parse tests.
            records.next();
        }
    }
    (branch, rows)
}

/// The `XY` pair and the path of one `1`/`2`/`u` record; the path is the last
/// field of each shape, so `splitn` takes it whole and keeps its own spaces.
fn entry(record: &str) -> Option<(&str, &str)> {
    let fields = match record.get(..2)? {
        "1 " => 9,
        "2 " => 10,
        "u " => 11,
        _ => return None,
    };
    let mut parts = record.splitn(fields, ' ');
    let xy = parts.nth(1)?;
    let path = parts.last()?;
    if xy.len() != 2 || path.is_empty() {
        return None;
    }
    Some((xy, path))
}

/// One of the six wire words, from the record kind first and the `XY` pair
/// second. Deletion beats addition: `AD` is a file staged then removed, and
/// the working tree is where the user will look for it.
fn classify(xy: &str, unmerged: bool) -> WorkspaceGitFileStatus {
    if unmerged {
        return WorkspaceGitFileStatus::Conflicted;
    }
    let mut codes = xy.chars();
    let index = codes.next().unwrap_or('.');
    let worktree = codes.next().unwrap_or('.');
    if index == 'D' || worktree == 'D' {
        WorkspaceGitFileStatus::Deleted
    } else if index == 'A' || worktree == 'A' {
        WorkspaceGitFileStatus::Added
    } else if matches!(index, 'R' | 'C') || matches!(worktree, 'R' | 'C') {
        WorkspaceGitFileStatus::Renamed
    } else {
        WorkspaceGitFileStatus::Modified
    }
}

/// What one path's numstats add up to; `inexact` travels to the row as
/// `capped`.
#[derive(Default)]
pub(super) struct Counted {
    additions: u64,
    deletions: u64,
    inexact: bool,
}

/// Fold one `-z` `--numstat` dump into `counts`. Two dumps (worktree and
/// index) accumulate onto the same path.
pub(super) fn merge_numstat(stdout: &str, counts: &mut HashMap<String, Counted>) {
    let records: Vec<&str> = stdout.split('\0').filter(|r| !r.is_empty()).collect();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        let mut fields = record.splitn(3, '\t');
        let additions = fields.next().and_then(parse_count);
        let deletions = fields.next().and_then(parse_count);
        let path = fields.next().unwrap_or_default();
        if path.is_empty() {
            // `-z` writes a rename as `additions<tab>deletions<tab>` then the
            // old and the new path as the next two records; `status` reports
            // the new one, so that is the key.
            let renamed = records.get(index + 1).copied();
            index += 2;
            if let Some(new_path) = renamed {
                accumulate(counts, new_path, additions, deletions);
            }
            continue;
        }
        accumulate(counts, path, additions, deletions);
    }
}

/// `Some` for a counted total, `None` for git's `-`: a binary has no lines.
fn parse_count(field: &str) -> Option<u64> {
    field.parse().ok()
}

fn accumulate(
    counts: &mut HashMap<String, Counted>,
    path: &str,
    additions: Option<u64>,
    deletions: Option<u64>,
) {
    let counted = counts.entry(path.to_string()).or_default();
    counted.additions += additions.unwrap_or(0);
    counted.deletions += deletions.unwrap_or(0);
    if additions.is_none() || deletions.is_none() {
        counted.inexact = true;
    }
}

/// One row's counts: from the numstat map, from the untracked file itself, or
/// from nothing at all. `degraded` says the numstat round could not be
/// trusted — one dump failed or was cut — so every number that came from it
/// is a floor and must say so; an untracked row counts its own file and is
/// unaffected by it.
pub(super) fn build_row(
    root: &Path,
    path: &str,
    status: WorkspaceGitFileStatus,
    counts: &HashMap<String, Counted>,
    degraded: bool,
) -> WorkspaceGitRow {
    let (additions, deletions, capped) = match status {
        WorkspaceGitFileStatus::Untracked => {
            let (lines, capped) = count_untracked_lines(&root.join(path));
            (lines, 0, capped)
        }
        // git's numstat for an unmerged path is stage bookkeeping, not a
        // delta on the working tree: zero with `capped` beside it says so.
        WorkspaceGitFileStatus::Conflicted => (0, 0, true),
        _ => match counts.get(path) {
            Some(counted) => (
                counted.additions,
                counted.deletions,
                counted.inexact || degraded,
            ),
            // No numstat line is git finding no line delta — a mode-only
            // change — and zero is its exact answer. A degraded round cannot
            // tell that from a line the failed or truncated dump never
            // delivered, so there the zero is a floor, never an exact zero.
            None => (0, 0, degraded),
        },
    };
    WorkspaceGitRow {
        path: path.to_string(),
        additions,
        deletions,
        status,
        capped,
    }
}

/// Lines in one untracked file, bounded by [`UNTRACKED_COUNT_MAX_BYTES`] and
/// honest about stopping: the second answer is `capped`.
fn count_untracked_lines(path: &Path) -> (u64, bool) {
    // The size is asked before the file is opened, so a 4 GiB asset costs one
    // stat and reports no count at all rather than a partial one.
    let Ok(metadata) = std::fs::metadata(path) else {
        return (0, true);
    };
    if metadata.len() > UNTRACKED_COUNT_MAX_BYTES as u64 {
        return (0, true);
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return (0, true);
    };
    let mut buffer = [0u8; 8 * 1024];
    let mut lines = 0u64;
    let mut bytes = 0usize;
    loop {
        let Ok(read) = file.read(&mut buffer) else {
            return (lines, true);
        };
        if read == 0 {
            return (lines, false);
        }
        if buffer[..read].contains(&0) {
            // git calls a file with a NUL byte binary; counting its bytes
            // anyway would be the number the panel shows.
            return (0, true);
        }
        bytes += read;
        lines += buffer[..read].iter().filter(|byte| **byte == b'\n').count() as u64;
        // The pre-check above already ruled a static file out; this is the
        // guard for one that grew while it was being read.
        if bytes > UNTRACKED_COUNT_MAX_BYTES {
            // One probe byte: a file that ends here, or a count that stopped.
            let mut probe = [0u8; 1];
            return match file.read(&mut probe) {
                Ok(0) => (lines, false),
                _ => (lines, true),
            };
        }
    }
}

#[cfg(test)]
#[path = "workspace_git_status_parse_tests.rs"]
mod tests;
