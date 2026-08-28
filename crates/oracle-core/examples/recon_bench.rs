//! recon_bench — measurement harness for the oracle-core v1 performance investigation.
//! Additive example; uses ONLY production code paths (no copies of pipeline logic).
//!
//! Modes:
//!   corpus --root DIR [--limit N] --out DIR          collector+chunker+prefix+window stats
//!   embed  --model-dir DIR --variant V --ep cpu|directml --chunks F.jsonl
//!          [--n K] [--queries F.json] --out F.json   timed embedding via OnnxEmbedder
//!   eval   --chunks F.jsonl --queries F.json --dense F.json [--ref F.json] [--k 5]
//!                                                    lexical vs dense vs hybrid + fidelity
//!   store  --chunks F.jsonl --out DIR               sqlite+lance commit-cadence timing

use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use oracle_core::embed::{
    resolve_embed_window_bytes, resolve_embed_window_overlap_bytes, window_text, CancelFlag,
};
use oracle_core::ingest::chunking::build_chunks_for_file;
use oracle_core::ingest::collect::collect_text_files;
use oracle_core::ingest::retrieval_text::{chunk_embedding_text, query_embedding_text, ChunkMeta};
use oracle_core::onnx_embedder::{EpArg, OnnxEmbedder};

const CODE_EXTS: &[&str] = &[
    ".css", ".java", ".js", ".jsx", ".kt", ".kts", ".mjs", ".cjs", ".mts", ".cts", ".ps1", ".py",
    ".r", ".rmd", ".rs", ".sh", ".sql", ".ts", ".tsx",
];

fn arg_of(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn pct(sorted: &[usize], p: f64) -> usize {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
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

// ───────────────────────────── corpus mode ─────────────────────────────

struct TreeStat {
    files: u64,
    bytes: u64,
}

fn walk_stats(dir: &Path, depth: usize, out: &mut HashMap<String, TreeStat>) {
    // stats-only walk (independent of collector) for the excluded-vs-selected ratio
    if depth > 12 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        if ft.is_dir() {
            // do not descend into junctions/symlinks
            if e.path()
                .symlink_metadata()
                .map(|m| m.is_symlink())
                .unwrap_or(false)
            {
                continue;
            }
            walk_stats(&e.path(), depth + 1, out);
        } else if ft.is_file() {
            let bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
            let top = name.clone(); // file bucket
            let ent = out.entry(top).or_insert(TreeStat { files: 0, bytes: 0 });
            ent.files += 1;
            ent.bytes += bytes;
        }
    }
}

fn top_level_stats(root: &Path) -> Vec<(String, u64, u64)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else {
        return out;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            let mut map = HashMap::new();
            walk_stats(&e.path(), 0, &mut map);
            let files: u64 = map.values().map(|s| s.files).sum();
            let bytes: u64 = map.values().map(|s| s.bytes).sum();
            out.push((name, files, bytes));
        } else {
            let bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
            out.push((name, 1, bytes));
        }
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.2));
    out
}

