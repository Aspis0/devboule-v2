//! Cheap Augur scan, invoked because the surface opened.
//!
//! Evidence and line numbers are deliberately NOT in the wire shape yet —
//! the inspector comes later. The fire bands only need id, file, severity,
//! rule, and title.

use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use devboule_augur::{Context, Cost, Finding, Ledger, Registry};
use oracle_core::{
    collect_text_files_with_cancel_limits_report, CancelFlag, OracleDataPaths,
};

use crate::city::{MAX_CITY_FILE_BYTES, MAX_CITY_FILES};

#[derive(Debug)]
pub enum FindingsError {
    UnreadableRoot {
        path: PathBuf,
        source: std::io::Error,
    },
    Cancelled,
    Ledger(String),
}

impl Display for FindingsError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnreadableRoot { path, source } => {
                write!(f, "findings root unreadable ({}): {source}", path.display())
            }
            Self::Cancelled => f.write_str("findings walk cancelled"),
            Self::Ledger(message) => write!(f, "findings ledger: {message}"),
        }
    }
}

impl std::error::Error for FindingsError {}

pub(crate) struct CollectedFiles {
    pub root: PathBuf,
    pub paths: Vec<PathBuf>,
    pub truncated: bool,
}

struct SurvivedFile {
    absolute: PathBuf,
    id: String,
}

/// Opening the surface is the ask. Walk the same files the city uses, run
/// every cheap detector, persist, return the active mapped findings.
pub fn get_findings(root: &Path) -> Result<serde_json::Value, FindingsError> {
    let collected = collect_workspace_files(root)?;
    scan_collected_files(&collected)
}

pub(crate) fn collect_workspace_files(root: &Path) -> Result<CollectedFiles, FindingsError> {
    fs::read_dir(root).map_err(|source| FindingsError::UnreadableRoot {
        path: root.to_path_buf(),
        source,
    })?;
    let root = fs::canonicalize(root).map_err(|source| FindingsError::UnreadableRoot {
        path: root.to_path_buf(),
        source,
    })?;
    let cancel = CancelFlag::new();
    let report =
        collect_text_files_with_cancel_limits_report(&root, &cancel, Some(MAX_CITY_FILES), None)
            .ok_or(FindingsError::Cancelled)?;
    Ok(CollectedFiles {
        root,
        paths: report.paths,
        truncated: report.truncated,
    })
}

pub(crate) fn scan_collected_files(
    collected: &CollectedFiles,
) -> Result<serde_json::Value, FindingsError> {
    let started = Instant::now();
    let (survived, skipped_files) = survive_city_files(&collected.root, &collected.paths);
    let index = FileIdIndex::from_survived(&survived);
    let files: Vec<PathBuf> = survived.iter().map(|file| file.absolute.clone()).collect();
    let ctx = Context::new(&collected.root, &files);
    let review = Registry::builtin().review(&ctx, Cost::Cheap);

    let ledger_path = OracleDataPaths::from_root_without_env(&collected.root).augur_ledger();
    let ledger = Ledger::open(&ledger_path)
        .map_err(|error| FindingsError::Ledger(error.to_string()))?;
    ledger
        .record_scan(&review.findings, &review.completed, &review.registered)
        .map_err(|error| FindingsError::Ledger(error.to_string()))?;
    let active = ledger
        .active()
        .map_err(|error| FindingsError::Ledger(error.to_string()))?;

    let (findings, dropped_findings) = attach_findings(&active, &index);
    let scan_ms = started.elapsed().as_millis() as u64;
    let mut body = serde_json::json!({
        "findings": findings,
        "scanned": true,
        "completed": review.completed,
        "failed": review.failed.iter().map(|failed| failed.detector).collect::<Vec<_>>(),
        "scanMs": scan_ms,
        "droppedFindings": dropped_findings,
    });
    if skipped_files != 0 {
        body["skippedFiles"] = serde_json::json!(skipped_files);
    }
    if collected.truncated {
        body["truncatedFiles"] = serde_json::json!(1);
    }
    Ok(body)
}

/// Mirror of `city.rs` per-file read: a file that vanishes or becomes
/// unreadable between the walk and the scan is skipped and counted, never a
/// panic, never a failed review.
fn survive_city_files(root: &Path, paths: &[PathBuf]) -> (Vec<SurvivedFile>, usize) {
    let mut survived = Vec::new();
    let mut skipped = 0usize;
    for path in paths {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if metadata.len() > MAX_CITY_FILE_BYTES {
            skipped += 1;
            continue;
        }
        // Existence/readability only — do not slurp bytes here. Secrets will
        // read the contents; a second full pass was doubling the scan.
        if fs::File::open(path).is_err() {
            skipped += 1;
            continue;
        }
        let Some(relative) = path.strip_prefix(root).ok() else {
            skipped += 1;
            continue;
        };
        let id = relative.to_string_lossy().replace('\\', "/");
        if id.is_empty() {
            skipped += 1;
            continue;
        }
        survived.push(SurvivedFile {
            absolute: path.clone(),
            id,
        });
    }
    (survived, skipped)
}

