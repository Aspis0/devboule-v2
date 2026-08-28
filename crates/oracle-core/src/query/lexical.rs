//! Verbatim port of the Oracle lexical scoring stack from Python.
//!
//! Ported from `oracle/server/query_engine.py` with golden-verified parity.
//! All scoring uses f64 throughout, matching Python's float semantics.
//! Accumulation order matches Python to preserve float equality.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const STOPWORDS: &[&str] = &[
    "about", "after", "and", "are", "can", "does", "for", "from", "how", "into", "the", "this",
    "that", "what", "when", "where", "which", "with",
];

// ---------------------------------------------------------------------------
// Tokenization regex (compiled once via OnceLock)
// ---------------------------------------------------------------------------

fn token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[a-z0-9_/-]+").unwrap())
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn ends_with_any(s: &str, suffixes: &[&str]) -> bool {
    suffixes.iter().any(|suffix| s.ends_with(suffix))
}

fn starts_with_any(s: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| s.starts_with(prefix))
}

// ---------------------------------------------------------------------------
// ScoredChunk — input type for the scorer (mirrors the Python chunk dict)
// ---------------------------------------------------------------------------

/// A chunk ready for lexical scoring.  Fields correspond exactly to the
/// Python `chunk.get(...)` calls in `query_engine.py`.
#[derive(Debug, Clone, Deserialize)]
pub struct ScoredChunk {
    pub id: String,
    #[serde(default)]
    pub file_id: String,
    #[serde(default)]
    pub file_sorgente: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub chunk_index: usize,
    #[serde(default)]
    pub start_char: usize,
    #[serde(default)]
    pub end_char: usize,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub symbol_name: String,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub line_start: usize,
    #[serde(default)]
    pub line_end: usize,
    #[serde(default)]
    pub symbols_used: String,
    #[serde(default)]
    pub area: String,
    #[serde(default)]
    pub cluster_semantic: String,
    #[serde(default)]
    pub label: String,
}

// ---------------------------------------------------------------------------
// ChunkContextPayload — output of lexical_chunk_context
// ---------------------------------------------------------------------------

/// Result payload produced by [`lexical_chunk_context`].
/// Mirrors the Python `chunk_context_payload()` return dict.
#[derive(Debug, Clone)]
pub struct ChunkContextPayload {
    pub chunk_id: String,
    pub file_source: String,
    pub chunk_index: usize,
    pub start_char: usize,
    pub end_char: usize,
    pub score: f64,
    pub retrieval: String,
    pub text: String,
    pub last_modified: String,
    pub kind: String,
    pub symbol_name: String,
    pub signature: String,
    pub language: String,
    pub line_start: usize,
    pub line_end: usize,
    pub symbols_used: String,
}

// ---------------------------------------------------------------------------
// query_terms — tokenization + stopword filtering
// ---------------------------------------------------------------------------

/// Extract query terms: lowercase, regex tokenise `[a-z0-9_/-]+`,
/// drop tokens shorter than 3 chars and stopwords.
pub fn query_terms(query: &str) -> HashSet<String> {
    let lower = query.to_lowercase();
    token_re()
        .find_iter(&lower)
        .map(|m| m.as_str().to_string())
        .filter(|term| term.len() >= 3 && !STOPWORDS.contains(&term.as_str()))
        .collect()
}

// ---------------------------------------------------------------------------
// semantic_expansions — hand-built synonym map (every entry byte-exact)
// ---------------------------------------------------------------------------

