//! Operator-run proof for the real ONNX cross-encoder.
//!
//! The normal suite exercises declaration parsing and the absent-model no-op.
//! This test intentionally opens a real graph and is therefore ignored in CI.
//! Run it with:
//!   cargo test -p oracle-core --test reranker_real_onnx_test -- --ignored --nocapture

use oracle_core::onnx_embedder::EpArg;
use oracle_core::query::reranker::OnnxReranker;
use std::path::PathBuf;

fn model_dir() -> PathBuf {
    std::env::var_os("ORACLE_RERANKER_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../recon/models/ms-marco-TinyBERT-L-2-v2")
        })
}

#[test]
#[ignore]
fn real_onnx_reranker_scores_query_document_pairs() {
    let model = model_dir();
    let mut reranker = OnnxReranker::load(&model, EpArg::Cpu)
        .expect("real reranker model must load from its declared artifact");
    let documents = vec![
        "fn parse_config(path: &Path) -> Result<Config> { read_json(path) }".to_string(),
        "The weather forecast calls for rain tomorrow afternoon.".to_string(),
    ];
    let scores = reranker
        .score_pairs("How does the code parse a config file?", &documents)
        .expect("real reranker must score declared BERT pairs");
    assert_eq!(scores.len(), documents.len());
    assert!(scores.iter().all(|score| score.is_finite()));
    assert!(
        scores[0] > scores[1],
        "code document should outrank unrelated weather document: {scores:?}"
    );
}