struct FileIdIndex {
    by_exact: HashMap<String, String>,
    by_folded: HashMap<String, String>,
}

impl FileIdIndex {
    fn from_survived(files: &[SurvivedFile]) -> Self {
        let mut by_exact = HashMap::new();
        let mut by_folded = HashMap::new();
        for file in files {
            by_exact.insert(file.id.clone(), file.id.clone());
            by_folded.insert(fold_file_id(&file.id), file.id.clone());
        }
        Self {
            by_exact,
            by_folded,
        }
    }

    #[cfg(test)]
    fn from_ids(ids: &[String]) -> Self {
        let files: Vec<SurvivedFile> = ids
            .iter()
            .map(|id| SurvivedFile {
                absolute: PathBuf::from(id),
                id: id.clone(),
            })
            .collect();
        Self::from_survived(&files)
    }

    fn reconcile(&self, finding_file: &Path) -> Option<String> {
        let posix = finding_file.to_string_lossy().replace('\\', "/");
        if let Some(id) = self.by_exact.get(&posix) {
            return Some(id.clone());
        }
        self.by_folded.get(&fold_file_id(&posix)).cloned()
    }
}

fn fold_file_id(id: &str) -> String {
    if cfg!(windows) {
        id.to_lowercase()
    } else {
        id.to_string()
    }
}

