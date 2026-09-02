//! Cheap deterministic detector: a source file with no test beside it.
//!
//! Different shape from secrets (set membership over the walked list, not
//! a regex over bytes). Smoke only: missing tests are a smell, not a fire.
//! Inline `#[cfg(test)]` modules do not count — "beside" means a sibling.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::detector::{Context, Cost, Detector};
use crate::error::Error;
use crate::finding::{Draft, Finding, Severity};

pub struct Untested;

impl Default for Untested {
    fn default() -> Self {
        Self
    }
}

impl Detector for Untested {
    fn id(&self) -> &'static str {
        "untested"
    }

    fn cost(&self) -> Cost {
        Cost::Cheap
    }

    fn scan(&self, ctx: &Context<'_>) -> Result<Vec<Finding>, Error> {
        let mut classified = Vec::new();
        for file in ctx.files {
            let Some(posix) = posix_relative(ctx.root, file) else {
                continue;
            };
            let (parent, name) = parent_and_name(&posix);
            let is_test = path_looks_like_test(&posix);
            let is_source = source_extension(name).is_some_and(is_source_extension);
            classified.push(Classified {
                original: file.clone(),
                parent: parent.to_string(),
                name: name.to_string(),
                posix,
                is_test,
                is_source,
            });
        }

        let mut covered = HashSet::new();
        for item in &classified {
            if !item.is_test {
                continue;
            }
            let Some(stem) = covered_stem(&item.name).or_else(|| source_stem(&item.name)) else {
                continue;
            };
            covered.insert(coverage_key(&item.parent, &stem));
            // One-level hop: src/__tests__/foo.test.ts covers src/foo.ts.
            if last_segment_is_test_dir(&item.parent) {
                if let Some(grandparent) = parent_of(&item.parent) {
                    covered.insert(coverage_key(grandparent, &stem));
                } else {
                    covered.insert(coverage_key("", &stem));
                }
            }
        }

        let mut findings = Vec::new();
        for item in &classified {
            if item.is_test || !item.is_source {
                continue;
            }
            let Some(stem) = source_stem(&item.name) else {
                continue;
            };
            if covered.contains(&coverage_key(&item.parent, &stem)) {
                continue;
            }
            // Line 1-1 is the whole finding. grounded() would slurp the file
            // just to count lines; a non-empty file has line 1. Integrity of
            // "inside root" still goes through grounded_on → canonical_file.
            let Some(finding) = untested_finding(ctx.root, item) else {
                continue;
            };
            findings.push(finding);
        }
        Ok(findings)
    }
}

struct Classified {
    original: PathBuf,
    posix: String,
    parent: String,
    name: String,
    is_test: bool,
    is_source: bool,
}

fn posix_relative(root: &Path, file: &Path) -> Option<String> {
    let joined = if file.is_absolute() {
        file.to_path_buf()
    } else {
        root.join(file)
    };
    let relative = if let Ok(stripped) = joined.strip_prefix(root) {
        stripped.to_path_buf()
    } else {
        let root_abs = std::path::absolute(root).ok()?;
        let file_abs = std::path::absolute(&joined).ok()?;
        file_abs.strip_prefix(&root_abs).ok()?.to_path_buf()
    };
    let posix = relative.to_string_lossy().replace('\\', "/");
    if posix.is_empty() {
        None
    } else {
        Some(posix)
    }
}

fn parent_and_name(posix: &str) -> (&str, &str) {
    match posix.rfind('/') {
        Some(index) => (&posix[..index], &posix[index + 1..]),
        None => ("", posix),
    }
}

fn untested_finding(root: &Path, item: &Classified) -> Option<Finding> {
    let joined = if item.original.is_absolute() {
        item.original.clone()
    } else {
        root.join(&item.original)
    };
    let len = std::fs::metadata(&joined).ok()?.len();
    if len == 0 {
        return None;
    }
    Finding::grounded_on(
        root,
        Draft {
            file: item.original.clone(),
            start_line: 1,
            end_line: 1,
            rule: "test.missing",
            severity: Severity::Smoke,
            source: "untested",
            title: "No test file beside this one",
            raw_excerpt: &item.posix,
        },
        1,
    )
}

fn last_segment_is_test_dir(parent: &str) -> bool {
    let name = parent.rsplit('/').next().unwrap_or(parent);
    matches!(
        name.to_lowercase().as_str(),
        "test" | "tests" | "__tests__"
    )
}

fn parent_of(parent: &str) -> Option<&str> {
    parent.rfind('/').map(|index| &parent[..index])
}

fn fold_dir(dir: &str) -> String {
    if cfg!(windows) {
        // Same fold as polis-backend FileIdIndex / augur path identity.
        dir.to_lowercase()
    } else {
        dir.to_string()
    }
}

fn source_extension(name: &str) -> Option<&str> {
    let dot = name.rfind('.')?;
    if dot == 0 {
        return None;
    }
    Some(&name[dot + 1..])
}

