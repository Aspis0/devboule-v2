//! Cheap Augur scan, invoked because the surface opened.
//!
//! `findings.get` ships id/file/severity/rule/title. `finding.inspect` ships
//! metadata and line ranges only — never file content, never a snippet.
//! The ledger stores "[redacted-secret]" for secrets; re-reading the file
//! to mask a line would trust a smaller redaction set than the detector.

use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use devboule_augur::{Context, Cost, Finding, FindingId, Ledger, Registry};
use oracle_core::{collect_text_files_with_cancel_limits_report, CancelFlag, OracleDataPaths};

use crate::city::{MAX_CITY_FILES, MAX_CITY_FILE_BYTES};

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

#[derive(Debug)]
pub enum InspectError {
    InvalidId,
    NotFound,
    Ledger(String),
}

impl Display for InspectError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId => f.write_str("finding.inspect requires a 64-hex id"),
            Self::NotFound => f.write_str("finding not found"),
            Self::Ledger(message) => write!(f, "findings ledger: {message}"),
        }
    }
}

impl std::error::Error for InspectError {}

const INSPECT_ID_LEN: usize = 64;

fn is_64_hex(value: &str) -> bool {
    value.len() == INSPECT_ID_LEN && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn inspect_id_from_payload(payload: Option<&serde_json::Value>) -> Result<FindingId, InspectError> {
    let Some(id) = payload
        .and_then(|value| value.get("id"))
        .and_then(serde_json::Value::as_str)
    else {
        return Err(InspectError::InvalidId);
    };
    if !is_64_hex(id) {
        return Err(InspectError::InvalidId);
    }
    FindingId::from_stored(id.to_string()).ok_or(InspectError::InvalidId)
}

/// Indexed lookup. No walk, no scan, no file bytes.
pub fn inspect_finding(
    root: &Path,
    payload: Option<&serde_json::Value>,
) -> Result<serde_json::Value, InspectError> {
    let id = inspect_id_from_payload(payload)?;
    let root = fs::canonicalize(root).map_err(|error| InspectError::Ledger(error.to_string()))?;
    let ledger_path = OracleDataPaths::from_root_without_env(&root).augur_ledger();
    let ledger =
        Ledger::open(&ledger_path).map_err(|error| InspectError::Ledger(error.to_string()))?;
    let Some(finding) = ledger
        .get(&id)
        .map_err(|error| InspectError::Ledger(error.to_string()))?
    else {
        return Err(InspectError::NotFound);
    };
    Ok(inspect_body(&finding))
}

fn inspect_body(finding: &Finding) -> serde_json::Value {
    let locations: Vec<serde_json::Value> = finding
        .locations()
        .iter()
        .map(|location| {
            serde_json::json!({
                "startLine": location.start_line(),
                "endLine": location.end_line(),
            })
        })
        .collect();
    serde_json::json!({
        "id": finding.id().as_str(),
        "rule": finding.rule(),
        "severity": finding.severity().as_str(),
        "title": finding.title(),
        "source": finding.source(),
        "startLine": finding.start_line(),
        "endLine": finding.end_line(),
        "locations": locations,
    })
}

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
    // scanMs is the host-perceived wait: walk + survive + review + ledger.
    let started = Instant::now();
    let collected = collect_workspace_files(root)?;
    scan_collected_files(&collected, started)
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
    started: Instant,
) -> Result<serde_json::Value, FindingsError> {
    let (survived, skipped_files) = survive_city_files(&collected.root, &collected.paths);
    let index = FileIdIndex::from_survived(&survived);
    let files: Vec<PathBuf> = survived.iter().map(|file| file.absolute.clone()).collect();
    let ctx = Context::new(&collected.root, &files);
    let review = Registry::builtin().review(&ctx, Cost::Cheap);

    let ledger_path = OracleDataPaths::from_root_without_env(&collected.root).augur_ledger();
    let ledger =
        Ledger::open(&ledger_path).map_err(|error| FindingsError::Ledger(error.to_string()))?;
    ledger
        .record_scan(&review.findings, &review.completed, &review.registered)
        .map_err(|error| FindingsError::Ledger(error.to_string()))?;
    let active = ledger
        .active()
        .map_err(|error| FindingsError::Ledger(error.to_string()))?;

    let (findings, dropped_findings) = attach_findings(&active, &index);
    let failed: Vec<&str> = review.failed.iter().map(|failed| failed.detector).collect();
    let scan_ms = started.elapsed().as_millis() as u64;
    Ok(pack_findings_response(
        findings,
        dropped_findings,
        &review.completed,
        &failed,
        scan_ms,
        skipped_files,
        collected.truncated,
    ))
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
    by_folded: HashMap<String, FoldedTarget>,
}

