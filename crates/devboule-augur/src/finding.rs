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

    /// Port of v1 `compute_sin_id`, with two corrections the v1 never needed:
    /// evidence normalisation is per-rule (secret values are not excerpts),
    /// and fields are length-prefixed so a `0x1f` inside a field cannot forge
    /// a neighbour. `occurrence` distinguishes two copies of the same secret
    /// in one file without putting the line back into the hash.
    pub fn of(rule: &str, file: &Path, line: Option<usize>, evidence: &str) -> Self {
        Self::of_at(rule, file, line, evidence, 0)
    }

    pub fn of_at(
        rule: &str,
        file: &Path,
        line: Option<usize>,
        evidence: &str,
        occurrence: u32,
    ) -> Self {
        let line_token = if line_is_decorative(rule) {
            String::new()
        } else {
            line.map(|n| n.to_string()).unwrap_or_default()
        };
        let mut hasher = Sha256::new();
        put(&mut hasher, normalise_path(file).as_bytes());
        put(&mut hasher, rule.as_bytes());
        put(&mut hasher, line_token.as_bytes());
        put(&mut hasher, evidence_key(rule, evidence).as_bytes());
        put(&mut hasher, &occurrence.to_be_bytes());
        let digest = hasher.finalize();
        FindingId(
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        )
    }
}

fn put(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
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
    let mut parts: Vec<String> = Vec::new();
    for component in file.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::Prefix(prefix) => {
                parts.push(prefix.as_os_str().to_string_lossy().to_lowercase());
            }
            other => parts.push(other.as_os_str().to_string_lossy().into_owned()),
        }
    }
    parts.join("/")
}

/// Relativise `file` against `root` and collapse `.` / `..` / separators /
/// drive-letter case. Used by [`Finding::grounded`] so identity and the
/// stored path agree.
pub(crate) fn canonical_file(root: &Path, file: &Path) -> PathBuf {
    let joined = if file.is_absolute() {
        file.to_path_buf()
    } else {
        root.join(file)
    };
    let joined_norm = PathBuf::from(normalise_path(&joined));
    let root_norm = PathBuf::from(normalise_path(root));
    joined_norm
        .strip_prefix(&root_norm)
        .map(Path::to_path_buf)
        .unwrap_or(joined_norm)
}

/// Evidence normalisation is per-rule, at the same choke point as the line.
/// The v1 folded case and whitespace because evidence was a source excerpt.
/// A secret *is* the matched value: case is identity, not cosmetics.
fn evidence_key(rule: &str, evidence: &str) -> String {
    if evidence_is_verbatim(rule) {
        evidence.to_string()
    } else {
        fold_excerpt(evidence)
    }
}

fn evidence_is_verbatim(rule: &str) -> bool {
    rule == "secret" || rule.starts_with("secret.") || crate::ruleset::is_gitleaks_rule(rule)
}

fn fold_excerpt(evidence: &str) -> String {
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
    /// Among equal (rule, evidence) matches in this file, counting from 0.
    /// Line is not this number; it is display-only for decorative-line rules.
    pub occurrence: u32,
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
            title: outbound_text(&title),
            evidence,
        })
    }
}

impl Finding {
    /// Build a finding only if `file` exists and the line range is inside it.
    pub fn grounded(root: &Path, draft: Draft<'_>) -> Option<Self> {
        let absolute = if draft.file.is_absolute() {
            draft.file.clone()
        } else {
            root.join(&draft.file)
        };
        let contents = read_source(&absolute)?;
        Self::grounded_on(root, draft, &contents)
    }

    /// Ground against text already in memory so a detector that has the file
    /// does not re-read it once per match.
    pub(crate) fn grounded_on(root: &Path, draft: Draft<'_>, contents: &str) -> Option<Self> {
        if draft.start_line == 0 || draft.end_line < draft.start_line {
            return None;
        }
        let lines = contents.lines().count();
        if draft.end_line > lines {
            return None;
        }
        let file = canonical_file(root, &draft.file);
        Some(Self {
            id: FindingId::of_at(
                draft.rule,
                &file,
                Some(draft.start_line),
                draft.raw_excerpt,
                draft.occurrence,
            ),
            file,
            start_line: draft.start_line,
            end_line: draft.end_line,
            rule: draft.rule.to_string(),
            severity: draft.severity,
            source: draft.source.to_string(),
            title: outbound_text(draft.title),
            evidence: evidence_for(draft.rule, draft.raw_excerpt),
        })
    }
}

