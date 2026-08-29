//! Build frozen-corpus chunk files for the document/structured geometry study.
//!
//! This is an additive measurement helper.  It keeps the production code
//! path for code files and overrides only the document/structured profiles.

use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use oracle_core::{
    build_chunks_for_file, build_chunks_for_file_with_limits, chunk_geometry_fingerprint,
    collect_text_files, ChunkMeta,
};

fn arg(name: &str) -> String {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn frozen_roots(path: &Path) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let repos = raw
        .get("repos")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("{} has no repos[]", path.display()))?;
    let mut out = Vec::new();
    for repo in repos {
        let id = repo.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let root = PathBuf::from(repo.get("path").and_then(|v| v.as_str()).unwrap_or(""));
        if !id.is_empty() && root.is_dir() {
            out.push((id.to_string(), root));
        }
    }
    Ok(out)
}

fn ext(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_ascii_lowercase()))
        .unwrap_or_default()
}

fn has_component(path: &Path, wanted: &str) -> bool {
    path.components()
        .any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case(wanted))
}

fn profile_kind(path: &Path) -> &'static str {
    let suffix = ext(path);
    if matches!(suffix.as_str(), ".md" | ".txt") || has_component(path, "docs") {
        return "document";
    }
    if matches!(
        suffix.as_str(),
        ".gradle"
            | ".html"
            | ".json"
            | ".jsonc"
            | ".properties"
            | ".toml"
            | ".xml"
            | ".yaml"
            | ".yml"
    ) {
        return "structured";
    }
    if matches!(
        suffix.as_str(),
        ".css"
            | ".java"
            | ".js"
            | ".jsx"
            | ".kt"
            | ".kts"
            | ".mjs"
            | ".cjs"
            | ".mts"
            | ".cts"
            | ".ps1"
            | ".py"
            | ".r"
            | ".rmd"
            | ".rs"
            | ".sh"
            | ".sql"
            | ".ts"
            | ".tsx"
    ) {
        return "code";
    }
    "other"
}

fn override_limits(profile: &str, kind: &str) -> Option<(usize, usize)> {
    match (profile, kind) {
        ("a", _) => None,
        ("b", "document") | ("b", "structured") => Some((1024, 164)),
        ("c", "document") | ("c", "structured") => Some((2048, 328)),
        (_, _) => None,
    }
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
    let profile = arg("--profile");
    let out_dir = PathBuf::from(arg("--out"));
    let roots_file = PathBuf::from(arg("--queries"));
    if !matches!(profile.as_str(), "a" | "b" | "c") {
        anyhow::bail!("--profile must be a, b, or c");
    }
    std::fs::create_dir_all(&out_dir)?;

    let jobs = frozen_roots(&roots_file)?;
    let t_collect = Instant::now();
    let mut files: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (_, root) in &jobs {
        files.extend(
            collect_text_files(root)
                .into_iter()
                .map(|p| (p, root.clone())),
        );
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let collect_ms = t_collect.elapsed().as_millis();

    let chunks_path = out_dir.join("chunks.jsonl");
    let mut writer = BufWriter::new(std::fs::File::create(&chunks_path)?);
    let t_chunk = Instant::now();
    let mut chunks_total = 0usize;
    let mut bytes_selected = 0u64;
    let mut by_type: HashMap<String, (usize, usize)> = HashMap::new();
    let mut by_ext: HashMap<String, (usize, usize)> = HashMap::new();
    let mut raw_chars = 0usize;
    let mut chunk_chars: Vec<usize> = Vec::new();

    for (path, root) in &files {
        bytes_selected += path.metadata().map(|m| m.len()).unwrap_or(0);
        let kind = profile_kind(path);
        let suffix = ext(path);
        let chunks = match override_limits(&profile, kind) {
            Some((max_chars, overlap)) => {
                build_chunks_for_file_with_limits(path, root, max_chars, overlap)
            }
            None => build_chunks_for_file(path, root),
        };
        let type_entry = by_type.entry(kind.to_string()).or_default();
        type_entry.0 += 1;
        type_entry.1 += chunks.len();
        let ext_entry = by_ext.entry(suffix).or_default();
        ext_entry.0 += 1;
        ext_entry.1 += chunks.len();
        chunks_total += chunks.len();
        for chunk in chunks {
            let meta = meta_of(&chunk);
            raw_chars += meta.text.chars().count();
            chunk_chars.push(meta.text.chars().count());
            serde_json::to_writer(&mut writer, &chunk)?;
            writer.write_all(b"\n")?;
        }
    }
    writer.flush()?;
    let chunk_ms = t_chunk.elapsed().as_millis();
    chunk_chars.sort_unstable();
    let percentile = |p: f64| -> usize {
        if chunk_chars.is_empty() {
            return 0;
        }
        let i = ((chunk_chars.len() as f64 - 1.0) * p).round() as usize;
        chunk_chars[i.min(chunk_chars.len() - 1)]
    };
    let map_rows = |map: HashMap<String, (usize, usize)>| -> Vec<serde_json::Value> {
        let mut rows: Vec<_> = map
            .into_iter()
            .map(|(name, (files, chunks))| serde_json::json!({"name": name, "files": files, "chunks": chunks}))
            .collect();
        rows.sort_by(|a, b| b["chunks"].as_u64().cmp(&a["chunks"].as_u64()));
        rows
    };
    let report = serde_json::json!({
        "profile": profile,
        "geometry_fingerprint_production": chunk_geometry_fingerprint(),
        "roots": jobs.iter().map(|(id, root)| serde_json::json!({"id": id, "path": root})).collect::<Vec<_>>(),
        "files_selected": files.len(),
        "bytes_selected": bytes_selected,
        "chunks_total": chunks_total,
        "raw_chars_total": raw_chars,
        "chunk_raw_chars": {"p50": percentile(0.50), "p90": percentile(0.90), "p99": percentile(0.99), "max": chunk_chars.last().copied().unwrap_or(0)},
        "by_type": map_rows(by_type),
        "by_ext": map_rows(by_ext),
        "timings_ms": {"collect": collect_ms, "chunk": chunk_ms},
    });
    std::fs::write(
        out_dir.join("corpus.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
