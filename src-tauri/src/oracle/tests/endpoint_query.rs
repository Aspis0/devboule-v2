//! The query route's refusal ladder and its success path over the wire:
//! both verbatim F3 phrases, the verbatim F2 phrase with no data directory
//! created, `no_vectors`, `warming` with its background warm, the requested
//! limit reaching the search, and `ORACLE_DIR` unable to redirect the index.
//! The parser and source pins live in [`super::endpoint_query_unit`].

use std::fs;
use std::path::PathBuf;

use oracle_core::{model_dir_for, OracleDataPaths, SqliteStore, BGE_SMALL_APPROX_BYTES};

use super::host::{published, query_body, response_json, send, unique_paths, TestHost, QUERY_PATH};
use super::support::{copy_model_bundle, TestEnvironment};
use crate::oracle::runtime::{OracleRuntime, DEFAULT_ORACLE_MODEL};
use crate::oracle::{OracleEndpoint, OracleResult};

/// The model gate answers with exactly the two sentences
/// `ensure_model_is_available` writes for the panel — first the missing
/// config, then the missing graph — so the daemon can relay them verbatim.
#[test]
fn the_model_gate_answers_with_both_verbatim_f3_phrases() {
    let _env = TestEnvironment::new("onnx");
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    let runtime = OracleRuntime::from_environment();
    runtime.configure_root(root.clone());
    let model_dir = model_dir_for(
        &OracleDataPaths::from_root(&root).root,
        DEFAULT_ORACLE_MODEL,
    );
    let expected_config = format!(
        "Oracle model `{DEFAULT_ORACLE_MODEL}` is not ready: {} is missing model_config.json. The model download is about {} MB; wait for it to finish or retry it in the Oracle panel.",
        model_dir.display(),
        BGE_SMALL_APPROX_BYTES / 1_000_000
    );
    let expected_graph = format!(
        "Oracle model `{DEFAULT_ORACLE_MODEL}` is not ready: its ONNX graph or tokenizer is missing under {}. Wait for the download to finish, or retry it in the Oracle panel.",
        model_dir.display()
    );

    let (paths, _endpoint_dir) = unique_paths();
    let endpoint = OracleEndpoint::default();
    let host = TestHost::new(runtime);
    endpoint.start_at(&paths, host.clone()).expect("start");
    let record = published(&paths);
    let bearer = format!("Bearer {}", record.token);

    let (status, response) = send(
        record.port,
        "POST",
        QUERY_PATH,
        &bearer,
        &query_body(&root, "anything", 10),
    );
    assert_eq!(status, 200, "{response}");
    let value = response_json(&response);
    assert_eq!(value["ok"].as_bool(), Some(false), "{response}");
    assert_eq!(value["reason"], "no_model", "{response}");
    assert_eq!(
        value["message"].as_str(),
        Some(expected_config.as_str()),
        "first F3 phrase must match verbatim: {response}"
    );

    let model_source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("recon")
        .join("models")
        .join(DEFAULT_ORACLE_MODEL);
    copy_model_bundle(&model_source, &model_dir, &["model_config.json"]);

    let (status, response) = send(
        record.port,
        "POST",
        QUERY_PATH,
        &bearer,
        &query_body(&root, "anything", 10),
    );
    assert_eq!(status, 200, "{response}");
    let value = response_json(&response);
    assert_eq!(value["reason"], "no_model", "{response}");
    assert_eq!(
        value["message"].as_str(),
        Some(expected_graph.as_str()),
        "second F3 phrase must match verbatim: {response}"
    );

    endpoint.stop();
}

