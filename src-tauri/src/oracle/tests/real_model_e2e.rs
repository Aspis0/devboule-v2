//! End-to-end proofs over the real model and a real repository, through the
//! product runtime and command paths. Both are `#[ignore]`d because the
//! sandbox cannot link or execute the local ONNX Runtime reliably; they run
//! manually with `--ignored --nocapture`.

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use oracle_core::{configured_model_present, OracleDataPaths};

use super::support::{
    copy_model_bundle, normalize_expected_path, runtime_with_config, wait_for_index,
    wait_for_model_ready, RealRepoQuery, TestEnvironment,
};
use crate::oracle::commands::{
    oracle_ask_inner, oracle_files_inner, oracle_index_start_inner, oracle_stats_inner,
    oracle_status_inner, oracle_workspace_set_inner,
};
use crate::oracle::runtime::DEFAULT_ORACLE_MODEL;
use crate::oracle::{FileTab, OracleModelState};

/// Real model proof. Run manually from the repository root with:
///
///     cargo test -p devboule --lib oracle::tests::real_model_choose_index_query -- --ignored --nocapture
///
/// The test uses `recon/models/bge-small-en-v1.5` by default (or the path
/// in `DEVBOULE_E2E_MODEL_DIR`) and is ignored because the sandbox cannot
/// link or execute the local ONNX Runtime reliably.
#[test]
#[ignore]
fn real_model_choose_index_query() {
    let env = TestEnvironment::new("onnx");
    env.set("ORACLE_RS_EP", "cpu");
    let source = std::env::var_os("DEVBOULE_E2E_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("recon")
                .join("models")
                .join(DEFAULT_ORACLE_MODEL)
        });
    let source = source
        .canonicalize()
        .unwrap_or_else(|error| panic!("real model directory {}: {error}", source.display()));
    for required in [
        "model_config.json",
        "tokenizer.json",
        "onnx/model_quantized.onnx",
    ] {
        assert!(
            source.join(required).is_file(),
            "real model is missing {} under {}",
            required,
            source.display()
        );
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    let model_target = OracleDataPaths::from_root(&root)
        .root
        .join("models")
        .join(DEFAULT_ORACLE_MODEL);
    for required in [
        "model_config.json",
        "tokenizer.json",
        "onnx/model_quantized.onnx",
    ] {
        let target = model_target.join(required);
        fs::create_dir_all(target.parent().expect("model parent")).expect("model parent");
        fs::copy(source.join(required), &target).expect("copy real model asset");
    }

    let deployment = root.join("src").join("zephyr_release.rs");
    fs::create_dir_all(deployment.parent().expect("source parent")).expect("source parent");
    fs::write(
        &deployment,
        "pub fn reconcile_zephyr_release() {\n    // The release gate records the heliograph attestation in the deployment ledger.\n}\n",
    )
    .expect("deployment source");
    fs::write(
        root.join("cooking.txt"),
        "A sourdough starter needs flour, water, and time before baking.\n",
    )
    .expect("decoy text");
    fs::write(
        root.join("billing.txt"),
        "Invoices are collected from a saved card at the end of each cycle.\n",
    )
    .expect("decoy text");

    // The settings file must live outside the indexed workspace, as it does
    // in production (`app_config_dir()`). Putting it under `root` made the
    // indexer pick up `config/oracle-settings.json` as a project file.
    let config_home = tempfile::tempdir().expect("config tempdir");
    let config = config_home.path().join("config");
    let runtime = runtime_with_config(&config);
    let chosen = oracle_workspace_set_inner(&runtime, root.to_str().unwrap())
        .expect("choose temporary workspace");
    let chosen_path = PathBuf::from(chosen.path.as_deref().expect("chosen path"));
    assert!(chosen_path.is_absolute());
    assert_eq!(chosen_path.file_name(), root.file_name());
    wait_for_model_ready(&runtime);
    assert!(configured_model_present(&model_target, true));

    oracle_index_start_inner(&runtime).expect("start real index");
    wait_for_index(&runtime);
    let status = tauri::async_runtime::block_on(oracle_status_inner(&runtime))
        .expect("status after real index");
    assert_eq!(status.state, "ready");
    assert_eq!(status.indexed_files, 3);

    let indexed = tauri::async_runtime::block_on(oracle_files_inner(&runtime, FileTab::Indexed, 1))
        .expect("indexed files");
    assert!(indexed
        .iter()
        .any(|file| file.path == "src/zephyr_release.rs"));

    let response = tauri::async_runtime::block_on(oracle_ask_inner(
        &runtime,
        "Where is the heliograph attestation recorded for the release gate?".to_string(),
    ))
    .expect("real Oracle query");
    assert!(
        !response.results.is_empty(),
        "real query returned no results"
    );
    assert_eq!(
        response.results[0].path,
        "src/zephyr_release.rs",
        "real query returned the wrong top file: {:?}",
        response
            .results
            .iter()
            .map(|result| &result.path)
            .collect::<Vec<_>>()
    );
    assert!(response
        .results
        .iter()
        .any(|result| result.path == "src/zephyr_release.rs"));
}

