//! Oracle store-layer configuration constants and path resolution.
//!
//! Mirrors the subset of `oracle/config.py` consumed by the store layer
//! (`SQLiteStore`, `LanceStore`, the chunk-index manifest) plus the
//! chunk-profile version logic from `oracle/ingestion/retrieval_text.py`.

use std::env;
use std::path::{Component, Path, PathBuf};

/// Last-resort embedding width for paths that have neither a loaded model nor
/// stored vectors (for example the dimensionless hash fallback or a legacy
/// empty-schema test). Model-backed indexing/querying must use model metadata.
#[cfg(feature = "full")]
pub const EMBED_DIMS: usize = 384;

/// Store directory / file names (relative to the Oracle data dir).
/// Mirrors the `*_PATH` constants in `oracle/config.py`.
pub const VECTORS_DIR: &str = "vectors.lancedb";
pub const CHUNKS_DIR: &str = "chunks.lancedb";
pub const FILE_VECTORS_DIR: &str = "file_vectors.lancedb";
pub const METADATA_SQLITE: &str = "metadata.sqlite";
pub const CHUNK_MANIFEST: &str = "chunk-index-manifest.json";
/// Code-knowledge-graph database. Name and `CKG_DB_PATH` override are v1's,
/// so an existing graph is found rather than silently rebuilt beside itself.
pub const CKG_SQLITE: &str = "ckg.sqlite";
/// Augur findings ledger. Lives next to the CKG so a workspace has one data
/// dir; the plugin install dir is never used.
pub const AUGUR_SQLITE: &str = "augur.sqlite";

/// Env var selecting the real (Qwen) query embedder vs. the hash fallback.
/// Mirrors `ORACLE_QUERY_EMBEDDER` usage in `oracle/store/lance_store.py`.
#[cfg(feature = "full")]
pub const ENV_QUERY_EMBEDDER: &str = "ORACLE_QUERY_EMBEDDER";

/// Env var forcing the real (Qwen) embedder; blocks the hash fallback.
/// Mirrors `ORACLE_REQUIRE_REAL_EMBEDDER` in `oracle/ingestion/embedder.py`.
#[cfg(feature = "full")]
pub const ENV_REQUIRE_REAL_EMBEDDER: &str = "ORACLE_REQUIRE_REAL_EMBEDDER";

/// Default Oracle data directory name (relative to the workspace root).
/// Mirrors `ORACLE_DIR = Path(os.getenv("ORACLE_DIR", "oracle-data"))`.
pub const DEFAULT_ORACLE_DIR: &str = "oracle-data";

/// Hard cap on public retrieval `limit` parameters (MCP tools, HTTP /ask,
/// /context, /similar, and engine-side neighbor/preview fan-out derived from
/// those limits).
pub const MAX_BOUNDED_LIMIT: usize = 100;

// ── Chunk-profile version constants ────────────────────────────────────────
// Byte-identical to `oracle/ingestion/retrieval_text.py`.

/// Raw (non-semantic-prefix) chunk-profile version string.
///
/// Bumped 2026-08-29 to invalidate indexes built with the previous model and
/// token-window recipe. `t512` records bge-small-en-v1.5's token limit.
#[cfg(feature = "full")]
pub const RAW_CHUNK_PROFILE_VERSION: &str = "adaptive-bge-small-en-v1.5-2026-08-29-t512";

/// Semantic-prefix chunk-profile version string (the default).
///
/// Bumped 2026-08-29 to invalidate indexes built with the previous model and
/// chunk geometry. `c1024-o164` records the active code-chunk geometry and
/// `t512` records bge-small-en-v1.5's token limit.
#[cfg(feature = "full")]
pub const SEMANTIC_PREFIX_PROFILE_VERSION: &str =
    "semantic-prefix-bge-small-en-v1.5-2026-08-29-c1024-o164-t512";

/// Profile names that normalize to the semantic-prefix profile.
/// Mirrors `SEMANTIC_PROFILE_NAMES` in `oracle/ingestion/retrieval_text.py`.
#[cfg(feature = "full")]
pub const SEMANTIC_PROFILE_NAMES: &[&str] =
    &["semantic-prefix-v2", "semantic_prefix_v2", "semantic", "v2"];

