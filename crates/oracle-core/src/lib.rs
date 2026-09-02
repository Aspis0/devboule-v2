//! oracle-core: Rust-native Oracle runtime (indexing and retrieval).
//!
//! Oracle points at files and lines; it does not answer. Module layout:
//! store/, ingest/, query/, embed. `oracle-cli` and `examples/recon_bench`
//! are the evaluation bench for chunking and models.

mod cancel;
#[cfg(feature = "full")]
mod cli;
#[cfg(feature = "full")]
mod cluster;
#[cfg(any(feature = "city", feature = "full"))]
mod config;
/// Redaction must be available to Augur without the full Oracle runtime
/// (ort/onnx). Under `full` it lives at `query::redact`; under `city` alone
/// the same file is compiled as this module.
#[cfg(all(feature = "city", not(feature = "full")))]
#[path = "query/redact.rs"]
mod redact;
#[cfg(feature = "full")]
mod doctor;
#[cfg(feature = "embedder")]
mod embed;
#[cfg(any(feature = "city", feature = "full"))]
mod ingest;
#[cfg(feature = "embedder")]
mod model_download;
#[cfg(feature = "full")]
mod query;
#[cfg(any(feature = "city", feature = "full"))]
mod store;

#[cfg(feature = "full")]
use clap::ValueEnum;

/// Embedding backend selector shared by the CLI and (later) runtime config.
#[cfg(feature = "full")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendArg {
    Candle,
    Onnx,
}

#[cfg(feature = "full")]
pub use cluster::refresh_clusters;
#[cfg(any(feature = "city", feature = "full"))]
pub use config::{OracleDataPaths, MAX_BOUNDED_LIMIT};
#[cfg(feature = "full")]
pub use doctor::{build_report, DoctorCheck, DoctorReport};
#[cfg(feature = "full")]
pub use embed::{
    configured_model_present, default_backend, BackendChoice, EmbedderPool, EpArg,
};
pub use cancel::CancelFlag;
#[cfg(feature = "full")]
pub use ingest::collect::collect_text_files;
#[cfg(any(feature = "city", feature = "full"))]
pub use ingest::collect::collect_text_files_with_cancel_limited;
#[cfg(any(feature = "city", feature = "full"))]
pub use ingest::collect::collect_text_files_with_cancel_limits;
#[cfg(any(feature = "city", feature = "full"))]
pub use ingest::collect::collect_text_files_with_cancel_limits_report;
#[cfg(feature = "full")]
pub use ingest::indexer::{
    chunk_index_status, index_file_chunks, IndexResult, IndexStatus, IndexStatusSnapshot,
    IndexerConfig, TextEmbedder,
};
#[cfg(feature = "full")]
pub use model_download::{
    ensure_bge_small_onnx, ensure_bge_small_onnx_with_cancel, ensure_model_onnx_at_with_cancel,
    ensure_model_onnx_with_cancel, ensure_reranker_onnx, ensure_reranker_onnx_at_with_cancel,
    ensure_reranker_onnx_with_cancel, model_dir, model_dir_for, ModelBundleDescriptor,
    BGE_SMALL_APPROX_BYTES, BGE_SMALL_BUNDLE, BGE_SMALL_FILES, BGE_SMALL_MODEL_ID,
    RERANKER_APPROX_BYTES, RERANKER_BUNDLE, RERANKER_FILES, RERANKER_HF_BASE, RERANKER_MODEL_ID,
};
#[cfg(feature = "full")]
pub use query::engine::{
    ClusterInfo, ClusterMember, ClusterMemberResponse, ClusterResponse, ContextChunk,
    DuplicateGroup, HealthResponse, QueryEmbedder, QueryEngine, ResultEntry, SnapshotResponse,
};
#[cfg(feature = "full")]
pub use query::focus::FocusSpan;
#[cfg(feature = "full")]
pub use query::pool_embedder::PoolQueryEmbedder;
#[cfg(feature = "full")]
pub use query::redact::redact_secret_tokens;
#[cfg(all(feature = "city", not(feature = "full")))]
pub use redact::redact_secret_tokens;
#[cfg(feature = "full")]
pub use query::reranker::{
    configured_reranker_present, default_model_dir, RerankerHandle, SharedReranker,
};
pub use store::ckg::{CkgEdgeRow, CkgNodeRow, CkgStore};
#[cfg(feature = "full")]
pub use store::lance::{LanceHit, LanceRow, LanceStore};
#[cfg(feature = "full")]
pub use store::manifest::{
    file_needs_index, load_manifest, manifest_files_for_root, Manifest, ManifestFileEntry,
    RootEntry,
};
#[cfg(feature = "full")]
pub use store::sqlite::{FileChunk, FileCluster, NodeCard, SqliteStore};