fn run_corpus() -> anyhow::Result<()> {
    let root = PathBuf::from(arg_of("--root").expect("--root DIR"));
    let limit: usize = arg_of("--limit").and_then(|v| v.parse().ok()).unwrap_or(0);
    let out_dir = PathBuf::from(arg_of("--out").expect("--out DIR"));
    std::fs::create_dir_all(&out_dir)?;

    let t0 = Instant::now();
    let mut files = collect_text_files(&root);
    let collect_ms = t0.elapsed().as_millis();
    files.sort(); // deterministic subset independent of priority order
    if limit > 0 && files.len() > limit {
        files.truncate(limit);
    }

    let t1 = Instant::now();
    let chunks_path = out_dir.join("chunks.jsonl");
    let f = std::fs::File::create(&chunks_path)?;
    let mut w = BufWriter::new(f);

    let mut raw_lens: Vec<usize> = Vec::new();
    let mut emb_lens: Vec<usize> = Vec::new();
    let mut window_counts: Vec<usize> = Vec::new();
    let mut per_ext: HashMap<String, (usize, usize)> = HashMap::new(); // ext -> (files, chunks)
    let mut per_dir: HashMap<String, (usize, usize, u64)> = HashMap::new(); // top dir -> (files, chunks, bytes)
    let mut per_file_chunks: Vec<(String, usize)> = Vec::new();
    let mut total_bytes_selected: u64 = 0;
    let mut chunks_total = 0usize;
    let mut windows_total = 0usize;
    let mut emb_chars_total = 0usize;
    let mut raw_chars_total = 0usize;
    let mut mid_body = 0usize;
    let mut odd_end = 0usize;
    let mut examples_mid: Vec<serde_json::Value> = Vec::new();
    let mut examples_end: Vec<serde_json::Value> = Vec::new();

    let win_bytes = resolve_embed_window_bytes();
    let overlap = resolve_embed_window_overlap_bytes();

    for path in &files {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_lowercase()))
            .unwrap_or_default();
        let top_dir = rel.split('/').next().unwrap_or("").to_string();
        let fbytes = path.metadata().map(|m| m.len()).unwrap_or(0);
        total_bytes_selected += fbytes;

        let chunks = build_chunks_for_file(path, &root);
        per_file_chunks.push((rel.clone(), chunks.len()));
        let e = per_ext.entry(ext.clone()).or_default();
        e.0 += 1;
        e.1 += chunks.len();
        let d = per_dir.entry(top_dir).or_default();
        d.0 += 1;
        d.2 += fbytes;
        d.1 += chunks.len();

        let is_code = CODE_EXTS.contains(&ext.as_str());
        for (ci, chunk) in chunks.iter().enumerate() {
            let meta = meta_of(chunk);
            let emb = chunk_embedding_text(&meta, None);
            let wc = window_text(&emb, win_bytes, overlap).len();
            windows_total += wc;
            window_counts.push(wc);
            chunks_total += 1;
            emb_chars_total += emb.len();
            raw_chars_total += meta.text.len();
            raw_lens.push(meta.text.chars().count());
            emb_lens.push(emb.len());

            if is_code && ci > 0 {
                let first = meta
                    .text
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or("");
                let starts_boundary = [
                    "use ",
                    "mod ",
                    "pub ",
                    "fn ",
                    "struct ",
                    "enum ",
                    "impl ",
                    "trait ",
                    "const ",
                    "static ",
                    "type ",
                    "#[",
                    "//",
                    "/*",
                    "*",
                    "import ",
                    "export ",
                    "class ",
                    "def ",
                    "from ",
                    "async ",
                    "package ",
                    "using ",
                    "namespace ",
                    "}",
                    ")",
                    "]",
                    ";",
                ]
                .iter()
                .any(|p| first.starts_with(p));
                if !first.is_empty() && !starts_boundary {
                    mid_body += 1;
                    if examples_mid.len() < 5 {
                        examples_mid.push(serde_json::json!({
                            "id": meta.id, "first_line": first.chars().take(90).collect::<String>()
                        }));
                    }
                }
                let last = meta
                    .text
                    .lines()
                    .rev()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or("");
                let ends_ok = ["}", "{", ";", ")", "]", ",", "*/", "\\"]
                    .iter()
                    .any(|p| last.ends_with(p));
                if !last.is_empty() && !ends_ok {
                    odd_end += 1;
                    if examples_end.len() < 5 {
                        examples_end.push(serde_json::json!({
                            "id": meta.id, "last_line": last.chars().take(90).collect::<String>()
                        }));
                    }
                }
            }

            let mut rec = chunk.clone();
            let obj = rec.as_object_mut().unwrap();
            obj.insert("emb_text".into(), serde_json::Value::String(emb));
            obj.insert("windows".into(), serde_json::Value::from(wc));
            serde_json::to_writer(&mut w, &rec)?;
            w.write_all(b"\n")?;
        }
    }
    w.flush()?;
    let chunk_ms = t1.elapsed().as_millis();

    raw_lens.sort_unstable();
    emb_lens.sort_unstable();
    window_counts.sort_unstable();
    let multi = window_counts.iter().filter(|&&w| w > 1).count();

    let mut ext_rows: Vec<serde_json::Value> = per_ext
        .iter()
        .map(|(k, (f, c))| serde_json::json!({"ext": k, "files": f, "chunks": c}))
        .collect();
    ext_rows.sort_by(|a, b| b["chunks"].as_u64().cmp(&a["chunks"].as_u64()));
    let mut dir_rows: Vec<serde_json::Value> = per_dir
        .iter()
        .map(|(k, (f, c, b))| serde_json::json!({"dir": k, "files": f, "chunks": c, "bytes": b}))
        .collect();
    dir_rows.sort_by(|a, b| b["chunks"].as_u64().cmp(&a["chunks"].as_u64()));
    per_file_chunks.sort_by_key(|a| std::cmp::Reverse(a.1));

    let report = serde_json::json!({
        "root": root,
        "limit": limit,
        "files_selected": files.len(),
        "bytes_selected": total_bytes_selected,
        "chunks_total": chunks_total,
        "windows_total": windows_total,
        "window_bytes_cfg": win_bytes,
        "overlap_bytes_cfg": overlap,
        "emb_chars_total": emb_chars_total,
        "raw_chars_total": raw_chars_total,
        "prefix_overhead_chars": emb_chars_total.saturating_sub(raw_chars_total),
        "chunks_multi_window": multi,
        "chunks_multi_window_pct": if chunks_total>0 {(multi as f64 *100.0/chunks_total as f64*10.0).round()/10.0} else {0.0},
        "chunk_raw_chars": {"p50": pct(&raw_lens,0.5),"p90": pct(&raw_lens,0.9),"p99": pct(&raw_lens,0.99),"max": raw_lens.last().copied().unwrap_or(0)},
        "chunk_emb_chars": {"p50": pct(&emb_lens,0.5),"p90": pct(&emb_lens,0.9),"p99": pct(&emb_lens,0.99),"max": emb_lens.last().copied().unwrap_or(0)},
        "windows_hist": {"1": window_counts.iter().filter(|&&w| w==1).count(),
                         "2": window_counts.iter().filter(|&&w| w==2).count(),
                         "3+": window_counts.iter().filter(|&&w| w>=3).count()},
        "by_ext": ext_rows.into_iter().take(15).collect::<Vec<_>>(),
        "by_dir": dir_rows,
        "top_files_by_chunks": per_file_chunks.into_iter().take(10).map(|(f,c)| serde_json::json!({"file": f, "chunks": c})).collect::<Vec<_>>(),
        "bad_boundary": {"mid_body_start": mid_body, "odd_end": odd_end,
                         "examples_mid": examples_mid, "examples_end": examples_end},
        "timings_ms": {"collect": collect_ms, "chunk_plus_prefix_plus_window": chunk_ms},
        "top_level_tree": top_level_stats(&root).into_iter().take(25).map(|(n,f,b)| serde_json::json!({"name": n, "files": f, "bytes": b})).collect::<Vec<_>>(),
    });
    std::fs::write(
        out_dir.join("corpus.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

// ───────────────────────────── embed mode ─────────────────────────────

struct CpuSampler {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<Vec<(f32, f32, usize)>>>,
}

impl CpuSampler {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let pid = std::process::id();
        let handle = std::thread::spawn(move || {
            let mut samples: Vec<(f32, f32, usize)> = Vec::new();
            let mut sys = sysinfo::System::new();
            let me = sysinfo::Pid::from_u32(pid);
            sys.refresh_cpu_usage();
            sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[me]), true);
            std::thread::sleep(std::time::Duration::from_millis(300));
            while !stop2.load(Ordering::Relaxed) {
                sys.refresh_cpu_usage();
                sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[me]), true);
                let g = sys.global_cpu_usage();
                let (p, th) = sys
                    .process(me)
                    .map(|pr| (pr.cpu_usage(), pr.tasks().map(|t| t.len()).unwrap_or(0)))
                    .unwrap_or((0.0, 0));
                samples.push((g, p, th));
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            samples
        });
        CpuSampler {
            stop,
            handle: Some(handle),
        }
    }
    fn stop(mut self) -> Vec<(f32, f32, usize)> {
        self.stop.store(true, Ordering::Relaxed);
        self.handle
            .take()
            .map(|h| h.join().unwrap_or_default())
            .unwrap_or_default()
    }
}

