//! Construction of the query engine over Oracle's stores, and the mapping of
//! engine contexts into the results the panel cites — line ranges, focus
//! windows, and match types included.

use std::path::Path;

use oracle_core::redact_secret_tokens;
use oracle_core::{ContextChunk, LanceStore, QueryEngine, SharedReranker, SqliteStore};

use crate::backend::error::CommandError;

use super::errors::core_error;
use super::runtime::ResolvedOraclePaths;
use super::types::{OracleMatchType, OracleResult};

pub(super) fn open_engine(
    paths: &ResolvedOraclePaths,
    reranker: Option<SharedReranker>,
) -> Result<QueryEngine, CommandError> {
    let sqlite = SqliteStore::new(&paths.data.metadata)
        .map_err(|error| core_error("opening Oracle metadata store failed", error))?;
    Ok(QueryEngine::new(
        sqlite,
        LanceStore::new(&paths.data.vectors),
        Some(LanceStore::new(&paths.data.chunks)),
        Some(LanceStore::new(&paths.data.file_vectors)),
    )
    .with_reranker(reranker))
}

pub(super) fn result_from_context(root: &Path, context: &ContextChunk) -> OracleResult {
    let (line_start, line_end) = line_range(root, context);
    let (focus_line_start, focus_line_end) = match context.focus {
        Some(focus) => focus_range(line_start, line_end, focus),
        None => (None, None),
    };
    OracleResult {
        path: context.file_source.clone(),
        line_start,
        line_end,
        focus_line_start,
        focus_line_end,
        snippet: redact_secret_tokens(&context.text),
        score: context.score,
        symbol_name: (!context.symbol_name.is_empty()).then(|| context.symbol_name.clone()),
        match_type: match context.retrieval.as_str() {
            "lexical" => Some(OracleMatchType::Lexical),
            "dense" => Some(OracleMatchType::Dense),
            "dense+lexical" => Some(OracleMatchType::DenseLexical),
            "dense+reranked" => Some(OracleMatchType::DenseReranked),
            _ => None,
        },
    }
}

/// Turn a chunk-relative focus window into absolute file lines.
///
/// The engine reports the window as an offset into the chunk text because only
/// this layer knows the chunk's line base: code chunks carry it in the index,
/// prose chunks have it derived from character offsets just above. A chunk with
/// no known base (both ends zero) gets no focus rather than a guessed one, and
/// a window that would fall outside the chunk's own range is dropped for the
/// same reason — a citation that cannot be trusted is worse than a wide one.
fn focus_range(
    line_start: usize,
    line_end: usize,
    focus: oracle_core::FocusSpan,
) -> (Option<usize>, Option<usize>) {
    if line_start == 0 || focus.line_count == 0 {
        return (None, None);
    }
    let start = line_start.saturating_add(focus.line_offset);
    if start > line_end {
        return (None, None);
    }
    let end = start
        .saturating_add(focus.line_count.saturating_sub(1))
        .min(line_end);
    (Some(start), Some(end))
}

fn line_range(root: &Path, context: &ContextChunk) -> (usize, usize) {
    if context.line_start > 0 || context.line_end > 0 {
        return (context.line_start, context.line_end.max(context.line_start));
    }
    let relative = Path::new(&context.file_source);
    if relative.is_absolute() {
        return (0, 0);
    }
    let Ok(source) = std::fs::read_to_string(root.join(relative)) else {
        return (0, 0);
    };
    let start = floor_char_boundary(&source, context.start_char.min(source.len()));
    let end = floor_char_boundary(&source, context.end_char.min(source.len()));
    if end < start {
        return (0, 0);
    }
    let line_start = source[..start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let line_end = source[..end].bytes().filter(|byte| *byte == b'\n').count() + 1;
    (line_start, line_end.max(line_start))
}

fn floor_char_boundary(value: &str, index: usize) -> usize {
    let mut index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}