/// Expand query terms into semantic synonyms.  Condition order matches
/// the Python source exactly.
pub fn semantic_expansions(terms: &HashSet<String>) -> HashSet<String> {
    let mut expanded = HashSet::new();

    if ["limit", "limits", "limiting", "limited"]
        .iter()
        .any(|t| terms.contains(*t))
    {
        expanded.extend(
            [
                "cap",
                "caps",
                "control",
                "controls",
                "max_scale",
                "min_scale",
                "scale-to-zero",
            ]
            .map(str::to_string),
        );
    }
    if ["spawn", "spawning"].iter().any(|t| terms.contains(*t)) {
        expanded.extend(
            [
                "provision",
                "provisioning",
                "create",
                "creation",
                "cold start",
                "scale-to-zero",
            ]
            .map(str::to_string),
        );
    }
    if terms.contains("gpu") {
        expanded.extend(["cuda", "vram"].map(str::to_string));
    }
    if [
        "output", "outputs", "result", "results", "release", "download",
    ]
    .iter()
    .any(|t| terms.contains(*t))
    {
        expanded.extend(["artifact", "download"].map(str::to_string));
    }
    if ["successful", "success", "completed", "complete"]
        .iter()
        .any(|t| terms.contains(*t))
    {
        expanded.extend(["done", "ready", "terminal"].map(str::to_string));
    }
    if ["privacy", "private", "safe", "zdr", "gdpr"]
        .iter()
        .any(|t| terms.contains(*t))
    {
        expanded
            .extend(["zdr", "gdpr", "zero data retention", "allowed provider"].map(str::to_string));
    }
    if [
        "agent", "agents", "terminal", "task", "tasks", "finished", "done",
    ]
    .iter()
    .any(|t| terms.contains(*t))
    {
        expanded.extend(
            [
                "project_claim_task",
                "project_update_status",
                "oracle_ask",
                "oracle_context",
                "read_project",
            ]
            .map(str::to_string),
        );
    }
    if ["paid", "stop", "stops", "cleanup", "resources", "resource"]
        .iter()
        .any(|t| terms.contains(*t))
    {
        expanded.extend(["delete", "release"].map(str::to_string));
    }

    expanded
}

// ===================================================================
// Source-quality bonus — generic path heuristics, not query-specific.
// ===================================================================

/// Prefer implementation files over tests/docs/generated output when the
/// query asks how/control. Gated: only added when the base score is already > 0.
fn source_quality_bonus(query: &str, terms: &HashSet<String>, source: &str) -> f64 {
    let q = query.to_lowercase();
    let asks_for_tests = ["test", "tests", "spec", "coverage", "regression"]
        .iter()
        .any(|t| terms.contains(*t));
    let asks_for_plan = [
        "plan",
        "plans",
        "roadmap",
        "proposal",
        "handoff",
        "docs",
        "documentation",
    ]
    .iter()
    .any(|t| terms.contains(*t));
    // `where`/`which` were listed here but are stopwords, so they never reached `terms` and never fired; counting them is a ranking decision to measure, not to revive in a prune.
    let asks_for_implementation =
        q.contains("how") || ["control", "controls"].iter().any(|t| terms.contains(*t));

    let mut bonus = 0.0_f64;

    let real_source_prefixes = ["src-tauri/src/", "src/"];
    if starts_with_any(source, &real_source_prefixes) {
        bonus += 3.0;
        if asks_for_implementation {
            bonus += 4.0;
        }
    }

    if source.contains("/tests/")
        || ends_with_any(source, &[".test.js", ".test.ts", ".spec.js", ".spec.ts"])
    {
        if asks_for_tests {
            bonus += 1.0;
        } else {
            bonus -= 10.0;
        }
    }

    let planning_markers = [
        "/docs/", " plan/", "-plan.", "roadmap", "handoff", "session", "bug log", "bugs.md",
        "proposal",
    ];
    if ends_with_any(source, &[".md", ".txt"])
        || planning_markers.iter().any(|m| source.contains(m))
    {
        if asks_for_plan {
            bonus += 1.0;
        } else {
            bonus -= 8.0;
        }
    }

    let generated_markers = ["/dist/", "/build/", "/coverage/", ".min.js", ".bundle.js"];
    if generated_markers.iter().any(|m| source.contains(m)) {
        bonus -= 8.0;
    }

    bonus
}

// ===================================================================
// Core scoring function
// ===================================================================

