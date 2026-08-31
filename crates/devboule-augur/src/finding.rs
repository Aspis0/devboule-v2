//! One finding, one shape. Everything Augur reports looks like this.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Identity of a finding. Computed only here, so records, suppression and
/// tests cannot disagree.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FindingId(String);

impl FindingId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn from_stored(value: String) -> Option<Self> {
        if value.is_empty() {
            None
        } else {
            Some(Self(value))
        }
    }

    /// Port of v1 `compute_sin_id`: `path ⟂ rule ⟂ line ⟂ evidence_key`.
    ///
    /// The line is dropped for rules whose evidence already carries the
    /// stable anchor (secrets, complexity, clone, hardcoded locations).
    /// Enforced here so a caller cannot accidentally put the line back.
    pub fn of(rule: &str, file: &Path, line: Option<usize>, evidence: &str) -> Self {
        let line_token = if line_is_decorative(rule) {
            String::new()
        } else {
            line.map(|n| n.to_string()).unwrap_or_default()
        };
        let mut hasher = Sha256::new();
        hasher.update(normalise_path(file).as_bytes());
        hasher.update([0x1f]);
        hasher.update(rule.as_bytes());
        hasher.update([0x1f]);
        hasher.update(line_token.as_bytes());
        hasher.update([0x1f]);
        hasher.update(evidence_key(evidence).as_bytes());
        let digest = hasher.finalize();
        FindingId(
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        )
    }
}

/// IDENTITY POLICY: for some rules the line is decorative — any edit above
/// the flagged item would shift it and mint a new id, silently dropping
/// dismissals. The evidence already carries the stable anchor (matched
/// secret, function name, partner path, hardcoded URI). Clippy lints keep
/// the line: two unused variables of the same shape on different lines are
/// different findings. Enforced here, the single choke point.
fn line_is_decorative(rule: &str) -> bool {
    matches!(
        rule,
        "complexity"
            | "clone"
            | "secret"
            | "path.absolute"
            | "path.unix-absolute"
            | "url.localhost"
    ) || rule.starts_with("secret.")
        || crate::ruleset::is_gitleaks_rule(rule)
}

fn normalise_path(file: &Path) -> String {
    file.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Lowercased, whitespace runs collapsed. Cosmetic rewrites do not mint an id.
pub fn evidence_key(evidence: &str) -> String {
    let lower = evidence.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut in_space = false;
    for character in lower.chars() {
        if character.is_whitespace() {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(character);
            in_space = false;
        }
    }
    out.trim().to_string()
}

impl std::fmt::Display for FindingId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Maps to the fire bands the city draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Smoke,
    Fire,
    Inferno,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Smoke => "smoke",
            Self::Fire => "fire",
            Self::Inferno => "inferno",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "smoke" => Some(Self::Smoke),
            "fire" => Some(Self::Fire),
            "inferno" => Some(Self::Inferno),
            _ => None,
        }
    }
}

/// Enough to try to ground a finding. Split out of [`Finding::grounded`] so
/// that function is not a nine-argument list.
pub struct Draft<'a> {
    pub file: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
    pub rule: &'a str,
    pub severity: Severity,
    pub source: &'a str,
    pub title: &'a str,
    pub raw_excerpt: &'a str,
}

/// One problem in a real file, at a real line range.
///
/// Fields are private so callers and storage do not freeze this layout.
/// Interchange lives in [`crate::exchange`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    id: FindingId,
    file: PathBuf,
    start_line: usize,
    end_line: usize,
    rule: String,
    severity: Severity,
    source: String,
    title: String,
    evidence: String,
}

