//! Build the code-knowledge graph from what indexing already computed.
//!
//! The v1 builder parsed every file a second time with tree-sitter and eight
//! grammars. This one does not parse anything: the semantic chunker has already
//! found each file's top-level items, with their kind, name and line span, and
//! the indexer holds those chunks in memory. Nodes and `CONTAIN` edges fall out
//! of that for free.
//!
//! `IMPORT` edges are the part that needs real work, and they deliberately do
//! **not** come from a chunk's `symbols_used`. That field drops every specifier
//! beginning with `.` — the relative imports that resolve to files in this
//! repository are exactly what it throws away — because it exists to feed
//! lexical scoring, not a graph. So import specifiers are extracted here, from
//! the file source, with patterns that keep the specifier intact.
//!
//! ## The rule that keeps this honest
//!
//! An `IMPORT` edge is emitted only when the specifier resolves to a path that
//! is **in the indexed set**. Never the filesystem, never a guess: an edge to a
//! file Oracle has not indexed is an edge whose endpoint nothing can open. When
//! a language's resolution is ambiguous the edge is dropped rather than
//! approximated, so the graph under-reports instead of lying.
//!
//! Resolution is implemented for TypeScript/JavaScript, Python and Rust. Other
//! languages contribute nodes and `CONTAIN` edges but no imports, which is
//! visible in the graph rather than silently wrong.
//!
//! ## Known limits, found by audit and accepted rather than hidden
//!
//! - **The source is re-read after the chunks are computed.** If a file changes
//!   in that window — seconds, while its batch embeds — the chunks describe the
//!   old text and the edges the new. It self-heals the next time that file is
//!   indexed. Closing it properly means threading the source through the
//!   embedding path, which would put a field on the chunk schema that feeds the
//!   index, and that is a worse trade for a race this small.
//! - **`crate::` finds the crate root with the *last* `/src/` in the importer's
//!   path.** A pathological layout — `project/src/src/module.rs` — picks the
//!   inner one and resolves against the wrong root. The result is a missing
//!   edge, or a wrong one if a same-named file happens to sit at that depth.
//!   Neither `find` nor `rfind` is right for every layout (`vendor/src/x/src/`
//!   wants the inner one), so this stays a documented heuristic.
//! - **The universe slightly over-approximates the index.** It comes from the
//!   collected file set, which includes files later skipped as oversized or
//!   sensitive. An edge can therefore name a real file that has no chunks. The
//!   path is still openable, which is what an import edge promises.
//!
//! ## What this cannot do
//!
//! There are no `CALL` edges here, and there were none in v1 either — its
//! builder emitted `CONTAIN` and `IMPORT` and nothing else. Any "find callers"
//! query is therefore a query over data that does not exist, and needs a call
//! extractor before it needs a query.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::store::ckg::{CkgEdgeRow, CkgNodeRow};

/// One file's contribution, as the indexer already has it.
pub struct FileGraphInput<'a> {
    /// Repository-relative POSIX path; this is also the FILE node's id.
    pub file_id: &'a str,
    /// Full file text, for import extraction only.
    pub source: &'a str,
    /// The chunks the semantic chunker produced for this file.
    pub chunks: &'a [serde_json::Value],
}

/// Nodes and edges for a set of files, ready for `CkgStore::replace_for_files`.
#[derive(Debug, Default)]
pub struct CkgGraph {
    pub nodes: Vec<CkgNodeRow>,
    pub edges: Vec<CkgEdgeRow>,
}

// ── Import specifiers ────────────────────────────────────────────────────────