fn is_source_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py" | "go"
    )
}

fn path_looks_like_test(posix: &str) -> bool {
    let lower = posix.to_ascii_lowercase();
    lower.split('/').any(|part| matches!(part, "test" | "tests" | "__tests__"))
        || name_looks_like_test(posix.rsplit('/').next().unwrap_or(posix))
}

fn name_looks_like_test(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains(".test.")
        || lower.contains(".spec.")
        || covered_stem(name)
            .map(|stem| stem != strip_last_extension(name))
            .unwrap_or(false)
}

fn strip_last_extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(dot) if dot > 0 => &name[..dot],
        _ => name,
    }
}

/// Stem a sibling test file would cover. `foo.test.ts` and `foo_test.rs`
/// both cover `foo`. `None` if the name is not a recognised test spelling.
fn covered_stem(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let without_ext = strip_last_extension(&lower);
    if let Some(stem) = without_ext.strip_suffix(".test") {
        return Some(stem.to_string());
    }
    if let Some(stem) = without_ext.strip_suffix(".spec") {
        return Some(stem.to_string());
    }
    if let Some(stem) = without_ext.strip_suffix("_test") {
        return Some(stem.to_string());
    }
    if let Some(stem) = without_ext.strip_suffix("_spec") {
        return Some(stem.to_string());
    }
    if let Some(stem) = without_ext.strip_prefix("test_") {
        return Some(stem.to_string());
    }
    None
}

fn source_stem(name: &str) -> Option<String> {
    let ext = source_extension(name)?;
    if !is_source_extension(ext) {
        return None;
    }
    Some(strip_last_extension(name).to_string())
}

fn coverage_key(parent: &str, stem: &str) -> String {
    format!("{}|{stem}", fold_dir(parent))
}

#[cfg(test)]
fn write_source(root: &Path, relative: &str, body: &str) -> PathBuf {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(&path, body).expect("write");
    PathBuf::from(relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detector::Context;

    #[test]
    fn a_source_file_with_no_sibling_test_is_smoke() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = write_source(temp.path(), "src/lib.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n");
        let files = [file];
        let findings = Untested::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("scan");
        let hit = findings
            .iter()
            .find(|finding| finding.rule() == "test.missing")
            .expect("untested source should be a finding");
        assert_eq!(hit.severity(), Severity::Smoke);
        assert_eq!(hit.source(), "untested");
        assert_eq!(hit.file(), Path::new("src/lib.rs"));
    }

    #[test]
    fn a_rust_sibling_test_silences_the_source() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = write_source(temp.path(), "src/lib.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n");
        let test = write_source(
            temp.path(),
            "src/lib_test.rs",
            "#[test] fn add_works() { assert_eq!(2, 2); }\n",
        );
        let files = [source, test];
        let findings = Untested::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("scan");
        assert!(
            findings.iter().all(|finding| finding.rule() != "test.missing"),
            "sibling test was ignored: {findings:?}"
        );
    }

    #[test]
    fn a_typescript_test_suffix_counts_as_beside() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = write_source(temp.path(), "src/foo.ts", "export const n = 1;\n");
        let test = write_source(temp.path(), "src/foo.test.ts", "export const n = 1;\n");
        let files = [source, test];
        let findings = Untested::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("scan");
        assert!(
            findings.is_empty(),
            "foo.test.ts should cover foo.ts: {findings:?}"
        );
    }

    #[test]
    fn a_test_file_is_not_itself_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let test = write_source(temp.path(), "src/foo.test.ts", "export const n = 1;\n");
        let files = [test];
        let findings = Untested::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("scan");
        assert!(
            findings.is_empty(),
            "a test file is not an untested source: {findings:?}"
        );
    }

    #[test]
    fn a_test_in_a_tests_directory_covers_the_parent_stem() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = write_source(temp.path(), "src/foo.ts", "export const n = 1;\n");
        let test = write_source(
            temp.path(),
            "src/__tests__/foo.test.ts",
            "export const n = 1;\n",
        );
        let files = [source, test];
        let findings = Untested::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("scan");
        assert!(
            findings.is_empty(),
            "src/__tests__/foo.test.ts should cover src/foo.ts (one-level hop): {findings:?}"
        );
    }

    #[test]
    fn a_vanished_file_does_not_fail_the_scan() {
        let temp = tempfile::tempdir().expect("tempdir");
        let kept = write_source(temp.path(), "src/kept.rs", "pub fn k() {}\n");
        let files = [kept, PathBuf::from("src/gone.rs")];
        let findings = Untested::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("vanished file must not fail the detector");
        assert!(
            findings
                .iter()
                .any(|finding| finding.file() == Path::new("src/kept.rs")),
            "kept file was lost because a neighbour vanished: {findings:?}"
        );
    }
}
