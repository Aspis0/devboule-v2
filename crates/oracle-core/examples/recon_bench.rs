//! recon_bench — measurement harness for the oracle-core v1 performance investigation.
//! Additive example; uses ONLY production code paths (no copies of pipeline logic).
//!
//! Modes:
//!   corpus --root DIR [--limit N] --out DIR          collector+chunker+prefix+window stats
//!   embed  --model-dir DIR --variant V --ep cpu|directml --chunks F.jsonl
//!          [--n K] [--queries F.json] --out F.json   timed embedding via OnnxEmbedder
//!   eval   --chunks F.jsonl --queries F.json --dense F.json [--ref F.json] [--k 5]
//!                                                    lexical vs dense vs hybrid_max vs hybrid_rrf + fidelity
//!   rerank --chunks F.jsonl --queries F.json --dense F.json --reranker DIR
//!                                                    dense reranking at candidates=20 and 50
//!          --k controls the requested chunk candidate depth (3*k); it is not a
//!          file-level recall cutoff. Eval metrics keep enough candidates to
//!          measure recall through 50 distinct files and MRR through 10 files.
//!   store  --chunks F.jsonl --out DIR               sqlite+lance commit-cadence timing

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use oracle_core::{
    build_chunks_for_file, build_chunks_for_file_with_limits, chunk_embedding_text_for_model,
    collect_text_files, is_test_source, query_embedding_text_for_model, resolve_embed_window_bytes,
    resolve_embed_window_overlap_bytes, window_text, CancelFlag, ChunkMeta, EpArg, FileChunk,
    LanceRow, LanceStore, OnnxEmbedder, SqliteStore,
};

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

fn frozen_repo_roots(queries_path: &Path) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(queries_path)?)?;
    let mut out = Vec::new();
    let Some(repos) = raw.get("repos").and_then(|v| v.as_array()) else {
        anyhow::bail!("{} has no repos[]", queries_path.display());
    };
    for repo in repos {
        let id = repo
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let path = PathBuf::from(repo.get("path").and_then(|v| v.as_str()).unwrap_or(""));
        if id.is_empty() || !path.is_dir() {
            eprintln!(
                "skipping repo id={id} path={} (missing frozen tree)",
                path.display()
            );
            continue;
        }
        out.push((id, path));
    }
    Ok(out)
}

fn chunk_one_file(
    path: &Path,
    root: &Path,
    limits: Option<(usize, usize)>,
) -> Vec<serde_json::Value> {
    match limits {
        Some((max_chars, overlap)) => {
            build_chunks_for_file_with_limits(path, root, max_chars, overlap)
        }
        None => build_chunks_for_file(path, root),
    }
}

