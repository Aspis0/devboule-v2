//! The index job: a second start is rejected while one runs, and cancellation
//! reaches the real indexer loop through the runtime's cancel flag.

use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use oracle_core::{CancelFlag, OracleDataPaths, SqliteStore, TextEmbedder};

use super::support::{assert_actionable, resolved_paths, SlowTestEmbedder, TestEnvironment};
use crate::oracle::commands::{oracle_index_cancel_inner, oracle_index_start_inner, run_index_job};
use crate::oracle::OracleRuntime;

#[test]
fn index_start_rejects_a_second_run() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = OracleRuntime::from_environment();
    runtime.configure_root(temp.path().to_path_buf());
    runtime.indexing.store(true, Ordering::Release);

    let error = oracle_index_start_inner(&runtime).expect_err("second index must fail");
    assert_actionable(error, &["already running"]);
    runtime.indexing.store(false, Ordering::Release);
}

#[test]
fn index_cancel_reaches_the_real_indexer_loop() {
    let env = TestEnvironment::new("candle");
    env.set("ORACLE_CHUNK_MIN_FREE_RAM_GB", "0");
    env.set("ORACLE_CHUNK_BATCH_FILES", "1");
    env.set("ORACLE_CHUNK_BATCH_CHARS", "10000000");
    env.set("ORACLE_CHUNK_ATTENTION_BUDGET", "1000000000");

    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    let file_count = 24;
    for index in 0..file_count {
        fs::write(
            root.join(format!("slow-{index}.txt")),
            format!("cancellation sentinel file {index}\n"),
        )
        .expect("test source file");
    }

    let runtime = OracleRuntime::from_environment();
    let paths = resolved_paths(&root);
    let cancel = CancelFlag::new();
    let started = Arc::new(AtomicBool::new(false));
    let embedder: Arc<dyn TextEmbedder> = Arc::new(SlowTestEmbedder {
        started: Arc::clone(&started),
    });
    let indexing = Arc::clone(&runtime.indexing);
    let index_cancel = Arc::clone(&runtime.index_cancel);
    let last_index_error = Arc::clone(&runtime.last_index_error);
    indexing.store(true, Ordering::Release);
    *index_cancel
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(cancel.clone());

    let worker = thread::spawn(move || {
        run_index_job(
            paths,
            embedder,
            indexing,
            index_cancel,
            last_index_error,
            cancel,
        );
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !started.load(Ordering::Acquire) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        started.load(Ordering::Acquire),
        "index worker never embedded"
    );
    oracle_index_cancel_inner(&runtime).expect("cancel command");
    worker.join().expect("index worker should not panic");

    let sqlite =
        SqliteStore::new(&OracleDataPaths::from_root(&root).metadata).expect("metadata store");
    assert!(
        sqlite.chunk_file_count().expect("chunk file count") < file_count,
        "index cancellation must stop the worker before all files are committed"
    );
    assert!(!runtime.indexing.load(Ordering::Acquire));
    assert!(runtime
        .index_cancel
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_none());
    assert!(runtime.index_error().is_none());
}