fn summarize(samples: &[(f32, f32, usize)]) -> serde_json::Value {
    if samples.is_empty() {
        return serde_json::json!({"samples": 0});
    }
    let g: Vec<f32> = samples.iter().map(|s| s.0).collect();
    let p: Vec<f32> = samples.iter().map(|s| s.1).collect();
    let th: Vec<usize> = samples.iter().map(|s| s.2).collect();
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    serde_json::json!({
        "samples": samples.len(),
        "sys_cpu_avg_pct": (mean(&g) * 10.0).round() / 10.0,
        "sys_cpu_peak_pct": (g.iter().cloned().fold(0.0f32, f32::max) * 10.0).round() / 10.0,
        "proc_cpu_avg_pct": (mean(&p) * 10.0).round() / 10.0,
        "proc_cpu_peak_pct": (p.iter().cloned().fold(0.0f32, f32::max) * 10.0).round() / 10.0,
        "threads_avg": th.iter().sum::<usize>() / th.len(),
        "threads_peak": th.iter().copied().max().unwrap_or(0),
    })
}

fn read_jsonl(path: &Path) -> anyhow::Result<Vec<serde_json::Value>> {
    let s = std::fs::read_to_string(path)?;
    Ok(s.lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<Vec<_>, _>>()?)
}

