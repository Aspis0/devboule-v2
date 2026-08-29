use std::cmp::Ordering;
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};

use oracle_core::{chunk_embedding_text_for_model, ChunkMeta};
use tokenizers::Tokenizer;

fn meta_of(chunk: &serde_json::Value) -> ChunkMeta {
    let gs = |key: &str| {
        chunk
            .get(key)
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string()
    };
    let gi = |key: &str| chunk.get(key).and_then(|value| value.as_i64()).unwrap_or(0);
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
            .and_then(|value| value.as_u64())
            .unwrap_or(0) as usize,
        id: gs("id"),
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let index = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[index]
}

fn main() -> anyhow::Result<()> {
    let chunks_path = env::args().nth(1).expect("chunks.jsonl");
    let tokenizer_path = env::args().nth(2).expect("tokenizer.json");
    let tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|error| anyhow::anyhow!(error))?;
    let file = BufReader::new(File::open(chunks_path)?);
    let mut ratios = Vec::new();
    let mut rows = Vec::new();
    let mut chunks = 0usize;
    let mut empty_tokenizations = 0usize;

    for line in file.lines() {
        let chunk: serde_json::Value = serde_json::from_str(&line?)?;
        let meta = meta_of(&chunk);
        let text = chunk_embedding_text_for_model(&meta, None, false);
        let byte_len = text.len();
        let token_count = tokenizer
            .encode(text.as_str(), false)
            .map_err(|error| anyhow::anyhow!(error))?
            .get_ids()
            .len();
        if token_count == 0 {
            empty_tokenizations += 1;
            continue;
        }
        let ratio = byte_len as f64 / token_count as f64;
        ratios.push(ratio);
        rows.push((ratio, byte_len, token_count, meta.id));
        chunks += 1;
    }

    ratios.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    rows.sort_by(|left, right| left.0.partial_cmp(&right.0).unwrap_or(Ordering::Equal));
    let min = &rows[0];
    let normal: Vec<_> = rows
        .iter()
        .filter(|(_, bytes, _, _)| (900..=1300).contains(bytes))
        .collect();
    let normal_min = normal
        .iter()
        .map(|(ratio, _, _, _)| *ratio)
        .fold(f64::INFINITY, f64::min);
    let normal_max_tokens = normal
        .iter()
        .map(|(_, _, tokens, _)| *tokens)
        .max()
        .unwrap_or(0);

    println!(
        "{{\"chunks\":{chunks},\"empty_tokenizations\":{empty_tokenizations},\"min\":{:.6},\"p1\":{:.6},\"p50\":{:.6},\"min_row\":{{\"bytes\":{},\"tokens\":{},\"id\":{}}},\"normal_900_1300_min\":{:.6},\"normal_900_1300_chunks\":{},\"normal_900_1300_max_tokens\":{}}}",
        min.0,
        percentile(&ratios, 0.01),
        percentile(&ratios, 0.50),
        min.1,
        min.2,
        serde_json::to_string(&min.3)?,
        normal_min,
        normal.len(),
        normal_max_tokens
    );
    Ok(())
}