#[cfg(feature = "full")]
#[doc(hidden)]
pub use cli::cmd_query;
#[cfg(feature = "full")]
#[doc(hidden)]
pub use config::{active_chunk_profile_version, EMBED_DIMS};
#[cfg(feature = "full")]
#[doc(hidden)]
pub use config::{query_embedder_is_hash, ENV_QUERY_EMBEDDER};
#[cfg(feature = "full")]
#[doc(hidden)]
pub use embed::embedder::MAX_LENGTH;
#[cfg(feature = "full")]
#[doc(hidden)]
pub use embed::model_descriptor::{KvGeometry, ModelDescriptor, QWEN3_MODEL_CONFIG_JSON};
#[cfg(feature = "full")]
#[doc(hidden)]
pub use embed::{
    cmd_bench, cmd_embed, reconstruct_from_windows, resolve_embed_window_bytes,
    resolve_embed_window_overlap_bytes, window_text, DeclaredModelConfig, DeviceArg, DtypeArg,
    Embedder, OnnxEmbedder, PoolingStrategy, TextWindow,
};
#[cfg(feature = "full")]
#[doc(hidden)]
pub use ingest::ast_chunker::{chunk_file_semantically, split_semantic, SemanticChunk};
#[cfg(feature = "full")]
#[doc(hidden)]
pub use ingest::chunking::{
    build_chunks_for_file, build_chunks_for_file_with_limits, chunk_geometry_fingerprint,
};
#[cfg(feature = "full")]
#[doc(hidden)]
pub use ingest::ckg_build::{build_graph_within, import_specifiers, FileGraphInput};
#[cfg(feature = "full")]
#[doc(hidden)]
pub use ingest::collect::priority_rank;
#[cfg(feature = "full")]
#[doc(hidden)]
pub use ingest::indexer::{
    prune_excluded_chunks, sync_text_chunks, EmbeddingRecipe, DEFAULT_BATCH_CHUNKS,
};
#[cfg(feature = "full")]
#[doc(hidden)]
pub use ingest::retrieval_text::{
    chunk_embedding_text, chunk_embedding_text_for_model, classify_domains, classify_source_kind,
    is_test_source, query_embedding_text, query_embedding_text_for_model, ChunkMeta,
};
#[cfg(feature = "full")]
#[doc(hidden)]
pub use query::engine::HashQueryEmbedder;
#[cfg(feature = "full")]
#[doc(hidden)]
pub use query::engine::{
    diversify_context_rows, group_by_file_fn, is_frontend_view_query, is_provider_backend_query,
    node_card_terms, node_card_token_re, summarize_chunk, truncate_chars, GroupChunk, GroupEntry,
};
#[cfg(feature = "full")]
#[doc(hidden)]
pub use query::focus::{
    plan_focus_windows, plan_focus_windows_with, select_focus, window_texts,
    FOCUS_WINDOWS_PER_CHUNK, MAX_FOCUS_WINDOWS_PER_QUERY, MIN_CHUNK_LINES_TO_NARROW,
};
#[cfg(feature = "full")]
#[doc(hidden)]
pub use query::lexical::{
    lexical_chunk_context, lexical_chunk_score, query_terms, semantic_expansions, ScoredChunk,
};
#[cfg(feature = "full")]
#[doc(hidden)]
pub use query::reranker::{
    OnnxReranker, RerankerConfig, DEFAULT_RERANKER_CANDIDATES, RERANKER_MODEL_CONFIG_JSON,
};
#[cfg(feature = "full")]
#[doc(hidden)]
pub use store::lance::hash_embed;
#[cfg(feature = "full")]
#[doc(hidden)]
pub use store::manifest::{
    file_signature, save_manifest, strip_verbatim_prefix, text_chunks_up_to_date,
};
#[cfg(feature = "full")]
#[doc(hidden)]
pub use store::sqlite::ARRAY_FIELDS;
