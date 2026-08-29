//! bge-small-en-v1.5 ONNX path (phase 3). The tests that open a real graph are
//! operator-run and ignored in CI. Run them with:
//!   cargo test -p oracle-core --test bge_small_embed_test -- --ignored --nocapture

use oracle_core::{CancelFlag, DeclaredModelConfig, EpArg, OnnxEmbedder, PoolingStrategy};
use std::path::{Path, PathBuf};

fn recon_model(name: &str) -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidate = manifest
        .join("../../../recon/models")
        .join(name)
        .canonicalize()
        .ok()?;
    if candidate.join("model_config.json").is_file() && candidate.join("tokenizer.json").is_file() {
        Some(candidate)
    } else {
        None
    }
}

fn graph_exists(dir: &Path) -> bool {
    DeclaredModelConfig::load(dir)
        .ok()
        .and_then(|d| d.graph_path(dir, true).ok())
        .is_some()
}

#[test]
fn bge_declared_config_is_cls_without_semantic_prefix() {
    let Some(dir) = recon_model("bge-small-en-v1.5") else {
        eprintln!("skipping: recon/models/bge-small-en-v1.5 not present");
        return;
    };
    let d = DeclaredModelConfig::load(&dir).unwrap();
    assert_eq!(d.pooling, PoolingStrategy::Cls);
    assert!(!d.uses_semantic_prefix);
    assert_eq!(d.dims, Some(384));
    assert_eq!(d.onnx_graph, "onnx/model_quantized.onnx");
}

#[test]
#[ignore]
fn bge_small_embeds_384_unit_vectors() {
    let Some(dir) = recon_model("bge-small-en-v1.5") else {
        eprintln!("skipping: recon/models/bge-small-en-v1.5 not present");
        return;
    };
    if !graph_exists(&dir) {
        eprintln!(
            "skipping: declared ONNX graph missing under {}",
            dir.display()
        );
        return;
    }
    let (mut embedder, _) = OnnxEmbedder::load_with_precision(&dir, EpArg::Cpu, true)
        .expect("bge-small must load from its declared graph");
    let desc = embedder.descriptor();
    assert_eq!(desc.dims, 384);
    assert_eq!(desc.pooling, PoolingStrategy::Cls);
    assert!(!desc.has_kv_cache);
    assert!(!desc.uses_semantic_prefix);

    let texts = vec!["A short test sentence.".to_string()];
    let vecs = embedder
        .embed_batched(&texts, 1, &CancelFlag::new())
        .expect("bge-small embed");
    assert_eq!(vecs.len(), 1);
    assert_eq!(vecs[0].len(), 384);
    let norm: f32 = vecs[0].iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!(
        (norm - 1.0).abs() < 1e-3,
        "expected L2-normalized vector, got {norm}"
    );
    assert!(vecs[0].iter().all(|x| x.is_finite()));
}

#[test]
#[ignore]
fn qwen3_descriptor_matches_hardcoded_identity_when_present() {
    let Some(dir) = recon_model("qwen3-onnx") else {
        eprintln!("skipping: recon/models/qwen3-onnx not present");
        return;
    };
    if !graph_exists(&dir) {
        eprintln!(
            "skipping: declared Qwen3 graph missing under {}",
            dir.display()
        );
        return;
    }
    let (embedder, _) =
        OnnxEmbedder::load_with_precision(&dir, EpArg::Cpu, true).expect("Qwen3 must still load");
    let desc = embedder.descriptor();
    assert_eq!(desc.pooling, PoolingStrategy::LastToken);
    assert!(desc.uses_semantic_prefix);
    assert_eq!(desc.dims, 1024);
    assert_eq!(desc.max_seq_tokens, 2560);
    assert_eq!(desc.window_overlap_bytes, 256);
    let geo = desc.kv_geometry.as_ref().expect("Qwen3 has KV cache");
    assert_eq!(geo.num_layers, 28);
    assert_eq!(geo.num_kv_heads, 8);
    assert_eq!(geo.head_dim, 128);
}