impl Finding {
    pub fn id(&self) -> &FindingId {
        &self.id
    }
    pub fn file(&self) -> &Path {
        &self.file
    }
    pub fn start_line(&self) -> usize {
        self.start_line
    }
    pub fn end_line(&self) -> usize {
        self.end_line
    }
    pub fn rule(&self) -> &str {
        &self.rule
    }
    pub fn severity(&self) -> Severity {
        self.severity
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn evidence(&self) -> &str {
        &self.evidence
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_snapshot(
        id: FindingId,
        file: PathBuf,
        start_line: usize,
        end_line: usize,
        rule: String,
        severity: Severity,
        source: String,
        title: String,
        evidence: String,
    ) -> Option<Self> {
        Some(Self {
            id,
            file,
            start_line,
            end_line,
            rule,
            severity,
            source,
            title,
            evidence,
        })
    }
}

impl Finding {
    /// Build a finding only if `file` exists and the line range is inside it.
    pub fn grounded(root: &Path, draft: Draft<'_>) -> Option<Self> {
        if draft.start_line == 0 || draft.end_line < draft.start_line {
            return None;
        }
        let absolute = if draft.file.is_absolute() {
            draft.file.clone()
        } else {
            root.join(&draft.file)
        };
        let contents = std::fs::read_to_string(&absolute).ok()?;
        let lines = contents.lines().count();
        if draft.end_line > lines {
            return None;
        }
        Some(Self {
            id: FindingId::of(
                draft.rule,
                &draft.file,
                Some(draft.start_line),
                draft.raw_excerpt,
            ),
            file: draft.file,
            start_line: draft.start_line,
            end_line: draft.end_line,
            rule: draft.rule.to_string(),
            severity: draft.severity,
            source: draft.source.to_string(),
            title: draft.title.to_string(),
            evidence: evidence_for(draft.rule, draft.raw_excerpt),
        })
    }
}

/// Gitleaks excerpts *are* the secret. oracle-core redacts retrieval-shaped
/// spans (`api_key = …`, `AKIA…`); a captured value like `sk-…T3BlbkFJ…`
/// would pass through. Secret rules always store the marker.
fn evidence_for(rule: &str, raw: &str) -> String {
    if rule == "secret" || rule.starts_with("secret.") || crate::ruleset::is_gitleaks_rule(rule) {
        let newlines = raw.bytes().filter(|byte| *byte == b'\n').count();
        let mut marker = String::from("[redacted-secret]");
        marker.extend(std::iter::repeat_n('\n', newlines));
        marker
    } else {
        oracle_core::redact_secret_tokens(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens;

    #[test]
    fn the_id_does_not_depend_on_line_numbers() {
        let file = Path::new("src/auth.rs");
        let excerpt = tokens::aws_example();
        let at_line_10 = FindingId::of("secret", file, Some(10), &excerpt);
        let at_line_40 = FindingId::of("secret", file, Some(40), &excerpt);
        assert_eq!(
            at_line_10, at_line_40,
            "the same secret three lines down must keep its id"
        );
    }

    #[test]
    fn the_id_changes_when_the_matched_content_changes() {
        let file = Path::new("src/auth.rs");
        let before = tokens::aws_access_token();
        let after = tokens::aws_other();
        assert_ne!(
            FindingId::of("secret", file, Some(1), &before),
            FindingId::of("secret", file, Some(1), &after),
            "a different secret must not keep the dismissed id"
        );
    }

    fn draft<'a>(file: PathBuf, start_line: usize, end_line: usize, excerpt: &'a str) -> Draft<'a> {
        Draft {
            file,
            start_line,
            end_line,
            rule: "secret",
            severity: Severity::Inferno,
            source: "secrets",
            title: "A secret",
            raw_excerpt: excerpt,
        }
    }

    #[test]
    fn a_finding_without_a_real_file_is_not_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let excerpt = tokens::aws_example();
        assert!(
            Finding::grounded(
                temp.path(),
                draft(PathBuf::from("no/such.rs"), 1, 1, &excerpt),
            )
            .is_none(),
            "invented files must not become fires"
        );
    }

    #[test]
    fn a_finding_past_the_end_of_the_file_is_not_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = temp.path().join("short.rs");
        std::fs::write(&file, "fn main() {}\n").expect("write");
        let excerpt = tokens::aws_example();
        assert!(
            Finding::grounded(
                temp.path(),
                draft(PathBuf::from("short.rs"), 20, 20, &excerpt),
            )
            .is_none(),
            "line 20 of a 1-line file is invented"
        );
    }

    #[test]
    fn evidence_does_not_quote_the_secret() {
        let temp = tempfile::tempdir().expect("tempdir");
        let excerpt = tokens::aws_example();
        let file = temp.path().join("auth.rs");
        std::fs::write(&file, format!("const K: &str = \"{excerpt}\";\n")).expect("write");
        let finding =
            Finding::grounded(temp.path(), draft(PathBuf::from("auth.rs"), 1, 1, &excerpt))
                .expect("real file, real line");
        assert!(
            !finding.evidence().contains(&excerpt),
            "the evidence field wrote the secret down: {}",
            finding.evidence()
        );
        assert!(
            finding.evidence().contains("[redacted-secret]"),
            "redaction marker missing: {}",
            finding.evidence()
        );
    }

    #[test]
    fn complexity_ignores_the_line_because_the_fn_name_is_the_anchor() {
        let file = Path::new("src/a.rs");
        let evidence = "fn foo exceeds the cyclomatic threshold";
        assert_eq!(
            FindingId::of("complexity", file, Some(10), evidence),
            FindingId::of("complexity", file, Some(50), evidence),
        );
        assert_ne!(
            FindingId::of("complexity", file, Some(10), evidence),
            FindingId::of(
                "complexity",
                file,
                Some(10),
                "fn bar exceeds the cyclomatic threshold"
            ),
        );
    }

    #[test]
    fn clippy_keeps_the_line_because_two_identical_lints_are_different_findings() {
        let file = Path::new("src/lib.rs");
        let evidence = "unused variable: `x`";
        assert_ne!(
            FindingId::of("unused_variables", file, Some(1), evidence),
            FindingId::of("unused_variables", file, Some(40), evidence),
            "a clippy lint that moves is a different finding"
        );
    }

    #[test]
    fn cosmetic_evidence_does_not_change_the_id() {
        let file = Path::new("src/a.rs");
        assert_eq!(
            FindingId::of("secret", file, Some(1), "secret at line 1"),
            FindingId::of("secret", file, Some(1), "SECRET AT LINE 1  "),
        );
    }

    #[test]
    fn the_unit_separator_stops_field_collisions() {
        let a = FindingId::of("c", Path::new("ab"), Some(1), "d");
        let b = FindingId::of("bc", Path::new("a"), Some(1), "d");
        assert_ne!(a, b);
    }
}
