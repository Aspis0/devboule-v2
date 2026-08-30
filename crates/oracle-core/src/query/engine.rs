//! Query engine orchestration — Rust port of `oracle/server/query_engine.py`.
//!
//! Ports the QUERY ORCHESTRATION layer (context merge, similar node lookup,
//! cluster reads, health/snapshot).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::sync::OnceLock;

use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config;
use crate::ingest::retrieval_text;
use crate::query::focus::{self, FocusSpan};
use crate::query::lexical::{self, ScoredChunk};
use crate::query::redact::redact_secret_tokens;
use crate::query::reranker::{self, SharedReranker};
use crate::store::lance::{hash_embed, LanceHit, LanceStore};
use crate::store::sqlite::{FileChunk, NodeCard, SqliteStore};

/// Hard cap on chunks materialized/scored on the lexical path per query.
/// Prevents O(corpus) RAM spikes under concurrent agent retrieval.
const MAX_LEXICAL_SCAN: usize = 10_000;

/// Standard reciprocal-rank-fusion damping constant.
const RRF_K: f64 = 60.0;

fn rrf_contribution(rank: usize) -> f64 {
    1.0 / (RRF_K + rank as f64)
}

fn query_asks_for_tests(query: &str) -> bool {
    let lower = query.to_lowercase();
    [
        "test",
        "tests",
        "testing",
        "regression",
        "fixture",
        "fixtures",
        "spec",
        "assert",
        "assertion",
        "vitest",
    ]
    .iter()
    .any(|term| {
        lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == *term)
    })
}

/// Keep test hits after non-test hits for ordinary implementation questions.
/// The operation is a stable rank partition: it adds no score or model weight.
fn demote_test_rows_to_tail(rows: &mut Vec<ContextChunk>) {
    let mut non_tests = Vec::with_capacity(rows.len());
    let mut tests = Vec::new();
    for row in rows.drain(..) {
        if retrieval_text::is_test_source(&row.file_source) {
            tests.push(row);
        } else {
            non_tests.push(row);
        }
    }
    rows.extend(non_tests);
    rows.extend(tests);
}

fn apply_test_policy(query: &str, rows: &mut Vec<ContextChunk>) {
    if !query_asks_for_tests(query) {
        demote_test_rows_to_tail(rows);
    }
}

#[cfg(test)]
mod diversity_tests {
    use super::*;

    fn row(file_source: &str, chunk_id: &str) -> ContextChunk {
        ContextChunk {
            chunk_id: chunk_id.to_string(),
            file_source: file_source.to_string(),
            chunk_index: 0,
            start_char: 0,
            end_char: 10,
            score: 0.0,
            rerank_score: None,
            focus: None,
            retrieval: "dense".to_string(),
            text: String::new(),
            last_modified: String::new(),
            kind: String::new(),
            symbol_name: String::new(),
            signature: String::new(),
            language: String::new(),
            line_start: 1,
            line_end: 10,
            symbols_used: Vec::new(),
        }
    }

    #[test]
    fn diversification_keeps_first_ranked_chunk_per_file() {
        let rows = vec![
            row("src/session.rs", "session-a"),
            row("src/session.rs", "session-b"),
            row("README.md", "readme"),
            row("src/query.rs", "query"),
        ];
        let selected = diversify_context_rows(rows, 3);
        assert_eq!(
            selected
                .iter()
                .map(|row| row.chunk_id.as_str())
                .collect::<Vec<_>>(),
            vec!["session-a", "readme", "query"]
        );
    }

    #[test]
    fn diversification_is_empty_for_zero_limit() {
        assert!(diversify_context_rows(vec![row("a", "a")], 0).is_empty());
    }

    #[test]
    fn ordinary_questions_demote_test_hits_without_changing_scores() {
        let mut rows = vec![row("tests/query.rs", "test"), row("src/query.rs", "impl")];
        apply_test_policy("Where is the lexical fallback decided?", &mut rows);
        assert_eq!(rows[0].chunk_id, "impl");
        assert_eq!(rows[1].chunk_id, "test");
    }

    #[test]
    fn test_questions_keep_test_hits_at_their_rank() {
        let mut rows = vec![row("tests/query.rs", "test"), row("src/query.rs", "impl")];
        apply_test_policy("Which tests cover the lexical fallback?", &mut rows);
        assert_eq!(rows[0].chunk_id, "test");
    }
}

/// Score descending, then file and chunk index so equal scores stay ordered.
/// RRF makes ties common: its scores come from a small discrete set of ranks.
fn sort_context_rows(rows: &mut [ContextChunk]) {
    rows.sort_by(|a, b| {
        b.rerank_score
            .unwrap_or(b.score)
            .partial_cmp(&a.rerank_score.unwrap_or(a.score))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.file_source.cmp(&b.file_source))
            .then_with(|| a.chunk_index.cmp(&b.chunk_index))
    });
}