fn attach_findings(
    findings: &[Finding],
    index: &FileIdIndex,
) -> (Vec<serde_json::Value>, usize) {
    let mut mapped = Vec::new();
    let mut dropped = 0usize;
    for finding in findings {
        let Some(file_id) = index.reconcile(finding.file()) else {
            dropped += 1;
            continue;
        };
        mapped.push(serde_json::json!({
            "id": finding.id().as_str(),
            "fileId": file_id,
            "severity": finding.severity().as_str(),
            "rule": finding.rule(),
            "title": finding.title(),
        }));
    }
    (mapped, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use devboule_augur::{shipped_rule_matches, Draft, Finding, FindingId, Severity};
    use std::path::Path;

    fn aws_access_token() -> String {
        let prefix = "AKIA";
        let body = "BHCEFGHIJKLMNOPQ";
        format!("{prefix}{body}")
    }

    fn write(root: &Path, relative: &str, body: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(&path, body).expect("write");
    }

    fn grounded_secret(root: &Path, relative: &str, excerpt: &str) -> Finding {
        Finding::grounded(
            root,
            Draft {
                file: PathBuf::from(relative),
                start_line: 1,
                end_line: 1,
                rule: "aws-access-token",
                severity: Severity::Inferno,
                source: "secrets",
                title: "AWS Access Token",
                raw_excerpt: excerpt,
            },
        )
        .expect("grounded")
    }

    #[test]
    fn a_finding_in_a_file_the_walk_skipped_is_dropped_and_counted() {
        let temp = tempfile::tempdir().unwrap();
        let aws = aws_access_token();
        assert!(
            shipped_rule_matches("aws-access-token", &aws),
            "assembled fixture no longer matches gitleaks aws-access-token"
        );
        write(temp.path(), "kept.rs", "pub fn ok() {}\n");
        write(
            temp.path(),
            "skipped.rs",
            &format!("const KEY: &str = \"{aws}\";\n"),
        );
        let finding = grounded_secret(temp.path(), "skipped.rs", &aws);
        let index = FileIdIndex::from_ids(&["kept.rs".to_string()]);
        let (mapped, dropped) = attach_findings(&[finding], &index);
        assert_eq!(dropped, 1, "skipped file must be counted: {mapped:?}");
        assert!(
            mapped.is_empty(),
            "a finding that cannot attach to a building must not ship: {mapped:?}"
        );
    }

    #[test]
    fn mixed_case_file_id_matches_the_city_walk_casing() {
        let temp = tempfile::tempdir().unwrap();
        let aws = aws_access_token();
        assert!(
            shipped_rule_matches("aws-access-token", &aws),
            "assembled fixture no longer matches gitleaks aws-access-token"
        );
        write(
            temp.path(),
            "Src/Auth.rs",
            &format!("const KEY: &str = \"{aws}\";\n"),
        );
        let collected = collect_workspace_files(temp.path()).expect("walk");
        let walked_id = collected
            .paths
            .iter()
            .filter_map(|path| {
                path.strip_prefix(&collected.root)
                    .ok()
                    .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            })
            .find(|id| id.to_lowercase().ends_with("auth.rs"))
            .expect("walked Auth.rs");
        assert_eq!(
            walked_id, "Src/Auth.rs",
            "city walk must keep the created casing: {walked_id}"
        );
        let body = scan_collected_files(&collected).expect("scan");
        let secret = body["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .find(|finding| finding["rule"] == "aws-access-token")
            .expect("secret finding");
        assert_eq!(
            secret["fileId"].as_str().expect("fileId"),
            walked_id.as_str(),
            "fileId must be the walk's casing, not augur's fold: {secret}"
        );
    }

    #[test]
    fn a_dismissal_survives_a_second_scan() {
        let temp = tempfile::tempdir().unwrap();
        let aws = aws_access_token();
        assert!(
            shipped_rule_matches("aws-access-token", &aws),
            "assembled fixture no longer matches gitleaks aws-access-token"
        );
        write(
            temp.path(),
            "src/auth.rs",
            &format!("const KEY: &str = \"{aws}\";\n"),
        );
        let first = get_findings(temp.path()).expect("first scan");
        let id = first["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .find(|finding| finding["rule"] == "aws-access-token")
            .expect("secret")["id"]
            .as_str()
            .expect("id")
            .to_string();
        let canonical = fs::canonicalize(temp.path()).expect("canonical workspace");
        let ledger_path = OracleDataPaths::from_root_without_env(&canonical).augur_ledger();
        assert!(
            ledger_path.ends_with(Path::new("oracle-data").join("augur.sqlite")),
            "ledger must live under oracle-data, got {}",
            ledger_path.display()
        );
        let ledger = Ledger::open(&ledger_path).expect("open ledger");
        let finding_id = FindingId::from_stored(id.clone()).expect("id");
        ledger.dismiss(&finding_id).expect("dismiss");

        let second = get_findings(temp.path()).expect("second scan");
        let still_active = second["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .any(|finding| finding["id"] == id);
        assert!(
            !still_active,
            "a dismissed finding came back after a rescan: {}",
            second["findings"]
        );
        assert_eq!(second["scanned"], true);
    }

    #[test]
    fn a_file_deleted_between_walk_and_scan_is_skipped_and_counted() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "src/kept.rs", "pub fn kept() {}\n");
        write(temp.path(), "src/gone.rs", "pub fn gone() {}\n");
        let mut collected = collect_workspace_files(temp.path()).expect("walk");
        let gone = collected
            .paths
            .iter()
            .find(|path| path.ends_with("gone.rs"))
            .cloned()
            .expect("gone.rs walked");
        fs::remove_file(&gone).expect("delete between walk and scan");
        collected.paths.retain(|path| path != &gone);
        collected.paths.push(gone);
        let body = scan_collected_files(&collected).expect("review must complete");
        assert_eq!(
            body["skippedFiles"], 1,
            "vanished file must be counted: {body}"
        );
        assert_eq!(body["scanned"], true);
        assert!(
            body["completed"]
                .as_array()
                .expect("completed")
                .iter()
                .any(|id| id == "secrets"),
            "cheap detectors must still finish: {}",
            body["completed"]
        );
        assert!(
            !body["completed"]
                .as_array()
                .expect("completed")
                .iter()
                .any(|id| id == "clippy"),
            "clippy is Cost::Tool and must stay off the request path: {}",
            body["completed"]
        );
    }

    #[test]
    fn measure_devboule_v2_cheap_scan() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("devboule-v2 root");
        let collected = collect_workspace_files(repo).expect("walk");
        let (survived, skipped) = survive_city_files(&collected.root, &collected.paths);
        let files: Vec<PathBuf> = survived.iter().map(|file| file.absolute.clone()).collect();
        let ctx = Context::new(&collected.root, &files);
        let started = Instant::now();
        let review = Registry::builtin().review(&ctx, Cost::Cheap);
        let elapsed = started.elapsed();
        println!(
            "devboule-v2 cheap scan: {} ms, {} walked, {} survived, {} skipped, {} findings, completed={:?}, failed={:?}",
            elapsed.as_millis(),
            collected.paths.len(),
            survived.len(),
            skipped,
            review.findings.len(),
            review.completed,
            review
                .failed
                .iter()
                .map(|failed| failed.detector)
                .collect::<Vec<_>>(),
        );
        assert!(
            !review.completed.contains(&"clippy"),
            "clippy must stay off the cheap path"
        );
        assert!(review.completed.contains(&"secrets"));
        assert!(review.completed.contains(&"untested"));
    }

    #[test]
    fn cheap_scan_does_not_run_clippy() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "src/lib.rs", "pub fn x() {}\n");
        let body = get_findings(temp.path()).expect("scan");
        let completed = body["completed"].as_array().expect("completed");
        assert!(completed.iter().any(|id| id == "secrets"));
        assert!(completed.iter().any(|id| id == "untested"));
        assert!(
            !completed.iter().any(|id| id == "clippy"),
            "clippy ran on findings.get: {completed:?}"
        );
    }
}