/// Compute the lexical score for a single chunk against a query.
///
/// Base term scoring, then semantic expansions, then source-quality
/// (only when the base score is already positive), then clamp.
pub fn lexical_chunk_score(query: &str, terms: &HashSet<String>, chunk: &ScoredChunk) -> f64 {
    let source = chunk.file_sorgente.to_lowercase();
    let text = chunk.text.to_lowercase();

    // Base term scoring: +1.0 per term in text, +0.35 per term in source
    let mut score = 0.0_f64;
    for term in terms {
        if text.contains(term.as_str()) {
            score += 1.0;
        }
        if source.contains(term.as_str()) {
            score += 0.35;
        }
    }

    // Semantic expansion scoring: +0.55 per matched expansion in text
    for synonym in semantic_expansions(terms) {
        if text.contains(synonym.as_str()) {
            score += 0.55;
        }
    }

    if score > 0.0 {
        score += source_quality_bonus(query, terms, &source);
    }

    score.max(0.0)
}

// ===================================================================
// chunk_context_payload — mirrors Python's chunk_context_payload()
// ===================================================================

/// Build a context payload from a chunk, score, and retrieval tag.
/// Mirrors the Python `chunk_context_payload()` function.
pub fn chunk_context_payload(
    chunk: &ScoredChunk,
    score: f64,
    retrieval: &str,
) -> ChunkContextPayload {
    ChunkContextPayload {
        chunk_id: chunk.id.clone(),
        file_source: chunk.file_sorgente.clone(),
        chunk_index: chunk.chunk_index,
        start_char: chunk.start_char,
        end_char: chunk.end_char,
        score,
        retrieval: retrieval.to_string(),
        text: chunk.text.clone(),
        last_modified: String::new(), // not available from chunk dict in golden fixture
        kind: chunk.kind.clone(),
        symbol_name: chunk.symbol_name.clone(),
        signature: chunk.signature.clone(),
        language: chunk.language.clone(),
        line_start: chunk.line_start,
        line_end: chunk.line_end,
        symbols_used: chunk.symbols_used.clone(),
    }
}

// ===================================================================
// lexical_chunk_context — standalone ranking over a plain chunk list
// ===================================================================

/// Rank chunks by lexical relevance to the query.
///
/// Doc note: ported from the Python `lexical_chunk_context`.  The Python
/// version receives chunks from a SQLite store; this Rust version takes a
/// plain slice of [`ScoredChunk`], mirroring the pure ranking core.
pub fn lexical_chunk_context(
    query: &str,
    chunks: &[ScoredChunk],
    limit: usize,
) -> Vec<ChunkContextPayload> {
    let terms = query_terms(query);
    if terms.is_empty() {
        return vec![];
    }
    let mut rows: Vec<ChunkContextPayload> = Vec::new();
    for chunk in chunks {
        let score = lexical_chunk_score(query, &terms, chunk);
        if score > 0.0 {
            rows.push(chunk_context_payload(chunk, score, "lexical"));
        }
    }
    rows.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.file_source.cmp(&b.file_source))
            .then_with(|| a.chunk_index.cmp(&b.chunk_index))
    });
    let n = limit.max(1);
    rows.truncate(n);
    rows
}

// ===================================================================
// Tests
// ===================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_terms_basic() {
        let terms = query_terms("How do agents claim tasks and update project status?");
        let expected: HashSet<String> = ["agents", "claim", "project", "status", "tasks", "update"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(terms, expected);
    }

    #[test]
    fn test_query_terms_stopwords() {
        let terms = query_terms("What is the architecture of this project?");
        assert!(terms.contains("architecture"));
        assert!(terms.contains("project"));
        assert!(!terms.contains("what"));
        assert!(!terms.contains("the"));
        assert!(!terms.contains("is"));
    }

    #[test]
    fn test_semantic_expansions_agent() {
        let terms: HashSet<String> = ["agents", "claim", "project", "status", "tasks", "update"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let exp = semantic_expansions(&terms);
        assert!(exp.contains("project_claim_task"));
        assert!(exp.contains("project_update_status"));
        assert!(exp.contains("oracle_ask"));
        assert!(exp.contains("oracle_context"));
        assert!(exp.contains("read_project"));
    }
}
