//! Chunking helpers — build_chunks_for_file, split_text, chunk_limits_for_file.
//!
//! Port of the relevant functions from `oracle/ingestion/chunk_index.py`.

use std::path::Path;

use serde::Serialize;

use super::ast_chunker;

// ── Configuration constants (defaults, mirroring oracle/config.py) ───────────

pub const CHUNK_MAX_CHARS: usize = 2200;
pub const CHUNK_OVERLAP_CHARS: usize = 280;
pub const CHUNK_DOC_MAX_CHARS: usize = 12000;
pub const CHUNK_DOC_OVERLAP_CHARS: usize = 1200;
pub const CHUNK_STRUCTURED_MAX_CHARS: usize = 8000;
pub const CHUNK_STRUCTURED_OVERLAP_CHARS: usize = 900;
pub const CHUNK_CODE_MAX_CHARS: usize = 1024;
pub const CHUNK_CODE_OVERLAP_CHARS: usize = 164;
pub const CHUNK_MAX_FILE_BYTES: u64 = 1_200_000;

#[derive(Debug, Clone, Copy, Serialize)]
struct ChunkGeometry {
    default_max_chars: usize,
    default_overlap_chars: usize,
    docs_max_chars: usize,
    docs_overlap_chars: usize,
    structured_max_chars: usize,
    structured_overlap_chars: usize,
    code_max_chars: usize,
    code_overlap_chars: usize,
}

/// Single source of truth for the production chunk geometry.
///
/// Both file-limit selection and the recipe fingerprint use this function, so
/// the overlap that actually runs cannot drift away from the overlap recorded
/// for invalidation.
fn chunk_geometry() -> ChunkGeometry {
    ChunkGeometry {
        default_max_chars: CHUNK_MAX_CHARS,
        default_overlap_chars: CHUNK_OVERLAP_CHARS,
        docs_max_chars: CHUNK_DOC_MAX_CHARS,
        docs_overlap_chars: CHUNK_DOC_OVERLAP_CHARS,
        structured_max_chars: CHUNK_STRUCTURED_MAX_CHARS,
        structured_overlap_chars: CHUNK_STRUCTURED_OVERLAP_CHARS,
        code_max_chars: CHUNK_CODE_MAX_CHARS,
        code_overlap_chars: CHUNK_CODE_OVERLAP_CHARS,
    }
}

/// Stable description of the production chunk geometry.
pub fn chunk_geometry_fingerprint() -> String {
    serde_json::to_string(&chunk_geometry()).expect("chunk geometry is always serializable")
}

// ── Extension sets ───────────────────────────────────────────────────────────

pub fn is_text_extension(ext: &str) -> bool {
    matches!(
        ext,
        ".css"
            | ".gradle"
            | ".html"
            | ".java"
            | ".js"
            | ".jsx"
            | ".json"
            | ".jsonc"
            | ".kt"
            | ".kts"
            | ".md"
            | ".mjs"
            | ".cjs"
            | ".mts"
            | ".cts"
            | ".properties"
            | ".ps1"
            | ".py"
            | ".r"
            | ".rmd"
            | ".rs"
            | ".sh"
            | ".sql"
            | ".toml"
            | ".ts"
            | ".tsx"
            | ".xml"
            | ".txt"
            | ".yaml"
            | ".yml"
    )
}

fn is_doc_extension(ext: &str) -> bool {
    matches!(ext, ".md" | ".txt")
}

fn is_structured_extension(ext: &str) -> bool {
    matches!(
        ext,
        ".gradle"
            | ".html"
            | ".json"
            | ".jsonc"
            | ".properties"
            | ".toml"
            | ".xml"
            | ".yaml"
            | ".yml"
    )
}

fn is_code_extension(ext: &str) -> bool {
    matches!(
        ext,
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
    )
}

// ── Chunk limits ─────────────────────────────────────────────────────────────