/// Mirrors `oracle/ingestion/retrieval_text.py::normalize_profile`.
#[cfg(feature = "full")]
fn normalize_profile(value: &str) -> String {
    let profile = value.trim().to_lowercase();
    if SEMANTIC_PROFILE_NAMES.contains(&profile.as_str()) {
        "semantic-prefix-v2".to_string()
    } else {
        "raw".to_string()
    }
}

/// Mirrors `oracle/ingestion/retrieval_text.py::active_embed_profile`
/// (defaults to `"semantic-prefix-v2"` when `ORACLE_EMBED_PROFILE` is unset).
#[cfg(feature = "full")]
fn active_embed_profile() -> String {
    let raw = env::var("ORACLE_EMBED_PROFILE").unwrap_or_else(|_| "semantic-prefix-v2".to_string());
    normalize_profile(&raw)
}

/// Active chunk-profile version string.
///
/// Mirrors `oracle/ingestion/retrieval_text.py::active_chunk_profile_version`.
/// With no `profile` override and the default `ORACLE_EMBED_PROFILE`
/// (`"semantic-prefix-v2"`) this returns the active semantic-prefix version.
#[cfg(feature = "full")]
pub fn active_chunk_profile_version(profile: Option<&str>) -> String {
    let effective = match profile {
        Some(p) => normalize_profile(p),
        None => active_embed_profile(),
    };
    if effective == "semantic-prefix-v2" {
        SEMANTIC_PREFIX_PROFILE_VERSION.to_string()
    } else {
        RAW_CHUNK_PROFILE_VERSION.to_string()
    }
}

/// Whether the real (Qwen) embedder is hard-required.
///
/// Mirrors `oracle/ingestion/embedder.py::require_real_embedder`
/// (`ORACLE_REQUIRE_REAL_EMBEDDER` in `{"1","true","yes"}`).
#[cfg(feature = "full")]
pub fn require_real_embedder() -> bool {
    match env::var(ENV_REQUIRE_REAL_EMBEDDER) {
        Ok(v) => matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes"),
        Err(_) => false,
    }
}

/// Whether the hash query embedder is explicitly selected.
///
/// Mirrors the `ORACLE_QUERY_EMBEDDER=hash` debug knob in
/// `oracle/store/lance_store.py::embed_query_text`.
#[cfg(feature = "full")]
pub fn query_embedder_is_hash() -> bool {
    // Python: `os.getenv(...).lower() == "hash"` — any casing matches.
    env::var(ENV_QUERY_EMBEDDER)
        .map(|v| v.trim().to_lowercase() == "hash")
        .unwrap_or(false)
}

/// Canonical set of store paths under a workspace root.
///
/// Mirrors the `*_PATH` constants in `oracle/config.py`, resolved beneath
/// `<root>/oracle-data/` (or the `ORACLE_DIR` env override when set to an
/// absolute path). Each field is the exact path used by the corresponding
/// store module.
#[derive(Debug, Clone)]
pub struct OracleDataPaths {
    /// The Oracle data directory (`<root>/oracle-data`).
    pub root: PathBuf,
    /// Node/CKG vector store (`vectors.lancedb`).
    pub vectors: PathBuf,
    /// Chunk vector store (`chunks.lancedb`).
    pub chunks: PathBuf,
    /// Per-file vector store (`file_vectors.lancedb`).
    pub file_vectors: PathBuf,
    /// Metadata SQLite database (`metadata.sqlite`).
    pub metadata: PathBuf,
    /// Chunk-index manifest (`chunk-index-manifest.json`).
    pub manifest: PathBuf,
    /// Code-knowledge-graph database (`ckg.sqlite`).
    pub ckg: PathBuf,
}

impl OracleDataPaths {
    /// Compute every store path beneath `<root>/oracle-data/`.
    ///
    /// Mirrors `oracle/config.py`: the data dir is `ORACLE_DIR` when that env
    /// var is an absolute path, otherwise `<root>/<ORACLE_DIR>`. Each sub-path
    /// honors its own env override (`LANCE_DB_PATH`, `CHUNK_DB_PATH`, …) when
    /// set **and confined under the data dir**; escapes fall back to the
    /// conventional name under the data dir.
    pub fn from_root(root: &Path) -> Self {
        let data_dir = resolve_data_dir_from_root(root);
        Self::from_resolved_data_dir(data_dir)
    }