/// A workspace the daemon resolved but nobody indexed: the verbatim F2
/// phrase, and the folder left exactly as it was — the probe must not bring
/// `oracle-data/` into existence.
#[test]
fn a_root_without_an_index_gets_the_verbatim_f2_phrase_and_no_data_dir() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    let runtime = OracleRuntime::from_environment();
    runtime.configure_root(root.clone());
    let expected = format!(
        "The folder {} has no Oracle index ({} does not exist). Index this folder before searching it; Oracle will not answer from another folder's index.",
        root.display(),
        OracleDataPaths::from_root_without_env(&root).metadata.display()
    );

    let (paths, _endpoint_dir) = unique_paths();
    let endpoint = OracleEndpoint::default();
    let host = TestHost::new(runtime);
    endpoint.start_at(&paths, host).expect("start");
    let record = published(&paths);

    let (status, response) = send(
        record.port,
        "POST",
        QUERY_PATH,
        &format!("Bearer {}", record.token),
        &query_body(&root, "anything", 10),
    );
    assert_eq!(status, 200, "{response}");
    let value = response_json(&response);
    assert_eq!(value["ok"].as_bool(), Some(false), "{response}");
    assert_eq!(value["reason"], "no_index", "{response}");
    assert_eq!(
        value["message"].as_str(),
        Some(expected.as_str()),
        "F2 phrase must match verbatim: {response}"
    );
    assert!(
        !root.join("oracle-data").exists(),
        "the refusal must leave the workspace without an oracle-data directory"
    );

    endpoint.stop();
}

/// Without `chunks.lancedb` the engine would fall back to lexical retrieval
/// in silence; the route refuses instead.
#[test]
fn missing_chunk_vectors_are_refused_as_no_vectors() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    let runtime = OracleRuntime::from_environment();
    runtime.configure_root(root.clone());
    SqliteStore::new(&OracleDataPaths::from_root_without_env(&root).metadata)
        .expect("metadata store");
    let data = OracleDataPaths::from_root_without_env(&root);
    let expected = format!(
        "The workspace {} has no chunk vector store ({} does not exist), so semantic search would silently fall back to lexical-only results. Re-index this workspace, then retry.",
        root.display(),
        data.chunks.display()
    );

    let (paths, _endpoint_dir) = unique_paths();
    let endpoint = OracleEndpoint::default();
    let host = TestHost::new(runtime);
    endpoint.start_at(&paths, host).expect("start");
    let record = published(&paths);

    let (status, response) = send(
        record.port,
        "POST",
        QUERY_PATH,
        &format!("Bearer {}", record.token),
        &query_body(&root, "anything", 10),
    );
    assert_eq!(status, 200, "{response}");
    let value = response_json(&response);
    assert_eq!(value["reason"], "no_vectors", "{response}");
    assert_eq!(
        value["message"].as_str(),
        Some(expected.as_str()),
        "absent-store refusal must match verbatim: {response}"
    );

    endpoint.stop();
}

/// A model on disk but not yet resident: the honest `warming` refusal with
/// its retry sentence — never F1 — and the warm started in the background.
#[test]
fn a_query_before_the_model_is_loaded_reads_warming_and_starts_the_warm() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    let runtime = OracleRuntime::from_environment();
    runtime.configure_root(root.clone());
    let data = OracleDataPaths::from_root_without_env(&root);
    SqliteStore::new(&data.metadata).expect("metadata store");
    fs::create_dir_all(&data.chunks).expect("chunks store");

    let (paths, _endpoint_dir) = unique_paths();
    let endpoint = OracleEndpoint::default();
    let host = TestHost::new(runtime);
    endpoint.start_at(&paths, host.clone()).expect("start");
    let record = published(&paths);

    let (status, response) = send(
        record.port,
        "POST",
        QUERY_PATH,
        &format!("Bearer {}", record.token),
        &query_body(&root, "anything", 10),
    );
    assert_eq!(status, 200, "{response}");
    let value = response_json(&response);
    assert_eq!(value["ok"].as_bool(), Some(false), "{response}");
    assert_eq!(value["reason"], "warming", "{response}");
    assert_eq!(
        value["message"].as_str(),
        Some("Oracle's embedding model is still loading in the background. Retry this query in a few seconds."),
        "{response}"
    );
    assert!(
        !value["message"]
            .as_str()
            .unwrap_or_default()
            .contains("desktop app"),
        "warming must never be F1: {response}"
    );
    assert_eq!(
        host.warm_calls(),
        1,
        "the warming refusal must start the background warm"
    );

    endpoint.stop();
}