pub fn chunk_limits_for_file(path: &Path) -> (usize, usize) {
    let geometry = chunk_geometry();
    let suffix = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_lowercase()))
        .unwrap_or_default();

    let lower_parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();

    if is_doc_extension(&suffix) || lower_parts.iter().any(|p| p == "docs") {
        return (geometry.docs_max_chars, geometry.docs_overlap_chars);
    }
    if is_structured_extension(&suffix) {
        return (
            geometry.structured_max_chars,
            geometry.structured_overlap_chars,
        );
    }
    if is_code_extension(&suffix) {
        return (geometry.code_max_chars, geometry.code_overlap_chars);
    }
    (geometry.default_max_chars, geometry.default_overlap_chars)
}

// ── Split text (sliding window) ──────────────────────────────────────────────

/// Clamp a declared profile overlap to a legal hard-split overlap.
///
/// The caller supplies the profile value; there is no second overlap formula
/// for hard-split chunks.
pub fn hard_split_overlap(max_chars: usize, overlap: usize) -> usize {
    let max_chars = max_chars.max(1);
    overlap.min(max_chars.saturating_sub(1))
}

/// Hard-split `text` on char boundaries into pieces of at most `max_chars`
/// characters with `overlap` character overlap. Never emits a piece longer
/// than `max_chars`. Returns `(start_char, end_char, piece)` relative to `text`.
///
/// Used when a single line exceeds the file-type limit (line-oriented splitters
/// cannot break it) and as the core of [`split_text`] for runs without newlines.
pub fn hard_split_chars(
    text: &str,
    max_chars: usize,
    overlap: usize,
) -> Vec<(usize, usize, String)> {
    let max_chars = max_chars.max(1);
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![];
    }
    if chars.len() <= max_chars {
        return vec![(0, chars.len(), text.to_string())];
    }

    let mut chunks = Vec::new();
    let mut start: usize = 0;
    while start < chars.len() {
        let end = (start + max_chars).min(chars.len());
        let piece: String = chars[start..end].iter().collect();
        chunks.push((start, end, piece));
        if end >= chars.len() {
            break;
        }
        let mut next = end.saturating_sub(overlap);
        if next <= start {
            // Ensure progress even with extreme overlap.
            next = start + 1;
        }
        start = next;
    }
    chunks
}

pub fn split_text(text: &str, max_chars: usize, overlap: usize) -> Vec<(usize, usize, String)> {
    let clean = text.replace("\r\n", "\n");
    if clean.trim().is_empty() {
        return vec![];
    }

    let max_chars = max_chars.max(1);
    // Work with chars for correct Unicode handling
    let chars: Vec<char> = clean.chars().collect();
    let length = chars.len();
    let mut chunks = Vec::new();
    let mut start: usize = 0;

    while start < length {
        let mut end = (start + max_chars).min(length);

        // Newline snap in the back half (prefer soft boundaries when present).
        if end < length {
            let search_start = (start + max_chars / 2).min(end);
            let mut newline_pos = None;
            for i in (search_start..end).rev() {
                if chars[i] == '\n' {
                    newline_pos = Some(i);
                    break;
                }
            }
            if let Some(nl) = newline_pos {
                if nl > start {
                    end = nl + 1;
                }
            }
        }

        // Guarantee: never emit a piece longer than max_chars (hard-split if
        // the chosen span still exceeds — should not happen after the min above,
        // but keep the invariant explicit for a single long line).
        if end - start > max_chars {
            end = start + max_chars;
        }

        let piece: String = chars[start..end]
            .iter()
            .collect::<String>()
            .trim()
            .to_string();
        if !piece.is_empty() {
            // piece may be shorter than [start,end) after trim; still record
            // the char span used for overlap advance.
            chunks.push((start, end, piece));
        }

        if end >= length {
            break;
        }
        let mut next = end.saturating_sub(overlap);
        if next <= start {
            next = start + 1;
        }
        start = next;
    }

    chunks
}

// ── Read text file ───────────────────────────────────────────────────────────