    /// Compute store paths from the granted workspace root without consulting
    /// process environment overrides. Plugin capabilities must not be able to
    /// redirect a read to another workspace's Oracle data.
    pub fn from_root_without_env(root: &Path) -> Self {
        let data_dir = absolute_path(&root.join(DEFAULT_ORACLE_DIR));
        let data_dir = data_dir.canonicalize().unwrap_or(data_dir);
        Self::from_fixed_data_dir(data_dir)
    }

    /// Treat `data_dir` as the Oracle data directory itself (no re-join of
    /// `ORACLE_DIR`). Used by MCP where `ORACLE_DIR` already names the data dir.
    pub fn from_data_dir(data_dir: &Path) -> Self {
        let data_dir = absolute_path(data_dir);
        let data_dir = data_dir.canonicalize().unwrap_or(data_dir);
        Self::from_resolved_data_dir(data_dir)
    }

    fn from_resolved_data_dir(data_dir: PathBuf) -> Self {
        OracleDataPaths {
            vectors: confined_env_or(&["LANCE_DB_PATH"], data_dir.join(VECTORS_DIR), &data_dir),
            chunks: confined_env_or(&["CHUNK_DB_PATH"], data_dir.join(CHUNKS_DIR), &data_dir),
            file_vectors: confined_env_or(
                &["FILE_VECTORS_DB_PATH"],
                data_dir.join(FILE_VECTORS_DIR),
                &data_dir,
            ),
            metadata: confined_env_or(&["SQLITE_PATH"], data_dir.join(METADATA_SQLITE), &data_dir),
            manifest: confined_env_or(
                &["CHUNK_MANIFEST_PATH"],
                data_dir.join(CHUNK_MANIFEST),
                &data_dir,
            ),
            ckg: confined_env_or(&["CKG_DB_PATH"], data_dir.join(CKG_SQLITE), &data_dir),
            root: data_dir,
        }
    }

    fn from_fixed_data_dir(data_dir: PathBuf) -> Self {
        OracleDataPaths {
            vectors: data_dir.join(VECTORS_DIR),
            chunks: data_dir.join(CHUNKS_DIR),
            file_vectors: data_dir.join(FILE_VECTORS_DIR),
            metadata: data_dir.join(METADATA_SQLITE),
            manifest: data_dir.join(CHUNK_MANIFEST),
            ckg: data_dir.join(CKG_SQLITE),
            root: data_dir,
        }
    }

    /// Findings ledger path. Always `<oracle-data>/augur.sqlite` — no env
    /// override, same confinement as the CKG under `from_root_without_env`.
    pub fn augur_ledger(&self) -> PathBuf {
        self.root.join(AUGUR_SQLITE)
    }
}

/// Resolve the Oracle data directory under a workspace root once, to an
/// absolute (preferably canonical) path.
fn resolve_data_dir_from_root(root: &Path) -> PathBuf {
    let data_dir = match env::var("ORACLE_DIR") {
        Ok(dir) if Path::new(&dir).is_absolute() => PathBuf::from(dir),
        Ok(dir) if !dir.trim().is_empty() => root.join(dir),
        _ => root.join(DEFAULT_ORACLE_DIR),
    };
    let data_dir = absolute_path(&data_dir);
    data_dir.canonicalize().unwrap_or(data_dir)
}

/// Resolve `path` from the first set env var when the result stays under
/// `data_dir`; otherwise `default`. Escaping overrides are ignored (fail-closed).
fn confined_env_or(keys: &[&str], default: PathBuf, data_dir: &Path) -> PathBuf {
    for key in keys {
        if let Ok(v) = env::var(key) {
            if v.is_empty() {
                continue;
            }
            let candidate = PathBuf::from(&v);
            let resolved = if candidate.is_absolute() {
                candidate
            } else {
                data_dir.join(&candidate)
            };
            let resolved = absolute_path(&resolved);
            if path_is_under_dir(&resolved, data_dir) {
                return resolved.canonicalize().unwrap_or(resolved);
            }
            // Escape: ignore the override (fail-closed), but surface it so operators
            // notice a legitimate external-disk override was discarded.
            eprintln!(
                "oracle-core: ignoring env {key}={v:?} — resolves outside data dir {}; falling back to default",
                data_dir.display()
            );
        }
    }
    default
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize_path(path)
    } else {
        match env::current_dir() {
            Ok(cwd) => normalize_path(&cwd.join(path)),
            Err(_) => normalize_path(path),
        }
    }
}

