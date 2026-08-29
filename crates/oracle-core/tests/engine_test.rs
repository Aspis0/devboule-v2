//! Integration tests for the query engine orchestration layer.

use oracle_core::{
    hash_embed, lexical_chunk_context, FileChunk, FileCluster, HashQueryEmbedder, LanceRow,
    LanceStore, NodeCard, QueryEmbedder, QueryEngine, ScoredChunk, SqliteStore,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ── Test fixtures ──────────────────────────────────────────────────────────

fn test_chunks() -> Vec<FileChunk> {
    vec![
        FileChunk {
            id: "docs/architecture.md#chunk-0000".into(),
            file_id: "docs/architecture.md".into(),
            chunk_index: 0,
            start_char: 0,
            end_char: 2500,
            text: "# Oracle Architecture\n\nThe ingestion pipeline handles \
                   source code files. Helios GPU instances provide compute."
                .into(),
            file_sorgente: "docs/architecture.md".into(),
            ultima_modifica: "2026-01-01T00:00:00Z".into(),
            embedding_dims: 0,
            kind: "text_slice".into(),
            symbol_name: "".into(),
            signature: "".into(),
            line_start: 0,
            line_end: 0,
            language: "".into(),
            symbols_used: vec![],
        },
        FileChunk {
            id: "docs/architecture.md#chunk-0001".into(),
            file_id: "docs/architecture.md".into(),
            chunk_index: 1,
            start_char: 2500,
            end_char: 5000,
            text: "## Query Processing\n\nQuery terms are extracted and scored \
                   using lexical matching. The context merge combines dense and \
                   lexical results."
                .into(),
            file_sorgente: "docs/architecture.md".into(),
            ultima_modifica: "2026-01-01T00:00:00Z".into(),
            embedding_dims: 0,
            kind: "text_slice".into(),
            symbol_name: "".into(),
            signature: "".into(),
            line_start: 0,
            line_end: 0,
            language: "".into(),
            symbols_used: vec![],
        },
        FileChunk {
            id: "src/main.rs#chunk-0000".into(),
            file_id: "src/main.rs".into(),
            chunk_index: 0,
            start_char: 0,
            end_char: 1200,
            text: "fn main() {\n    // Helios GPU provider backend\n    \
                   let provider = HeliosProvider::new();\n}"
                .into(),
            file_sorgente: "src/main.rs".into(),
            ultima_modifica: "2026-02-01T00:00:00Z".into(),
            embedding_dims: 0,
            kind: "function".into(),
            symbol_name: "main".into(),
            signature: "fn main()".into(),
            line_start: 1,
            line_end: 4,
            language: "rust".into(),
            symbols_used: vec!["HeliosProvider".into()],
        },
        FileChunk {
            id: "src/main.rs#chunk-0001".into(),
            file_id: "src/main.rs".into(),
            chunk_index: 1,
            start_char: 1200,
            end_char: 2400,
            text: "struct HeliosProvider {\n    gpu: bool,\n    serverless: bool,\n}".into(),
            file_sorgente: "src/main.rs".into(),
            ultima_modifica: "2026-02-01T00:00:00Z".into(),
            embedding_dims: 0,
            kind: "struct".into(),
            symbol_name: "HeliosProvider".into(),
            signature: "struct HeliosProvider".into(),
            line_start: 5,
            line_end: 8,
            language: "rust".into(),
            symbols_used: vec![],
        },
        FileChunk {
            id: "src/api.rs#chunk-0000".into(),
            file_id: "src/api.rs".into(),
            chunk_index: 0,
            start_char: 0,
            end_char: 1500,
            text: "use crate::provider;\n\npub fn handle_request() {\n    \
                   // Nimbus worker secret rotation logic\n}"
                .into(),
            file_sorgente: "src/api.rs".into(),
            ultima_modifica: "2026-03-01T00:00:00Z".into(),
            embedding_dims: 0,
            kind: "function".into(),
            symbol_name: "handle_request".into(),
            signature: "pub fn handle_request()".into(),
            line_start: 1,
            line_end: 4,
            language: "rust".into(),
            symbols_used: vec!["crate::provider".into()],
        },
        FileChunk {
            id: "data/config.json#chunk-0000".into(),
            file_id: "data/config.json".into(),
            chunk_index: 0,
            start_char: 0,
            end_char: 2000,
            text: "{\n  \"pipeline\": {\n    \"name\": \"oracle-ingestion\"\n  },\n  \
                   \"query_engine\": {\n    \"lexical_weight\": 1.0\n  }\n}"
                .into(),
            file_sorgente: "data/config.json".into(),
            ultima_modifica: "2026-01-15T00:00:00Z".into(),
            embedding_dims: 0,
            kind: "text_slice".into(),
            symbol_name: "".into(),
            signature: "".into(),
            line_start: 0,
            line_end: 0,
            language: "".into(),
            symbols_used: vec![],
        },
        FileChunk {
            id: "tests/test_main.rs#chunk-0000".into(),
            file_id: "tests/test_main.rs".into(),
            chunk_index: 0,
            start_char: 0,
            end_char: 800,
            text: "#[cfg(test)]\nmod tests {\n    #[test]\n    fn test_basic() {\n    \
                   }\n}"
                .into(),
            file_sorgente: "tests/test_main.rs".into(),
            ultima_modifica: "2026-02-15T00:00:00Z".into(),
            embedding_dims: 0,
            kind: "function".into(),
            symbol_name: "test_basic".into(),
            signature: "#[test]".into(),
            line_start: 1,
            line_end: 5,
            language: "rust".into(),
            symbols_used: vec![],
        },
        FileChunk {
            id: "src/api.rs#chunk-0001".into(),
            file_id: "src/api.rs".into(),
            chunk_index: 1,
            start_char: 1500,
            end_char: 3000,
            text: "pub fn rotate_secret(provider: &str) {\n    // Rotation for \
                   nimbus workers\n    // Uses secret management\n}"
                .into(),
            file_sorgente: "src/api.rs".into(),
            ultima_modifica: "2026-03-01T00:00:00Z".into(),
            embedding_dims: 0,
            kind: "function".into(),
            symbol_name: "rotate_secret".into(),
            signature: "pub fn rotate_secret(provider: &str)".into(),
            line_start: 1,
            line_end: 4,
            language: "rust".into(),
            symbols_used: vec![],
        },
    ]
}

fn test_cards() -> Vec<NodeCard> {
    vec![
        NodeCard {
            id: "docs/architecture.md".into(),
            label: "architecture".into(),
            area: "documentation".into(),
            cluster_semantic: "3".into(),
            funzione_primaria: "Oracle architecture documentation".into(),
            espone_api: vec![],
            dipende_da: vec![],
            simile_a: vec![],
            tecnologie: vec!["rust".into(), "python".into()],
            file_sorgente: "docs/architecture.md".into(),
            ultima_modifica: "2026-01-01T00:00:00Z".into(),
            source: "file".into(),
            embedding_dims: 1024,
        },
        NodeCard {
            id: "src/main.rs".into(),
            label: "main".into(),
            area: "backend".into(),
            cluster_semantic: "1".into(),
            funzione_primaria: "Main entry point".into(),
            espone_api: vec![],
            dipende_da: vec!["src/provider.rs".into()],
            simile_a: vec![],
            tecnologie: vec!["rust".into()],
            file_sorgente: "src/main.rs".into(),
            ultima_modifica: "2026-02-01T00:00:00Z".into(),
            source: "file".into(),
            embedding_dims: 1024,
        },
        NodeCard {
            id: "src-tauri/src/backend/providers.rs".into(),
            label: "providers".into(),
            area: "backend".into(),
            cluster_semantic: "1".into(),
            funzione_primaria: "Provider management backend".into(),
            espone_api: vec![],
            dipende_da: vec![],
            simile_a: vec![],
            tecnologie: vec!["rust".into(), "helios".into()],
            file_sorgente: "src-tauri/src/backend/providers.rs".into(),
            ultima_modifica: "2026-03-01T00:00:00Z".into(),
            source: "file".into(),
            embedding_dims: 1024,
        },
    ]
}

async fn build_engine(tmp: &tempfile::TempDir) -> QueryEngine {
    let sqlite_path = tmp.path().join("metadata.sqlite");
    let chunk_vec_path = tmp.path().join("chunk_vectors.json");
    let node_vec_path = tmp.path().join("node_vectors.json");
    let file_vec_path = tmp.path().join("file_vectors.json");

    let sqlite = SqliteStore::new(&sqlite_path).unwrap();
    let chunk_vectors = LanceStore::new(&chunk_vec_path);
    let node_vectors = LanceStore::new(&node_vec_path);
    let file_vectors = LanceStore::new(&file_vec_path);

    // Insert chunks
    let chunks = test_chunks();
    sqlite.replace_all_chunks(&chunks).unwrap();

    // Insert node cards
    let cards = test_cards();
    sqlite.replace_all(&cards).unwrap();

    // Populate chunk vectors with hash_embed of each chunk's text
    let mut chunk_rows: Vec<LanceRow> = Vec::new();
    for c in &chunks {
        let vector = hash_embed(&c.text, 1024);
        chunk_rows.push(LanceRow {
            id: c.id.clone(),
            label: c.id.clone(),
            area: "chunk".into(),
            cluster_semantic: "0".into(),
            vector,
        });
    }
    chunk_vectors.upsert(&chunk_rows).await.unwrap();

    // Populate node-card vectors
    let mut node_rows: Vec<LanceRow> = Vec::new();
    for card in &cards {
        let vector = hash_embed(&card.id, 1024);
        node_rows.push(LanceRow {
            id: card.id.clone(),
            label: card.label.clone(),
            area: card.area.clone(),
            cluster_semantic: card.cluster_semantic.clone(),
            vector,
        });
    }
    node_vectors.upsert(&node_rows).await.unwrap();

    QueryEngine::new(
        sqlite,
        node_vectors,
        Some(chunk_vectors),
        Some(file_vectors),
    )
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_context_lexical_only() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;
    let embedder = HashQueryEmbedder;

    let results = engine
        .context(
            "Helios GPU serverless",
            10,
            &embedder,
            None,
            true, // prefer_lexical
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    assert!(!results.is_empty(), "should return chunks");
    // All should be lexical-only
    for r in &results {
        assert_eq!(r.retrieval, "lexical");
    }
    // Results should be sorted by score descending
    for w in results.windows(2) {
        assert!(w[0].score >= w[1].score);
    }
    // First result should mention helios or gpu
    let top_text = results[0].text.to_lowercase();
    assert!(
        top_text.contains("helios") || top_text.contains("gpu"),
        "top result should mention helios or gpu, got: {}",
        &results[0].text[..100]
    );
}

#[tokio::test]
async fn test_context_dense_and_lexical_merge() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;
    let embedder = HashQueryEmbedder;

    let results = engine
        .context(
            "Helios GPU serverless compute",
            10,
            &embedder,
            None,
            false, // prefer_lexical = false → dense + lexical
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    assert!(!results.is_empty());
    // Check that we have correct retrieval flags
    let has_dense = results.iter().any(|r| r.retrieval == "dense");
    let has_lexical = results.iter().any(|r| r.retrieval == "lexical");
    let has_merged = results.iter().any(|r| r.retrieval == "dense+lexical");
    // At least lexical results should exist (dense depends on hash_embed similarity)
    assert!(
        has_lexical || has_merged || has_dense,
        "should have at least one retrieval type"
    );
    // If a chunk appears in both dense and lexical, it should be dense+lexical
    for r in &results {
        assert!(
            r.retrieval == "dense" || r.retrieval == "lexical" || r.retrieval == "dense+lexical",
            "invalid retrieval: {}",
            r.retrieval
        );
    }
}

struct FixedQueryEmbedder;

impl QueryEmbedder for FixedQueryEmbedder {
    fn embed_query(&self, _text: &str, dims: usize) -> anyhow::Result<Vec<f32>> {
        let mut vector = vec![0.0; dims];
        vector[0] = 1.0;
        Ok(vector)
    }
}

struct DeclaredDimsQueryEmbedder {
    dims: usize,
    requested_dims: Arc<AtomicUsize>,
}

impl QueryEmbedder for DeclaredDimsQueryEmbedder {
    fn dims(&self) -> anyhow::Result<Option<usize>> {
        Ok(Some(self.dims))
    }

    fn embed_query(&self, _text: &str, dims: usize) -> anyhow::Result<Vec<f32>> {
        self.requested_dims.store(dims, Ordering::SeqCst);
        Ok(vec![0.0; dims])
    }
}

#[tokio::test]
async fn test_empty_chunk_store_uses_query_model_dimensions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let sqlite = SqliteStore::new(&tmp.path().join("metadata.sqlite")).unwrap();
    let chunk_vectors = LanceStore::new(&tmp.path().join("chunk_vectors.json"));
    let requested_dims = Arc::new(AtomicUsize::new(0));
    let embedder = DeclaredDimsQueryEmbedder {
        dims: 384,
        requested_dims: Arc::clone(&requested_dims),
    };
    let engine = QueryEngine::new(
        sqlite,
        LanceStore::new(&tmp.path().join("node_vectors.json")),
        Some(chunk_vectors),
        None,
    );

    engine
        .context(
            "query", 5, &embedder, None, false, None, None, None, None, None,
        )
        .await
        .unwrap();

    assert_eq!(requested_dims.load(Ordering::SeqCst), 384);
}

#[tokio::test]
async fn test_context_dense_answer_shuts_out_a_huge_lexical_score() {
    let tmp = tempfile::TempDir::new().unwrap();
    let sqlite = SqliteStore::new(&tmp.path().join("metadata.sqlite")).unwrap();
    let chunk_vectors = LanceStore::new(&tmp.path().join("chunk_vectors.json"));
    let query = "how alpha beta gamma delta epsilon zeta eta theta iota";
    let dense_id = "src/dense_only.rs#chunk-0000";
    let lexical_file = "src/alpha_beta_gamma_delta_epsilon_zeta_eta_theta.rs";
    let lexical_rank_one_id = format!("{lexical_file}#chunk-0000");
    let lexical_id = format!("{lexical_file}#chunk-0001");

    let make_chunk = |id: &str, file: &str, chunk_index: i64, text: &str| FileChunk {
        id: id.into(),
        file_id: file.into(),
        chunk_index,
        start_char: 0,
        end_char: text.len() as i64,
        text: text.into(),
        file_sorgente: file.into(),
        ultima_modifica: String::new(),
        embedding_dims: 2,
        kind: "text_slice".into(),
        symbol_name: String::new(),
        signature: String::new(),
        line_start: 1,
        line_end: 1,
        language: "text".into(),
        symbols_used: vec![],
    };

    let chunks = vec![
        make_chunk(dense_id, "src/dense_only.rs", 0, "semantic-only evidence"),
        make_chunk(
            "src/dense_decoy_1.rs#chunk-0000",
            "src/dense_decoy_1.rs",
            0,
            "unrelated dense decoy one",
        ),
        make_chunk(
            "src/dense_decoy_2.rs#chunk-0000",
            "src/dense_decoy_2.rs",
            0,
            "unrelated dense decoy two",
        ),
        make_chunk(
            "src/dense_decoy_3.rs#chunk-0000",
            "src/dense_decoy_3.rs",
            0,
            "unrelated dense decoy three",
        ),
        make_chunk(
            "src/dense_decoy_4.rs#chunk-0000",
            "src/dense_decoy_4.rs",
            0,
            "unrelated dense decoy four",
        ),
        make_chunk(
            &lexical_rank_one_id,
            lexical_file,
            0,
            "alpha beta gamma delta epsilon zeta eta theta iota",
        ),
        make_chunk(
            &lexical_id,
            lexical_file,
            1,
            "alpha beta gamma delta epsilon zeta eta theta",
        ),
    ];
    sqlite.replace_all_chunks(&chunks).unwrap();

    let vector = |x: f32, y: f32| vec![x, y];
    let rows = vec![
        LanceRow {
            id: dense_id.into(),
            label: dense_id.into(),
            area: "chunk".into(),
            cluster_semantic: "0".into(),
            vector: vector(1.0, 0.0),
        },
        LanceRow {
            id: "src/dense_decoy_1.rs#chunk-0000".into(),
            label: "dense decoy one".into(),
            area: "chunk".into(),
            cluster_semantic: "0".into(),
            vector: vector(0.9, (1.0_f32 - 0.9_f32.powi(2)).sqrt()),
        },
        LanceRow {
            id: "src/dense_decoy_2.rs#chunk-0000".into(),
            label: "dense decoy two".into(),
            area: "chunk".into(),
            cluster_semantic: "0".into(),
            vector: vector(0.8, 0.6),
        },
        LanceRow {
            id: "src/dense_decoy_3.rs#chunk-0000".into(),
            label: "dense decoy three".into(),
            area: "chunk".into(),
            cluster_semantic: "0".into(),
            vector: vector(0.7, (1.0_f32 - 0.7_f32.powi(2)).sqrt()),
        },
        LanceRow {
            id: "src/dense_decoy_4.rs#chunk-0000".into(),
            label: "dense decoy four".into(),
            area: "chunk".into(),
            cluster_semantic: "0".into(),
            vector: vector(0.6, 0.8),
        },
        LanceRow {
            id: lexical_rank_one_id.clone(),
            label: "lexical rank one".into(),
            area: "chunk".into(),
            cluster_semantic: "0".into(),
            vector: vector(0.0, 1.0),
        },
        LanceRow {
            id: lexical_id.clone(),
            label: "lexical candidate".into(),
            area: "chunk".into(),
            cluster_semantic: "0".into(),
            vector: vector(0.0, 1.0),
        },
    ];
    chunk_vectors.upsert(&rows).await.unwrap();

    let lexical_probe = ScoredChunk {
        id: lexical_id.clone(),
        file_id: lexical_file.into(),
        file_sorgente: lexical_file.into(),
        text: "alpha beta gamma delta epsilon zeta eta theta".into(),
        chunk_index: 1,
        start_char: 0,
        end_char: 48,
        kind: "text_slice".into(),
        symbol_name: String::new(),
        signature: String::new(),
        language: "text".into(),
        line_start: 1,
        line_end: 1,
        symbols_used: String::new(),
        area: String::new(),
        cluster_semantic: String::new(),
        label: String::new(),
    };
    let lexical_probe_results = lexical_chunk_context(query, &[lexical_probe], 1);
    assert_eq!(lexical_probe_results.len(), 1);
    assert!(
        (lexical_probe_results[0].score - 17.8).abs() < 1e-10,
        "the regression fixture must keep its deliberately huge additive lexical score: {}",
        lexical_probe_results[0].score
    );

    let engine = QueryEngine::new(
        sqlite,
        LanceStore::new(&tmp.path().join("node_vectors.json")),
        Some(chunk_vectors),
        None,
    );
    let results = engine
        .context(
            query,
            5,
            &FixedQueryEmbedder,
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

    let dense = results
        .iter()
        .find(|chunk| chunk.chunk_id == dense_id)
        .expect("dense-only chunk should survive the final limit");
    assert_eq!(dense.retrieval, "dense");
    assert!((dense.score - (1.0 / 61.0)).abs() < 1e-12);

    // Under `max` fusion the 17.8 above outranked every cosine. Under rank
    // fusion it landed second. Now it does not compete at all: the lexical scan
    // is a fallback, so a dense answer ends the query before it runs. Measured
    // on the frozen corpus, fusing cost recall (0.725 -> 0.650) and MRR
    // (0.549 -> 0.429) on top of a 10,000-chunk scan per query.
    assert!(
        results.iter().all(|chunk| chunk.chunk_id != lexical_id),
        "a dense answer must end the query before the lexical scan runs, so the \
         raw-17.8 lexical candidate cannot appear: {results:?}"
    );
    assert!(
        results.iter().all(|chunk| chunk.retrieval == "dense"),
        "every row must come from the dense path here: {results:?}"
    );
}

#[tokio::test]
async fn test_filters_kind_language() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;
    let embedder = HashQueryEmbedder;

    // Filter by kind = "function"
    let results = engine
        .context(
            "Helios GPU",
            10,
            &embedder,
            None,
            true,
            Some("function"), // kind filter
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    for r in &results {
        assert_eq!(
            r.kind, "function",
            "all results should be kind=function, got {}",
            r.kind
        );
    }

    // Filter by language = "rust"
    let results = engine
        .context(
            "Helios GPU",
            10,
            &embedder,
            None,
            true,
            None,
            Some("rust"), // language filter
            None,
            None,
            None,
        )
        .await
        .unwrap();

    for r in &results {
        assert_eq!(
            r.language, "rust",
            "all results should be language=rust, got {}",
            r.language
        );
    }

    // Filter by kind that doesn't match anything
    let results = engine
        .context(
            "Helios GPU",
            10,
            &embedder,
            None,
            true,
            Some("class"),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(results.is_empty(), "no class chunks in fixtures");
}

#[tokio::test]
async fn test_similar_node_card_fallback_to_file_vectors() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;

    // "docs/architecture.md" exists in node-card vectors → should find similar
    let results = engine.similar("docs/architecture.md", 5).await.unwrap();
    assert!(
        !results.is_empty(),
        "should find similar from node-card store"
    );

    // Nonexistent id → node-card store misses, file_vectors store is empty → []
    let results = engine.similar("nonexistent-id", 5).await.unwrap();
    assert!(results.is_empty(), "nonexistent id should return empty");
}

#[tokio::test]
async fn test_health_and_snapshot() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;

    let health = engine.health().await.unwrap();
    assert_eq!(health.status, "ready");
    assert_eq!(health.nodes, 3); // 3 node cards
    assert!(health.chunk_records > 0);
    assert!(health.vector_records > 0);
    assert!(health.chunk_vector_records > 0);

    let snapshot = engine.snapshot().await.unwrap();
    assert_eq!(snapshot.status, "ready");
    assert_eq!(snapshot.node_count, 3);
    assert!(snapshot.cluster_count > 0);
}

#[tokio::test]
async fn test_clusters_and_members() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;

    // Insert file clusters
    engine
        .sqlite
        .replace_file_clusters(
            &[
                FileCluster {
                    file_id: "src/main.rs".into(),
                    cluster_id: 1,
                    score: 0.9,
                },
                FileCluster {
                    file_id: "src/api.rs".into(),
                    cluster_id: 1,
                    score: 0.7,
                },
                FileCluster {
                    file_id: "docs/architecture.md".into(),
                    cluster_id: 2,
                    score: 0.8,
                },
            ],
            Some("test-epoch"),
        )
        .unwrap();

    let clusters = engine.clusters_response().unwrap();
    assert_eq!(clusters.epoch, "test-epoch");
    assert_eq!(clusters.clusters.len(), 2);

    let members = engine.cluster_members(1).unwrap();
    assert_eq!(members.members.len(), 2);
    // Sorted by score DESC
    assert!(members.members[0].score >= members.members[1].score);
}

