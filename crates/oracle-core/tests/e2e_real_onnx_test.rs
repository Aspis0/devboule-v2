//! End-to-end proof with the REAL ONNX embedder (operator-run, `--ignored`).
//!
//! Every other test stubs the model with a `FakeEmbedder`/`HashQueryEmbedder`.
//! This one wires the *actual* stack together: index a tiny corpus through the
//! real Qwen3 ONNX `EmbedderPool`, then query it through the real `QueryEngine`.
//!
//! The proof is deliberately a DENSE proof: both queries share **zero** literal
//! tokens with the file they should retrieve, so a pure-lexical fallback cannot
//! find them. If the semantically-correct file still wins, dense retrieval is
//! genuinely working end to end (real embeddings → LanceDB search → ranking),
//! not just the lexical path riding along.
//!
//! Requires the local model at `models/qwen3-onnx/`. Run with:
//!   cargo test --test e2e_real_onnx_test -- --ignored --nocapture

use oracle_core::embed::{BackendChoice, CancelFlag, EmbedderPool};
use oracle_core::ingest::indexer::{self, IndexerConfig};
use oracle_core::query::engine::{ContextChunk, QueryEngine};
use oracle_core::query::pool_embedder::PoolQueryEmbedder;
use oracle_core::store::lance::LanceStore;
use oracle_core::store::sqlite::SqliteStore;
use std::path::PathBuf;

/// Model bundle location. Override with `ORACLE_E2E_MODEL_DIR`; the models are
/// gigabytes and live outside the repo, so there is no committed default that
/// is right on every machine.
fn model_dir() -> PathBuf {
    std::env::var_os("ORACLE_E2E_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models/qwen3-onnx"))
}

/// Three files in three clearly-distinct semantic domains. The queries below
/// are worded to avoid every literal token that appears in their target file.
const CORPUS: &[(&str, &str)] = &[
    (
        "billing.md",
        "# Charges\n\nCharges are collected from each customer's saved card at \
         the close of the cycle. Invoices enumerate every line item and the \
         total amount owed.\n",
    ),
    (
        "astronomy.md",
        "# Sky\n\nOur natural satellite completes one revolution around the \
         planet about every twenty-seven days, held in place by gravity.\n",
    ),
    (
        "cooking.md",
        "# Starter\n\nA sourdough culture must be fed with flour and water each \
         day until it foams and doubles, ready for the oven.\n",
    ),
];

/// Best (highest) score seen for chunks whose file source contains `needle`.
fn best_for(rows: &[ContextChunk], needle: &str) -> f64 {
    rows.iter()
        .filter(|c| c.file_source.contains(needle))
        .map(|c| c.score)
        .fold(f64::MIN, f64::max)
}

#[tokio::test]
#[ignore]
async fn real_onnx_dense_retrieval_discriminates() {
    let model = model_dir();
    if !model.join("tokenizer.json").exists() {
        eprintln!("skipping: no local model at {}", model.display());
        return;
    }

    // ── World: tempdir corpus + fresh stores ─────────────────────────────
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    for (name, body) in CORPUS {
        std::fs::write(root.join(name), body).unwrap();
    }
    let data = root.join("oracle-data");
    std::fs::create_dir_all(&data).unwrap();
    let sqlite_path = data.join("metadata.sqlite");
    let chunk_vec_path = data.join("chunks.lancedb");
    let node_vec_path = data.join("node_vectors.lancedb");
    let file_vec_path = data.join("file_vectors.lancedb");
    let manifest_path = data.join("chunk-index-manifest.json");

    let sqlite = SqliteStore::new(&sqlite_path).unwrap();
    let chunk_vectors = LanceStore::new(&chunk_vec_path);

    // ── Index the corpus through the REAL ONNX pool ──────────────────────
    // int8 is the graph `model_config.json` declares as primary and the one the
    // app loads; proving the fp32 path would prove something we do not ship.
    let pool = EmbedderPool::new(BackendChoice::Ort {
        model_dir: model,
        int8: true,
    });
    let cancel = CancelFlag::new();
    let cfg = IndexerConfig {
        min_free_gb: 0.0,
        max_gpu_temp_c: None,
        ..Default::default()
    };
    indexer::index_file_chunks(
        &root,
        &sqlite,
        &chunk_vectors,
        &manifest_path,
        &pool,
        &cancel,
        &cfg,
        None,
    )
    .await
    .unwrap();

    // The stores must actually have been populated with real vectors.
    assert!(
        sqlite.chunk_count().unwrap() >= 3,
        "expected >=3 chunks indexed"
    );
    let vec_rows = chunk_vectors.count().await.unwrap();
    assert!(vec_rows >= 3, "expected >=3 chunk vectors, got {vec_rows}");

    // ── Query through the REAL QueryEngine (empty node/file stores are ───
    //    fine: context() only reads chunk_vectors + sqlite). ─────────────
    let engine = QueryEngine::new(
        sqlite,
        LanceStore::new(&node_vec_path),
        Some(chunk_vectors),
        Some(LanceStore::new(&file_vec_path)),
    );
    // The shipping query path: same pool, and the same semantic-prefix decision
    // the indexer made on the chunks. Embedding the raw query here instead would
    // prove a path nothing runs.
    let embedder = PoolQueryEmbedder::new(&pool, &cancel).unwrap();

    // Query A — worded with NO token from billing.md; must still pick it.
    let a = engine
        .context(
            "how does the platform handle payments and monetary transactions from subscribers",
            10,
            &embedder,
            None,
            false, // prefer_lexical = false -> dense path drives ranking
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(!a.is_empty(), "query A returned no context");
    println!("A top: {} ({:.4})", a[0].file_source, a[0].score);
    assert!(
        a[0].file_source.contains("billing"),
        "dense retrieval failed: expected billing.md, got {} (retrieval={})",
        a[0].file_source,
        a[0].retrieval
    );
    assert!(
        best_for(&a, "billing") > best_for(&a, "astronomy")
            && best_for(&a, "billing") > best_for(&a, "cooking"),
        "billing did not beat both decoys"
    );

    // Query B — a different domain, again lexically disjoint from astronomy.md.
    // Proves the engine discriminates by semantics rather than returning a
    // constant winner.
    let b = engine
        .context(
            "movement of celestial objects through outer space",
            10,
            &embedder,
            None,
            false,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(!b.is_empty(), "query B returned no context");
    println!("B top: {} ({:.4})", b[0].file_source, b[0].score);
    assert!(
        b[0].file_source.contains("astronomy"),
        "dense retrieval failed: expected astronomy.md, got {} (retrieval={})",
        b[0].file_source,
        b[0].retrieval
    );
}