/// Collapse `.` / `..` without requiring the path to exist on disk.
fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(s) => out.push(s),
        }
    }
    out
}

/// Canonical form for confinement checks when `path` may not exist yet.
///
/// `Path::canonicalize` fails for missing files; on macOS that leaves a
/// non-canonical absolute path (e.g. `/var/...`) while an existing `root`
/// becomes `/private/var/...`, so a naive `starts_with` false-negatives.
/// Resolve the deepest existing ancestor, then rejoin the missing suffix so
/// both sides share the same symlink resolution.
fn canonicalize_for_compare(path: &Path) -> PathBuf {
    let path = absolute_path(path);
    if let Ok(canon) = path.canonicalize() {
        return canon;
    }

    let mut suffix = Vec::new();
    let mut cur = path.as_path();
    while let Some(parent) = cur.parent() {
        if parent == cur {
            break;
        }
        if let Some(name) = cur.file_name() {
            suffix.push(name.to_os_string());
        } else {
            break;
        }
        if parent.exists() {
            if let Ok(canon_parent) = parent.canonicalize() {
                let mut out = canon_parent;
                for component in suffix.into_iter().rev() {
                    out.push(component);
                }
                return out;
            }
        }
        cur = parent;
    }
    path
}

fn path_is_under_dir(path: &Path, root: &Path) -> bool {
    let root = root.canonicalize().unwrap_or_else(|_| absolute_path(root));
    let path = canonicalize_for_compare(path);
    path.starts_with(&root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn path_is_under_dir_accepts_missing_file_under_symlinked_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let real_root = tmp.path().join("real_data");
        fs::create_dir_all(&real_root).expect("mkdir real_data");

        let link_parent = tmp.path().join("links");
        fs::create_dir_all(&link_parent).expect("mkdir links");
        let linked_root = link_parent.join("data_link");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_root, &linked_root).expect("symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&real_root, &linked_root).expect("symlink_dir");

        // File not created yet — classic first-run store path.
        let missing = linked_root.join("metadata.sqlite");
        assert!(!missing.exists());

        // Root already canonical (as resolve_data_dir does); path still via symlink.
        let root_canon = real_root.canonicalize().expect("canonicalize real_root");
        assert!(
            path_is_under_dir(&missing, &root_canon),
            "missing file under symlinked path must compare equal to canonical root"
        );

        // Root expressed via the symlink should also accept.
        assert!(path_is_under_dir(&missing, &linked_root));
    }

    #[test]
    fn path_is_under_dir_still_rejects_escape() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("data");
        fs::create_dir_all(&root).expect("mkdir data");
        let outside = tmp.path().join("outside.db");
        assert!(!path_is_under_dir(&outside, &root));
        // Missing escape candidate (no create) must also be rejected.
        let missing_outside = tmp.path().join("also_outside").join("db.sqlite");
        assert!(!path_is_under_dir(&missing_outside, &root));
    }

    #[test]
    fn root_only_paths_ignore_oracle_dir_override() {
        let temp = tempfile::tempdir().expect("tempdir");
        let foreign = temp.path().join("foreign");
        fs::create_dir_all(&foreign).expect("foreign root");
        let previous = env::var_os("ORACLE_DIR");
        env::set_var("ORACLE_DIR", &foreign);
        let paths = OracleDataPaths::from_root_without_env(temp.path());
        match previous {
            Some(value) => env::set_var("ORACLE_DIR", value),
            None => env::remove_var("ORACLE_DIR"),
        }
        assert_eq!(paths.root, temp.path().join(DEFAULT_ORACLE_DIR));
        assert_eq!(paths.ckg, paths.root.join(CKG_SQLITE));
        assert_eq!(paths.augur_ledger(), paths.root.join(AUGUR_SQLITE));
        assert_eq!(
            paths.augur_ledger().parent().as_deref(),
            paths.ckg.parent(),
            "augur ledger must sit beside the CKG so path drift is impossible"
        );
    }
}