fn run_embed() -> anyhow::Result<()> {
    let model_dir = PathBuf::from(arg_of("--model-dir").expect("--model-dir"));
    let variant = arg_of("--variant").expect("--variant");
    let ep_s = arg_of("--ep").unwrap_or_else(|| "cpu".into());
    let ep = match ep_s.as_str() {
        "directml" | "dml" => EpArg::Directml,
        "coreml" => EpArg::Coreml,
        _ => EpArg::Cpu,
    };
    let chunks_file = PathBuf::from(arg_of("--chunks").expect("--chunks"));
    let n: usize = arg_of("--n").and_then(|v| v.parse().ok()).unwrap_or(32);
    let out = PathBuf::from(arg_of("--out").expect("--out"));
    let queries: Vec<String> = match arg_of("--queries") {
        Some(qp) => {
            let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(qp)?)?;
            let parsed: Vec<String> = match raw {
                serde_json::Value::Array(items) => items
                    .into_iter()
                    .map(|it| match it {
                        serde_json::Value::String(s) => s,
                        obj => obj
                            .get("q")
                            .and_then(|q| q.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect(),
                other => {
                    return Err(anyhow::anyhow!(
                        "queries file must be an array, got {other}"
                    ))
                }
            };
            parsed
        }
        None => Vec::new(),
    };

    let records = read_jsonl(&chunks_file)?;
    let records = &records[..records.len().min(n)];
    let texts: Vec<String> = records
        .iter()
        .map(|r| {
            let m = meta_of(r);
            chunk_embedding_text(&m, None)
        })
        .collect();
    let win_bytes = resolve_embed_window_bytes();
    let overlap = resolve_embed_window_overlap_bytes();
    let mut windows_total = 0usize;
    let mut emb_chars = 0usize;
    for t in &texts {
        windows_total += window_text(t, win_bytes, overlap).len();
        emb_chars += t.len();
    }

    std::env::set_var("ORACLE_RS_ONNX_VARIANT", &variant);
    let t0 = Instant::now();
    let (mut emb, load_ms) = OnnxEmbedder::load(&model_dir, ep)
        .map_err(|e| anyhow::anyhow!("LOAD FAILED variant={variant} ep={ep_s}: {e:#}"))?;
    let load_total_ms = t0.elapsed().as_millis();

    // warmup (excluded from timing): one short text
    let warm = Instant::now();
    let _ = emb.embed_batched(&["fn warmup() {}\n".to_string()], 1, &CancelFlag::new())?;
    let warmup_ms = warm.elapsed().as_millis();

    let sampler = CpuSampler::start();
    let t1 = Instant::now();
    let mut per_group: Vec<u128> = Vec::new();
    let mut vectors: Vec<Vec<f32>> = Vec::new();
    for group in texts.chunks(32) {
        let g0 = Instant::now();
        let v = emb.embed_batched(group, group.len(), &CancelFlag::new())?;
        per_group.push(g0.elapsed().as_millis());
        vectors.extend(v);
    }
    let embed_ms = t1.elapsed().as_millis();
    let samples_chunk = sampler.stop();

    let (query_vectors, query_ms) = if !queries.is_empty() {
        let qtexts: Vec<String> = queries
            .iter()
            .map(|q| query_embedding_text(q, None))
            .collect();
        let sampler = CpuSampler::start();
        let t = Instant::now();
        let v = emb.embed_batched(&qtexts, qtexts.len(), &CancelFlag::new())?;
        let ms = t.elapsed().as_millis();
        let _ = sampler.stop();
        (Some(v), Some(ms))
    } else {
        (None, None)
    };

    let out_json = serde_json::json!({
        "variant": variant, "ep": ep_s, "model_id": emb_model_id(&emb),
        "load_ms": load_ms, "load_total_ms": load_total_ms, "warmup_ms": warmup_ms,
        "n_chunks": texts.len(), "emb_chars_total": emb_chars,
        "windows_total": windows_total,
        "attention_budget_env": std::env::var("ORACLE_CHUNK_ATTENTION_BUDGET").unwrap_or_else(|_| "default".into()),
        "embed_ms_total": embed_ms, "per_group_ms": per_group,
        "chunks_per_sec": if embed_ms>0 {(texts.len() as f64)*1000.0/embed_ms as f64} else {0.0},
        "windows_per_sec": if embed_ms>0 {(windows_total as f64)*1000.0/embed_ms as f64} else {0.0},
        "query_ms": query_ms, "n_queries": queries.len(),
        "dim": vectors.first().map(|v| v.len()).unwrap_or(0),
        "chunk_ids": records.iter().map(|r| r["id"].as_str().unwrap_or("")).collect::<Vec<_>>(),
        "vectors": vectors,
        "query_vectors": query_vectors,
        "cpu_during_embed": summarize(&samples_chunk),
    });
    std::fs::write(&out, serde_json::to_string(&out_json)?)?;
    println!(
        "variant={variant} ep={ep_s} load_ms={load_ms} warmup_ms={warmup_ms} chunks={} windows={windows_total} embed_ms={embed_ms} cps={:.2} wps={:.2} cpu={}",
        texts.len(),
        if embed_ms>0 {(texts.len() as f64)*1000.0/embed_ms as f64} else {0.0},
        if embed_ms>0 {(windows_total as f64)*1000.0/embed_ms as f64} else {0.0},
        out_json["cpu_during_embed"],
    );
    Ok(())
}

fn emb_model_id(_e: &OnnxEmbedder) -> String {
    // model id is derived from variant; reconstruct for the record
    format!(
        "Qwen3-Embedding-0.6B-ONNX-{}",
        std::env::var("ORACLE_RS_ONNX_VARIANT").unwrap_or_default()
    )
}

// ───────────────────────────── eval mode ─────────────────────────────

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for i in 0..a.len().min(b.len()) {
        dot += a[i] as f64 * b[i] as f64;
        na += a[i] as f64 * a[i] as f64;
        nb += b[i] as f64 * b[i] as f64;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

fn run_eval() -> anyhow::Result<()> {
    let chunks_file = PathBuf::from(arg_of("--chunks").expect("--chunks"));
    let queries_file = PathBuf::from(arg_of("--queries").expect("--queries"));
    let dense_file = PathBuf::from(arg_of("--dense").expect("--dense"));
    let ref_file = arg_of("--ref").map(PathBuf::from);
    let k: usize = arg_of("--k").and_then(|v| v.parse().ok()).unwrap_or(5);

    let records = read_jsonl(&chunks_file)?;
    let queries: Vec<serde_json::Value> =
        serde_json::from_value(serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(&queries_file)?,
        )?)?;
    let dense: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&dense_file)?)?;

    // lexical scoring input
    let scored: Vec<oracle_core::query::lexical::ScoredChunk> = records
        .iter()
        .map(|r| serde_json::from_value(r.clone()))
        .collect::<Result<Vec<_>, _>>()?;

    let dvecs: Vec<Vec<f32>> = dense["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            v.as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                .collect()
        })
        .collect();
    let dq: Vec<Vec<f32>> = dense["query_vectors"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|v| {
                    v.as_array()
                        .unwrap()
                        .iter()
                        .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default();
    let ids: Vec<String> = dense["chunk_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap_or("").to_string())
        .collect();
    // index vec rows by chunk id (dense file order == chunk_ids order)
    let vec_of: HashMap<&str, &[f32]> = ids
        .iter()
        .enumerate()
        .filter_map(|(i, id)| dvecs.get(i).map(|v| (id.as_str(), v.as_slice())))
        .collect();

    let limit = k * 3;
    let mut rows = Vec::new();
    for (qi, q) in queries.iter().enumerate() {
        let query = q["q"].as_str().unwrap_or("");
        let targets: Vec<String> = q["targets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // lexical
        let lex = oracle_core::query::lexical::lexical_chunk_context(query, &scored, limit);
        // dense
        let mut denser: Vec<(String, f64)> = Vec::new();
        if let Some(qv) = dq.get(qi) {
            for (i, r) in records.iter().enumerate() {
                let id = r["id"].as_str().unwrap_or("");
                if let Some(v) = vec_of.get(id) {
                    denser.push((id.to_string(), cosine(qv, v)));
                }
                let _ = i;
            }
            denser.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            denser.truncate(limit);
        }
        // hybrid: max(dense, lexical) over the union (engine.context semantics)
        let mut score_by_id: HashMap<String, f64> = HashMap::new();
        for (id, s) in &denser {
            *score_by_id.entry(id.clone()).or_insert(0.0) = s.max(0.0);
        }
        for l in &lex {
            let e = score_by_id.entry(l.chunk_id.clone()).or_insert(0.0);
            *e = e.max(l.score);
        }
        let mut hybrid: Vec<(String, f64)> = score_by_id.into_iter().collect();
        hybrid.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        hybrid.truncate(limit);

        // file-level metrics helpers
        let file_of = |id: &str| -> String {
            records
                .iter()
                .find(|r| r["id"].as_str() == Some(id))
                .and_then(|r| r["file_sorgente"].as_str())
                .unwrap_or("")
                .to_string()
        };
        let metrics = |ranked: &[(String, f64)]| -> serde_json::Value {
            let files: Vec<String> = ranked
                .iter()
                .map(|(id, _)| file_of(id))
                .filter(|f| !f.is_empty())
                .collect();
            let mut distinct: Vec<String> = Vec::new();
            for f in files {
                if !distinct.contains(&f) {
                    distinct.push(f);
                }
            }
            let top5: Vec<&String> = distinct.iter().take(5).collect();
            let hits = targets.iter().filter(|t| top5.contains(t)).count();
            let recall5 = if targets.is_empty() {
                0.0
            } else {
                hits as f64 / targets.len() as f64
            };
            let hit5 = if hits > 0 { 1.0 } else { 0.0 };
            let mut mrr = 0.0;
            for (i, f) in distinct.iter().enumerate() {
                if targets.contains(f) {
                    mrr = 1.0 / (i as f64 + 1.0);
                    break;
                }
                if i >= 9 {
                    break;
                }
            }
            serde_json::json!({"recall@5": recall5, "hit@5": hit5, "mrr@10": mrr})
        };

        let lex_ranked: Vec<(String, f64)> =
            lex.iter().map(|l| (l.chunk_id.clone(), l.score)).collect();
        rows.push(serde_json::json!({
            "q": query, "targets": targets,
            "lexical": metrics(&lex_ranked),
            "dense": metrics(&denser),
            "hybrid": metrics(&hybrid),
        }));
    }

    let mean = |key: &str, mode: &str| -> f64 {
        let vals: Vec<f64> = rows.iter().filter_map(|r| r[mode][key].as_f64()).collect();
        if vals.is_empty() {
            0.0
        } else {
            vals.iter().sum::<f64>() / vals.len() as f64
        }
    };
    let mut summary = serde_json::json!({
        "n_queries": rows.len(),
        "lexical": {"recall@5": mean("recall@5","lexical"), "hit@5": mean("hit@5","lexical"), "mrr@10": mean("mrr@10","lexical")},
        "dense": {"recall@5": mean("recall@5","dense"), "hit@5": mean("hit@5","dense"), "mrr@10": mean("mrr@10","dense")},
        "hybrid": {"recall@5": mean("recall@5","hybrid"), "hit@5": mean("hit@5","hybrid"), "mrr@10": mean("mrr@10","hybrid")},
    });

    // fidelity vs fp32 reference on the same chunk set
    if let Some(rf) = ref_file {
        let refr: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(rf)?)?;
        let rvecs: Vec<Vec<f32>> = refr["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                v.as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                    .collect()
            })
            .collect();
        let rq: Vec<Vec<f32>> = refr["query_vectors"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| {
                        v.as_array()
                            .unwrap()
                            .iter()
                            .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                            .collect()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let ref_ids: Vec<String> = refr["chunk_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap_or("").to_string())
            .collect();

        let mut cosims: Vec<f64> = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            if let Some(j) = ref_ids.iter().position(|r| r == id) {
                if let (Some(a), Some(b)) = (dvecs.get(i), rvecs.get(j)) {
                    cosims.push(cosine(a, b));
                }
            }
        }
        cosims.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mean_c = cosims.iter().sum::<f64>() / cosims.len() as f64;
        let min_c = cosims.first().copied().unwrap_or(0.0);
        let p05 = pct(
            &(cosims
                .iter()
                .map(|c| (c * 10000.0) as usize)
                .collect::<Vec<_>>()),
            0.05,
        ) as f64
            / 10000.0;

        // query ranking stability
        let mut top1_changed = 0usize;
        let mut ov5_sum = 0.0;
        let mut nq = 0usize;
        for (qi, _) in queries.iter().enumerate() {
            let (Some(a), Some(b)) = (dq.get(qi), rq.get(qi)) else {
                continue;
            };
            let rank = |qv: &[f32], mat: &[Vec<f32>], ids_: &[String]| -> Vec<String> {
                let mut r: Vec<(String, f64)> = ids_
                    .iter()
                    .enumerate()
                    .filter_map(|(i, id)| mat.get(i).map(|v| (id.clone(), cosine(qv, v))))
                    .collect();
                r.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
                r.into_iter().map(|(id, _)| id).collect()
            };
            let rr = rank(a, &rvecs, &ref_ids);
            let rc = rank(b, &dvecs, &ids);
            if rr.is_empty() || rc.is_empty() {
                continue;
            }
            nq += 1;
            if rr[0] != rc[0] {
                top1_changed += 1;
            }
            let s5: std::collections::HashSet<&String> = rr.iter().take(5).collect();
            let o = rc.iter().take(5).filter(|id| s5.contains(id)).count();
            ov5_sum += o as f64 / 5.0;
        }
        summary["fidelity_vs_fp32"] = serde_json::json!({
            "n_chunks": cosims.len(),
            "cosine_mean": (mean_c*10000.0).round()/10000.0,
            "cosine_min": (min_c*10000.0).round()/10000.0,
            "cosine_p05": p05,
            "queries": nq,
            "top1_changed_pct": if nq>0 {(top1_changed as f64)*100.0/nq as f64} else {0.0},
            "overlap_at5_mean": if nq>0 {ov5_sum/nq as f64} else {0.0},
        });
    }

    let out = serde_json::json!({"summary": summary, "per_query": rows});
    println!("{}", serde_json::to_string_pretty(&out)?);
    if let Some(op) = arg_of("--save") {
        std::fs::write(op, serde_json::to_string_pretty(&out)?)?;
    }
    Ok(())
}

// ───────────────────────────── store mode ─────────────────────────────

fn run_store() -> anyhow::Result<()> {
    let chunks_file = PathBuf::from(arg_of("--chunks").expect("--chunks"));
    let out_dir = PathBuf::from(arg_of("--out").expect("--out"));
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir)?;

    let records = read_jsonl(&chunks_file)?;
    let mut by_file: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    for r in &records {
        let fid = r["file_sorgente"].as_str().unwrap_or("").to_string();
        by_file.entry(fid).or_default().push(r.clone());
    }
    let mut file_ids: Vec<String> = by_file.keys().cloned().collect();
    file_ids.sort();

    let sqlite = oracle_core::store::sqlite::SqliteStore::new(&out_dir.join("metadata.sqlite"))?;
    let lance = oracle_core::store::lance::LanceStore::new(&out_dir.join("chunks.lancedb"));
    let rt = tokio::runtime::Runtime::new()?;

    let mut sqlite_ms = 0u128;
    let mut lance_ms = 0u128;
    let mut rows_written = 0usize;
    for batch in file_ids.chunks(4) {
        let mut fc_rows = Vec::new();
        let mut lance_rows = Vec::new();
        let mut ids = Vec::new();
        for fid in batch {
            for r in &by_file[fid] {
                let gs = |k: &str| r[k].as_str().unwrap_or("").to_string();
                let gi = |k: &str| r[k].as_i64().unwrap_or(0);
                let symbols_used: Vec<String> =
                    serde_json::from_str(&gs("symbols_used")).unwrap_or_default();
                fc_rows.push(oracle_core::store::sqlite::FileChunk {
                    id: gs("id"),
                    file_id: gs("file_id"),
                    chunk_index: gi("chunk_index"),
                    start_char: gi("start_char"),
                    end_char: gi("end_char"),
                    text: gs("text"),
                    file_sorgente: gs("file_sorgente"),
                    ultima_modifica: String::new(),
                    embedding_dims: 1024,
                    kind: gs("kind"),
                    symbol_name: gs("symbol_name"),
                    signature: String::new(),
                    line_start: gi("line_start"),
                    line_end: gi("line_end"),
                    language: gs("language"),
                    symbols_used,
                });
                lance_rows.push(oracle_core::store::lance::LanceRow {
                    id: gs("id"),
                    label: gs("label"),
                    area: String::new(),
                    cluster_semantic: gs("cluster_semantic"),
                    vector: vec![0.0f32; 1024],
                });
                ids.push(gs("id"));
            }
        }
        let t0 = Instant::now();
        rt.block_on(lance.replace_ids(&[], &lance_rows))?;
        lance_ms += t0.elapsed().as_millis();
        let t1 = Instant::now();
        sqlite.replace_chunks_for_files(batch, &fc_rows)?;
        sqlite_ms += t1.elapsed().as_millis();
        rows_written += fc_rows.len();
    }

    println!(
        "{}",
        serde_json::json!({
            "files": file_ids.len(), "rows_written": rows_written,
            "sqlite_ms": sqlite_ms, "lance_ms": lance_ms,
            "sqlite_rows_per_sec": if sqlite_ms>0 {(rows_written as f64)*1000.0/sqlite_ms as f64} else {0.0},
            "lance_rows_per_sec": if lance_ms>0 {(rows_written as f64)*1000.0/lance_ms as f64} else {0.0},
        })
    );
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "corpus" => run_corpus(),
        "embed" => run_embed(),
        "eval" => run_eval(),
        "store" => run_store(),
        _ => {
            eprintln!("modes: corpus | embed | eval | store");
            Ok(())
        }
    }
}