fn run_corpus() -> anyhow::Result<()> {
    let limit: usize = arg_of("--limit").and_then(|v| v.parse().ok()).unwrap_or(0);
    let out_dir = PathBuf::from(arg_of("--out").expect("--out DIR"));
    let max_chars = arg_of("--max-chars").and_then(|v| v.parse().ok());
    let overlap = arg_of("--overlap").and_then(|v| v.parse().ok());
    let limits = match (max_chars, overlap) {
        (Some(m), Some(o)) => Some((m, o)),
        (None, None) => None,
        _ => anyhow::bail!("--max-chars and --overlap must be passed together"),
    };
    std::fs::create_dir_all(&out_dir)?;

    let jobs: Vec<(String, PathBuf)> = if let Some(qp) = arg_of("--queries") {
        frozen_repo_roots(&PathBuf::from(qp))?
    } else {
        let root = PathBuf::from(arg_of("--root").expect("--root DIR or --queries"));
        vec![("root".into(), root)]
    };

    let t0 = Instant::now();
    let mut files: Vec<(PathBuf, PathBuf)> = Vec::new(); // (path, root)
    for (_id, root) in &jobs {
        for p in collect_text_files(root) {
            files.push((p, root.clone()));
        }
    }
    let collect_ms = t0.elapsed().as_millis();
    files.sort_by(|a, b| a.0.cmp(&b.0));
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

    for (path, root) in &files {
        let rel = path
            .strip_prefix(root)
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

        let chunks = chunk_one_file(path, root, limits);
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
            let emb = chunk_embedding_text_for_model(&meta, None, true);
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
            obj.insert("windows_byte_estimate".into(), serde_json::Value::from(wc));
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
        "roots": jobs.iter().map(|(id, p)| serde_json::json!({"id": id, "path": p})).collect::<Vec<_>>(),
        "max_chars": limits.map(|l| l.0),
        "overlap": limits.map(|l| l.1),
        "limit": limit,
        "files_selected": files.len(),
        "bytes_selected": total_bytes_selected,
        "chunks_total": chunks_total,
        "windows_total_byte_estimate": windows_total,
        "window_bytes_cfg": win_bytes,
        "overlap_bytes_cfg": overlap,
        "emb_chars_total": emb_chars_total,
        "raw_chars_total": raw_chars_total,
        "prefix_overhead_chars": emb_chars_total.saturating_sub(raw_chars_total),
        "chunks_multi_window_byte_estimate": multi,
        "chunks_multi_window_byte_estimate_pct": if chunks_total>0 {(multi as f64 *100.0/chunks_total as f64*10.0).round()/10.0} else {0.0},
        "chunk_raw_chars": {"p50": pct(&raw_lens,0.5),"p90": pct(&raw_lens,0.9),"p99": pct(&raw_lens,0.99),"max": raw_lens.last().copied().unwrap_or(0)},
        "chunk_emb_chars": {"p50": pct(&emb_lens,0.5),"p90": pct(&emb_lens,0.9),"p99": pct(&emb_lens,0.99),"max": emb_lens.last().copied().unwrap_or(0)},
        "windows_hist_byte_estimate": {"1": window_counts.iter().filter(|&&w| w==1).count(),
                         "2": window_counts.iter().filter(|&&w| w==2).count(),
                         "3+": window_counts.iter().filter(|&&w| w>=3).count()},
        "by_ext": ext_rows.into_iter().take(15).collect::<Vec<_>>(),
        "by_dir": dir_rows,
        "top_files_by_chunks": per_file_chunks.into_iter().take(10).map(|(f,c)| serde_json::json!({"file": f, "chunks": c})).collect::<Vec<_>>(),
        "bad_boundary": {"mid_body_start": mid_body, "odd_end": odd_end,
                         "examples_mid": examples_mid, "examples_end": examples_end},
        "timings_ms": {"collect": collect_ms, "chunk_plus_prefix_plus_window": chunk_ms},
        "top_level_tree": jobs.iter().flat_map(|(_id, root)| {
            top_level_stats(root).into_iter().take(25).map(|(n,f,b)| serde_json::json!({"name": n, "files": f, "bytes": b}))
        }).collect::<Vec<_>>(),
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

/// Copied from `oracle_core::query::engine` (private as of commit 42059c8).
/// Production formula: `1 / (60 + rank)`, 1-based rank counted only on kept
/// results after filters; contributions **sum** when a chunk is in both lists.
/// This bench copy can silently diverge if engine.rs changes.
const RRF_K: f64 = 60.0;

fn rrf_contribution(rank: usize) -> f64 {
    1.0 / (RRF_K + rank as f64)
}

fn sanitize_f32_vectors(vecs: &mut [Vec<f32>]) -> usize {
    let mut n = 0usize;
    for v in vecs.iter_mut() {
        for x in v.iter_mut() {
            if !x.is_finite() {
                n += 1;
                *x = 0.0;
            }
        }
    }
    n
}

/// Stream `vectors` / `query_vectors` with `to_writer` so we never materialize a
/// 10M-node `serde_json::Value` tree (that path OOMs Qwen3's 1024-d dump).
fn write_dense_json(
    path: &Path,
    mut meta: serde_json::Value,
    vectors: &[Vec<f32>],
    query_vectors: &Option<Vec<Vec<f32>>>,
) -> anyhow::Result<()> {
    let obj = meta
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("embed meta must be an object"))?;
    obj.remove("vectors");
    obj.remove("query_vectors");
    let mut f = BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
    let s = serde_json::to_string(&meta)?;
    let inner = s.trim();
    let inner = inner
        .strip_prefix('{')
        .and_then(|t| t.strip_suffix('}'))
        .unwrap_or(inner);
    write!(f, "{{")?;
    if !inner.is_empty() {
        write!(f, "{inner},")?;
    }
    write!(f, "\"vectors\":")?;
    serde_json::to_writer(&mut f, vectors)?;
    write!(f, ",\"query_vectors\":")?;
    serde_json::to_writer(&mut f, query_vectors)?;
    write!(f, "}}")?;
    f.flush()?;
    Ok(())
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

    std::env::set_var("ORACLE_RS_ONNX_VARIANT", &variant);
    let t0 = Instant::now();
    let (mut emb, load_ms) = OnnxEmbedder::load(&model_dir, ep)
        .map_err(|e| anyhow::anyhow!("LOAD FAILED variant={variant} ep={ep_s}: {e:#}"))?;
    let load_total_ms = t0.elapsed().as_millis();
    let uses_semantic_prefix = emb.descriptor().uses_semantic_prefix;
    let max_seq = emb.max_seq_tokens();

    let texts: Vec<String> = records
        .iter()
        .map(|r| {
            let m = meta_of(r);
            chunk_embedding_text_for_model(&m, None, uses_semantic_prefix)
        })
        .collect();
    let mut emb_chars = 0usize;
    for t in &texts {
        emb_chars += t.len();
    }
    let (windows_total, windows_truncated) = emb.token_window_stats(&texts)?;

    println!(
        "embed start n_chunks={} windows_token_aware={windows_total} dim_hint={} variant={variant} ep={ep_s}",
        texts.len(),
        emb.descriptor().dims
    );
    let _ = std::io::stdout().flush();

    // warmup (excluded from timing): one short text
    let warm = Instant::now();
    let _ = emb.embed_batched(&["fn warmup() {}\n".to_string()], 1, &CancelFlag::new())?;
    let warmup_ms = warm.elapsed().as_millis();
    eprintln!("embed warmup_ms={warmup_ms}");
    let _ = std::io::stderr().flush();

    let partial_path = {
        let mut p = out.clone();
        p.set_extension("partial.jsonl");
        p
    };
    let mut vectors: Vec<Vec<f32>> = vec![Vec::new(); texts.len()];
    if partial_path.exists() {
        let ptxt = std::fs::read_to_string(&partial_path)?;
        let mut loaded = 0usize;
        for line in ptxt.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let i = row["i"].as_u64().unwrap_or(u64::MAX) as usize;
            let Some(arr) = row["v"].as_array() else {
                continue;
            };
            if i >= vectors.len() {
                continue;
            }
            vectors[i] = arr
                .iter()
                .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                .collect();
            loaded += 1;
        }
        eprintln!(
            "embed resume: loaded {loaded} vectors from {}",
            partial_path.display()
        );
        let _ = std::io::stderr().flush();
    }
    let mut next = 0usize;
    while next < vectors.len() && !vectors[next].is_empty() {
        next += 1;
    }

    let mut partial = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&partial_path)?;
    let sampler = CpuSampler::start();
    let t1 = Instant::now();
    let mut per_group: Vec<u128> = Vec::new();
    let n_groups = texts.len().div_ceil(32);
    if next < texts.len() {
        let resume_group = next / 32;
        for (gi, group) in texts.chunks(32).enumerate() {
            if gi < resume_group {
                continue;
            }
            let lo = gi * 32;
            let hi = (lo + group.len()).min(texts.len());
            let g0 = Instant::now();
            let v = emb.embed_batched(group, group.len(), &CancelFlag::new())?;
            if v.len() != group.len() {
                anyhow::bail!(
                    "embed_batched returned {} vectors for group {gi} (expected {})",
                    v.len(),
                    group.len()
                );
            }
            let gms = g0.elapsed().as_millis();
            per_group.push(gms);
            for (off, mut vec) in v.into_iter().enumerate() {
                let i = lo + off;
                let _ = sanitize_f32_vectors(std::slice::from_mut(&mut vec));
                vectors[i] = vec;
                write!(partial, "{{\"i\":{i},\"v\":")?;
                serde_json::to_writer(&mut partial, &vectors[i])?;
                writeln!(partial, "}}")?;
            }
            partial.flush()?;
            eprintln!(
                "embed group {}/{n_groups} idx {lo}..{hi} group_ms={gms}",
                gi + 1
            );
            let _ = std::io::stderr().flush();
        }
    }
    let embed_ms = t1.elapsed().as_millis();
    let samples_chunk = sampler.stop();

    if vectors.iter().any(|v| v.is_empty()) {
        let missing = vectors.iter().filter(|v| v.is_empty()).count();
        anyhow::bail!("embed incomplete: {missing} empty vectors (partial at {next})");
    }
    let nan_chunk = sanitize_f32_vectors(&mut vectors);

    let (mut query_vectors, query_ms) = if !queries.is_empty() {
        let qtexts: Vec<String> = queries
            .iter()
            .map(|q| query_embedding_text_for_model(q, None, uses_semantic_prefix))
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
    let nan_query = query_vectors
        .as_mut()
        .map(|v| sanitize_f32_vectors(v))
        .unwrap_or(0);
    if nan_chunk + nan_query > 0 {
        eprintln!("embed non-finite components zeroed: chunk={nan_chunk} query={nan_query}");
    }

    let out_json = serde_json::json!({
        "variant": variant, "ep": ep_s, "model_id": emb.descriptor().id,
        "uses_semantic_prefix": uses_semantic_prefix,
        "max_seq_tokens": max_seq,
        "windows_truncated": windows_truncated,
        "non_finite_components_zeroed": {"chunk": nan_chunk, "query": nan_query},
        "load_ms": load_ms, "load_total_ms": load_total_ms, "warmup_ms": warmup_ms,
        "n_chunks": texts.len(), "emb_chars_total": emb_chars,
        "windows_total": windows_total,
        "attention_budget_env": std::env::var("ORACLE_CHUNK_ATTENTION_BUDGET").unwrap_or_else(|_| "default".into()),
        "embed_ms_total": embed_ms, "per_group_ms": per_group,
        "chunks_per_sec": if embed_ms>0 {(texts.len().saturating_sub(next) as f64)*1000.0/embed_ms as f64} else {0.0},
        "windows_per_sec": if embed_ms>0 {(windows_total as f64)*1000.0/embed_ms as f64} else {0.0},
        "query_ms": query_ms, "n_queries": queries.len(),
        "dim": vectors.first().map(|v| v.len()).unwrap_or(0),
        "chunk_ids": records.iter().map(|r| r["id"].as_str().unwrap_or("")).collect::<Vec<_>>(),
        "resumed_from": next,
        "cpu_during_embed": summarize(&samples_chunk),
    });
    write_dense_json(&out, out_json.clone(), &vectors, &query_vectors)?;
    let _ = std::fs::remove_file(&partial_path);
    println!(
        "variant={variant} ep={ep_s} load_ms={load_ms} warmup_ms={warmup_ms} chunks={} windows={windows_total} embed_ms={embed_ms} cps={:.2} wps={:.2} cpu={}",
        texts.len(),
        if embed_ms>0 {(texts.len() as f64)*1000.0/embed_ms as f64} else {0.0},
        if embed_ms>0 {(windows_total as f64)*1000.0/embed_ms as f64} else {0.0},
        out_json["cpu_during_embed"],
    );
    Ok(())
}