/// The success envelope over the wire, with the limit the caller actually
/// asked for reaching the search — and the store paths built from the
/// request's root, not from any environment override.
#[test]
fn the_ok_path_answers_with_results_and_the_requested_limit() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    let runtime = OracleRuntime::from_environment();
    runtime.configure_root(root.clone());
    let data = OracleDataPaths::from_root_without_env(&root);
    SqliteStore::new(&data.metadata).expect("metadata store");
    fs::create_dir_all(&data.chunks).expect("chunks store");

    let (paths, _endpoint_dir) = unique_paths();
    let endpoint = OracleEndpoint::default();
    let host = TestHost::new(runtime);
    host.set_loaded(true);
    host.serve_results(vec![OracleResult {
        path: "src/endpoint_query.rs".to_string(),
        line_start: 10,
        line_end: 12,
        focus_line_start: None,
        focus_line_end: None,
        snippet: "let (status, body) = respond(host, &request.body);".to_string(),
        score: 0.75,
        symbol_name: None,
        match_type: None,
    }]);
    endpoint.start_at(&paths, host.clone()).expect("start");
    let record = published(&paths);

    let (status, response) = send(
        record.port,
        "POST",
        QUERY_PATH,
        &format!("Bearer {}", record.token),
        &query_body(&root, "where is the gate", 5),
    );
    assert_eq!(status, 200, "{response}");
    let value = response_json(&response);
    assert_eq!(value["ok"].as_bool(), Some(true), "{response}");
    assert_eq!(value["query"], "where is the gate", "{response}");
    assert_eq!(
        value["results"][0]["path"], "src/endpoint_query.rs",
        "{response}"
    );
    assert_eq!(value["results"][0]["score"], 0.75, "{response}");

    let searched = host.searched().expect("the route reached the search");
    assert_eq!(
        searched.data_root,
        OracleDataPaths::from_root_without_env(&root).root,
        "the search must read the request root's own oracle-data"
    );
    assert_eq!(searched.query, "where is the gate");
    assert_eq!(
        searched.limit, 5,
        "the requested limit must reach the search"
    );

    endpoint.stop();
}

/// The index the route reads comes from the request's root alone: with
/// `ORACLE_DIR` pointing at a decoy that *does* hold an index, the route
/// still refuses the real root instead of answering from the decoy.
#[test]
fn oracle_dir_cannot_redirect_the_index_the_endpoint_reads() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    let decoy = temp.path().join("decoy-oracle");
    fs::create_dir_all(&decoy).expect("decoy dir");
    let runtime = OracleRuntime::from_environment();
    runtime.configure_root(root.clone());
    _env.set("ORACLE_DIR", &decoy);
    SqliteStore::new(&OracleDataPaths::from_root(&root).metadata).expect("decoy index");
    assert!(
        !root.join("oracle-data").exists(),
        "the decoy must hold the only index in this test"
    );
    let expected = format!(
        "The folder {} has no Oracle index ({} does not exist). Index this folder before searching it; Oracle will not answer from another folder's index.",
        root.display(),
        OracleDataPaths::from_root_without_env(&root).metadata.display()
    );

    let (paths, _endpoint_dir) = unique_paths();
    let endpoint = OracleEndpoint::default();
    let host = TestHost::new(runtime);
    endpoint.start_at(&paths, host).expect("start");
    let record = published(&paths);

    let (status, response) = send(
        record.port,
        "POST",
        QUERY_PATH,
        &format!("Bearer {}", record.token),
        &query_body(&root, "anything", 10),
    );
    assert_eq!(status, 200, "{response}");
    let value = response_json(&response);
    assert_eq!(value["reason"], "no_index", "{response}");
    assert_eq!(
        value["message"].as_str(),
        Some(expected.as_str()),
        "the route must read from the request root, not from ORACLE_DIR: {response}"
    );

    endpoint.stop();
}