pub fn read_text_file(path: &Path) -> Option<String> {
    // Refuse non-regular files (devices, dirs) before following content.
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() || meta.file_type().is_symlink() => {}
        _ => return None,
    }
    let raw = std::fs::read(path).ok()?;
    if raw.contains(&0u8) {
        return None;
    }
    Some(String::from_utf8_lossy(&raw).to_string())
}

/// True when `path` resolves to a regular file under `root` (symlink-safe).
fn path_resolves_under_root(path: &Path, root: &Path) -> bool {
    let Ok(canon_root) = std::fs::canonicalize(root) else {
        return false;
    };
    let Ok(canon) = std::fs::canonicalize(path) else {
        return false;
    };
    match std::fs::metadata(&canon) {
        Ok(m) if m.is_file() => canon.starts_with(&canon_root),
        _ => false,
    }
}

// ── Build chunks for file (the main entry point) ────────────────────────────

pub fn build_chunks_for_file(path: &Path, root: &Path) -> Vec<serde_json::Value> {
    let (max_chars, overlap) = chunk_limits_for_file(path);
    build_chunks_for_file_with_limits(path, root, max_chars, overlap)
}

/// Like [`build_chunks_for_file`], but `max_chars` and `overlap` come from the
/// caller and apply to every file. AST semantic chunking still runs first
/// (same as production); `max_chars` also bounds those structural chunks.
pub fn build_chunks_for_file_with_limits(
    path: &Path,
    root: &Path,
    max_chars: usize,
    overlap: usize,
) -> Vec<serde_json::Value> {
    if !path_resolves_under_root(path, root) {
        return vec![];
    }
    let text = match read_text_file(path) {
        Some(t) => t,
        None => return vec![],
    };

    let file_id = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");

    if let Some(semantic_chunks) = ast_chunker::chunk_file_semantically_with_overlap(
        path,
        root,
        Some(&text),
        max_chars,
        overlap,
    ) {
        return semantic_chunks;
    }

    let suffix = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_else(|| "text".to_string());

    let cluster_suffix = suffix.trim_start_matches('.').to_string();
    let cluster_semantic = if cluster_suffix.is_empty() {
        "text".to_string()
    } else {
        cluster_suffix
    };

    let file_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let pieces = split_text(&text, max_chars, overlap);

    let mut chunks = Vec::new();
    for (index, (start, end, piece)) in pieces.into_iter().enumerate() {
        chunks.push(serde_json::json!({
            "id": format!("{}#chunk-{:04}", file_id, index),
            "file_id": file_id,
            "label": format!("{} chunk {}", file_name, index + 1),
            "area": "FileChunk",
            "cluster_semantic": cluster_semantic,
            "chunk_index": index,
            "start_char": start,
            "end_char": end,
            "text": piece,
            "file_sorgente": file_id,
            "kind": "text_slice",
            "symbol_name": "",
            "signature": "",
            "line_start": 0,
            "line_end": 0,
            "language": "",
            "symbols_used": "[]",
        }));
    }

    chunks
}

