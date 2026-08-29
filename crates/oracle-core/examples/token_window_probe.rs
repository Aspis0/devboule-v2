//! Inspect the exact token-aware ONNX window plan for frozen corpus chunks.

use std::path::PathBuf;

use oracle_core::{chunk_embedding_text_for_model, ChunkMeta, EpArg, OnnxEmbedder};

fn arg(name: &str) -> String {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn meta_of(chunk: &serde_json::Value) -> ChunkMeta {
    let gs = |k: &str| {
        chunk
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let gi = |k: &str| chunk.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
    ChunkMeta {
        file_id: gs("file_id"),
        file_sorgente: gs("file_sorgente"),
        text: gs("text"),
        kind: gs("kind"),
        symbol_name: gs("symbol_name"),
        language: gs("language"),
        line_start: gi("line_start"),
        line_end: gi("line_end"),
        symbols_used: gs("symbols_used"),
        chunk_index: chunk
            .get("chunk_index")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        id: gs("id"),
    }
}

fn main() -> anyhow::Result<()> {
    let model_dir = PathBuf::from(arg("--model-dir"));
    let chunks_path = PathBuf::from(arg("--chunks"));
    let n: usize = arg("--n").parse()?;
    let raw = std::fs::read_to_string(chunks_path)?;
    let records: Vec<serde_json::Value> = raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(n)
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;

    let (mut embedder, load_ms) = OnnxEmbedder::load(&model_dir, EpArg::Cpu)?;
    let uses_semantic_prefix = embedder.descriptor().uses_semantic_prefix;
    let texts: Vec<String> = records
        .iter()
        .map(|record| {
            let meta = meta_of(record);
            chunk_embedding_text_for_model(&meta, None, uses_semantic_prefix)
        })
        .collect();
    let (windows_total, windows_truncated) = embedder.token_window_stats(&texts)?;
    println!(
        "{}",
        serde_json::json!({
            "model_id": embedder.descriptor().id,
            "load_ms": load_ms,
            "n_chunks": texts.len(),
            "max_seq_tokens": embedder.max_seq_tokens(),
            "special_tokens": embedder.special_token_count(),
            "windows_total": windows_total,
            "windows_truncated": windows_truncated,
        })
    );
    Ok(())
}