/// Keep the result list at file granularity.
///
/// The public result is a list of files, while the vector index is a list of
/// chunks.  Selecting the first ranked chunk for each file prevents repeated
/// or overlapping line ranges from consuming the caller's result slots.  It
/// is deliberately a hard file boundary rather than a score adjustment: the
/// file-level evaluation and the UI both treat `limit` as a number of files.
pub fn diversify_context_rows(rows: Vec<ContextChunk>, limit: usize) -> Vec<ContextChunk> {
    if limit == 0 {
        return Vec::new();
    }
    let mut seen_files = HashSet::new();
    rows.into_iter()
        .filter(|row| seen_files.insert(row.file_source.clone()))
        .take(limit)
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// Traits
// ═══════════════════════════════════════════════════════════════════════════

/// Query embedding trait — decouples the engine from any specific embedder.
pub trait QueryEmbedder: Send + Sync {
    fn embed_query(&self, text: &str, dims: usize) -> Result<Vec<f32>>;
    /// Return the loaded model's width when the embedder knows it. `None` is
    /// reserved for dimensionless fallbacks such as the hash embedder.
    fn dims(&self) -> Result<Option<usize>> {
        Ok(None)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// HashQueryEmbedder — deterministic blake2b fallback
// ═══════════════════════════════════════════════════════════════════════════

/// Hash-based query embedder, mirroring the `ORACLE_QUERY_EMBEDDER=hash` /
/// `require_real_embedder` gate logic from Python
/// `lance_store.py::embed_query_text`.
pub struct HashQueryEmbedder;

impl QueryEmbedder for HashQueryEmbedder {
    fn embed_query(&self, text: &str, dims: usize) -> Result<Vec<f32>> {
        // When require_real_embedder is set, hash embeddings are garbage
        // (they retrieve random results).  Match Python's hard block.
        if config::require_real_embedder() {
            anyhow::bail!(
                "Qwen embedding model is unavailable. \
                 Run Oracle doctor / check the runtime install."
            );
        }
        let prefixed = retrieval_text::query_embedding_text(text, None);
        Ok(hash_embed(&prefixed, dims))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Response types — snake_case JSON matches Python dicts EXACTLY
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ContextChunk {
    pub chunk_id: String,
    pub file_source: String,
    pub chunk_index: usize,
    pub start_char: usize,
    pub end_char: usize,
    /// Reciprocal-rank-fusion score; it is not a cosine similarity.
    pub score: f64,
    /// Raw query/document score when an optional cross-encoder reordered this
    /// candidate. It is query-time metadata and never enters the index recipe.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_score: Option<f64>,
    /// Sub-span of this chunk that the cross-encoder scored highest, in lines
    /// relative to `text`. Advisory and additive: the whole chunk stays in
    /// `text` and the chunk's own range stays in `line_start`/`line_end`, so a
    /// caller that ignores this field sees exactly what it saw before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<FocusSpan>,
    pub retrieval: String,
    pub text: String,
    pub last_modified: String,
    pub kind: String,
    pub symbol_name: String,
    pub signature: String,
    pub language: String,
    pub line_start: usize,
    pub line_end: usize,
    pub symbols_used: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResultEntry {
    pub id: String,
    pub label: String,
    pub node_type: String,
    pub cluster: i64,
    pub score: f64,
    pub file_source: String,
    pub function_primary: String,
    pub dependencies: Vec<String>,
    pub chunk_id: Option<String>,
    pub chunk_index: Option<i64>,
    pub start_char: Option<i64>,
    pub end_char: Option<i64>,
    pub chunk_preview: String,
    pub kind: String,
    pub symbol_name: String,
    pub signature: String,
    pub language: String,
    pub line_start: i64,
    pub line_end: i64,
    pub symbols_used: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GroupEntry {
    pub file: String,
    pub total_score: f64,
    pub chunks: Vec<GroupChunk>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub symbol_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub signature: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub language: String,
    pub line_start: i64,
    pub line_end: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GroupChunk {
    pub chunk_id: String,
    pub score: f64,
    pub retrieval: String,
    pub start_char: usize,
    pub end_char: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HealthResponse {
    pub status: String,
    pub phase: String,
    pub nodes: usize,
    pub vector_records: usize,
    pub chunk_files: usize,
    pub chunk_records: usize,
    pub chunk_vector_records: usize,
    pub chunk_profile: String,
    pub query_profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SnapshotResponse {
    pub status: String,
    pub source: String,
    pub phase: String,
    pub node_count: usize,
    pub edge_count: usize,
    pub cluster_count: usize,
    pub duplicate_labels: Vec<DuplicateGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DuplicateGroup {
    pub label: String,
    pub node_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterResponse {
    pub epoch: String,
    pub clusters: Vec<ClusterInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterInfo {
    #[serde(rename = "clusterId")]
    pub cluster_id: i64,
    pub size: usize,
    #[serde(rename = "sampleFiles")]
    pub sample_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMemberResponse {
    #[serde(rename = "clusterId")]
    pub cluster_id: i64,
    pub members: Vec<ClusterMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMember {
    #[serde(rename = "fileId")]
    pub file_id: String,
    pub score: f64,
}

// ═══════════════════════════════════════════════════════════════════════════
// QueryEngine
// ═══════════════════════════════════════════════════════════════════════════

pub struct QueryEngine {
    pub sqlite: SqliteStore,
    pub vectors: LanceStore,
    pub chunk_vectors: Option<LanceStore>,
    pub file_vectors: Option<LanceStore>,
    pub reranker: Option<SharedReranker>,
}

impl QueryEngine {
    /// Build the engine.  Mirrors `routes.py::make_engine` constructor wiring.
    pub fn new(
        sqlite: SqliteStore,
        vectors: LanceStore,
        chunk_vectors: Option<LanceStore>,
        file_vectors: Option<LanceStore>,
    ) -> Self {
        Self {
            sqlite,
            vectors,
            chunk_vectors,
            file_vectors,
            reranker: None,
        }
    }

    /// Attach an optional lazy reranker. The handle is shareable so a Tauri
    /// runtime can keep one loaded ONNX session across independently opened
    /// query engines.
    pub fn with_reranker(mut self, reranker: Option<SharedReranker>) -> Self {
        self.reranker = reranker;
        self
    }

    // ── helpers ──────────────────────────────────────────────────────────

    /// Embed the query text through the provided embedder, using the store's
    /// canonical dimension or the loaded model when the store is empty.
    async fn embed_query(&self, embedder: &dyn QueryEmbedder, query: &str) -> Result<Vec<f32>> {
        let dims = self.dims(embedder).await?;
        let vector = embedder.embed_query(query, dims)?;
        if vector.len() != dims {
            anyhow::bail!(
                "query embedder returned {} dimensions, expected {dims}",
                vector.len()
            );
        }
        Ok(vector)
    }

    /// Canonical embedding dimensionality, read from the chunk-vector store
    /// when available, otherwise from the loaded query model. The constant is
    /// only the last fallback for a dimensionless debug embedder and empty store.
    async fn dims(&self, embedder: &dyn QueryEmbedder) -> Result<usize> {
        // Only when rows exist: an empty table still declares the width it was
        // created with, and the loaded model is the better answer then. The
        // width comes from the schema rather than from a row because this runs
        // on every query, and reading a row meant loading the whole index to
        // look at one number.
        if let Some(ref cv) = self.chunk_vectors {
            if cv.count().await.unwrap_or(0) > 0 {
                if let Ok(Some(dims)) = cv.vector_dims().await {
                    return Ok(dims);
                }
            }
        }
        if let Some(dims) = embedder.dims()? {
            if dims == 0 {
                anyhow::bail!("query embedder declares zero dimensions");
            }
            return Ok(dims);
        }
        // No model or stored vector exists here: HashQueryEmbedder is the
        // dimensionless debug fallback, so there is no better source.
        Ok(config::EMBED_DIMS)
    }

    // ── public API ───────────────────────────────────────────────────────

    /// Dense + lexical context retrieval.
    ///
    /// `embedder` is REQUIRED: Python always embeds the query with its
    /// built-in model when the dense path runs (the only Python skip
    /// conditions are `prefer_lexical` and a missing chunk store, mirrored
    /// here). Callers that genuinely want lexical-only pass
    /// `prefer_lexical = true`, never a dummy embedder.
    // Mirrors the Python query-engine filter surface (kind/language/symbols/imports/module).
    #[allow(clippy::too_many_arguments)]
    pub async fn context(
        &self,
        query: &str,
        limit: usize,
        embedder: &dyn QueryEmbedder,
        allowed_file_ids: Option<&HashSet<String>>,
        prefer_lexical: bool,
        kind: Option<&str>,
        language: Option<&str>,
        symbols: Option<&[String]>,
        imports: Option<&[String]>,
        module: Option<&str>,
    ) -> Result<Vec<ContextChunk>> {
        let limit = limit.clamp(1, config::MAX_BOUNDED_LIMIT);
        let mut combined: HashMap<String, ContextChunk> = HashMap::new();

        // ── Dense path ───────────────────────────────────────────────
        if !prefer_lexical {
            if let Some(ref chunk_vectors) = self.chunk_vectors {
                let query_vec = self.embed_query(embedder, query).await?;
                // The index is chunk-level but the public result is file-level.
                // Search through the existing public bound so duplicate chunks
                // cannot consume all `limit` slots before file diversification.
                // A reranker may request a smaller candidate depth, but it still
                // needs at least the bound to give the final selector enough
                // distinct files to choose from.
                let search_limit = if self.reranker.is_some() {
                    limit
                        .max(reranker::resolve_candidate_limit())
                        .max(config::MAX_BOUNDED_LIMIT)
                } else {
                    limit.max(config::MAX_BOUNDED_LIMIT)
                };
                let hits = chunk_vectors.search(&query_vec, search_limit).await?;
                // Rank counts kept hits only. A filtered-out hit must not burn a
                // rank, or dense contributions shrink whenever filters are active
                // while the lexical side — already filtered before scoring — keeps
                // its ranks dense. Both sides rank the list they actually return.
                let mut rank = 0usize;
                for hit in hits {
                    if let Some(chunk) = self.sqlite.get_chunk(&hit.id)? {
                        if allowed_file_ids.is_none_or(|ids| ids.contains(&chunk.file_id))
                            && chunk_matches_filters(
                                &chunk, kind, language, symbols, imports, module,
                            )
                        {
                            rank += 1;
                            let ctx = ContextChunk::from_file_chunk(
                                &chunk,
                                rrf_contribution(rank),
                                "dense",
                            );
                            combined.insert(chunk.id.clone(), ctx);
                        }
                    }
                }
            }
        }

        // ── Lexical path: a fallback, not a second opinion ──────────────────
        // Measured on the frozen 40-question corpus with bge-small, recall@5 /
        // MRR@10: dense alone 0.725 / 0.549, rank-fused with lexical 0.650 /
        // 0.429, lexical alone 0.263 / 0.188. Fusing does not add signal the
        // lexical side does not have — RRF weights both lists equally, so a
        // retriever three times weaker drags the ranking down. It also cost a
        // 10,000-chunk scan on every query that already had a dense answer.
        //
        // This is a verdict on *our* lexical scorer, not on lexical retrieval:
        // it is an ad-hoc additive scale, not BM25, and its window covers 10,000
        // of 23,210 chunks. Exit condition: re-measure if a real BM25 over the
        // full corpus replaces it. The rank-fusion merge below is kept for that
        // day; today the dense branch above leaves nothing for it to merge.
        if !combined.is_empty() {
            let mut rows: Vec<ContextChunk> = combined.into_values().collect();
            sort_context_rows(&mut rows);
            if let Some(ref reranker) = self.reranker {
                let candidate_count = reranker::resolve_candidate_limit().min(rows.len());
                let mut tail = rows.split_off(candidate_count);
                let documents: Vec<String> = rows.iter().map(|row| row.text.clone()).collect();
                let scores = reranker.score_pairs(query, &documents)?;
                if scores.len() != rows.len() {
                    anyhow::bail!(
                        "reranker returned {} scores for {} candidates",
                        scores.len(),
                        rows.len()
                    );
                }
                for (row, score) in rows.iter_mut().zip(scores) {
                    row.rerank_score = Some(score);
                    row.retrieval = "dense+reranked".to_string();
                }
                sort_context_rows(&mut rows);
                rows.append(&mut tail);
            }
            apply_test_policy(query, &mut rows);
            let mut selected = diversify_context_rows(rows, limit.max(1));
            self.narrow_citations(query, &mut selected);
            return Ok(selected);
        }

        let total_chunks = self.sqlite.chunk_count().unwrap_or(0);
        let scan_cap = MAX_LEXICAL_SCAN;
        if total_chunks > scan_cap {
            eprintln!(
                "[oracle] lexical scan truncated: corpus has {total_chunks} chunks, \
                 scanning at most {scan_cap} (not full coverage)"
            );
        }
        let all_chunks = self.sqlite.all_chunks_limited(scan_cap)?;
        let filtered: Vec<FileChunk> = all_chunks
            .into_iter()
            .filter(|c| allowed_file_ids.is_none_or(|ids| ids.contains(&c.file_id)))
            .filter(|c| chunk_matches_filters(c, kind, language, symbols, imports, module))
            .collect();
        let scored: Vec<ScoredChunk> = filtered.iter().map(file_chunk_to_scored).collect();
        let lexical_limit = config::MAX_BOUNDED_LIMIT.max(limit);
        let lexical_results = lexical::lexical_chunk_context(query, &scored, lexical_limit);

        // Build a map from FileChunk id → FileChunk for fast lookup
        let chunk_by_id: HashMap<String, &FileChunk> =
            filtered.iter().map(|c| (c.id.clone(), c)).collect();

        for (rank, lr) in lexical_results.iter().enumerate() {
            let lexical_score = rrf_contribution(rank + 1);
            if let Some(existing) = combined.get_mut(&lr.chunk_id) {
                existing.score += lexical_score;
                existing.retrieval = "dense+lexical".to_string();
            } else if let Some(fc) = chunk_by_id.get(&lr.chunk_id) {
                let ctx = ContextChunk::from_file_chunk(fc, lexical_score, "lexical");
                combined.insert(lr.chunk_id.clone(), ctx);
            }
        }

        let mut rows: Vec<ContextChunk> = combined.into_values().collect();
        sort_context_rows(&mut rows);
        apply_test_policy(query, &mut rows);
        let mut selected = diversify_context_rows(rows, limit.max(1));
        self.narrow_citations(query, &mut selected);
        Ok(selected)
    }

    /// Point the citation at the lines that answer, not at the whole chunk.
    ///
    /// This runs on the *selected* results only, after diversification, so its
    /// cost is a function of `limit` and not of the candidate depth. All the
    /// windows of all the results go through one `score_pairs` call: the query
    /// side of the pair is identical, and batching lets the reranker fill its
    /// padded tensor instead of running one graph pass per result.
    ///
    /// A result whose chunk is too short to narrow, or that falls past the
    /// per-query window budget, simply keeps its chunk-wide range. That is a
    /// smaller citation lost, never a wrong one: nothing here can change which
    /// chunks were retrieved or in which order.
    ///
    /// Returns `()` and not `Result` on purpose. The focus is advisory, so a
    /// cross-encoder that fails here must cost the caller a hint, never the
    /// answer it already had — the reranking pass above may treat its own
    /// failure as fatal because it decides the *order*, but this only decides
    /// where to look inside a result that is already chosen. The failure is
    /// reported rather than swallowed: absence of `focus` is visible in the
    /// response, and the reason goes to the log.
    fn narrow_citations(&self, query: &str, rows: &mut [ContextChunk]) {
        if let Err(error) = self.try_narrow_citations(query, rows) {
            eprintln!("[oracle] citation focus unavailable for this query: {error:#}");
        }
    }

    fn try_narrow_citations(&self, query: &str, rows: &mut [ContextChunk]) -> Result<()> {
        let Some(ref reranker) = self.reranker else {
            return Ok(());
        };
        let mut plans: Vec<(usize, Vec<(usize, usize)>)> = Vec::new();
        let mut documents: Vec<String> = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let plan = focus::plan_focus_windows(row.text.lines().count());
            if plan.is_empty() || documents.len() + plan.len() > focus::MAX_FOCUS_WINDOWS_PER_QUERY
            {
                continue;
            }
            documents.extend(focus::window_texts(&row.text, &plan));
            plans.push((index, plan));
        }
        if documents.is_empty() {
            return Ok(());
        }
        let scores = reranker.score_pairs(query, &documents)?;
        if scores.len() != documents.len() {
            anyhow::bail!(
                "reranker returned {} scores for {} focus windows",
                scores.len(),
                documents.len()
            );
        }
        let mut cursor = 0usize;
        for (index, plan) in plans {
            let window_scores = &scores[cursor..cursor + plan.len()];
            cursor += plan.len();
            rows[index].focus = focus::select_focus(&plan, window_scores);
        }
        Ok(())
    }

    /// Similar nodes: node-card store first, file_vectors fallback.
    pub async fn similar(&self, node_id: &str, limit: usize) -> Result<Vec<ResultEntry>> {
        let limit = limit.clamp(1, config::MAX_BOUNDED_LIMIT);
        let hits = self.vectors.similar(node_id, limit).await?;
        if !hits.is_empty() {
            return Ok(hits
                .into_iter()
                .map(|h| self.hit_to_result_entry(h))
                .collect());
        }
        if let Some(ref fv) = self.file_vectors {
            let hits = fv.similar(node_id, limit).await?;
            return Ok(hits
                .into_iter()
                .map(|h| self.hit_to_result_entry(h))
                .collect());
        }
        Ok(vec![])
    }

    fn hit_to_result_entry(&self, hit: LanceHit) -> ResultEntry {
        let card = self.sqlite.get_node(&hit.id).ok().flatten();
        match card {
            Some(card) => ResultEntry {
                id: card.id.clone(),
                label: card.label.clone(),
                node_type: "file".to_string(),
                cluster: parse_cluster(&card.cluster_semantic),
                score: hit.score as f64,
                file_source: card.file_sorgente.clone(),
                function_primary: card.funzione_primaria.clone(),
                dependencies: card.dipende_da.clone(),
                chunk_id: None,
                chunk_index: None,
                start_char: None,
                end_char: None,
                chunk_preview: String::new(),
                kind: "file".to_string(),
                symbol_name: card.label.clone(),
                signature: String::new(),
                language: String::new(),
                line_start: 0,
                line_end: 0,
                symbols_used: card.dipende_da.clone(),
            },
            None => ResultEntry {
                id: hit.id.clone(),
                label: hit.label.clone(),
                node_type: "chunk".to_string(),
                cluster: 0,
                score: hit.score as f64,
                file_source: hit.id.clone(),
                // Python: summarize_chunk({}) fallback message for card-less rows.
                function_primary: "Chunk-level match from the full-file Oracle index.".to_string(),
                dependencies: vec![],
                chunk_id: None,
                chunk_index: None,
                start_char: None,
                end_char: None,
                chunk_preview: "Chunk-level match from the full-file Oracle index.".to_string(),
                kind: "text_slice".to_string(),
                symbol_name: hit.label.clone(),
                signature: String::new(),
                language: String::new(),
                line_start: 0,
                line_end: 0,
                symbols_used: vec![],
            },
        }
    }

    /// Node card lookup.
    pub fn node(&self, node_id: &str) -> Result<NodeCard> {
        self.sqlite
            .get_node(node_id)?
            .ok_or_else(|| anyhow::anyhow!("Node not found: {}", node_id))
    }

    /// Cluster members by name.
    pub fn cluster(&self, name: &str) -> Result<Vec<NodeCard>> {
        self.sqlite.by_cluster(name)
    }

    /// Area members by name.
    pub fn area(&self, name: &str) -> Result<Vec<NodeCard>> {
        self.sqlite.by_area(name)
    }

    /// Duplicate label groups.
    pub fn duplicates(&self) -> Result<Vec<Vec<String>>> {
        let all_nodes = self.sqlite.all_nodes()?;
        let mut by_label: HashMap<String, Vec<&NodeCard>> = HashMap::new();
        for node in &all_nodes {
            by_label.entry(node.label.clone()).or_default().push(node);
        }
        let mut groups: Vec<Vec<String>> = Vec::new();
        for nodes in by_label.values() {
            let areas: HashSet<&str> = nodes.iter().map(|n| n.area.as_str()).collect();
            if nodes.len() > 1 && areas.len() > 1 {
                let mut ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
                ids.sort();
                groups.push(ids);
            }
        }
        groups.sort_by(|a, b| a[0].cmp(&b[0]));
        Ok(groups)
    }

    /// Health probe.
    pub async fn health(&self) -> Result<HealthResponse> {
        let nodes = self.sqlite.all_nodes()?;
        let vector_records = self.vectors.count().await?;
        let chunk_files = self.sqlite.chunk_file_count()?;
        let chunk_records = self.sqlite.chunk_count()?;
        let chunk_vector_records = match self.chunk_vectors {
            Some(ref cv) => cv.count().await?,
            None => 0,
        };
        let last_updated = nodes
            .iter()
            .map(|n| n.ultima_modifica.as_str())
            .max()
            .map(|s| s.to_string());
        let last_updated = last_updated.filter(|s| !s.is_empty());

        Ok(HealthResponse {
            status: "ready".to_string(),
            phase: "phase1".to_string(),
            nodes: nodes.len(),
            vector_records,
            chunk_files,
            chunk_records,
            chunk_vector_records,
            chunk_profile: config::active_chunk_profile_version(None),
            query_profile: active_query_profile(),
            last_updated,
        })
    }

    /// Snapshot probe.
    pub async fn snapshot(&self) -> Result<SnapshotResponse> {
        let health = self.health().await?;
        let dup_groups = self.duplicates()?;
        let dup_labels: Vec<DuplicateGroup> = dup_groups
            .iter()
            .filter_map(|ids| {
                self.sqlite
                    .get_node(&ids[0])
                    .ok()
                    .flatten()
                    .map(|card| DuplicateGroup {
                        label: card.label,
                        node_ids: ids.clone(),
                    })
            })
            .collect();
        let all_nodes = self.sqlite.all_nodes()?;
        let cluster_count: usize = all_nodes
            .iter()
            .map(|n| n.cluster_semantic.clone())
            .collect::<HashSet<_>>()
            .len();

        Ok(SnapshotResponse {
            status: health.status,
            source: "rust-oracle".to_string(),
            phase: "phase1".to_string(),
            node_count: health.nodes,
            edge_count: 0,
            cluster_count,
            duplicate_labels: dup_labels,
        })
    }

    /// Clusters response (epoch + cluster list).
    pub fn clusters_response(&self) -> Result<ClusterResponse> {
        let rows = self.sqlite.get_file_clusters()?;
        let epoch = self.sqlite.get_clusters_epoch()?.unwrap_or_default();
        let mut by_cluster: BTreeMap<i64, Vec<&crate::store::sqlite::FileCluster>> =
            BTreeMap::new();
        for row in &rows {
            by_cluster.entry(row.cluster_id).or_default().push(row);
        }
        let clusters_list: Vec<ClusterInfo> = by_cluster
            .into_iter()
            .map(|(cid, members)| {
                let sample: Vec<String> =
                    members.iter().take(3).map(|m| m.file_id.clone()).collect();
                ClusterInfo {
                    cluster_id: cid,
                    size: members.len(),
                    sample_files: sample,
                }
            })
            .collect();
        Ok(ClusterResponse {
            epoch,
            clusters: clusters_list,
        })
    }

    /// Members of a specific cluster.
    pub fn cluster_members(&self, cluster_id: i64) -> Result<ClusterMemberResponse> {
        let members = self.sqlite.get_cluster_members(cluster_id)?;
        Ok(ClusterMemberResponse {
            cluster_id,
            members: members
                .into_iter()
                .map(|m| ClusterMember {
                    file_id: m.file_id,
                    score: m.score,
                })
                .collect(),
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Node-card lexical scoring (NOT chunk scoring — different algorithm)
// ═══════════════════════════════════════════════════════════════════════════

pub fn node_card_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[a-z0-9_/-]+").unwrap())
}

/// Extract terms for node-card scoring.  Unlike [`lexical::query_terms`],
/// this does NOT filter stopwords — matching the Python original exactly.
pub fn node_card_terms(query: &str) -> HashSet<String> {
    let lower = query.to_lowercase();
    node_card_token_re()
        .find_iter(&lower)
        .map(|m| m.as_str().to_string())
        .filter(|term| term.len() >= 3)
        .collect()
}

pub fn is_provider_backend_query(terms: &HashSet<String>) -> bool {
    let provider_terms: &[&str] = &["worker", "workers", "serverless", "gpu", "provider"];
    let operation_terms: &[&str] = &[
        "secret",
        "secrets",
        "rotation",
        "rotate",
        "token",
        "inventory",
        "sync",
    ];
    let resource_terms: &[&str] = &[
        "container",
        "containers",
        "cpu",
        "function",
        "functions",
        "vm",
        "instance",
        "instances",
    ];
    let has_provider = provider_terms.iter().any(|t| terms.contains(*t));
    let has_op_or_resource = operation_terms
        .iter()
        .chain(resource_terms.iter())
        .any(|t| terms.contains(*t));
    has_provider && has_op_or_resource
}

pub fn is_frontend_view_query(terms: &HashSet<String>) -> bool {
    [
        "page",
        "view",
        "screen",
        "frontend",
        "ui",
        "implemented",
        "where",
    ]
    .iter()
    .any(|t| terms.contains(*t))
}

// ═══════════════════════════════════════════════════════════════════════════
// Filter helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Check if a chunk matches structural pre-filters.
/// Mirrors `QueryEngine._chunk_matches_filters`.
fn chunk_matches_filters(
    chunk: &FileChunk,
    kind: Option<&str>,
    language: Option<&str>,
    symbols: Option<&[String]>,
    imports: Option<&[String]>,
    module: Option<&str>,
) -> bool {
    if let Some(filter_kind) = kind {
        let chunk_kind = chunk.kind.to_lowercase();
        if !chunk_kind.is_empty() && chunk_kind != filter_kind.to_lowercase() {
            return false;
        }
    }
    if let Some(filter_lang) = language {
        let chunk_lang = chunk.language.to_lowercase();
        if !chunk_lang.is_empty() && chunk_lang != filter_lang.to_lowercase() {
            return false;
        }
    }
    if let Some(syms) = symbols {
        if !syms.is_empty() {
            let text_lower = chunk.text.to_lowercase();
            let sym_lower = chunk.symbol_name.to_lowercase();
            let symbols_used_lower: Vec<String> = chunk
                .symbols_used
                .iter()
                .map(|s| s.to_lowercase())
                .collect();
            let any_match = syms.iter().any(|s| {
                let sl = s.to_lowercase();
                text_lower.contains(&sl) || sl == sym_lower || symbols_used_lower.contains(&sl)
            });
            if !any_match {
                return false;
            }
        }
    }
    if let Some(imps) = imports {
        if !imps.is_empty() {
            let syms_str = chunk.symbols_used.join(" ").to_lowercase();
            let text_lower = chunk.text.to_lowercase();
            let any_match = imps.iter().any(|imp| {
                let il = imp.to_lowercase();
                syms_str.contains(&il) || text_lower.contains(&il)
            });
            if !any_match {
                return false;
            }
        }
    }
    if let Some(filter_module) = module {
        // Python: `chunk.get("file_id") or chunk.get("file_sorgente") or ""`.
        let file_id = if chunk.file_id.is_empty() {
            chunk.file_sorgente.to_lowercase()
        } else {
            chunk.file_id.to_lowercase()
        };
        if !file_id.contains(&filter_module.to_lowercase()) {
            return false;
        }
    }
    true
}

// ═══════════════════════════════════════════════════════════════════════════
// Group-by-file
// ═══════════════════════════════════════════════════════════════════════════

/// Group context chunks and results by file.
/// Mirrors `QueryEngine._group_by_file`.
pub fn group_by_file_fn(
    context_chunks: &[ContextChunk],
    results: &[ResultEntry],
) -> Vec<GroupEntry> {
    let mut by_file: HashMap<String, GroupEntry> = HashMap::new();

    for chunk in context_chunks {
        let f = &chunk.file_source;
        if f.is_empty() {
            continue;
        }
        let entry = by_file.entry(f.clone()).or_insert_with(|| GroupEntry {
            file: f.clone(),
            total_score: 0.0,
            chunks: Vec::new(),
            kind: String::new(),
            symbol_name: String::new(),
            signature: String::new(),
            language: String::new(),
            line_start: 0,
            line_end: 0,
        });
        entry.total_score += chunk.score;
        let text_truncated = truncate_chars(&chunk.text, 500);
        entry.chunks.push(GroupChunk {
            chunk_id: chunk.chunk_id.clone(),
            score: chunk.score,
            retrieval: chunk.retrieval.clone(),
            start_char: chunk.start_char,
            end_char: chunk.end_char,
            text: text_truncated,
        });
    }

    // Augment with result metadata
    for r in results {
        let f = &r.file_source;
        if let Some(entry) = by_file.get_mut(f) {
            if entry.kind.is_empty() {
                entry.kind = r.kind.clone();
                entry.symbol_name = r.symbol_name.clone();
                entry.signature = r.signature.clone();
                entry.language = r.language.clone();
                entry.line_start = r.line_start;
                entry.line_end = r.line_end;
            }
        }
    }

    let mut grouped: Vec<GroupEntry> = by_file.into_values().collect();
    grouped.sort_by(|a, b| {
        b.total_score
            .partial_cmp(&a.total_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    grouped
}

// ═══════════════════════════════════════════════════════════════════════════
// Conversion helpers
// ═══════════════════════════════════════════════════════════════════════════

impl ContextChunk {
    fn from_file_chunk(chunk: &FileChunk, score: f64, retrieval: &str) -> Self {
        Self {
            chunk_id: chunk.id.clone(),
            file_source: chunk.file_sorgente.clone(),
            chunk_index: chunk.chunk_index as usize,
            start_char: chunk.start_char as usize,
            end_char: chunk.end_char as usize,
            score,
            rerank_score: None,
            focus: None,
            retrieval: retrieval.to_string(),
            // Redact at the retrieval serialization boundary so citations
            // (file + line, previews) never carry secret-looking tokens.
            text: redact_secret_tokens(&chunk.text),
            last_modified: chunk.ultima_modifica.clone(),
            kind: chunk.kind.clone(),
            symbol_name: chunk.symbol_name.clone(),
            signature: chunk.signature.clone(),
            language: chunk.language.clone(),
            line_start: chunk.line_start as usize,
            line_end: chunk.line_end as usize,
            symbols_used: chunk.symbols_used.clone(),
        }
    }
}

fn file_chunk_to_scored(c: &FileChunk) -> ScoredChunk {
    ScoredChunk {
        id: c.id.clone(),
        file_id: c.file_id.clone(),
        file_sorgente: c.file_sorgente.clone(),
        text: c.text.clone(),
        chunk_index: c.chunk_index as usize,
        start_char: c.start_char as usize,
        end_char: c.end_char as usize,
        kind: c.kind.clone(),
        symbol_name: c.symbol_name.clone(),
        signature: c.signature.clone(),
        language: c.language.clone(),
        line_start: c.line_start as usize,
        line_end: c.line_end as usize,
        symbols_used: serde_json::to_string(&c.symbols_used).unwrap_or_default(),
        area: String::new(),
        cluster_semantic: String::new(),
        label: String::new(),
    }
}

/// Truncate to at most `max_chars` Unicode scalar values.
/// Byte-offset slices (`s[..n]`) panic when `n` lands inside a multi-byte char.
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        None => s.to_string(),
        Some((idx, _)) => s[..idx].to_string(),
    }
}

pub fn summarize_chunk(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "Chunk-level match from the full-file Oracle index.".to_string();
    }
    let cleaned: String = trimmed.split_whitespace().collect::<Vec<&str>>().join(" ");
    truncate_chars(&cleaned, 420)
}

fn parse_cluster(value: &str) -> i64 {
    value.parse::<i64>().unwrap_or(0)
}

/// `active_query_profile()` — mirrors
/// `oracle/ingestion/retrieval_text.py::active_query_profile`.
fn active_query_profile() -> String {
    let raw = env::var("ORACLE_QUERY_PROFILE").unwrap_or_else(|_| {
        env::var("ORACLE_EMBED_PROFILE").unwrap_or_else(|_| "semantic-prefix-v2".to_string())
    });
    let profile = raw.trim().to_lowercase();
    if config::SEMANTIC_PROFILE_NAMES.contains(&profile.as_str()) {
        "semantic-prefix-v2".to_string()
    } else {
        "raw".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cluster() {
        assert_eq!(parse_cluster("42"), 42);
        assert_eq!(parse_cluster("abc"), 0);
        assert_eq!(parse_cluster(""), 0);
    }

    #[test]
    fn test_summarize_chunk() {
        assert_eq!(
            summarize_chunk(""),
            "Chunk-level match from the full-file Oracle index."
        );
        assert_eq!(summarize_chunk("  hello  "), "hello");
        let long = "x".repeat(500);
        assert_eq!(summarize_chunk(&long).len(), 420);
    }

    /// 419 ASCII bytes then `è` (U+00E8, two UTF-8 bytes) so byte 420 sits
    /// inside the accented character. `cleaned[..420]` panics on this input;
    /// the cut is an ordinary French comment, not a synthetic emoji pad.
    #[test]
    fn summarize_chunk_does_not_split_a_multibyte_char() {
        let text = format!(
            "{}è une fonction qui gère les requêtes d'authentification.",
            "x".repeat(419)
        );
        assert!(
            text.len() > 420 && !text.is_char_boundary(420),
            "fixture must straddle a char at byte 420 (len={}, boundary={})",
            text.len(),
            text.is_char_boundary(420)
        );
        let out = summarize_chunk(&text);
        assert!(
            out.ends_with('è'),
            "cut must keep the whole character: {out:?}"
        );
        assert_eq!(out.chars().count(), 420);
    }

    /// 499 ASCII bytes then `è` so byte 500 is mid-character. Hits the
    /// `chunk.text[..500]` path in `group_by_file_fn` on a normal query.
    #[test]
    fn group_by_file_does_not_split_a_multibyte_char() {
        let text = format!(
            "{}è il punto di ingresso: valida il token e apre la sessione.",
            "x".repeat(499)
        );
        assert!(
            text.len() > 500 && !text.is_char_boundary(500),
            "fixture must straddle a char at byte 500 (len={}, boundary={})",
            text.len(),
            text.is_char_boundary(500)
        );
        let chunk = ContextChunk {
            chunk_id: "src/auth.rs#chunk-0000".into(),
            file_source: "src/auth.rs".into(),
            chunk_index: 0,
            start_char: 0,
            end_char: text.len(),
            score: 1.0,
            rerank_score: None,
            focus: None,
            retrieval: "lexical".into(),
            text,
            last_modified: String::new(),
            kind: "function".into(),
            symbol_name: "login".into(),
            signature: String::new(),
            language: "rust".into(),
            line_start: 1,
            line_end: 20,
            symbols_used: vec![],
        };
        let groups = group_by_file_fn(&[chunk], &[]);
        assert_eq!(groups.len(), 1);
        let preview = &groups[0].chunks[0].text;
        assert!(
            preview.ends_with('è'),
            "cut must keep the whole character: {preview:?}"
        );
        assert_eq!(preview.chars().count(), 500);
    }

    #[test]
    fn test_node_card_terms_includes_stopwords() {
        let terms = node_card_terms("What is the architecture of this project?");
        // "the" (3 chars) is a stopword but node_card_terms does NOT filter it
        assert!(terms.contains("the"));
        assert!(terms.contains("what"));
        assert!(terms.contains("architecture"));
        assert!(terms.contains("project"));
    }

    #[test]
    fn test_provider_backend_query() {
        let terms: HashSet<String> = ["worker", "secret"].iter().map(|s| s.to_string()).collect();
        assert!(is_provider_backend_query(&terms));

        let terms2: HashSet<String> = ["hello", "world"].iter().map(|s| s.to_string()).collect();
        assert!(!is_provider_backend_query(&terms2));
    }

    #[test]
    fn test_frontend_view_query() {
        let terms: HashSet<String> = ["view", "page"].iter().map(|s| s.to_string()).collect();
        assert!(is_frontend_view_query(&terms));

        let terms2: HashSet<String> = ["hello", "world"].iter().map(|s| s.to_string()).collect();
        assert!(!is_frontend_view_query(&terms2));
    }

    #[test]
    fn context_chunk_redacts_secret_tokens_at_boundary() {
        let chunk = FileChunk {
            id: "f#chunk-0000".into(),
            file_id: "f".into(),
            chunk_index: 0,
            start_char: 0,
            end_char: 40,
            text: r#"password = "hunter2secret""#.into(),
            file_sorgente: "src/x.py".into(),
            ultima_modifica: String::new(),
            embedding_dims: 0,
            kind: "function".into(),
            symbol_name: "x".into(),
            signature: String::new(),
            line_start: 1,
            line_end: 1,
            language: "python".into(),
            symbols_used: vec![],
        };
        let ctx = ContextChunk::from_file_chunk(&chunk, 1.0, "lexical");
        assert!(
            !ctx.text.contains("hunter2secret"),
            "raw secret must not leave the retrieval boundary: {}",
            ctx.text
        );
        assert!(ctx.text.contains("[redacted-secret]"));
    }

    #[test]
    fn bounded_limit_clamps_to_max() {
        assert_eq!(1usize.clamp(1, config::MAX_BOUNDED_LIMIT), 1);
        assert_eq!(5usize.clamp(1, config::MAX_BOUNDED_LIMIT), 5);
        assert_eq!(
            10_000usize.clamp(1, config::MAX_BOUNDED_LIMIT),
            config::MAX_BOUNDED_LIMIT
        );
        assert_eq!(0usize.clamp(1, config::MAX_BOUNDED_LIMIT), 1);
    }
}