/// Anything that leaves a Finding string field or an Error goes through here.
pub(crate) fn outbound_text(text: &str) -> String {
    oracle_core::redact_secret_tokens(text)
}

const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;

pub(crate) fn read_source(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
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
            occurrence: 0,
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
    fn cosmetic_excerpt_does_not_change_the_id() {
        let file = Path::new("src/a.rs");
        assert_eq!(
            FindingId::of(
                "complexity",
                file,
                Some(1),
                "fn foo exceeds the cyclomatic threshold"
            ),
            FindingId::of(
                "complexity",
                file,
                Some(1),
                "FN FOO EXCEEDS THE CYCLOMATIC THRESHOLD  "
            ),
            "source excerpts still fold case and whitespace"
        );
    }

    #[test]
    fn a_secret_that_differs_only_in_case_is_a_different_finding() {
        let file = Path::new("src/auth.rs");
        let upper = "GhP_caseSensitiveTokenBody00000000001";
        let lower = "ghp_casesensitivetokenbody00000000001";
        assert_ne!(
            FindingId::of("secret", file, Some(1), upper),
            FindingId::of("secret", file, Some(1), lower),
            "secret values are case-sensitive; excerpt normalisation must not apply"
        );
    }

    #[test]
    fn two_occurrences_are_told_apart_by_ordinal_not_line() {
        let file = Path::new("src/auth.rs");
        let token = "same-token-twice";
        assert_ne!(
            FindingId::of_at("secret", file, Some(1), token, 0),
            FindingId::of_at("secret", file, Some(4), token, 1),
        );
        assert_eq!(
            FindingId::of_at("secret", file, Some(1), token, 0),
            FindingId::of_at("secret", file, Some(9), token, 0),
            "an edit above must not mint a new id for the same occurrence"
        );
    }

    #[test]
    fn relative_and_dot_paths_hash_the_same() {
        let evidence = "fn foo exceeds the cyclomatic threshold";
        assert_eq!(
            FindingId::of("complexity", Path::new("src/a.rs"), Some(1), evidence),
            FindingId::of("complexity", Path::new("./src/a.rs"), Some(1), evidence),
        );
    }

    #[test]
    fn grounded_paths_are_relative_to_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let excerpt = tokens::aws_example();
        std::fs::write(
            temp.path().join("auth.rs"),
            format!("const K: &str = \"{excerpt}\";\n"),
        )
        .expect("write");
        let relative =
            Finding::grounded(temp.path(), draft(PathBuf::from("auth.rs"), 1, 1, &excerpt))
                .expect("relative");
        let absolute = Finding::grounded(
            temp.path(),
            draft(temp.path().join("auth.rs"), 1, 1, &excerpt),
        )
        .expect("absolute");
        assert_eq!(relative.id(), absolute.id());
        assert_eq!(relative.file(), Path::new("auth.rs"));
        assert_eq!(absolute.file(), Path::new("auth.rs"));
    }

    #[test]
    fn a_title_is_redacted_before_it_leaves() {
        let temp = tempfile::tempdir().expect("tempdir");
        let excerpt = tokens::aws_access_token();
        std::fs::write(temp.path().join("a.rs"), "let x = 1;\n").expect("write");
        let title = format!("unused variable looks like {excerpt}");
        let finding = Finding::grounded(
            temp.path(),
            Draft {
                file: PathBuf::from("a.rs"),
                start_line: 1,
                end_line: 1,
                rule: "unused_variables",
                severity: Severity::Fire,
                source: "clippy",
                title: &title,
                raw_excerpt: "let x = 1;",
                occurrence: 0,
            },
        )
        .expect("grounded");
        assert!(
            !finding.title().contains(&excerpt),
            "title leaked a secret: {}",
            finding.title()
        );
    }

    #[test]
    fn the_unit_separator_stops_field_collisions() {
        let a = FindingId::of("c", Path::new("ab"), Some(1), "d");
        let b = FindingId::of("bc", Path::new("a"), Some(1), "d");
        assert_ne!(a, b);
    }

    #[test]
    fn a_unit_separator_inside_a_field_does_not_forge_another_id() {
        let a = FindingId::of("b\x1fc", Path::new("a"), Some(1), "d");
        let b = FindingId::of("c", Path::new("a\x1fb"), Some(1), "d");
        assert_ne!(
            a, b,
            "0x1f inside a field must not rewrite the field boundaries"
        );
    }
}