#[cfg(test)]
mod with_limits_tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn with_limits_honors_max_chars_and_overlap_on_sliding_window() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // `.txt` skips AST, so the sliding-window path sees the overlap we pass.
        let path = root.join("note.txt");
        let text: String = (0..3000)
            .map(|i| char::from(b'A' + (i % 26) as u8))
            .collect();
        fs::write(&path, &text).unwrap();

        let max_chars = 1024;
        let overlap = 164;
        let chunks = build_chunks_for_file_with_limits(&path, root, max_chars, overlap);
        assert!(chunks.len() > 1, "3000 chars must split at 1024");
        for c in &chunks {
            let n = c["text"].as_str().unwrap().chars().count();
            assert!(n <= max_chars, "chunk has {n} chars > {max_chars}");
        }
        let a = chunks[0]["text"].as_str().unwrap();
        let b = chunks[1]["text"].as_str().unwrap();
        let tail: String = a
            .chars()
            .rev()
            .take(overlap)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        let head: String = b.chars().take(overlap).collect();
        assert_eq!(
            tail, head,
            "consecutive sliding-window chunks must overlap by {overlap} chars"
        );
    }

    #[test]
    fn with_limits_keeps_ast_path_and_bounds_it() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let path = root.join("two.rs");
        // Two functions far apart so AST can emit two structural chunks.
        let mut src = String::from("pub fn alpha() {\n    let x = 1;\n}\n");
        src.push_str(&"// pad\n".repeat(80));
        src.push_str("pub fn beta() {\n    let y = 2;\n}\n");
        fs::write(&path, &src).unwrap();

        let chunks = build_chunks_for_file_with_limits(&path, root, 1024, 164);
        assert!(
            chunks
                .iter()
                .any(|c| c["kind"].as_str() == Some("function")),
            "AST path must stay on; got {:?}",
            chunks.iter().map(|c| c["kind"].clone()).collect::<Vec<_>>()
        );
        for c in &chunks {
            let n = c["text"].as_str().unwrap_or("").chars().count();
            assert!(n <= 1024, "AST chunk has {n} chars > 1024");
        }
    }

    #[test]
    fn default_path_matches_with_limits_when_overlap_is_declared() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let path = root.join("note.txt");
        let text: String = (0..4000)
            .map(|i| char::from(b'0' + (i % 10) as u8))
            .collect();
        fs::write(&path, &text).unwrap();

        let old = build_chunks_for_file(&path, root);
        let (max_chars, declared) = chunk_limits_for_file(&path);
        let new = build_chunks_for_file_with_limits(&path, root, max_chars, declared);
        assert_eq!(old, new);
    }
}

#[cfg(test)]
mod split_text_tests {
    use super::*;

    #[test]
    fn hard_split_single_line_40k_respects_max_chars() {
        let max_chars = CHUNK_CODE_MAX_CHARS;
        let overlap = hard_split_overlap(max_chars, CHUNK_CODE_OVERLAP_CHARS);
        let original: String = (0..40_000)
            .map(|i| char::from(b'A' + (i % 26) as u8))
            .collect();
        let pieces = hard_split_chars(&original, max_chars, overlap);
        assert!(pieces.len() > 1);
        for (_, _, p) in &pieces {
            assert!(
                p.chars().count() <= max_chars,
                "piece has {} chars > max_chars={max_chars}",
                p.chars().count()
            );
        }
        // Reconstruct with overlap removed via char offsets.
        let mut rebuilt = String::new();
        let mut cursor = 0usize;
        for (s, e, _) in &pieces {
            if *e <= cursor {
                continue;
            }
            let from = cursor.saturating_sub(*s);
            let piece_chars: Vec<char> = original.chars().skip(*s).take(e - s).collect();
            let take: String = piece_chars.into_iter().skip(from).collect();
            rebuilt.push_str(&take);
            cursor = *e;
        }
        assert_eq!(rebuilt, original);
    }

    #[test]
    fn split_text_single_line_40k_respects_max_chars() {
        let max_chars = CHUNK_CODE_MAX_CHARS;
        let overlap = CHUNK_CODE_OVERLAP_CHARS;
        let original: String = (0..40_000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let pieces = split_text(&original, max_chars, overlap);
        assert!(pieces.len() > 1);
        for (_, _, p) in &pieces {
            assert!(
                p.chars().count() <= max_chars,
                "split_text piece has {} chars",
                p.chars().count()
            );
        }
        // Offsets must cover the full range with only overlap gaps.
        assert_eq!(pieces[0].0, 0);
        assert_eq!(pieces.last().unwrap().1, original.chars().count());
    }

    #[test]
    fn production_geometry_uses_declared_code_overlap() {
        assert_eq!(
            chunk_limits_for_file(Path::new("src/example.py")),
            (CHUNK_CODE_MAX_CHARS, CHUNK_CODE_OVERLAP_CHARS)
        );
        let fingerprint = chunk_geometry_fingerprint();
        assert!(fingerprint.contains("\"code_max_chars\":1024"));
        assert!(fingerprint.contains("\"code_overlap_chars\":164"));
        assert!(!fingerprint.contains("max_chars/8"));
    }
}
