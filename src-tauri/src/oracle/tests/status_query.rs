//! Result mapping and the status the runtime reports for its models and a
//! given index snapshot.

use std::path::Path;

use oracle_core::{
    ContextChunk, IndexStatusSnapshot, RERANKER_APPROX_BYTES, RERANKER_FILES, RERANKER_MODEL_ID,
};

use super::support::TestEnvironment;
use crate::oracle::query::result_from_context;
use crate::oracle::status::status_from_snapshot;
use crate::oracle::{OracleMatchType, OracleModelState, OracleRuntime};

#[test]
fn result_mapping_preserves_the_reranked_match_type() {
    let context = ContextChunk {
        chunk_id: "chunk-1".to_string(),
        file_source: "src/lib.rs".to_string(),
        chunk_index: 0,
        start_char: 0,
        end_char: 10,
        score: 0.5,
        rerank_score: Some(0.9),
        focus: None,
        retrieval: "dense+reranked".to_string(),
        text: "fn answer() {}".to_string(),
        last_modified: String::new(),
        kind: "function".to_string(),
        symbol_name: "answer".to_string(),
        signature: String::new(),
        language: "rust".to_string(),
        line_start: 1,
        line_end: 1,
        symbols_used: Vec::new(),
    };

    let result = result_from_context(Path::new("."), &context);
    assert_eq!(result.match_type, Some(OracleMatchType::DenseReranked));
    assert_eq!(
        serde_json::to_value(&result).unwrap()["match_type"],
        "dense+reranked"
    );
}

#[test]
fn status_exposes_a_missing_optional_reranker() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = OracleRuntime::from_environment();
    runtime.configure_root(temp.path().to_path_buf());

    let status = runtime.reranker_status();
    assert_eq!(status.state, OracleModelState::Missing);
    assert_eq!(status.model_id, RERANKER_MODEL_ID);
    assert_eq!(status.total_files, RERANKER_FILES.len());
    assert_eq!(status.approximate_bytes, RERANKER_APPROX_BYTES);
    assert!(status.message.unwrap().contains("reranker"));
}

#[test]
fn status_exposes_pending_files_as_an_incomplete_index() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = OracleRuntime::from_environment();
    runtime.configure_root(temp.path().to_path_buf());
    let snapshot = IndexStatusSnapshot {
        root: temp.path().display().to_string(),
        manifest_path: temp.path().join("manifest.json").display().to_string(),
        expected_files: 200,
        indexed_files: 80,
        pending_files: 120,
        stale_files: 0,
        sqlite_chunk_files: 80,
        sqlite_chunks: 160,
        vector_records: 160,
        chunk_profile: "test".to_string(),
        first_pending: Vec::new(),
        first_stale: Vec::new(),
        free_gb: 10.0,
        pause_reason: Some("Oracle paused indexing because available memory is low.".to_string()),
    };

    let status = status_from_snapshot(&runtime, &snapshot);
    assert_eq!(status.state, "incomplete");
    assert_eq!(status.indexed_files, 80);
    assert_eq!(status.pending_files, 120);
    assert_eq!(
        status.pause_reason.as_deref(),
        Some("Oracle paused indexing because available memory is low.")
    );
}
