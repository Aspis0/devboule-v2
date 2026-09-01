use std::collections::{BTreeMap, HashSet};
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use oracle_core::{
    collect_text_files_with_cancel_limits_report, CancelFlag, CkgStore, OracleDataPaths,
};
use regex::Regex;

pub const MAX_CITY_FILES: usize = 5000;
pub const MAX_CITY_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug)]
pub enum CityBuildError {
    UnreadableRoot {
        path: PathBuf,
        source: std::io::Error,
    },
    #[allow(dead_code)]
    UnreadableFile {
        path: PathBuf,
        source: std::io::Error,
    },
    Cancelled,
    Ckg(String),
}

impl Display for CityBuildError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnreadableRoot { path, source } => {
                write!(f, "city root unreadable ({}): {source}", path.display())
            }
            Self::UnreadableFile { path, source } => {
                write!(f, "city file unreadable ({}): {source}", path.display())
            }
            Self::Cancelled => f.write_str("city walk cancelled"),
            Self::Ckg(message) => write!(f, "city CKG read failed: {message}"),
        }
    }
}

impl std::error::Error for CityBuildError {}

struct CitySource {
    id: String,
    source: Option<String>,
    lines: usize,
}

#[derive(Default)]
struct CityBuildStats {
    truncated_files: usize,
    skipped_files: usize,
}

static TYPESCRIPT_IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bimport\s+(?:type\s+)?[^;\n]*?\s+from\s+["']([^"']+)["']"#)
        .expect("city TypeScript import regex")
});
static RUST_CRATE_USE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\buse\s+crate::([A-Za-z0-9_]+(?:::[A-Za-z0-9_]+)*)")
        .expect("city Rust import regex")
});

/// Build the runtime City JSON. This blocking work runs in the backend
/// process, away from the GUI thread. The Oracle collector carries the
/// established ignore, symlink, and text-extension policy.
pub fn build_city(root: &Path) -> Result<serde_json::Value, CityBuildError> {
    fs::read_dir(root).map_err(|source| CityBuildError::UnreadableRoot {
        path: root.to_path_buf(),
        source,
    })?;
    let root = fs::canonicalize(root).map_err(|source| CityBuildError::UnreadableRoot {
        path: root.to_path_buf(),
        source,
    })?;

    let cancel = CancelFlag::new();
    let report =
        collect_text_files_with_cancel_limits_report(&root, &cancel, Some(MAX_CITY_FILES), None)
            .ok_or(CityBuildError::Cancelled)?;
    build_city_from_paths(&root, &report.paths, report.truncated)
}

fn build_city_from_paths(
    root: &Path,
    paths: &[PathBuf],
    truncated: bool,
) -> Result<serde_json::Value, CityBuildError> {
    let mut sources = Vec::with_capacity(paths.len());
    let mut stats = CityBuildStats {
        truncated_files: usize::from(truncated),
        ..CityBuildStats::default()
    };
    for path in paths {
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                stats.skipped_files += 1;
                continue;
            }
        };
        // Keep the city contract's independent 2 MB ceiling here as a second
        // line of defense if the collector policy ever changes.
        if metadata.len() > MAX_CITY_FILE_BYTES {
            stats.skipped_files += 1;
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => {
                stats.skipped_files += 1;
                continue;
            }
        };
        let Some(relative) = path.strip_prefix(root).ok() else {
            stats.skipped_files += 1;
            continue;
        };
        let id = relative.to_string_lossy().replace('\\', "/");
        if id.is_empty() {
            stats.skipped_files += 1;
            continue;
        }
        let lines = count_lines(&bytes);
        sources.push(CitySource {
            id,
            source: String::from_utf8(bytes).ok(),
            lines,
        });
    }
    sources.sort_by(|left, right| left.id.cmp(&right.id));

    let known_files: HashSet<&str> = sources.iter().map(|source| source.id.as_str()).collect();
    let imports = match ckg_imports(&root, &sources, &known_files)? {
        Some(imports) => imports,
        None => regex_imports(&sources, &known_files),
    };
    let files = sources
        .iter()
        .map(|source| {
            serde_json::json!({
                "id": source.id,
                "path": source.id,
                "lines": source.lines,
                "district": source.id.split('/').next().unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();

    let mut city = serde_json::json!({
        "files": files,
        "imports": imports,
        "agents": [],
        "findings": [],
        "dataSource": "host",
    });
    if stats.truncated_files != 0 {
        city["truncatedFiles"] = serde_json::json!(stats.truncated_files);
    }
    if stats.skipped_files != 0 {
        city["skippedFiles"] = serde_json::json!(stats.skipped_files);
    }
    Ok(city)
}

fn count_lines(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let mut breaks = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                breaks += 1;
                if bytes.get(index + 1) == Some(&b'\n') {
                    index += 1;
                }
            }
            b'\n' => breaks += 1,
            _ => {}
        }
        index += 1;
    }
    if bytes.ends_with(b"\n") || bytes.ends_with(b"\r") {
        breaks
    } else {
        breaks + 1
    }
}

