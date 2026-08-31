//! SARIF 2.1.0 at the crate boundary.
//!
//! Findings leave this crate as a SARIF log. The ledger does not store SARIF;
//! it stores an opaque payload produced here so the on-disk shape can change
//! without detectors noticing.

use serde_sarif::sarif::Sarif;

use crate::finding::{Finding, FindingId, Severity};

#[derive(serde::Serialize, serde::Deserialize)]
struct LedgerPayload {
    fingerprint: String,
    uri: String,
    #[serde(rename = "startLine")]
    start_line: usize,
    #[serde(rename = "endLine")]
    end_line: usize,
    #[serde(rename = "ruleId")]
    rule_id: String,
    level: String,
    message: String,
    snippet: String,
    tool: String,
}

pub fn encode_ledger(finding: &Finding) -> String {
    let payload = LedgerPayload {
        fingerprint: finding.id().as_str().to_string(),
        uri: finding.file().to_string_lossy().into_owned(),
        start_line: finding.start_line(),
        end_line: finding.end_line(),
        rule_id: finding.rule().to_string(),
        level: sarif_level(finding.severity()).to_string(),
        message: finding.title().to_string(),
        snippet: finding.evidence().to_string(),
        tool: finding.source().to_string(),
    };
    serde_json::to_string(&payload).expect("payload is always valid JSON")
}

pub fn decode_ledger(json: &str) -> Option<Finding> {
    let payload: LedgerPayload = serde_json::from_str(json).ok()?;
    Finding::from_snapshot(
        FindingId::from_stored(payload.fingerprint)?,
        std::path::PathBuf::from(payload.uri),
        payload.start_line,
        payload.end_line,
        payload.rule_id,
        from_sarif_level(&payload.level)?,
        payload.tool,
        payload.message,
        payload.snippet,
    )
}

/// Findings as a SARIF 2.1.0 log. This is what leaves the crate.
pub fn to_sarif(findings: &[Finding]) -> Sarif {
    let results: Vec<serde_json::Value> = findings
        .iter()
        .map(|finding| {
            serde_json::json!({
                "ruleId": finding.rule(),
                "level": sarif_level(finding.severity()),
                "message": { "text": finding.title() },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": finding.file().to_string_lossy() },
                        "region": {
                            "startLine": finding.start_line(),
                            "endLine": finding.end_line()
                        }
                    }
                }],
                "partialFingerprints": {
                    "devboule/v1": finding.id().as_str()
                }
            })
        })
        .collect();
    let document = serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": { "name": "devboule-augur" }
            },
            "results": results
        }]
    });
    serde_json::from_value(document).expect("this document is SARIF 2.1.0")
}

fn sarif_level(severity: Severity) -> &'static str {
    match severity {
        Severity::Smoke => "note",
        Severity::Fire => "warning",
        Severity::Inferno => "error",
    }
}

fn from_sarif_level(level: &str) -> Option<Severity> {
    match level {
        "note" => Some(Severity::Smoke),
        "warning" => Some(Severity::Fire),
        "error" => Some(Severity::Inferno),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finding::Draft;
    use std::path::PathBuf;

    #[test]
    fn findings_leave_as_sarif_with_partial_fingerprints() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("a.rs"), "let x = 1;\n").expect("write");
        let finding = Finding::grounded(
            temp.path(),
            Draft {
                file: PathBuf::from("a.rs"),
                start_line: 1,
                end_line: 1,
                rule: "unused_variables",
                severity: Severity::Fire,
                source: "clippy",
                title: "unused variable: `x`",
                raw_excerpt: "let x = 1;",
                occurrence: 0,
            },
        )
        .expect("grounded");
        let log = to_sarif(std::slice::from_ref(&finding));
        let json = serde_json::to_value(&log).expect("json");
        assert_eq!(json["version"], "2.1.0");
        let fingerprint = json["runs"][0]["results"][0]["partialFingerprints"]["devboule/v1"]
            .as_str()
            .expect("fingerprint");
        assert_eq!(fingerprint, finding.id().as_str());
    }

    #[test]
    fn the_ledger_payload_does_not_use_finding_field_names() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("a.rs"), "let x = 1;\n").expect("write");
        let finding = Finding::grounded(
            temp.path(),
            Draft {
                file: PathBuf::from("a.rs"),
                start_line: 1,
                end_line: 1,
                rule: "demo",
                severity: Severity::Fire,
                source: "test",
                title: "demo",
                raw_excerpt: "let x = 1;",
                occurrence: 0,
            },
        )
        .expect("grounded");
        let json = encode_ledger(&finding);
        assert!(json.contains("\"startLine\"") && json.contains("\"ruleId\""));
        assert!(!json.contains("\"start_line\"") && !json.contains("\"raw_excerpt\""));
        let back = decode_ledger(&json).expect("decode");
        assert_eq!(back.id(), finding.id());
    }
}
