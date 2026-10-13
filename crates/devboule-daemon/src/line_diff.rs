//! The line diff of two texts, as the rows a diff view shows and the counts
//! those rows add up to.
//!
//! A longest-common-subsequence walk decides what changed. The shared head and
//! tail are trimmed before the table is built, which keeps a one-line edit in
//! a large file cheap, and a middle past [`MAX_DIFF_CELLS`] is taken as wholly
//! changed rather than building a table this process cannot afford.

use devboule_protocol::{WorkspaceGitDiffLine, WorkspaceGitDiffLineKind};

/// Cell ceiling of the LCS table: a million cells is four megabytes.
const MAX_DIFF_CELLS: usize = 1_000_000;

/// The lines `old` and `new` do not share, removed lines first within each
/// changed run. Shared lines are not rows.
///
/// A trailing newline adds no line on either side (`lines()` is the rule for
/// both), so a file's rows do not depend on how its last line ended.
pub(crate) fn replace_lines(old: &str, new: &str) -> Vec<WorkspaceGitDiffLine> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let (old_mid, new_mid) = middle(&old_lines, &new_lines);
    let Some(table) = lcs_table(old_mid, new_mid) else {
        return old_mid
            .iter()
            .map(|line| row(WorkspaceGitDiffLineKind::Remove, line))
            .chain(
                new_mid
                    .iter()
                    .map(|line| row(WorkspaceGitDiffLineKind::Add, line)),
            )
            .collect();
    };
    let width = new_mid.len() + 1;
    let (mut left, mut right) = (old_mid.len(), new_mid.len());
    let mut backward = Vec::new();
    while left > 0 || right > 0 {
        if left > 0 && right > 0 && old_mid[left - 1] == new_mid[right - 1] {
            left -= 1;
            right -= 1;
        } else if right == 0
            || (left > 0 && table[(left - 1) * width + right] > table[left * width + right - 1])
        {
            backward.push(row(WorkspaceGitDiffLineKind::Remove, old_mid[left - 1]));
            left -= 1;
        } else {
            backward.push(row(WorkspaceGitDiffLineKind::Add, new_mid[right - 1]));
            right -= 1;
        }
    }
    backward.reverse();
    backward
}

/// The rows of a provider's own unified diff: each hunk header whole, then the
/// hunk's lines without their `+`, `-` or space marker. A file's `---` and
/// `+++` header is not content, but Codex sends an added or deleted file as
/// bare `+` or `-` lines with no `@@`, so those count outside a hunk too.
pub(crate) fn unified_lines(text: &str) -> Vec<WorkspaceGitDiffLine> {
    let mut lines = Vec::new();
    let mut in_hunks = false;
    for line in text.lines() {
        if line.starts_with("diff --git") {
            in_hunks = false;
            continue;
        }
        if line.starts_with("@@") {
            in_hunks = true;
            lines.push(row(WorkspaceGitDiffLineKind::Header, line));
            continue;
        }
        if !in_hunks && (line.starts_with("--- ") || line.starts_with("+++ ")) {
            continue;
        }
        let kind = match line.as_bytes().first() {
            Some(b'+') => WorkspaceGitDiffLineKind::Add,
            Some(b'-') => WorkspaceGitDiffLineKind::Remove,
            Some(b' ') => WorkspaceGitDiffLineKind::Context,
            _ => continue,
        };
        lines.push(row(kind, &line[1..]));
    }
    lines
}

/// The added and removed line counts of a set of rows, as `(added, removed)`.
pub(crate) fn changed_counts(lines: &[WorkspaceGitDiffLine]) -> (u64, u64) {
    lines
        .iter()
        .fold((0, 0), |(added, removed), line| match line.kind {
            WorkspaceGitDiffLineKind::Add => (added + 1, removed),
            WorkspaceGitDiffLineKind::Remove => (added, removed + 1),
            WorkspaceGitDiffLineKind::Context | WorkspaceGitDiffLineKind::Header => {
                (added, removed)
            }
        })
}

/// The `(added, removed)` counts [`replace_lines`] gives for a middle it takes as
/// wholly changed, read without building the table or any row: the lines between
/// the shared head and tail, all removed and all added.
pub(crate) fn wholly_changed_counts(old: &str, new: &str) -> (u64, u64) {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let (old_mid, new_mid) = middle(&old_lines, &new_lines);
    (new_mid.len() as u64, old_mid.len() as u64)
}

fn row(kind: WorkspaceGitDiffLineKind, text: &str) -> WorkspaceGitDiffLine {
    WorkspaceGitDiffLine {
        kind,
        text: text.to_string(),
    }
}

/// The differing middle of two texts: their shared head and tail removed.
fn middle<'a, 'b>(old: &'a [&'b str], new: &'a [&'b str]) -> (&'a [&'b str], &'a [&'b str]) {
    let mut head = 0;
    while head < old.len() && head < new.len() && old[head] == new[head] {
        head += 1;
    }
    let mut tail = 0;
    while tail < old.len() - head
        && tail < new.len() - head
        && old[old.len() - 1 - tail] == new[new.len() - 1 - tail]
    {
        tail += 1;
    }
    (&old[head..old.len() - tail], &new[head..new.len() - tail])
}

/// The LCS table over a differing middle, or `None` when the pair is past
/// [`MAX_DIFF_CELLS`] and the caller must take the middle as wholly changed.
fn lcs_table(old: &[&str], new: &[&str]) -> Option<Vec<u32>> {
    if (old.len() + 1).saturating_mul(new.len() + 1) > MAX_DIFF_CELLS {
        return None;
    }
    let width = new.len() + 1;
    let mut table = vec![0u32; (old.len() + 1) * width];
    for left in 1..=old.len() {
        for right in 1..=new.len() {
            table[left * width + right] = if old[left - 1] == new[right - 1] {
                table[(left - 1) * width + right - 1] + 1
            } else {
                table[(left - 1) * width + right].max(table[left * width + right - 1])
            };
        }
    }
    Some(table)
}

#[cfg(test)]
#[path = "line_diff_tests.rs"]
mod tests;