/// Real-repository proof through the product runtime and command paths.
/// Run from PowerShell with:
///
///     $env:ORACLE_REAL_REPO_ROOT='C:\\path\\to\\repo'; $env:ORACLE_REAL_REPO_QUERIES='C:\\path\\to\\queries.json'; cargo test -p devboule --lib oracle::tests::real_repo_index_and_query -- --ignored --nocapture
///
/// The workspace is supplied by `ORACLE_REAL_REPO_ROOT`; only Oracle's
/// data/config directories are temporary. The test is ignored because the
/// sandbox cannot link or execute the local ONNX Runtime reliably.
#[test]
#[ignore]
fn real_repo_index_and_query() {
    let Some(workspace) = std::env::var_os("ORACLE_REAL_REPO_ROOT")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    else {
        println!("skipping real_repo_index_and_query: ORACLE_REAL_REPO_ROOT is not set");
        return;
    };
    let Some(queries_path) = std::env::var_os("ORACLE_REAL_REPO_QUERIES")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    else {
        println!("skipping real_repo_index_and_query: ORACLE_REAL_REPO_QUERIES is not set");
        return;
    };

    let env = TestEnvironment::new("onnx");
    env.set("ORACLE_RS_EP", "cpu");
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|error| panic!("real repository {}: {error}", workspace.display()));
    let queries_path = queries_path.canonicalize().unwrap_or_else(|error| {
        panic!(
            "real repository query file {}: {error}",
            queries_path.display()
        )
    });
    assert!(
        workspace.is_dir(),
        "real repository root is not a directory: {}",
        workspace.display()
    );
    assert!(
        queries_path.is_file(),
        "real repository query file is not a file: {}",
        queries_path.display()
    );

    let queries: Vec<RealRepoQuery> =
        serde_json::from_str(&fs::read_to_string(&queries_path).unwrap_or_else(|error| {
            panic!(
                "reading real repository query file {} failed: {error}",
                queries_path.display()
            )
        }))
        .unwrap_or_else(|error| {
            panic!(
                "parsing real repository query file {} failed: {error}",
                queries_path.display()
            )
        });

    let data_home = tempfile::tempdir().expect("Oracle data tempdir");
    env.set("ORACLE_DIR", data_home.path());
    let model_sources = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("recon")
        .join("models");
    let reranker_model_id = "ms-marco-TinyBERT-L-2-v2";
    copy_model_bundle(
        &model_sources.join(DEFAULT_ORACLE_MODEL),
        &data_home.path().join("models").join(DEFAULT_ORACLE_MODEL),
        &[
            "model_config.json",
            "tokenizer.json",
            "onnx/model_quantized.onnx",
        ],
    );
    copy_model_bundle(
        &model_sources.join(reranker_model_id),
        &data_home.path().join("models").join(reranker_model_id),
        &[
            "model_config.json",
            "tokenizer.json",
            "onnx/model_int8.onnx",
        ],
    );

    let config_home = tempfile::tempdir().expect("Oracle config tempdir");
    let runtime = runtime_with_config(&config_home.path().join("config"));
    let chosen = oracle_workspace_set_inner(&runtime, workspace.to_str().unwrap())
        .expect("choose real repository workspace");
    assert_eq!(
        chosen.path.as_deref(),
        Some(workspace.to_str().expect("workspace path is UTF-8"))
    );
    wait_for_model_ready(&runtime);
    assert_eq!(
        runtime.model_status().state,
        OracleModelState::Ready,
        "real embedder model did not become ready: {:?}",
        runtime.model_status()
    );

    let indexing_started = Instant::now();
    oracle_index_start_inner(&runtime).expect("start real repository index");
    wait_for_index(&runtime);
    let indexing_elapsed = indexing_started.elapsed();
    let status = tauri::async_runtime::block_on(oracle_status_inner(&runtime))
        .expect("status after real repository index");
    let stats = tauri::async_runtime::block_on(oracle_stats_inner(&runtime))
        .expect("stats after real repository index");
    println!(
        "real repository indexed in {:?}: {} files, {} chunks",
        indexing_elapsed, stats.indexed_files, stats.indexed_chunks
    );
    assert_eq!(status.state, "ready", "real repository index is not ready");
    assert_eq!(
        stats.pending_files, 0,
        "real repository index has pending files"
    );
    assert_eq!(
        stats.stale_files, 0,
        "real repository index has stale files"
    );
    assert!(
        stats.indexed_chunks > 0,
        "real repository index produced no chunks"
    );

    let mut top1 = 0;
    let mut top5 = 0;
    let mut missing = 0;
    let mut focused = 0;
    let mut unfocused = 0;
    for (index, query) in queries.iter().enumerate() {
        let expected = normalize_expected_path(&workspace, &query.expect);
        let response = tauri::async_runtime::block_on(oracle_ask_inner(&runtime, query.q.clone()))
            .unwrap_or_else(|error| {
                panic!(
                    "real repository query {} failed: {}",
                    index + 1,
                    error.message
                )
            });
        let rank = response
            .results
            .iter()
            .position(|result| result.path == expected)
            .map(|position| position + 1);

        println!("\nquery {}: {}", index + 1, query.q);
        for (position, result) in response.results.iter().take(5).enumerate() {
            let focus = match (result.focus_line_start, result.focus_line_end) {
                (Some(start), Some(end)) => {
                    assert!(
                        start >= result.line_start && end <= result.line_end && start <= end,
                        "focus {start}-{end} escapes the cited range {}-{} for {}",
                        result.line_start,
                        result.line_end,
                        result.path
                    );
                    focused += 1;
                    format!(" -> start at {start}-{end}")
                }
                (None, None) => {
                    unfocused += 1;
                    String::new()
                }
                _ => panic!("half a focus span on {}", result.path),
            };
            println!(
                "  {}. {} (lines {}-{}){}",
                position + 1,
                result.path,
                result.line_start,
                result.line_end,
                focus
            );
        }
        match rank {
            Some(position) => println!("  expected: {} (position {})", expected, position),
            None => {
                println!("  expected: {} (missing)", expected);
                missing += 1;
            }
        }
        if rank == Some(1) {
            top1 += 1;
        }
        if matches!(rank, Some(position) if position <= 5) {
            top5 += 1;
        }
    }

    // The reranker was once written, measured, committed and never
    // delivered, because nothing downloaded its model and no test asserted
    // that it had run. The citation focus rides on that same reranker, so
    // this asserts the focus arrived rather than printing a line range that
    // looks identical whether it did or not.
    assert!(
        focused > 0,
        "no result carried a focus span across {} queries. The reranker model is \
         staged by this test, so either the narrowing never ran or every retrieved \
         chunk was too short to narrow — both are regressions",
        queries.len()
    );
    println!("\nfocus: {focused} results narrowed, {unfocused} left at chunk width");

    // The code-knowledge graph has the same failure mode as the reranker
    // had: a store that exists, is exported, and is empty looks exactly like
    // a store that works. Assert against real data rather than a row count,
    // by asking a question whose answer is knowable from this repository.
    {
        let paths = runtime.paths().expect("oracle paths after indexing");
        let ckg = oracle_core::CkgStore::new(&paths.data.ckg).expect("opening the ckg store");
        let engine_file = "crates/oracle-core/src/query/engine.rs";
        let imports = ckg
            .imports_of(engine_file)
            .expect("reading imports out of the ckg");
        let targets: Vec<&str> = imports.iter().map(|edge| edge.dst.as_str()).collect();
        println!("\nckg: {engine_file} imports {} files", targets.len());
        for target in &targets {
            println!("  -> {target}");
        }
        assert!(
            !targets.is_empty(),
            "the graph has no imports for {engine_file}, which uses `crate::` on \
             several lines: either nothing built the graph or resolution is broken"
        );
        for expected in [
            "crates/oracle-core/src/query/focus.rs",
            "crates/oracle-core/src/query/reranker.rs",
        ] {
            assert!(
                targets.contains(&expected),
                "the graph is missing the edge {engine_file} -> {expected}"
            );
        }
        // A neighbourhood walk must reach further than one hop, otherwise
        // the recursive query is returning direct edges and nothing else.
        let reach = ckg
            .neighborhood(engine_file, 2, Some("IMPORT"))
            .expect("walking the ckg");
        println!("ckg: {} files within two imports", reach.len());
        assert!(
            reach.iter().any(|(_, depth)| *depth == 2),
            "no node sits two imports away, so the recursive walk is not walking"
        );
    }

    let outside_top5 = queries.len().saturating_sub(top5 + missing);
    println!(
        "\nsummary: first={}/{}, top5={}/{}, missing={}/{}, outside_top5={}/{}",
        top1,
        queries.len(),
        top5,
        queries.len(),
        missing,
        queries.len(),
        outside_top5,
        queries.len()
    );
}