fn ckg_imports(
    root: &Path,
    sources: &[CitySource],
    known_files: &HashSet<&str>,
) -> Result<Option<Vec<serde_json::Value>>, CityBuildError> {
    let path = OracleDataPaths::from_root_without_env(root).ckg;
    if !path.is_file() {
        return Ok(None);
    }
    let store = CkgStore::open_read_only(&path)
        .map_err(|error| CityBuildError::Ckg(format!("opening {}: {error}", path.display())))?;
    let mut weights: BTreeMap<(String, String), u64> = BTreeMap::new();
    for source in sources {
        let edges = store
            .imports_of(&source.id)
            .map_err(|error| CityBuildError::Ckg(format!("reading {}: {error}", source.id)))?;
        for edge in edges {
            let Some(target) = ckg_target_file(&edge.dst, known_files) else {
                continue;
            };
            if target != source.id {
                *weights.entry((source.id.clone(), target)).or_default() += 1;
            }
        }
    }
    if weights.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        weights
            .into_iter()
            .map(|((from, to), weight)| {
                serde_json::json!({ "from": from, "to": to, "weight": weight })
            })
            .collect(),
    ))
}

fn ckg_target_file(raw: &str, known_files: &HashSet<&str>) -> Option<String> {
    let normalized = raw.replace('\\', "/").trim_start_matches("./").to_string();
    if known_files.contains(normalized.as_str()) {
        return Some(normalized);
    }
    let file = normalized.split('#').next().unwrap_or_default();
    known_files.contains(file).then(|| file.to_string())
}

fn regex_imports(sources: &[CitySource], known_files: &HashSet<&str>) -> Vec<serde_json::Value> {
    let mut weights: BTreeMap<(String, String), u64> = BTreeMap::new();
    for source in sources {
        let targets = if let Some(source_text) = source.source.as_deref() {
            if is_typescript_file(&source.id) {
                TYPESCRIPT_IMPORT
                    .captures_iter(source_text)
                    .filter_map(|captures| captures.get(1).map(|value| value.as_str().to_string()))
                    .filter_map(|specifier| resolve_typescript(&source.id, &specifier, known_files))
                    .collect::<Vec<_>>()
            } else if source.id.ends_with(".rs") {
                RUST_CRATE_USE
                    .captures_iter(source_text)
                    .filter_map(|captures| captures.get(1).map(|value| value.as_str().to_string()))
                    .filter_map(|specifier| resolve_rust(&source.id, &specifier, known_files))
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        for target in targets {
            if target != source.id {
                *weights.entry((source.id.clone(), target)).or_default() += 1;
            }
        }
    }
    weights
        .into_iter()
        .map(|((from, to), weight)| serde_json::json!({ "from": from, "to": to, "weight": weight }))
        .collect()
}

fn is_typescript_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx"]
        .iter()
        .any(|extension| lower.ends_with(extension))
}

fn normalize_relative(path: &str) -> Option<String> {
    let mut parts = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            value => parts.push(value),
        }
    }
    Some(parts.join("/"))
}

fn resolve_typescript(
    source_path: &str,
    specifier: &str,
    known_files: &HashSet<&str>,
) -> Option<String> {
    if !specifier.starts_with('.') {
        return None;
    }
    let importer_dir = source_path
        .rsplit_once('/')
        .map(|(dir, _)| dir)
        .unwrap_or("");
    let base = normalize_relative(&format!("{importer_dir}/{specifier}"))?;
    let mut candidates = vec![base.clone()];
    for extension in [".ts", ".tsx", ".js", ".jsx", ".mjs", ".json"] {
        candidates.push(format!("{base}{extension}"));
    }
    for extension in [".ts", ".tsx", ".js", ".jsx", ".mjs", ".json"] {
        candidates.push(format!("{base}/index{extension}"));
    }
    candidates
        .into_iter()
        .find(|candidate| known_files.contains(candidate.as_str()))
}

fn resolve_rust(source_path: &str, specifier: &str, known_files: &HashSet<&str>) -> Option<String> {
    let source_segments: Vec<&str> = source_path.split('/').collect();
    let src_index = source_segments
        .iter()
        .rposition(|segment| *segment == "src")?;
    let source_root = source_segments[..=src_index].join("/");
    let module_segments: Vec<&str> = specifier.split("::").collect();
    for length in (1..=module_segments.len()).rev() {
        let module_path = format!("{source_root}/{}", module_segments[..length].join("/"));
        for candidate in [format!("{module_path}.rs"), format!("{module_path}/mod.rs")] {
            if known_files.contains(candidate.as_str()) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_file_read_failures_are_skipped_and_counted() {
        let temp = tempfile::tempdir().unwrap();
        let good = temp.path().join("good.ts");
        let undecodable = temp.path().join("undecodable.ts");
        let missing = temp.path().join("missing.ts");
        fs::write(&good, b"one\ntwo\n").unwrap();
        fs::write(&undecodable, [b'a', b'\n', 0xff, b'\r']).unwrap();

        let city = build_city_from_paths(temp.path(), &[good, undecodable, missing], false)
            .expect("one bad file must not abort the city");
        assert_eq!(city["skippedFiles"], 1);
        assert_eq!(city["files"].as_array().unwrap().len(), 2);
        assert_eq!(
            city["files"]
                .as_array()
                .unwrap()
                .iter()
                .find(|file| file["path"] == "undecodable.ts")
                .unwrap()["lines"],
            2
        );
    }
}