enum FoldedTarget {
    Unique(String),
    Ambiguous,
}

impl FileIdIndex {
    fn from_survived(files: &[SurvivedFile]) -> Self {
        let mut by_exact = HashMap::new();
        let mut by_folded: HashMap<String, FoldedTarget> = HashMap::new();
        for file in files {
            by_exact.insert(file.id.clone(), file.id.clone());
            let folded = fold_file_id(&file.id);
            match by_folded.get(&folded) {
                None => {
                    by_folded.insert(folded, FoldedTarget::Unique(file.id.clone()));
                }
                Some(FoldedTarget::Unique(existing)) if existing == &file.id => {}
                Some(_) => {
                    by_folded.insert(folded, FoldedTarget::Ambiguous);
                }
            }
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
        match self.by_folded.get(&fold_file_id(&posix)) {
            Some(FoldedTarget::Unique(id)) => Some(id.clone()),
            Some(FoldedTarget::Ambiguous) | None => None,
        }
    }
}

fn fold_file_id(id: &str) -> String {
    if cfg!(windows) {
        id.to_lowercase()
    } else {
        id.to_string()
    }
}

fn attach_findings(findings: &[Finding], index: &FileIdIndex) -> (Vec<serde_json::Value>, usize) {
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

fn severity_rank(severity: &str) -> u8 {
    match severity {
        "inferno" => 0,
        "fire" => 1,
        "smoke" => 2,
        _ => 3,
    }
}

/// Keep the wire list inside the 1 MiB−4 KiB frame. Sort inferno > fire >
/// smoke, then fileId, then id; omit the tail and say how many were cut.
fn pack_findings_response(
    mut findings: Vec<serde_json::Value>,
    dropped_findings: usize,
    completed: &[&str],
    failed: &[&str],
    scan_ms: u64,
    skipped_files: usize,
    walk_truncated: bool,
) -> serde_json::Value {
    findings.sort_by(|left, right| {
        let left_rank = severity_rank(left["severity"].as_str().unwrap_or(""));
        let right_rank = severity_rank(right["severity"].as_str().unwrap_or(""));
        left_rank
            .cmp(&right_rank)
            .then_with(|| {
                left["fileId"]
                    .as_str()
                    .unwrap_or("")
                    .cmp(right["fileId"].as_str().unwrap_or(""))
            })
            .then_with(|| {
                left["id"]
                    .as_str()
                    .unwrap_or("")
                    .cmp(right["id"].as_str().unwrap_or(""))
            })
    });
    let context = FindingsBodyContext {
        dropped_findings,
        completed,
        failed,
        scan_ms,
        skipped_files,
        walk_truncated,
    };
    let original = findings.len();
    let mut lo = 0usize;
    let mut hi = original;
    let mut best = 0usize;
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let body = findings_body(&findings[..mid], original - mid, &context);
        if crate::city_response_within_frame(&body) {
            best = mid;
            if mid == original {
                break;
            }
            lo = mid.saturating_add(1);
            if lo > hi {
                break;
            }
        } else if mid == 0 {
            break;
        } else {
            hi = mid - 1;
        }
    }
    findings_body(&findings[..best], original - best, &context)
}

struct FindingsBodyContext<'a> {
    dropped_findings: usize,
    completed: &'a [&'a str],
    failed: &'a [&'a str],
    scan_ms: u64,
    skipped_files: usize,
    walk_truncated: bool,
}