/// Patterns that capture the *specifier* — the thing being imported — rather
/// than the names bound from it. Group 1 is always the specifier.
static IMPORT_SPECIFIERS: LazyLock<HashMap<&'static str, Vec<Regex>>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    let js_ts = vec![
        // `import ... from "spec"`, `export ... from "spec"`, `import "spec"`
        Regex::new(r#"(?m)^\s*(?:import|export)\b[^;\n]*?from\s*['"]([^'"]+)['"]"#).unwrap(),
        Regex::new(r#"(?m)^\s*import\s*['"]([^'"]+)['"]"#).unwrap(),
        Regex::new(r#"require\s*\(\s*['"]([^'"]+)['"]\s*\)"#).unwrap(),
        Regex::new(r#"\bimport\s*\(\s*['"]([^'"]+)['"]\s*\)"#).unwrap(),
    ];
    map.insert("typescript", js_ts.clone());
    map.insert("javascript", js_ts);
    map.insert(
        "python",
        vec![
            Regex::new(r"(?m)^\s*from\s+([.\w]+)\s+import\b").unwrap(),
            Regex::new(r"(?m)^\s*import\s+([.\w]+)").unwrap(),
        ],
    );
    map.insert(
        "rust",
        vec![Regex::new(r"(?m)^\s*(?:pub\s+)?use\s+([\w:]+)").unwrap()],
    );
    map
});

/// Extract the import specifiers of one file, in source order, deduplicated.
pub fn import_specifiers(source: &str, language: &str) -> Vec<String> {
    let Some(patterns) = IMPORT_SPECIFIERS.get(language) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    for pattern in patterns {
        for captures in pattern.captures_iter(source) {
            if let Some(specifier) = captures.get(1) {
                let specifier = specifier.as_str().trim();
                if !specifier.is_empty() {
                    seen.insert(specifier.to_string());
                }
            }
        }
    }
    seen.into_iter().collect()
}

// ── Resolution ───────────────────────────────────────────────────────────────

/// Extensions tried when a specifier omits one, in the order a bundler or
/// interpreter would try them.
const TS_EXTENSIONS: &[&str] = &[".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"];

fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(index) => &path[..index],
        None => "",
    }
}

/// Collapse `.` and `..` segments. Returns `None` if the path escapes the root,
/// which is a specifier pointing outside the repository and so not ours.
fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

fn join(base: &str, relative: &str) -> Option<String> {
    if base.is_empty() {
        normalize(relative)
    } else {
        normalize(&format!("{base}/{relative}"))
    }
}

/// First candidate that is an indexed file. `None` when nothing resolves, which
/// is the common and correct outcome for a package import.
fn first_indexed(candidates: &[String], indexed: &HashSet<&str>) -> Option<String> {
    candidates
        .iter()
        .find(|candidate| indexed.contains(candidate.as_str()))
        .cloned()
}

/// TypeScript and JavaScript: only relative specifiers can name a file in this
/// repository. A bare specifier is a package, and a package is not a node here.
fn resolve_ts(specifier: &str, importer_dir: &str, indexed: &HashSet<&str>) -> Option<String> {
    if !specifier.starts_with('.') {
        return None;
    }
    let base = join(importer_dir, specifier)?;
    let mut candidates = vec![base.clone()];
    for extension in TS_EXTENSIONS {
        candidates.push(format!("{base}{extension}"));
    }
    for extension in TS_EXTENSIONS {
        candidates.push(format!("{base}/index{extension}"));
    }
    first_indexed(&candidates, indexed)
}

/// Python: a leading dot count is a relative package level, otherwise the
/// dotted name is tried from the repository root.
fn resolve_py(specifier: &str, importer_dir: &str, indexed: &HashSet<&str>) -> Option<String> {
    let leading_dots = specifier.chars().take_while(|c| *c == '.').count();
    let tail = specifier.trim_start_matches('.').replace('.', "/");

    let base = if leading_dots == 0 {
        tail.clone()
    } else {
        // One dot is "this package"; each further dot climbs one level.
        let mut directory = importer_dir.to_string();
        for _ in 1..leading_dots {
            directory = parent_dir(&directory).to_string();
        }
        if tail.is_empty() {
            directory.clone()
        } else {
            join(&directory, &tail)?
        }
    };
    if base.is_empty() {
        return None;
    }
    first_indexed(
        &[format!("{base}.py"), format!("{base}/__init__.py")],
        indexed,
    )
}

