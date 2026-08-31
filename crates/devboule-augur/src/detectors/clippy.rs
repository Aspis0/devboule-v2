//! Subprocess detector: `cargo clippy --message-format=json`.
//!
//! We do not reimplement any analysis. Clippy is the expert; we read what it
//! printed and turn diagnostics that resolve to a real file and line into
//! findings.

use crate::detector::{Context, Cost, Detector};
use crate::error::Error;
use crate::finding::Finding;

pub struct Clippy {
    /// How to invoke clippy. Tests replace this with a captured fixture.
    run: fn(&std::path::Path) -> Result<String, Error>,
}

impl Default for Clippy {
    fn default() -> Self {
        Self { run: run_clippy }
    }
}

impl Clippy {
    #[cfg(test)]
    pub fn from_output(run: fn(&std::path::Path) -> Result<String, Error>) -> Self {
        Self { run }
    }
}

fn run_clippy(root: &std::path::Path) -> Result<String, Error> {
    let output = std::process::Command::new("cargo")
        .args([
            "clippy",
            "--message-format=json",
            "--offline",
            "--all-targets",
            "--quiet",
        ])
        .current_dir(root)
        .output()
        .map_err(|error| Error::tool(format!("could not run clippy: {error}")))?;
    if output.stdout.is_empty() && !output.status.success() {
        return Err(Error::tool(format!(
            "clippy failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

impl Detector for Clippy {
    fn id(&self) -> &'static str {
        "clippy"
    }

    fn cost(&self) -> Cost {
        Cost::Tool
    }

    fn scan(&self, ctx: &Context<'_>) -> Result<Vec<Finding>, Error> {
        let jsonl = (self.run)(ctx.root)?;
        let mut findings = findings_from_json(ctx.root, &jsonl);
        if !ctx.files.is_empty() {
            findings.retain(|finding| {
                ctx.files
                    .iter()
                    .any(|asked| asked_this_file(asked, finding.file()))
            });
        }
        Ok(findings)
    }
}

fn asked_this_file(asked: &std::path::Path, found: &std::path::Path) -> bool {
    let asked: Vec<_> = asked.components().collect();
    let found: Vec<_> = found.components().collect();
    if asked.is_empty() || found.is_empty() {
        return false;
    }
    asked == found || asked.ends_with(&found) || found.ends_with(&asked)
}

/// Turn cargo/clippy JSON lines into findings that resolve to a real file.
pub fn findings_from_json(root: &std::path::Path, jsonl: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    for line in jsonl.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(envelope) = serde_json::from_str::<Envelope>(trimmed) else {
            continue;
        };
        if envelope.reason.as_deref() != Some("compiler-message") {
            continue;
        }
        let Some(message) = envelope.message else {
            continue;
        };
        if message.level != "warning" && message.level != "error" {
            continue;
        }
        let Some(span) = message.spans.iter().find(|span| span.is_primary) else {
            continue;
        };
        let relative = span.file_name.replace('\\', "/");
        let excerpt = span
            .text
            .first()
            .map(|text| text.text.as_str())
            .unwrap_or(message.message.as_str());
        let rule = message
            .code
            .as_ref()
            .map(|code| code.code.as_str())
            .unwrap_or("clippy");
        let severity = if message.level == "error" {
            crate::finding::Severity::Inferno
        } else {
            crate::finding::Severity::Fire
        };
        if let Some(finding) = Finding::grounded(
            root,
            crate::finding::Draft {
                file: std::path::PathBuf::from(&relative),
                start_line: span.line_start,
                end_line: span.line_end.max(span.line_start),
                rule,
                severity,
                source: "clippy",
                title: &message.message,
                raw_excerpt: excerpt,
                occurrence: 0,
            },
        ) {
            findings.push(finding);
        }
    }
    findings
}

#[derive(serde::Deserialize)]
struct Envelope {
    reason: Option<String>,
    message: Option<Diagnostic>,
}

#[derive(serde::Deserialize)]
struct Diagnostic {
    level: String,
    message: String,
    #[serde(default)]
    spans: Vec<Span>,
    code: Option<DiagnosticCode>,
}

#[derive(serde::Deserialize)]
struct DiagnosticCode {
    code: String,
}

#[derive(serde::Deserialize)]
struct Span {
    file_name: String,
    line_start: usize,
    line_end: usize,
    #[serde(default)]
    is_primary: bool,
    #[serde(default)]
    text: Vec<SpanText>,
}

#[derive(serde::Deserialize)]
struct SpanText {
    text: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detector::Context;
    use std::path::PathBuf;

    const FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/clippy-unused-variable.jsonl"
    ));

    #[test]
    fn asked_this_file_does_not_treat_a_suffix_string_as_the_same_path() {
        assert!(asked_this_file(
            std::path::Path::new("src/lib.rs"),
            std::path::Path::new("src/lib.rs")
        ));
        assert!(
            !asked_this_file(
                std::path::Path::new("src/lib.rs"),
                std::path::Path::new("src/notlib.rs")
            ),
            "string ends_with must not keep an unrelated file"
        );
        assert!(
            !asked_this_file(
                std::path::Path::new("lib.rs"),
                std::path::Path::new("src/notlib.rs")
            ),
            "lib.rs is not a component suffix of src/notlib.rs"
        );
    }

    #[test]
    fn a_clippy_warning_becomes_a_finding_on_the_real_line() {
        let temp = tempfile::tempdir().expect("tempdir");
        let src = temp.path().join("src");
        std::fs::create_dir_all(&src).expect("mkdir");
        std::fs::write(src.join("lib.rs"), "pub fn unused() { let x = 1; }\n").expect("write");
        let findings = findings_from_json(temp.path(), FIXTURE);
        assert_eq!(
            findings.len(),
            1,
            "expected one diagnostic, got {findings:?}"
        );
        assert_eq!(findings[0].rule(), "unused_variables");
        assert_eq!(findings[0].start_line(), 1);
        assert_eq!(findings[0].source(), "clippy");
        assert!(findings[0].title().contains("unused variable"));
    }

    #[test]
    fn a_clippy_span_without_a_file_on_disk_is_dropped() {
        let temp = tempfile::tempdir().expect("tempdir");
        let findings = findings_from_json(temp.path(), FIXTURE);
        assert!(
            findings.is_empty(),
            "a diagnostic for a missing file must not become a fire: {findings:?}"
        );
    }

    #[test]
    fn the_detector_reads_whatever_the_tool_printed() {
        fn fixture(_root: &std::path::Path) -> Result<String, Error> {
            Ok(FIXTURE.to_string())
        }
        let temp = tempfile::tempdir().expect("tempdir");
        let src = temp.path().join("src");
        std::fs::create_dir_all(&src).expect("mkdir");
        std::fs::write(src.join("lib.rs"), "pub fn unused() { let x = 1; }\n").expect("write");
        let files: [PathBuf; 0] = [];
        let ctx = Context::new(temp.path(), &files);
        let findings = Clippy::from_output(fixture).scan(&ctx).expect("scan");
        assert_eq!(findings.len(), 1);
    }
}