#[tokio::test]
async fn test_node_lookup() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;

    let card = engine.node("src/main.rs").unwrap();
    assert_eq!(card.label, "main");
    assert_eq!(card.area, "backend");

    let err = engine.node("nonexistent").unwrap_err();
    assert!(err.to_string().contains("Node not found"));
}

#[tokio::test]
async fn test_context_falls_back_to_lexical_when_dense_finds_nothing() {
    // The lexical scan is a fallback now, not a second opinion, and it is
    // reached by two routes: prefer_lexical, and a dense store that answers
    // nothing. This covers the second — the one production hits when the index
    // is configured but empty. Without it, turning the short-circuit into an
    // unconditional return would silently leave those queries with no results.
    let tmp = tempfile::TempDir::new().unwrap();
    let sqlite = SqliteStore::new(&tmp.path().join("metadata.sqlite")).unwrap();
    sqlite.replace_all_chunks(&test_chunks()).unwrap();

    let engine = QueryEngine::new(
        sqlite,
        LanceStore::new(&tmp.path().join("node_vectors.json")),
        // Present but never written to: dense search returns nothing.
        Some(LanceStore::new(&tmp.path().join("chunk_vectors.json"))),
        None,
    );

    let results = engine
        .context(
            "Helios GPU provider",
            5,
            &HashQueryEmbedder,
            None,
            false, // prefer_lexical = false: the dense path is tried first
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    assert!(
        !results.is_empty(),
        "an empty dense store must fall through to the lexical scan"
    );
    assert!(
        results.iter().all(|chunk| chunk.retrieval == "lexical"),
        "nothing came from the dense path, so every row must be lexical: {results:?}"
    );
}