fn findings_body(
    findings: &[serde_json::Value],
    truncated_findings: usize,
    context: &FindingsBodyContext<'_>,
) -> serde_json::Value {
    // scanMs is walk+survive+scan+ledger — the wait the host budget spends.
    let mut body = serde_json::json!({
        "findings": findings,
        "scanned": true,
        "completed": context.completed,
        "failed": context.failed,
        "scanMs": context.scan_ms,
        "droppedFindings": context.dropped_findings,
    });
    if truncated_findings != 0 {
        body["truncatedFindings"] = serde_json::json!(truncated_findings);
    }
    if context.skipped_files != 0 {
        body["skippedFiles"] = serde_json::json!(context.skipped_files);
    }
    if context.walk_truncated {
        // Lower bound (0/1), not a repository-wide omitted count.
        body["truncatedFiles"] = serde_json::json!(1);
    }
    body
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
        let body = scan_collected_files(&collected, Instant::now()).expect("scan");
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
        let body = scan_collected_files(&collected, Instant::now()).expect("review must complete");
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

    #[test]
    fn an_ambiguous_folded_file_id_is_dropped_not_guessed() {
        let index = FileIdIndex::from_ids(&["Src/Auth.rs".to_string(), "src/auth.rs".to_string()]);
        assert_eq!(
            index.reconcile(Path::new("src/auth.rs")).as_deref(),
            Some("src/auth.rs"),
            "exact match still wins"
        );
        assert_eq!(
            index.reconcile(Path::new("Src/Auth.rs")).as_deref(),
            Some("Src/Auth.rs"),
            "exact match still wins"
        );
        #[cfg(windows)]
        {
            assert!(
                index.reconcile(Path::new("SRC/AUTH.RS")).is_none(),
                "a folded-only match that is ambiguous must not pick a building"
            );
        }
    }

    #[test]
    fn a_synthetic_flood_is_capped_and_never_blows_the_frame() {
        let findings: Vec<serde_json::Value> = (0..8_000)
            .map(|index| {
                let severity = match index % 3 {
                    0 => "inferno",
                    1 => "fire",
                    _ => "smoke",
                };
                serde_json::json!({
                    "id": format!("{index:064}"),
                    "fileId": format!("src/f{index:04}.rs"),
                    "severity": severity,
                    "rule": "test.missing",
                    "title": "No test file beside this one",
                })
            })
            .collect();
        let body = pack_findings_response(findings, 0, &["secrets", "untested"], &[], 12, 0, false);
        assert!(
            crate::city_response_within_frame(&body),
            "capped findings.get must fit the frame: {} bytes",
            serde_json::to_vec(&body)
                .map(|bytes| bytes.len())
                .unwrap_or(0)
        );
        let kept = body["findings"].as_array().expect("findings").len();
        assert!(
            kept < 8_000,
            "the cap must omit some of the flood: kept={kept}"
        );
        let omitted = body["truncatedFindings"]
            .as_u64()
            .expect("truncatedFindings");
        assert_eq!(omitted, 8_000 - kept as u64);
        let first = body["findings"][0]["severity"].as_str().unwrap();
        assert_eq!(
            first, "inferno",
            "severity desc: inferno first, got {first}"
        );
    }

    fn inspect_err(root: &Path, payload: Option<serde_json::Value>) -> InspectError {
        inspect_finding(root, payload.as_ref()).expect_err("must refuse")
    }

    #[test]
    fn inspect_rejects_missing_and_malformed_ids() {
        let temp = tempfile::tempdir().unwrap();
        let cases: Vec<Option<serde_json::Value>> = vec![
            None,
            Some(serde_json::json!({})),
            Some(serde_json::json!({"id": ""})),
            Some(serde_json::json!({"id": "short"})),
            Some(serde_json::json!({"id": "g".repeat(64)})),
            Some(serde_json::json!({"id": 12})),
            Some(serde_json::json!({"id": null})),
        ];
        for payload in cases {
            match inspect_err(temp.path(), payload.clone()) {
                InspectError::InvalidId => {}
                other => panic!("expected InvalidId for {payload:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn inspect_unknown_id_is_not_found() {
        let temp = tempfile::tempdir().unwrap();
        let id = "a".repeat(64);
        match inspect_err(temp.path(), Some(serde_json::json!({"id": id}))) {
            InspectError::NotFound => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
