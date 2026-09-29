//! Pure parsing of the `git log` record stream: the record-separated,
//! NUL-field-separated format the log read asks git for, into the commit
//! entries the reply carries. No process is spawned here and no repository
//! is opened, so every rule below is provable from a literal string
//! (`workspace_git_log_parse_tests.rs`); the orchestration that runs `git`
//! lives in `workspace_git_log.rs`.
//!
//! Translated from Paseo's `parseCheckoutCommitRecords` (Apache-2.0,
//! Copyright (c) 2025-present Mohamed Boudra,
//! `packages/server/src/utils/checkout-git.ts`), modified for this daemon:
//! the `--raw --numstat` half of Paseo's parser is gone with the files array
//! it fed — a record here is the header line and nothing else.

/// Bytes git emits between fields and records. We split parsed output on
/// these — `%x00`/`%x1e` in the `--format` string git itself expands, so a
/// subject carrying any byte but a NUL stays parseable.
const FIELD_SEPARATOR: char = '\0';
const RECORD_SEPARATOR: char = '\u{1e}';

/// One commit of the run, in git's own order (newest first). The two
/// classification flags are absent here: they are facts about remotes and
/// the base ref, which this stream does not carry — the orchestrator
/// fills them onto the wire entry.
#[derive(Debug)]
pub(super) struct Commit {
    pub(super) sha: String,
    pub(super) short_sha: String,
    pub(super) author_name: String,
    pub(super) author_date: String,
    pub(super) subject: String,
}

/// The commits of one `git log` run. A record git could not give five
/// fields, or whose sha is empty, is skipped rather than guessed at — the
/// same rule the source's parser applies.
pub(super) fn parse_commit_records(stdout: &str) -> Vec<Commit> {
    stdout
        .split(RECORD_SEPARATOR)
        .filter(|record| !record.is_empty())
        .filter_map(parse_record)
        .collect()
}

/// One record: the NUL-field-separated header line. A record cut short by
/// the accumulator's cap keeps fewer than five fields and is skipped — the
/// caller flags the truncation instead of guessing at a partial header.
fn parse_record(record: &str) -> Option<Commit> {
    let mut lines = record.split('\n');
    let fields: Vec<&str> = lines.next()?.split(FIELD_SEPARATOR).collect();
    if fields.len() < 5 {
        return None;
    }
    let sha = fields.first()?.trim();
    if sha.is_empty() {
        return None;
    }
    Some(Commit {
        sha: sha.to_string(),
        short_sha: fields.get(1)?.trim().to_string(),
        author_name: fields.get(2)?.to_string(),
        author_date: fields.get(3)?.trim().to_string(),
        subject: fields.get(4)?.to_string(),
    })
}

#[cfg(test)]
#[path = "workspace_git_log_parse_tests.rs"]
mod tests;