/// Rust: `crate::`, `super::` and `self::` name modules inside this tree; a
/// leading identifier that is not one of those is an external crate.
///
/// `crate::` is resolved against the importer's own crate source root, found by
/// walking up to the last `src/` segment in its path. That is a heuristic, and
/// it is the reason a resolution that does not land on an indexed file is
/// dropped rather than approximated.
fn resolve_rust(specifier: &str, importer: &str, indexed: &HashSet<&str>) -> Option<String> {
    let mut segments: Vec<&str> = specifier.split("::").filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return None;
    }
    // A trailing `Type` or `function` is an item inside the module, not a path
    // segment. Trying both the full path and one segment shorter covers it.
    let root = segments.remove(0);
    let importer_dir = parent_dir(importer);

    let base_dir = match root {
        "crate" => {
            let src_index = importer.rfind("/src/").map(|index| index + 5);
            match src_index {
                Some(index) => importer[..index].trim_end_matches('/').to_string(),
                None if importer.starts_with("src/") => "src".to_string(),
                None => return None,
            }
        }
        "self" => importer_dir.to_string(),
        "super" => parent_dir(importer_dir).to_string(),
        _ => return None,
    };

    let mut candidates = Vec::new();
    for depth in [segments.len(), segments.len().saturating_sub(1)] {
        if depth == 0 {
            continue;
        }
        let module_path = segments[..depth].join("/");
        let Some(joined) = join(&base_dir, &module_path) else {
            continue;
        };
        candidates.push(format!("{joined}.rs"));
        candidates.push(format!("{joined}/mod.rs"));
    }
    first_indexed(&candidates, indexed)
}

fn resolve(
    specifier: &str,
    language: &str,
    importer: &str,
    indexed: &HashSet<&str>,
) -> Option<String> {
    let importer_dir = parent_dir(importer);
    match language {
        "typescript" | "javascript" => resolve_ts(specifier, importer_dir, indexed),
        "python" => resolve_py(specifier, importer_dir, indexed),
        "rust" => resolve_rust(specifier, importer, indexed),
        _ => None,
    }
}

// ── Graph construction ───────────────────────────────────────────────────────

fn chunk_str<'a>(chunk: &'a serde_json::Value, key: &str) -> &'a str {
    chunk
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or("")
}

fn chunk_i64(chunk: &serde_json::Value, key: &str) -> i64 {
    chunk.get(key).and_then(|value| value.as_i64()).unwrap_or(0)
}