fn load_eval_queries(path: &Path) -> anyhow::Result<Vec<serde_json::Value>> {
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    if let Some(repos) = raw.get("repos").and_then(|v| v.as_array()) {
        let mut out = Vec::new();
        for repo in repos {
            let id = repo.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let root = PathBuf::from(repo.get("path").and_then(|v| v.as_str()).unwrap_or(""));
            if !root.is_dir() {
                eprintln!("eval: skip queries for {id} (frozen path missing)");
                continue;
            }
            let Some(qs) = repo.get("queries").and_then(|v| v.as_array()) else {
                continue;
            };
            for q in qs {
                let mut rec = q.clone();
                if let Some(obj) = rec.as_object_mut() {
                    obj.insert("repo".into(), serde_json::Value::String(id.to_string()));
                }
                out.push(rec);
            }
        }
        return Ok(out);
    }
    match raw {
        serde_json::Value::Array(items) => Ok(items),
        other => anyhow::bail!("queries file must be an array or {{repos: [...]}}, got {other}"),
    }
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
    let queries = load_eval_queries(&queries_file)?;
    let dense: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&dense_file)?)?;

    // lexical scoring input
    let scored: Vec<oracle_core::ScoredChunk> = records
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

    // `--k` requests chunk depth (historically expanded to 3*k), not the
    // file-level recall threshold. Keep the full chunk ranking here so that
    // recall@50 is really the first 50 distinct files and mrr@10 can always
    // inspect the first 10 distinct files, even when the caller uses --k 5.
    let requested_candidate_limit = k.saturating_mul(3);
    let limit = requested_candidate_limit.max(records.len());
    let file_of: HashMap<&str, &str> = records
        .iter()
        .filter_map(|r| {
            r["id"]
                .as_str()
                .map(|id| (id, r["file_sorgente"].as_str().unwrap_or("")))
        })
        .collect();
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
        let lex = oracle_core::lexical_chunk_context(query, &scored, limit);
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
        // hybrid_max: historical max(dense, lexical) over the union.
        // Not production — engine.context switched to RRF at 42059c8. Kept as baseline.
        let mut score_by_id: HashMap<String, f64> = HashMap::new();
        for (id, s) in &denser {
            *score_by_id.entry(id.clone()).or_insert(0.0) = s.max(0.0);
        }
        for l in &lex {
            let e = score_by_id.entry(l.chunk_id.clone()).or_insert(0.0);
            *e = e.max(l.score);
        }
        let mut hybrid_max: Vec<(String, f64)> = score_by_id.into_iter().collect();
        hybrid_max.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        hybrid_max.truncate(limit);

        // hybrid_rrf: copy of engine.rs (see rrf_contribution). Rank is 1-based on
        // the kept lists above (eval has no file/kind filters, so every hit is kept).
        // Dense and lexical lists are both top `limit` (= k*3); production dense
        // search uses the final `limit` while lexical uses limit*3. Same candidate
        // set as hybrid_max so the fusion rule is the only difference.
        let mut rrf_by_id: HashMap<String, f64> = HashMap::new();
        for (i, (id, _)) in denser.iter().enumerate() {
            *rrf_by_id.entry(id.clone()).or_insert(0.0) += rrf_contribution(i + 1);
        }
        for (rank, l) in lex.iter().enumerate() {
            *rrf_by_id.entry(l.chunk_id.clone()).or_insert(0.0) += rrf_contribution(rank + 1);
        }
        let mut hybrid_rrf: Vec<(String, f64)> = rrf_by_id.into_iter().collect();
        hybrid_rrf.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        hybrid_rrf.truncate(limit);

        // file-level metrics helpers
        let metrics = |ranked: &[(String, f64)]| -> serde_json::Value {
            let files: Vec<String> = ranked
                .iter()
                .map(|(id, _)| file_of.get(id.as_str()).copied().unwrap_or(""))
                .map(String::from)
                .filter(|f| !f.is_empty())
                .collect();
            let mut seen = HashSet::new();
            let distinct: Vec<String> = files
                .into_iter()
                .filter(|f| seen.insert(f.clone()))
                .collect();
            let hits_at = |cutoff: usize| {
                targets
                    .iter()
                    .filter(|t| distinct.iter().take(cutoff).any(|f| f == *t))
                    .count()
            };
            let recall_at = |cutoff: usize| {
                let hits = hits_at(cutoff);
                if targets.is_empty() {
                    0.0
                } else {
                    hits as f64 / targets.len() as f64
                }
            };
            let hits5 = hits_at(5);
            let recall5 = if targets.is_empty() {
                0.0
            } else {
                hits5 as f64 / targets.len() as f64
            };
            let hit5 = if hits5 > 0 { 1.0 } else { 0.0 };
            let mut mrr = 0.0;
            for (i, f) in distinct.iter().take(10).enumerate() {
                if targets.contains(f) {
                    mrr = 1.0 / (i as f64 + 1.0);
                    break;
                }
            }
            serde_json::json!({
                "recall@5": recall5,
                "recall@10": recall_at(10),
                "recall@20": recall_at(20),
                "recall@50": recall_at(50),
                "hit@5": hit5,
                "mrr@10": mrr,
            })
        };

        let lex_ranked: Vec<(String, f64)> =
            lex.iter().map(|l| (l.chunk_id.clone(), l.score)).collect();
        let dense_test_mild = demote_tests_one_rank(&denser, &file_of);
        let dense_test_conditional = conditional_test_policy(query, &denser, &file_of);
        let dense_test_tail = demote_tests_to_tail(&denser, &file_of);
        let dense_test_conditional_tail = conditional_test_tail_policy(query, &denser, &file_of);
        let dense_distinct_files = distinct_file_ranking(&denser, &file_of);
        rows.push(serde_json::json!({
            "q": query,
            "kind": q["kind"].as_str().unwrap_or(""),
            "repo": q["repo"].as_str().unwrap_or(""),
            "targets": targets,
            "lexical": metrics(&lex_ranked),
            "dense": metrics(&denser),
            "dense_returned_slots": metric_for_ranked_prefix(&denser, &file_of, &targets, k),
            "dense_distinct_files": metrics(&dense_distinct_files),
            "dense_test_mild": metrics(&dense_test_mild),
            "dense_test_conditional": metrics(&dense_test_conditional),
            "dense_test_tail": metrics(&dense_test_tail),
            "dense_test_conditional_tail": metrics(&dense_test_conditional_tail),
            "hybrid_max": metrics(&hybrid_max),
            "hybrid_rrf": metrics(&hybrid_rrf),
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
    let pack = |mode: &str| {
        serde_json::json!({
            "recall@5": mean("recall@5", mode),
            "recall@10": mean("recall@10", mode),
            "recall@20": mean("recall@20", mode),
            "recall@50": mean("recall@50", mode),
            "hit@5": mean("hit@5", mode),
            "mrr@10": mean("mrr@10", mode),
        })
    };
    let mean_kind = |key: &str, mode: &str, kind: &str| -> f64 {
        let vals: Vec<f64> = rows
            .iter()
            .filter(|r| r["kind"].as_str() == Some(kind))
            .filter_map(|r| r[mode][key].as_f64())
            .collect();
        if vals.is_empty() {
            0.0
        } else {
            vals.iter().sum::<f64>() / vals.len() as f64
        }
    };
    let pack_kind = |mode: &str, kind: &str| {
        serde_json::json!({
            "n": rows.iter().filter(|r| r["kind"].as_str() == Some(kind)).count(),
            "recall@5": mean_kind("recall@5", mode, kind),
            "recall@10": mean_kind("recall@10", mode, kind),
            "recall@20": mean_kind("recall@20", mode, kind),
            "recall@50": mean_kind("recall@50", mode, kind),
            "hit@5": mean_kind("hit@5", mode, kind),
            "mrr@10": mean_kind("mrr@10", mode, kind),
        })
    };
    let dense_miss_at5_queries = rows
        .iter()
        .filter(|r| {
            r["targets"].as_array().is_some_and(|t| !t.is_empty())
                && r["dense"]["hit@5"].as_f64() == Some(0.0)
        })
        .count();
    let dense_miss_at5_recovered_by50 = rows
        .iter()
        .filter(|r| {
            r["targets"].as_array().is_some_and(|t| !t.is_empty())
                && r["dense"]["hit@5"].as_f64() == Some(0.0)
                && r["dense"]["recall@50"].as_f64().is_some_and(|v| v > 0.0)
        })
        .count();
    let mut summary = serde_json::json!({
        "n_queries": rows.len(),
        "eval_config": {
            "k": k,
            "requested_candidate_chunks": requested_candidate_limit,
            "candidate_chunks_used": limit,
            "candidate_depth_note": "--k requests chunk depth (3*k); eval uses the full available chunk ranking so recall@50 and mrr@10 are not truncated by --k.",
        },
        "uses_semantic_prefix": dense.get("uses_semantic_prefix"),
        "lexical": pack("lexical"),
        "dense": pack("dense"),
        "dense_returned_slots": {
            "recall@5": mean("recall@5", "dense_returned_slots"),
            "mrr@10": mean("mrr@10", "dense_returned_slots"),
        },
        "dense_distinct_files": pack("dense_distinct_files"),
        "dense_test_mild": pack("dense_test_mild"),
        "dense_test_conditional": pack("dense_test_conditional"),
        "dense_test_tail": pack("dense_test_tail"),
        "dense_test_conditional_tail": pack("dense_test_conditional_tail"),
        "hybrid_max": pack("hybrid_max"),
        "hybrid_rrf": pack("hybrid_rrf"),
        "dense_miss_at5_recovered_by50": {
            "definition": "Among non-empty queries where dense hit@5 is 0, queries with any target in dense recall@50.",
            "miss_at5_queries": dense_miss_at5_queries,
            "recovered_by50_queries": dense_miss_at5_recovered_by50,
            "recovery_rate": if dense_miss_at5_queries > 0 {
                dense_miss_at5_recovered_by50 as f64 / dense_miss_at5_queries as f64
            } else {
                0.0
            },
        },
        "rrf_note": "hybrid_rrf copies oracle_core::query::engine::{RRF_K, rrf_contribution} (private). k=60, 1/(k+rank), ranks 1-based on kept lists, scores sum on overlap.",
        "by_kind": {
            "literal": {
                "lexical": pack_kind("lexical", "literal"),
                "dense": pack_kind("dense", "literal"),
                "dense_test_mild": pack_kind("dense_test_mild", "literal"),
                "dense_test_conditional": pack_kind("dense_test_conditional", "literal"),
                "dense_test_tail": pack_kind("dense_test_tail", "literal"),
                "dense_test_conditional_tail": pack_kind("dense_test_conditional_tail", "literal"),
                "hybrid_max": pack_kind("hybrid_max", "literal"),
                "hybrid_rrf": pack_kind("hybrid_rrf", "literal"),
            },
            "conceptual": {
                "lexical": pack_kind("lexical", "conceptual"),
                "dense": pack_kind("dense", "conceptual"),
                "dense_test_mild": pack_kind("dense_test_mild", "conceptual"),
                "dense_test_conditional": pack_kind("dense_test_conditional", "conceptual"),
                "dense_test_tail": pack_kind("dense_test_tail", "conceptual"),
                "dense_test_conditional_tail": pack_kind("dense_test_conditional_tail", "conceptual"),
                "hybrid_max": pack_kind("hybrid_max", "conceptual"),
                "hybrid_rrf": pack_kind("hybrid_rrf", "conceptual"),
            },
        },
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

fn percentile_ms(values: &[u128], p: f64) -> u128 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let index = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn metric_for_ranked(
    ranked: &[(String, f64)],
    file_of: &HashMap<&str, &str>,
    targets: &[String],
) -> serde_json::Value {
    let files: Vec<String> = ranked
        .iter()
        .map(|(id, _)| file_of.get(id.as_str()).copied().unwrap_or(""))
        .map(String::from)
        .filter(|f| !f.is_empty())
        .collect();
    let mut seen = HashSet::new();
    let distinct: Vec<String> = files
        .into_iter()
        .filter(|f| seen.insert(f.clone()))
        .collect();
    let hits_at = |cutoff: usize| {
        targets
            .iter()
            .filter(|target| distinct.iter().take(cutoff).any(|file| file == *target))
            .count()
    };
    let recall_at = |cutoff: usize| {
        if targets.is_empty() {
            0.0
        } else {
            hits_at(cutoff) as f64 / targets.len() as f64
        }
    };
    let hits5 = hits_at(5);
    let mut mrr = 0.0;
    for (index, file) in distinct.iter().take(10).enumerate() {
        if targets.contains(file) {
            mrr = 1.0 / (index as f64 + 1.0);
            break;
        }
    }
    serde_json::json!({
        "recall@5": recall_at(5),
        "recall@10": recall_at(10),
        "recall@20": recall_at(20),
        "recall@50": recall_at(50),
        "hit@5": if hits5 > 0 { 1.0 } else { 0.0 },
        "mrr@10": mrr,
    })
}

fn metric_for_ranked_prefix(
    ranked: &[(String, f64)],
    file_of: &HashMap<&str, &str>,
    targets: &[String],
    slots: usize,
) -> serde_json::Value {
    let prefix: Vec<(String, f64)> = ranked.iter().take(slots).cloned().collect();
    metric_for_ranked(&prefix, file_of, targets)
}

fn query_asks_for_tests(query: &str) -> bool {
    let lower = query.to_lowercase();
    [
        "test",
        "tests",
        "testing",
        "regression",
        "fixture",
        "fixtures",
        "spec",
        "assert",
        "assertion",
        "vitest",
    ]
    .iter()
    .any(|term| {
        lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == *term)
    })
}

/// Move a test hit down by one ranked position, with non-tests winning ties.
/// This is a rank-only probe: it adds no hand-tuned score or model weight.
fn demote_tests_one_rank(
    ranked: &[(String, f64)],
    file_of: &HashMap<&str, &str>,
) -> Vec<(String, f64)> {
    let mut rows: Vec<(usize, bool, String, f64)> = ranked
        .iter()
        .enumerate()
        .map(|(index, (id, score))| {
            let is_test = file_of
                .get(id.as_str())
                .is_some_and(|path| is_test_source(path));
            (index, is_test, id.clone(), *score)
        })
        .collect();
    rows.sort_by_key(|(index, is_test, _, _)| {
        let test_rank = if *is_test { 1 } else { 0 };
        (*index + test_rank, test_rank, *index)
    });
    rows.into_iter()
        .map(|(_, _, id, score)| (id, score))
        .collect()
}

/// Stable partition that keeps non-test hits ahead of test hits.
/// This is a rank boundary, not a score weight; it is measured separately
/// because it is intentionally stronger than the one-position probe above.
fn demote_tests_to_tail(
    ranked: &[(String, f64)],
    file_of: &HashMap<&str, &str>,
) -> Vec<(String, f64)> {
    ranked
        .iter()
        .filter(|(id, _)| {
            !file_of
                .get(id.as_str())
                .is_some_and(|path| is_test_source(path))
        })
        .chain(ranked.iter().filter(|(id, _)| {
            file_of
                .get(id.as_str())
                .is_some_and(|path| is_test_source(path))
        }))
        .cloned()
        .collect()
}

fn conditional_test_policy(
    query: &str,
    ranked: &[(String, f64)],
    file_of: &HashMap<&str, &str>,
) -> Vec<(String, f64)> {
    if query_asks_for_tests(query) {
        ranked.to_vec()
    } else {
        demote_tests_one_rank(ranked, file_of)
    }
}

fn conditional_test_tail_policy(
    query: &str,
    ranked: &[(String, f64)],
    file_of: &HashMap<&str, &str>,
) -> Vec<(String, f64)> {
    if query_asks_for_tests(query) {
        ranked.to_vec()
    } else {
        demote_tests_to_tail(ranked, file_of)
    }
}

fn distinct_file_ranking(
    ranked: &[(String, f64)],
    file_of: &HashMap<&str, &str>,
) -> Vec<(String, f64)> {
    let mut seen = HashSet::new();
    ranked
        .iter()
        .filter(|(id, _)| {
            file_of
                .get(id.as_str())
                .is_some_and(|file| seen.insert((*file).to_string()))
        })
        .cloned()
        .collect()
}

fn mean_metric(rows: &[serde_json::Value], mode: &str, key: &str) -> f64 {
    let values: Vec<f64> = rows
        .iter()
        .filter_map(|row| row[mode][key].as_f64())
        .collect();
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn metric_pack(rows: &[serde_json::Value], mode: &str) -> serde_json::Value {
    serde_json::json!({
        "recall@5": mean_metric(rows, mode, "recall@5"),
        "mrr@10": mean_metric(rows, mode, "mrr@10"),
    })
}

fn target_is_test(row: &serde_json::Value) -> bool {
    row["targets"].as_array().is_some_and(|targets| {
        targets
            .iter()
            .filter_map(|target| target.as_str())
            .any(is_test_source)
    })
}

fn metric_pack_target_subset(
    rows: &[serde_json::Value],
    mode: &str,
    test_targets: bool,
) -> serde_json::Value {
    let subset: Vec<serde_json::Value> = rows
        .iter()
        .filter(|row| target_is_test(row) == test_targets)
        .cloned()
        .collect();
    serde_json::json!({
        "n": subset.len(),
        "recall@5": mean_metric(&subset, mode, "recall@5"),
        "mrr@10": mean_metric(&subset, mode, "mrr@10"),
    })
}

/// Evaluate the actual query-time ONNX reranker over the frozen dense ranking.
/// The 20/50 depths are separate passes over the same dense candidates so the
/// latency and quality trade-off is visible instead of being hidden in one
/// chosen cutoff.
fn run_rerank_eval() -> anyhow::Result<()> {
    let chunks_file = PathBuf::from(arg_of("--chunks").expect("--chunks"));
    let queries_file = PathBuf::from(arg_of("--queries").expect("--queries"));
    let dense_file = PathBuf::from(arg_of("--dense").expect("--dense"));
    let reranker_dir = PathBuf::from(arg_of("--reranker").expect("--reranker"));
    let out = arg_of("--out").map(PathBuf::from);

    let records = read_jsonl(&chunks_file)?;
    let queries = load_eval_queries(&queries_file)?;
    let dense: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&dense_file)?)?;
    let dvecs: Vec<Vec<f32>> = dense["vectors"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("dense file has no vectors[]"))?
        .iter()
        .map(|v| {
            v.as_array()
                .ok_or_else(|| anyhow::anyhow!("dense vector is not an array"))
                .map(|items| {
                    items
                        .iter()
                        .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                        .collect()
                })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let query_vectors: Vec<Vec<f32>> = dense["query_vectors"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("dense file has no query_vectors[]"))?
        .iter()
        .map(|v| {
            v.as_array()
                .ok_or_else(|| anyhow::anyhow!("dense query vector is not an array"))
                .map(|items| {
                    items
                        .iter()
                        .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                        .collect()
                })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let ids: Vec<String> = dense["chunk_ids"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("dense file has no chunk_ids[]"))?
        .iter()
        .map(|v| v.as_str().unwrap_or("").to_string())
        .collect();
    if query_vectors.len() != queries.len() {
        anyhow::bail!(
            "dense query_vectors has {} rows but queries has {}",
            query_vectors.len(),
            queries.len()
        );
    }
    if dvecs.len() != ids.len() {
        anyhow::bail!("dense vectors and chunk_ids have different lengths");
    }
    let vec_of: HashMap<&str, &[f32]> = ids
        .iter()
        .enumerate()
        .filter_map(|(i, id)| dvecs.get(i).map(|v| (id.as_str(), v.as_slice())))
        .collect();
    let file_of: HashMap<&str, &str> = records
        .iter()
        .filter_map(|r| {
            r["id"]
                .as_str()
                .map(|id| (id, r["file_sorgente"].as_str().unwrap_or("")))
        })
        .collect();
    let text_of: HashMap<&str, &str> = records
        .iter()
        .filter_map(|r| {
            r["id"]
                .as_str()
                .map(|id| (id, r["text"].as_str().unwrap_or("")))
        })
        .collect();

    let mut dense_rankings: Vec<Vec<(String, f64)>> = Vec::with_capacity(queries.len());
    for query_vector in &query_vectors {
        let mut ranking: Vec<(String, f64)> = ids
            .iter()
            .filter_map(|id| {
                vec_of
                    .get(id.as_str())
                    .map(|vector| (id.clone(), cosine(query_vector, vector)))
            })
            .collect();
        ranking.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        dense_rankings.push(ranking);
    }

    let load_start = Instant::now();
    let mut reranker = oracle_core::OnnxReranker::load(&reranker_dir, EpArg::Cpu)?;
    let load_total_ms = load_start.elapsed().as_millis();
    let warmup_start = Instant::now();
    reranker.score_pairs("reranker warmup", &["fn warmup() {}".to_string()])?;
    let warmup_ms = warmup_start.elapsed().as_millis();

    let baseline_rows: Vec<serde_json::Value> = queries
        .iter()
        .zip(&dense_rankings)
        .map(|(query, ranking)| {
            let targets: Vec<String> = query["targets"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            serde_json::json!({
                "q": query["q"].as_str().unwrap_or(""),
                "kind": query["kind"].as_str().unwrap_or(""),
                "targets": targets,
                "dense": metric_for_ranked(ranking, &file_of, &targets),
                "dense_diverse": metric_for_ranked(
                    &distinct_file_ranking(ranking, &file_of),
                    &file_of,
                    &targets,
                ),
                "dense_test_mild": metric_for_ranked(
                    &demote_tests_one_rank(ranking, &file_of),
                    &file_of,
                    &targets,
                ),
                "dense_test_conditional": metric_for_ranked(
                    &conditional_test_policy(
                        query["q"].as_str().unwrap_or(""),
                        ranking,
                        &file_of,
                    ),
                    &file_of,
                    &targets,
                ),
                "dense_test_tail": metric_for_ranked(
                    &demote_tests_to_tail(ranking, &file_of),
                    &file_of,
                    &targets,
                ),
                "dense_test_conditional_tail": metric_for_ranked(
                    &conditional_test_tail_policy(
                        query["q"].as_str().unwrap_or(""),
                        ranking,
                        &file_of,
                    ),
                    &file_of,
                    &targets,
                ),
            })
        })
        .collect();

    let sampler = CpuSampler::start();
    let mut latency_by_depth: HashMap<String, Vec<u128>> = HashMap::new();
    let mut reranked_rows: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    for depth in [20usize, 50usize] {
        let key = depth.to_string();
        let mut latencies = Vec::with_capacity(queries.len());
        let mut rows = Vec::with_capacity(queries.len());
        for (query, dense_ranking) in queries.iter().zip(&dense_rankings) {
            let query_text = query["q"].as_str().unwrap_or("");
            let targets: Vec<String> = query["targets"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let candidate_count = depth.min(dense_ranking.len());
            let documents: Vec<String> = dense_ranking
                .iter()
                .take(candidate_count)
                .map(|(id, _)| text_of.get(id.as_str()).copied().unwrap_or("").to_string())
                .collect();
            let t0 = Instant::now();
            let scores = reranker.score_pairs(query_text, &documents)?;
            latencies.push(t0.elapsed().as_millis());
            if scores.len() != candidate_count {
                anyhow::bail!(
                    "reranker returned {} scores for {} candidates at depth {depth}",
                    scores.len(),
                    candidate_count
                );
            }
            let mut prefix: Vec<(String, f64)> = dense_ranking
                .iter()
                .take(candidate_count)
                .zip(scores)
                .map(|((id, _), score)| (id.clone(), score))
                .collect();
            prefix.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let mut final_ranking = prefix;
            final_ranking.extend(dense_ranking.iter().skip(candidate_count).cloned());
            rows.push(serde_json::json!({
                "q": query_text,
                "kind": query["kind"].as_str().unwrap_or(""),
                "targets": targets,
                "reranked": metric_for_ranked(&final_ranking, &file_of, &targets),
                "reranked_diverse": metric_for_ranked(
                    &distinct_file_ranking(&final_ranking, &file_of),
                    &file_of,
                    &targets,
                ),
                "reranked_test_mild": metric_for_ranked(
                    &demote_tests_one_rank(&final_ranking, &file_of),
                    &file_of,
                    &targets,
                ),
                "reranked_test_conditional": metric_for_ranked(
                    &conditional_test_policy(query_text, &final_ranking, &file_of),
                    &file_of,
                    &targets,
                ),
                "reranked_test_tail": metric_for_ranked(
                    &demote_tests_to_tail(&final_ranking, &file_of),
                    &file_of,
                    &targets,
                ),
                "reranked_test_conditional_tail": metric_for_ranked(
                    &conditional_test_tail_policy(query_text, &final_ranking, &file_of),
                    &file_of,
                    &targets,
                ),
            }));
        }
        latency_by_depth.insert(key.clone(), latencies);
        reranked_rows.insert(key, rows);
    }
    let samples = sampler.stop();

    let latency_json = |depth: usize| {
        let values = latency_by_depth
            .get(&depth.to_string())
            .cloned()
            .unwrap_or_default();
        let total: u128 = values.iter().sum();
        serde_json::json!({
            "n_queries": values.len(),
            "total_ms": total,
            "avg_ms": if values.is_empty() { 0.0 } else { total as f64 / values.len() as f64 },
            "p50_ms": percentile_ms(&values, 0.50),
            "p95_ms": percentile_ms(&values, 0.95),
            "p99_ms": percentile_ms(&values, 0.99),
            "pairs": values.len() * depth,
            "pairs_per_sec": if total > 0 { (values.len() * depth) as f64 * 1000.0 / total as f64 } else { 0.0 },
        })
    };
    let empty_rows: Vec<serde_json::Value> = Vec::new();
    let result = serde_json::json!({
        "model_id": reranker.config().id,
        "model_dir": reranker_dir,
        "onnx_graph": reranker.config().onnx_graph,
        "max_seq_tokens": reranker.config().max_seq_tokens,
        "load_ms": load_total_ms,
        "warmup_ms": warmup_ms,
        "n_queries": queries.len(),
        "baseline": {
            "recall@5": mean_metric(&baseline_rows, "dense", "recall@5"),
            "mrr@10": mean_metric(&baseline_rows, "dense", "mrr@10"),
        },
        "diversity": {
            "without_reorder": metric_pack(&baseline_rows, "dense_diverse"),
            "with_reorder": {
                "20": metric_pack(reranked_rows.get("20").unwrap_or(&empty_rows), "reranked_diverse"),
                "50": metric_pack(reranked_rows.get("50").unwrap_or(&empty_rows), "reranked_diverse"),
            },
        },
        "reranked": {
            "20": metric_pack(reranked_rows.get("20").unwrap_or(&empty_rows), "reranked"),
            "50": metric_pack(reranked_rows.get("50").unwrap_or(&empty_rows), "reranked"),
        },
        "test_policy": {
            "definition": "mild moves each test hit down by one rank; conditional applies that move only when the query does not ask for tests/regressions/fixtures/assertions; tail stably partitions non-tests before tests under the same condition.",
            "without_reorder": {
                "none": metric_pack(&baseline_rows, "dense"),
                "mild": metric_pack(&baseline_rows, "dense_test_mild"),
                "conditional": metric_pack(&baseline_rows, "dense_test_conditional"),
                "tail": metric_pack(&baseline_rows, "dense_test_conditional_tail"),
            },
            "with_reorder": {
                "20": {
                    "none": metric_pack(reranked_rows.get("20").unwrap_or(&empty_rows), "reranked"),
                    "mild": metric_pack(reranked_rows.get("20").unwrap_or(&empty_rows), "reranked_test_mild"),
                    "conditional": metric_pack(reranked_rows.get("20").unwrap_or(&empty_rows), "reranked_test_conditional"),
                    "tail": metric_pack(reranked_rows.get("20").unwrap_or(&empty_rows), "reranked_test_conditional_tail"),
                },
                "50": {
                    "none": metric_pack(reranked_rows.get("50").unwrap_or(&empty_rows), "reranked"),
                    "mild": metric_pack(reranked_rows.get("50").unwrap_or(&empty_rows), "reranked_test_mild"),
                    "conditional": metric_pack(reranked_rows.get("50").unwrap_or(&empty_rows), "reranked_test_conditional"),
                    "tail": metric_pack(reranked_rows.get("50").unwrap_or(&empty_rows), "reranked_test_conditional_tail"),
                },
            },
        },
        "test_target_split": {
            "without_reorder": {
                "test_targets": {
                    "none": metric_pack_target_subset(&baseline_rows, "dense", true),
                    "mild": metric_pack_target_subset(&baseline_rows, "dense_test_mild", true),
                    "conditional": metric_pack_target_subset(&baseline_rows, "dense_test_conditional", true),
                    "tail": metric_pack_target_subset(&baseline_rows, "dense_test_conditional_tail", true),
                },
                "other_targets": {
                    "none": metric_pack_target_subset(&baseline_rows, "dense", false),
                    "mild": metric_pack_target_subset(&baseline_rows, "dense_test_mild", false),
                    "conditional": metric_pack_target_subset(&baseline_rows, "dense_test_conditional", false),
                    "tail": metric_pack_target_subset(&baseline_rows, "dense_test_conditional_tail", false),
                },
            },
            "with_reorder": {
                "20": {
                    "test_targets": {
                        "none": metric_pack_target_subset(reranked_rows.get("20").unwrap_or(&empty_rows), "reranked", true),
                        "mild": metric_pack_target_subset(reranked_rows.get("20").unwrap_or(&empty_rows), "reranked_test_mild", true),
                        "conditional": metric_pack_target_subset(reranked_rows.get("20").unwrap_or(&empty_rows), "reranked_test_conditional", true),
                        "tail": metric_pack_target_subset(reranked_rows.get("20").unwrap_or(&empty_rows), "reranked_test_conditional_tail", true),
                    },
                    "other_targets": {
                        "none": metric_pack_target_subset(reranked_rows.get("20").unwrap_or(&empty_rows), "reranked", false),
                        "mild": metric_pack_target_subset(reranked_rows.get("20").unwrap_or(&empty_rows), "reranked_test_mild", false),
                        "conditional": metric_pack_target_subset(reranked_rows.get("20").unwrap_or(&empty_rows), "reranked_test_conditional", false),
                        "tail": metric_pack_target_subset(reranked_rows.get("20").unwrap_or(&empty_rows), "reranked_test_conditional_tail", false),
                    },
                },
                "50": {
                    "test_targets": {
                        "none": metric_pack_target_subset(reranked_rows.get("50").unwrap_or(&empty_rows), "reranked", true),
                        "mild": metric_pack_target_subset(reranked_rows.get("50").unwrap_or(&empty_rows), "reranked_test_mild", true),
                        "conditional": metric_pack_target_subset(reranked_rows.get("50").unwrap_or(&empty_rows), "reranked_test_conditional", true),
                        "tail": metric_pack_target_subset(reranked_rows.get("50").unwrap_or(&empty_rows), "reranked_test_conditional_tail", true),
                    },
                    "other_targets": {
                        "none": metric_pack_target_subset(reranked_rows.get("50").unwrap_or(&empty_rows), "reranked", false),
                        "mild": metric_pack_target_subset(reranked_rows.get("50").unwrap_or(&empty_rows), "reranked_test_mild", false),
                        "conditional": metric_pack_target_subset(reranked_rows.get("50").unwrap_or(&empty_rows), "reranked_test_conditional", false),
                        "tail": metric_pack_target_subset(reranked_rows.get("50").unwrap_or(&empty_rows), "reranked_test_conditional_tail", false),
                    },
                },
            },
        },
        "latency_added_ms": {"20": latency_json(20), "50": latency_json(50)},
        "cpu_during_rerank": summarize(&samples),
    });
    if let Some(path) = out {
        std::fs::write(&path, serde_json::to_string_pretty(&result)?)?;
    }
    println!("{}", serde_json::to_string_pretty(&result)?);
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

    let sqlite = SqliteStore::new(&out_dir.join("metadata.sqlite"))?;
    let lance = LanceStore::new(&out_dir.join("chunks.lancedb"));
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
                fc_rows.push(FileChunk {
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
                lance_rows.push(LanceRow {
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

fn print_usage() {
    eprintln!(
        "recon_bench modes:\n\
  corpus --root DIR [--limit N] --out DIR\n\
  embed --model-dir DIR --variant V --ep cpu|directml --chunks F.jsonl [--n K] [--queries F.json] --out F.json\n\
  eval --chunks F.jsonl --queries F.json --dense F.json [--ref F.json] [--k K]\n\
       --k controls requested chunk candidate depth (3*K); it is not the file-level recall cutoff.\n\
       Eval reports recall@5/@10/@20/@50 on distinct files and mrr@10 on distinct files.\n\
  rerank --chunks F.jsonl --queries F.json --dense F.json --reranker DIR [--out F.json]\n\
       Scores the dense top-20 and top-50 with the declared ONNX cross-encoder.\n\
  store --chunks F.jsonl --out DIR"
    );
}

// ───────────────────────────── cite mode ─────────────────────────────

/// One (query, retrieved target file) pair, scored for citation quality.
struct CitationCase {
    evidence: usize,
    baseline_lines: usize,
    baseline_hits: usize,
    focus_lines: usize,
    focus_hits: usize,
    /// Expected hits of a window drawn uniformly at random from the same plan.
    /// This is the control that decides whether the cross-encoder is choosing
    /// or whether shrinking the citation would score the same by luck.
    random_hits: f64,
    /// Hits of the best window in the plan: the ceiling this geometry allows.
    best_hits: usize,
}

/// One retrieved target chunk, kept so every window geometry is scored against
/// the same retrieval result instead of re-running the search per variant.
struct MatchedCase {
    query: String,
    text: String,
    line_start: usize,
    line_end: usize,
    evidence: Vec<usize>,
}

/// Parse `path/to/file.rs:123 - source line` into (path, line).
fn parse_evidence(entry: &str) -> Option<(&str, usize)> {
    let (locator, _) = entry.split_once(" - ")?;
    let (path, line) = locator.rsplit_once(':')?;
    line.trim().parse().ok().map(|line| (path, line))
}

fn hits_in(span: (usize, usize), lines: &[usize]) -> usize {
    lines
        .iter()
        .filter(|line| **line >= span.0 && **line <= span.1)
        .count()
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let collected: Vec<f64> = values.collect();
    if collected.is_empty() {
        0.0
    } else {
        collected.iter().sum::<f64>() / collected.len() as f64
    }
}

/// Measure how precisely the shipped pipeline points at the answering lines.
///
/// The metric this set warns about is recall of evidence lines on its own: a
/// taller span covers more evidence by construction, so "wider is better" is
/// the trivial winner. Every recall here is therefore reported next to the span
/// length that bought it, and against two controls — a uniformly random window
/// of the same width, and the best window the plan could have picked.
fn run_citation_eval() -> anyhow::Result<()> {
    let chunks_file = PathBuf::from(arg_of("--chunks").expect("--chunks"));
    let queries_file = PathBuf::from(arg_of("--queries").expect("--queries"));
    let dense_file = PathBuf::from(arg_of("--dense").expect("--dense"));
    let reranker_dir = PathBuf::from(arg_of("--reranker").expect("--reranker"));
    let k: usize = arg_of("--k")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10); // the shipped Tauri QUERY_LIMIT
    let depth: usize = arg_of("--depth")
        .and_then(|v| v.parse().ok())
        .unwrap_or(oracle_core::DEFAULT_RERANKER_CANDIDATES);
    let variants: Vec<usize> = arg_of("--windows")
        .unwrap_or_else(|| "2,3,4,6,8".to_string())
        .split(',')
        .filter_map(|value| value.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .collect();
    let out = arg_of("--out").map(PathBuf::from);

    let records = read_jsonl(&chunks_file)?;
    let queries = load_eval_queries(&queries_file)?;
    let dense: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&dense_file)?)?;
    let dvecs: Vec<Vec<f32>> = dense["vectors"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("dense file has no vectors[]"))?
        .iter()
        .map(|v| {
            v.as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                        .collect()
                })
                .ok_or_else(|| anyhow::anyhow!("dense vector is not an array"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let query_vectors: Vec<Vec<f32>> = dense["query_vectors"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("dense file has no query_vectors[]"))?
        .iter()
        .map(|v| {
            v.as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                        .collect()
                })
                .ok_or_else(|| anyhow::anyhow!("dense query vector is not an array"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let ids: Vec<String> = dense["chunk_ids"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("dense file has no chunk_ids[]"))?
        .iter()
        .map(|v| v.as_str().unwrap_or("").to_string())
        .collect();
    if query_vectors.len() != queries.len() {
        anyhow::bail!(
            "dense query_vectors has {} rows but queries has {}",
            query_vectors.len(),
            queries.len()
        );
    }

    let vec_of: HashMap<&str, &[f32]> = ids
        .iter()
        .enumerate()
        .filter_map(|(i, id)| dvecs.get(i).map(|v| (id.as_str(), v.as_slice())))
        .collect();
    let file_of: HashMap<&str, &str> = records
        .iter()
        .filter_map(|r| {
            r["id"]
                .as_str()
                .map(|id| (id, r["file_sorgente"].as_str().unwrap_or("")))
        })
        .collect();
    let text_of: HashMap<&str, &str> = records
        .iter()
        .filter_map(|r| {
            r["id"]
                .as_str()
                .map(|id| (id, r["text"].as_str().unwrap_or("")))
        })
        .collect();
    let lines_of: HashMap<&str, (usize, usize)> = records
        .iter()
        .filter_map(|r| {
            let start = r["line_start"].as_i64().unwrap_or(0);
            let end = r["line_end"].as_i64().unwrap_or(0);
            r["id"]
                .as_str()
                .filter(|_| start > 0 && end >= start)
                .map(|id| (id, (start as usize, end as usize)))
        })
        .collect();

    let mut reranker = oracle_core::OnnxReranker::load(&reranker_dir, EpArg::Cpu)?;
    reranker.score_pairs("reranker warmup", &["fn warmup() {}".to_string()])?;

    // Retrieval runs once. Every geometry below is scored against the same
    // selected chunks, so the comparison isolates the window plan and cannot be
    // moved by a different set of results.
    let mut matched: Vec<MatchedCase> = Vec::new();
    let mut matched_without_line_base = 0usize;
    let mut queries_with_a_case = 0usize;

    for (query, query_vector) in queries.iter().zip(&query_vectors) {
        let query_text = query["q"].as_str().unwrap_or("");
        let targets: Vec<String> = query["targets"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let mut evidence_by_file: HashMap<String, Vec<usize>> = HashMap::new();
        for entry in query["evidence"].as_array().into_iter().flatten() {
            if let Some((path, line)) = entry.as_str().and_then(parse_evidence) {
                evidence_by_file
                    .entry(path.to_string())
                    .or_default()
                    .push(line);
            }
        }

        // Mirror the shipped query path: dense, rerank the head, test policy,
        // one chunk per file, then the caller's limit.
        let mut ranking: Vec<(String, f64)> = ids
            .iter()
            .filter_map(|id| {
                vec_of
                    .get(id.as_str())
                    .map(|vector| (id.clone(), cosine(query_vector, vector)))
            })
            .collect();
        ranking.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let candidate_count = depth.min(ranking.len());
        let documents: Vec<String> = ranking
            .iter()
            .take(candidate_count)
            .map(|(id, _)| text_of.get(id.as_str()).copied().unwrap_or("").to_string())
            .collect();
        let scores = reranker.score_pairs(query_text, &documents)?;
        let mut prefix: Vec<(String, f64)> = ranking
            .iter()
            .take(candidate_count)
            .zip(scores)
            .map(|((id, _), score)| (id.clone(), score))
            .collect();
        prefix.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let mut final_ranking = prefix;
        final_ranking.extend(ranking.iter().skip(candidate_count).cloned());
        let selected: Vec<(String, f64)> = distinct_file_ranking(
            &conditional_test_tail_policy(query_text, &final_ranking, &file_of),
            &file_of,
        )
        .into_iter()
        .take(k)
        .collect();

        let before = matched.len();
        for (chunk_id, _) in &selected {
            let Some(file) = file_of.get(chunk_id.as_str()).copied() else {
                continue;
            };
            if !targets.iter().any(|target| target == file) {
                continue;
            }
            let Some(evidence) = evidence_by_file.get(file).filter(|lines| !lines.is_empty())
            else {
                continue;
            };
            let Some(&(line_start, line_end)) = lines_of.get(chunk_id.as_str()) else {
                matched_without_line_base += 1;
                continue;
            };
            matched.push(MatchedCase {
                query: query_text.to_string(),
                text: text_of
                    .get(chunk_id.as_str())
                    .copied()
                    .unwrap_or("")
                    .to_string(),
                line_start,
                line_end,
                evidence: evidence.clone(),
            });
        }
        if matched.len() > before {
            queries_with_a_case += 1;
        }
    }

    let recall = |hits: f64, evidence: f64| if evidence > 0.0 { hits / evidence } else { 0.0 };
    let baseline = serde_json::json!({
        "evidence_recall": mean(matched.iter().map(|case| {
            recall(
                hits_in((case.line_start, case.line_end), &case.evidence) as f64,
                case.evidence.len() as f64,
            )
        })),
        "hit_rate": mean(matched.iter().map(|case| {
            (hits_in((case.line_start, case.line_end), &case.evidence) > 0) as u8 as f64
        })),
        "mean_lines": mean(matched.iter().map(|case| (case.line_end - case.line_start + 1) as f64)),
    });

    let mut by_geometry = serde_json::Map::new();
    for windows in &variants {
        let mut scored: Vec<CitationCase> = Vec::new();
        let mut too_short = 0usize;
        let mut windows_scored = 0usize;
        let started = Instant::now();
        for case in &matched {
            let plan = oracle_core::plan_focus_windows_with(*windows, case.text.lines().count());
            if plan.is_empty() {
                too_short += 1;
                continue;
            }
            let texts = oracle_core::window_texts(&case.text, &plan);
            windows_scored += texts.len();
            let window_scores = reranker.score_pairs(&case.query, &texts)?;
            let focus = oracle_core::select_focus(&plan, &window_scores)
                .ok_or_else(|| anyhow::anyhow!("a non-empty plan selected no window"))?;
            let absolute = |offset: usize, count: usize| {
                let start = case.line_start + offset;
                (start, (start + count.saturating_sub(1)).min(case.line_end))
            };
            let focus_span = absolute(focus.line_offset, focus.line_count);
            let per_window: Vec<usize> = plan
                .iter()
                .map(|&(offset, count)| hits_in(absolute(offset, count), &case.evidence))
                .collect();
            scored.push(CitationCase {
                evidence: case.evidence.len(),
                baseline_lines: case.line_end - case.line_start + 1,
                baseline_hits: hits_in((case.line_start, case.line_end), &case.evidence),
                focus_lines: focus_span.1 - focus_span.0 + 1,
                focus_hits: hits_in(focus_span, &case.evidence),
                random_hits: per_window.iter().sum::<usize>() as f64 / per_window.len() as f64,
                best_hits: per_window.iter().copied().max().unwrap_or(0),
            });
        }
        let elapsed_ms = started.elapsed().as_millis();
        let focus_recall = mean(
            scored
                .iter()
                .map(|c| recall(c.focus_hits as f64, c.evidence as f64)),
        );
        let focus_lines = mean(scored.iter().map(|c| c.focus_lines as f64));
        let base_recall = mean(
            scored
                .iter()
                .map(|c| recall(c.baseline_hits as f64, c.evidence as f64)),
        );
        let base_lines = mean(scored.iter().map(|c| c.baseline_lines as f64));
        by_geometry.insert(
            windows.to_string(),
            serde_json::json!({
                "cases": scored.len(),
                "too_short_to_narrow": too_short,
                "focus": {
                    "evidence_recall": focus_recall,
                    "hit_rate": mean(scored.iter().map(|c| (c.focus_hits > 0) as u8 as f64)),
                    "mean_lines": focus_lines,
                },
                "control_random_window": {
                    "evidence_recall": mean(scored.iter().map(|c| recall(c.random_hits, c.evidence as f64))),
                },
                "control_best_window": {
                    "evidence_recall": mean(scored.iter().map(|c| recall(c.best_hits as f64, c.evidence as f64))),
                    "hit_rate": mean(scored.iter().map(|c| (c.best_hits > 0) as u8 as f64)),
                },
                "baseline_on_the_same_cases": {
                    "evidence_recall": base_recall,
                    "hit_rate": mean(scored.iter().map(|c| (c.baseline_hits > 0) as u8 as f64)),
                    "mean_lines": base_lines,
                },
                "trade": {
                    "lines_saved_factor": if focus_lines > 0.0 { base_lines / focus_lines } else { 0.0 },
                    "recall_kept_fraction": if base_recall > 0.0 { focus_recall / base_recall } else { 0.0 },
                    "evidence_per_line_gain": if base_recall > 0.0 && focus_lines > 0.0 && base_lines > 0.0 {
                        (focus_recall / focus_lines) / (base_recall / base_lines)
                    } else { 0.0 },
                },
                "cost": {
                    "windows_scored": windows_scored,
                    "total_ms": elapsed_ms,
                    "ms_per_query": if queries_with_a_case > 0 { elapsed_ms as f64 / queries_with_a_case as f64 } else { 0.0 },
                    "pairs_per_sec": if elapsed_ms > 0 { windows_scored as f64 * 1000.0 / elapsed_ms as f64 } else { 0.0 },
                },
            }),
        );
    }

    let result = serde_json::json!({
        "mode": "cite",
        "model_id": reranker.config().id,
        "queries": queries.len(),
        "k": k,
        "rerank_depth": depth,
        "matched_cases": matched.len(),
        "queries_with_a_case": queries_with_a_case,
        "matched_without_line_base": matched_without_line_base,
        "note": "A case is one retrieved file that is a target of its query and carries evidence lines. Recall of evidence lines rewards taller spans mechanically, so every recall is paired with the span length that produced it and with two controls on the same geometry: a window drawn uniformly at random, and the best window the plan contains. Retrieval runs once; the geometries below are scored against the same selected chunks.",
        "baseline_chunk_span": baseline,
        "by_windows_per_chunk": by_geometry,
    });
    let rendered = serde_json::to_string_pretty(&result)?;
    println!("{rendered}");
    if let Some(path) = out {
        std::fs::write(&path, &rendered)?;
        eprintln!("[cite] wrote {}", path.display());
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "-h"
        || mode == "--help"
        || std::env::args().any(|arg| arg == "-h" || arg == "--help")
    {
        print_usage();
        return Ok(());
    }
    match mode.as_str() {
        "corpus" => run_corpus(),
        "embed" => run_embed(),
        "eval" => run_eval(),
        "rerank" => run_rerank_eval(),
        "cite" => run_citation_eval(),
        "store" => run_store(),
        _ => {
            print_usage();
            Ok(())
        }
    }
}
