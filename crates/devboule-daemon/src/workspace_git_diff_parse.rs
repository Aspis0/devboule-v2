//! Pure parsing of one file's diff: the bytes `git diff` printed become the
//! lines the panel renders, and the bytes of an untracked file become the
//! lines git would print for it. No process is spawned here and no
//! repository is opened, so every rule below is provable from a literal
//! string or a plain temp file (`workspace_git_diff_parse_tests.rs`); the
//! orchestration that runs `git` lives in `workspace_git_diff.rs`.

use std::io::Read;
use std::path::Path;

use devboule_protocol::{WorkspaceGitDiffLine, WorkspaceGitDiffLineKind};

use super::DIFF_FILE_MAX_BYTES;

/// What `git status --porcelain=v2 -z -- <path>` said about one path. Only
/// the record kind matters: the pathspec already scoped the dump to the one
/// file, and a rename's original path — the bare record `-z` writes next —
/// would never be classified, since only its position identifies it.
pub(super) enum PathRecord {
    Untracked,
    Tracked,
}

pub(super) fn path_record(stdout: &str) -> Option<PathRecord> {
    let first = stdout.split('\0').find(|record| !record.is_empty())?;
    match first.get(..2)? {
        "? " => Some(PathRecord::Untracked),
        "1 " | "2 " | "u " => Some(PathRecord::Tracked),
        _ => None,
    }
}

/// One parsed diff of one file: the flags git states in the preamble, the
/// lines of its hunks, and whether git called the file binary. `Debug` is
/// for the test assertions that quote it.
#[derive(Debug)]
pub(super) struct ParsedDiff {
    pub(super) is_new: bool,
    pub(super) is_deleted: bool,
    pub(super) binary: bool,
    pub(super) lines: Vec<WorkspaceGitDiffLine>,
}

/// Split one `git diff` of one path into lines by kind. The preamble before
/// the first hunk carries git's own verdicts — `new file mode`,
/// `deleted file mode`, `Binary files … differ`, and the `/dev/null` halves
/// of the `---`/`+++` pair — and is not line content; past the first `@@`,
/// every line belongs to a hunk except git's metadata lines (`\ No newline
/// at end of file`), which are skipped rather than guessed at.
pub(super) fn parse_diff(stdout: &str) -> ParsedDiff {
    let mut parsed = ParsedDiff {
        is_new: false,
        is_deleted: false,
        binary: false,
        lines: Vec::new(),
    };
    let mut in_hunks = false;
    for line in stdout.lines() {
        if line.starts_with("@@") {
            in_hunks = true;
            parsed
                .lines
                .push(line_of(WorkspaceGitDiffLineKind::Header, line));
            continue;
        }
        if !in_hunks {
            if line.starts_with("new file mode") {
                parsed.is_new = true;
            } else if line.starts_with("deleted file mode") {
                parsed.is_deleted = true;
            } else if line.starts_with("--- /dev/null") {
                parsed.is_new = true;
            } else if line.starts_with("+++ /dev/null") {
                parsed.is_deleted = true;
            } else if line.starts_with("Binary file") {
                parsed.binary = true;
            }
            continue;
        }
        if let Some(text) = line.strip_prefix('+') {
            parsed
                .lines
                .push(line_of(WorkspaceGitDiffLineKind::Add, text));
        } else if let Some(text) = line.strip_prefix('-') {
            parsed
                .lines
                .push(line_of(WorkspaceGitDiffLineKind::Remove, text));
        } else if line.starts_with(' ') || line.is_empty() {
            // git prints an empty context line as a single space (measured);
            // without it, the line has no prefix to strip.
            let text = line.strip_prefix(' ').unwrap_or(line);
            parsed
                .lines
                .push(line_of(WorkspaceGitDiffLineKind::Context, text));
        }
    }
    parsed
}

fn line_of(kind: WorkspaceGitDiffLineKind, text: &str) -> WorkspaceGitDiffLine {
    WorkspaceGitDiffLine {
        kind,
        text: text.to_string(),
    }
}

/// What reading an untracked file produced. `Lines` is the synthesis —
/// every line an addition, because that is what `git diff HEAD` would say
/// if the path were tracked. The synthesis exists because
/// `git diff --no-index /dev/null <file>` is not portable on Windows;
/// `Binary` is git's own rule (a NUL byte), checked here before any text
/// is handed out; `TooLarge` and `Unreadable` are refusals to invent an
/// answer.
pub(super) enum UntrackedFile {
    Lines(Vec<WorkspaceGitDiffLine>),
    Binary,
    TooLarge,
    Unreadable,
}

/// The lines of one untracked file, bounded by [`DIFF_FILE_MAX_BYTES`] the
/// same way the status slice bounds its line counter. One trailing `\r` is
/// stripped from each line — the normalization `str::lines()` already gives
/// git's own output in [`parse_diff`], so a CRLF file reads the same
/// whether its lines came from git or from the file itself. Bytes otherwise
/// untouched: invalid UTF-8 is replaced lossily, and git itself calls a
/// file binary only for a NUL.
pub(super) fn untracked_file(path: &Path) -> UntrackedFile {
    // `diff_of` walked every component of this path without following and
    // refused any link before this runs, so this open never follows a link —
    // unless one is swapped in between those stats and this open (the
    // declared stat→open race).
    let Ok(mut file) = std::fs::File::open(path) else {
        return UntrackedFile::Unreadable;
    };
    let mut contents = Vec::new();
    let mut buffer = [0u8; 8 * 1024];
    loop {
        let Ok(read) = file.read(&mut buffer) else {
            return UntrackedFile::Unreadable;
        };
        if read == 0 {
            break;
        }
        if buffer[..read].contains(&0) {
            return UntrackedFile::Binary;
        }
        contents.extend_from_slice(&buffer[..read]);
        if contents.len() as u64 > DIFF_FILE_MAX_BYTES {
            // The size was stat'ed before this file was opened; this guard
            // covers the file that grew in between. No test can hold a
            // writer still — the same declared residual as the status
            // slice's untracked counter.
            return UntrackedFile::TooLarge;
        }
    }
    let text = String::from_utf8_lossy(&contents);
    if text.is_empty() {
        return UntrackedFile::Lines(Vec::new());
    }
    let mut lines = text
        .split('\n')
        .map(|content| {
            let content = content.strip_suffix('\r').unwrap_or(content);
            line_of(WorkspaceGitDiffLineKind::Add, content)
        })
        .collect::<Vec<_>>();
    if text.ends_with('\n') {
        // The terminator ends the last line; it is not a line of its own.
        lines.pop();
    }
    UntrackedFile::Lines(lines)
}

#[cfg(test)]
#[path = "workspace_git_diff_parse_tests.rs"]
mod tests;