/// Convenience entry that treats the given files as the whole universe.
///
/// Only the tests want this: production always knows the full file set and
/// calls [`build_graph_within`] with it. Kept because a test that has to build
/// a universe by hand is a test that is easy to get wrong.
#[cfg(test)]
pub fn build_graph(files: &[FileGraphInput<'_>]) -> CkgGraph {
    let universe: HashSet<String> = files.iter().map(|file| file.file_id.to_string()).collect();
    build_graph_within(files, &universe)
}

/// Build nodes and edges for the given files, resolving imports against
/// `universe`.
///
/// The FILE node id is the repository-relative path and a symbol node id is
/// `<path>#<start>-<end>-<index>`, matching the scheme v1 wrote so anything
/// reading an existing database keeps working; `file_id_of_node` in the old MCP
/// splits on `#`, which this preserves.
///
/// The universe is separate from the inputs because indexing runs in batches and
/// an import almost always crosses a batch boundary. Resolving against only the
/// files in hand would drop those edges silently.
pub fn build_graph_within(files: &[FileGraphInput<'_>], universe: &HashSet<String>) -> CkgGraph {
    let indexed: HashSet<&str> = universe.iter().map(String::as_str).collect();
    let mut graph = CkgGraph::default();

    for file in files {
        // The extension decides the language, not the chunk metadata. A chunk
        // can legitimately carry an empty `language` — a text slice does — and
        // taking the language from there would silently switch import
        // extraction off for the whole file.
        let language = crate::ingest::ast_chunker::detect_language(Path::new(file.file_id));
        let total_lines = file.source.lines().count().max(1) as i64;

        graph.nodes.push(CkgNodeRow {
            id: file.file_id.to_string(),
            kind: "FILE".to_string(),
            name: None,
            file: file.file_id.to_string(),
            start_line: Some(1),
            end_line: Some(total_lines),
            lang: (!language.is_empty()).then(|| language.to_string()),
        });

        // One symbol node per named top-level item. A chunk without a symbol
        // name is a text slice or a module header, which is not an item; a
        // repeated span is a hard split of one item, which is not a second one.
        let mut seen_spans: HashSet<(i64, i64, String)> = HashSet::new();
        for chunk in file.chunks {
            let name = chunk_str(chunk, "symbol_name");
            if name.is_empty() {
                continue;
            }
            let start = chunk_i64(chunk, "line_start");
            let end = chunk_i64(chunk, "line_end");
            if start <= 0 || end < start {
                continue;
            }
            if !seen_spans.insert((start, end, name.to_string())) {
                continue;
            }
            let index = seen_spans.len() - 1;
            let node_id = format!("{}#{start}-{end}-{index}", file.file_id);
            graph.nodes.push(CkgNodeRow {
                id: node_id.clone(),
                kind: chunk_str(chunk, "kind").to_string(),
                name: Some(name.to_string()),
                file: file.file_id.to_string(),
                start_line: Some(start),
                end_line: Some(end),
                lang: (!language.is_empty()).then(|| language.to_string()),
            });
            graph.edges.push(CkgEdgeRow {
                src: file.file_id.to_string(),
                dst: node_id,
                kind: "CONTAIN".to_string(),
                src_file: file.file_id.to_string(),
            });
        }

        let mut imported: BTreeSet<String> = BTreeSet::new();
        for specifier in import_specifiers(file.source, language) {
            if let Some(target) = resolve(&specifier, language, file.file_id, &indexed) {
                if target != file.file_id {
                    imported.insert(target);
                }
            }
        }
        for target in imported {
            graph.edges.push(CkgEdgeRow {
                src: file.file_id.to_string(),
                dst: target,
                kind: "IMPORT".to_string(),
                src_file: file.file_id.to_string(),
            });
        }
    }

    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chunk(kind: &str, name: &str, start: i64, end: i64, language: &str) -> serde_json::Value {
        json!({
            "kind": kind,
            "symbol_name": name,
            "line_start": start,
            "line_end": end,
            "language": language,
        })
    }

    #[test]
    fn a_file_becomes_a_node_with_contain_edges_to_its_symbols() {
        let chunks = vec![
            chunk("function", "alpha", 1, 10, "rust"),
            chunk("function", "beta", 12, 20, "rust"),
        ];
        let graph = build_graph(&[FileGraphInput {
            file_id: "src/a.rs",
            source: "line\n".repeat(20).as_str(),
            chunks: &chunks,
        }]);
        assert_eq!(graph.nodes.len(), 3);
        assert_eq!(graph.nodes[0].kind, "FILE");
        assert_eq!(graph.nodes[0].end_line, Some(20));
        let contains: Vec<&CkgEdgeRow> =
            graph.edges.iter().filter(|e| e.kind == "CONTAIN").collect();
        assert_eq!(contains.len(), 2);
        assert!(contains.iter().all(|edge| edge.src == "src/a.rs"));
        assert!(graph.nodes[1].id.starts_with("src/a.rs#1-10-"));
    }

    #[test]
    fn a_chunk_without_a_symbol_is_not_an_item() {
        let chunks = vec![
            chunk("text_slice", "", 0, 0, ""),
            chunk("module_header", "", 1, 4, "rust"),
        ];
        let graph = build_graph(&[FileGraphInput {
            file_id: "notes.md",
            source: "a\nb\n",
            chunks: &chunks,
        }]);
        assert_eq!(graph.nodes.len(), 1, "only the FILE node");
        assert!(graph.edges.is_empty());
    }

    #[test]
    fn one_item_split_across_chunks_is_still_one_node() {
        let chunks = vec![
            chunk("function", "huge", 1, 400, "rust"),
            chunk("function", "huge", 1, 400, "rust"),
        ];
        let graph = build_graph(&[FileGraphInput {
            file_id: "src/a.rs",
            source: "x\n",
            chunks: &chunks,
        }]);
        assert_eq!(
            graph.nodes.iter().filter(|n| n.kind == "function").count(),
            1
        );
    }

    #[test]
    fn typescript_relative_imports_resolve_and_packages_do_not() {
        let source = r#"
import { a } from "./sibling";
import b from "../lib/deep";
import c from "./folder";
import react from "react";
const d = require("./required");
"#;
        let files = [
            ("src/app/main.ts", source),
            ("src/app/sibling.ts", ""),
            ("src/lib/deep.tsx", ""),
            ("src/app/folder/index.ts", ""),
            ("src/app/required.js", ""),
        ];
        let inputs: Vec<FileGraphInput<'_>> = files
            .iter()
            .map(|(path, text)| FileGraphInput {
                file_id: path,
                source: text,
                chunks: &[],
            })
            .collect();
        let graph = build_graph(&inputs);
        let mut targets: Vec<&str> = graph
            .edges
            .iter()
            .filter(|edge| edge.kind == "IMPORT" && edge.src == "src/app/main.ts")
            .map(|edge| edge.dst.as_str())
            .collect();
        targets.sort_unstable();
        assert_eq!(
            targets,
            vec![
                "src/app/folder/index.ts",
                "src/app/required.js",
                "src/app/sibling.ts",
                "src/lib/deep.tsx",
            ],
            "a bare package specifier must not become an edge"
        );
    }

    #[test]
    fn python_relative_and_absolute_imports_resolve() {
        let source =
            // `..deep` from package `pkg.app` is `pkg.deep`: one dot is the
            // current package and each further dot climbs one level.
            // `..pkg.deep` would mean `pkg.pkg.deep`, a different module.
            "from .sibling import x\nfrom ..deep import y\nimport top.mod\nimport os\n";
        let files = [
            ("pkg/app/main.py", source),
            ("pkg/app/sibling.py", ""),
            ("pkg/deep.py", ""),
            ("top/mod/__init__.py", ""),
        ];
        let inputs: Vec<FileGraphInput<'_>> = files
            .iter()
            .map(|(path, text)| FileGraphInput {
                file_id: path,
                source: text,
                chunks: &[],
            })
            .collect();
        let graph = build_graph(&inputs);
        let mut targets: Vec<&str> = graph
            .edges
            .iter()
            .filter(|edge| edge.kind == "IMPORT")
            .map(|edge| edge.dst.as_str())
            .collect();
        targets.sort_unstable();
        assert_eq!(
            targets,
            vec!["pkg/app/sibling.py", "pkg/deep.py", "top/mod/__init__.py"],
            "`import os` has no file in this repository and must not become an edge"
        );
    }

    #[test]
    fn rust_crate_super_and_self_resolve_against_the_crate_source_root() {
        let source = "use crate::query::engine::QueryEngine;\nuse super::sibling;\nuse self::inner::Thing;\nuse anyhow::Result;\n";
        let files = [
            ("crates/oracle-core/src/ingest/ckg_build.rs", source),
            ("crates/oracle-core/src/query/engine.rs", ""),
            ("crates/oracle-core/src/sibling.rs", ""),
            ("crates/oracle-core/src/ingest/inner/mod.rs", ""),
        ];
        let inputs: Vec<FileGraphInput<'_>> = files
            .iter()
            .map(|(path, text)| FileGraphInput {
                file_id: path,
                source: text,
                chunks: &[],
            })
            .collect();
        let graph = build_graph(&inputs);
        let mut targets: Vec<&str> = graph
            .edges
            .iter()
            .filter(|edge| edge.kind == "IMPORT")
            .map(|edge| edge.dst.as_str())
            .collect();
        targets.sort_unstable();
        assert_eq!(
            targets,
            vec![
                "crates/oracle-core/src/ingest/inner/mod.rs",
                "crates/oracle-core/src/query/engine.rs",
                "crates/oracle-core/src/sibling.rs",
            ],
            "`use anyhow::Result` is an external crate and must not become an edge"
        );
    }

    #[test]
    fn an_unresolvable_specifier_is_dropped_rather_than_guessed() {
        let source = "import x from \"./does-not-exist\";\n";
        let graph = build_graph(&[FileGraphInput {
            file_id: "src/a.ts",
            source,
            chunks: &[],
        }]);
        assert!(
            graph.edges.iter().all(|edge| edge.kind != "IMPORT"),
            "an edge whose endpoint is not indexed points at nothing openable"
        );
    }

    #[test]
    fn a_specifier_escaping_the_repository_root_resolves_to_nothing() {
        let graph = build_graph(&[FileGraphInput {
            file_id: "a.ts",
            source: "import x from \"../../outside\";\n",
            chunks: &[],
        }]);
        assert!(graph.edges.is_empty());
    }

    #[test]
    fn a_file_never_imports_itself() {
        let files = [("src/a.ts", "import x from \"./a\";\n"), ("src/a.ts", "")];
        let inputs: Vec<FileGraphInput<'_>> = files
            .iter()
            .map(|(path, text)| FileGraphInput {
                file_id: path,
                source: text,
                chunks: &[],
            })
            .collect();
        let graph = build_graph(&inputs);
        assert!(graph.edges.iter().all(|edge| edge.kind != "IMPORT"));
    }

    #[test]
    fn languages_without_a_resolver_still_contribute_nodes() {
        let chunks = vec![chunk("function", "main", 1, 5, "go")];
        let graph = build_graph(&[FileGraphInput {
            file_id: "main.go",
            source: "package main\nimport \"fmt\"\n",
            chunks: &chunks,
        }]);
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].kind, "CONTAIN");
    }
}
