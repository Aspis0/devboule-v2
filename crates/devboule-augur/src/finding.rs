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

    /// Port of v1 `compute_sin_id`. Evidence normalisation is per-rule.
    /// Fields are length-prefixed so a `0x1f` inside a field cannot forge
    /// a neighbour. Two byte-identical secrets in one file share this id:
    /// they are one finding with more than one location.
    pub fn of(rule: &str, file: &Path, line: Option<usize>, evidence: &str) -> Self {
        let line_token = if line_is_decorative(rule) {
            String::new()
        } else {
            line.map(|n| n.to_string()).unwrap_or_default()
        };
        let mut hasher = Sha256::new();
        put(&mut hasher, normalise_path(file).as_bytes());
        put(&mut hasher, rule_key(rule).as_bytes());
        put(&mut hasher, line_token.as_bytes());
        put(&mut hasher, evidence_key(rule, evidence).as_bytes());
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
fn rule_key(rule: &str) -> String {
    rule.to_ascii_lowercase()
}

fn line_is_decorative(rule: &str) -> bool {
    let rule = rule_key(rule);
    matches!(
        rule.as_str(),
        "complexity"
            | "clone"
            | "secret"
            | "path.absolute"
            | "path.unix-absolute"
            | "url.localhost"
    ) || rule.starts_with("secret.")
        || crate::ruleset::is_gitleaks_rule(&rule)
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
            other => {
                let part = other.as_os_str().to_string_lossy();
                parts.push(if cfg!(windows) {
                    part.to_lowercase()
                } else {
                    part.into_owned()
                });
            }
        }
    }
    parts.join("/")
}

/// Relativise `file` against an **absolute** `root`. Files outside the root
/// are rejected. On Windows the whole path is case-folded and verbatim
/// prefixes (`\\?\`) are stripped.
pub(crate) fn canonical_file(root: &Path, file: &Path) -> Option<PathBuf> {
    let root_abs = make_absolute(root)?;
    let joined = if file.is_absolute() {
        file.to_path_buf()
    } else {
        root_abs.join(file)
    };
    let joined_abs = make_absolute(&joined)?;
    let root_norm = fold_platform(&strip_verbatim(&root_abs));
    let file_norm = fold_platform(&strip_verbatim(&joined_abs));
    let root_norm = PathBuf::from(normalise_path(&root_norm));
    let file_norm = PathBuf::from(normalise_path(&file_norm));
    file_norm
        .strip_prefix(&root_norm)
        .ok()
        .filter(|rel| !rel.as_os_str().is_empty())
        .map(Path::to_path_buf)
}

fn make_absolute(path: &Path) -> Option<PathBuf> {
    std::path::absolute(path).ok()
}

fn strip_verbatim(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

fn fold_platform(path: &Path) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    } else {
        path.to_path_buf()
    }
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
    let rule = rule_key(rule);
    rule == "secret" || rule.starts_with("secret.") || crate::ruleset::is_gitleaks_rule(&rule)
}

/// Text that has already passed the redaction constructor. The inner string
/// is private; the only way to obtain one is [`Outbound::new`] / [`Outbound::secret`].
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Outbound(String);

impl Outbound {
    pub(crate) fn new(raw: &str) -> Self {
        Self(oracle_core::redact_secret_tokens(raw))
    }

    pub(crate) fn secret(raw: &str) -> Self {
        let newlines = raw.bytes().filter(|byte| *byte == b'\n').count();
        let mut marker = String::from("[redacted-secret]");
        marker.extend(std::iter::repeat_n('\n', newlines));
        Self(marker)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Outbound {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
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
}

/// One span inside a finding. A secret that appears twice is one finding
/// with two locations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
    start_line: usize,
    end_line: usize,
}

impl Location {
    pub(crate) fn new(start_line: usize, end_line: usize) -> Self {
        Self {
            start_line,
            end_line,
        }
    }
    pub fn start_line(self) -> usize {
        self.start_line
    }
    pub fn end_line(self) -> usize {
        self.end_line
    }
}

/// One problem in a real file. Fields are private and every string is
/// constructed through [`Outbound`], so a raw secret cannot sit on this type.
/// Interchange lives in [`crate::exchange`].
#[derive(Clone, PartialEq, Eq)]
pub struct Finding {
    id: FindingId,
    file: PathBuf,
    locations: Vec<Location>,
    rule: Outbound,
    severity: Severity,
    source: Outbound,
    title: Outbound,
    evidence: Outbound,
}

impl Finding {
    pub fn id(&self) -> &FindingId {
        &self.id
    }
    pub fn file(&self) -> &Path {
        &self.file
    }
    pub fn start_line(&self) -> usize {
        self.locations
            .first()
            .map(|loc| loc.start_line)
            .unwrap_or(1)
    }
    pub fn end_line(&self) -> usize {
        self.locations
            .last()
            .map(|loc| loc.end_line)
            .unwrap_or(self.start_line())
    }
    pub fn locations(&self) -> &[Location] {
        &self.locations
    }
    pub fn rule(&self) -> &str {
        self.rule.as_str()
    }
    pub fn severity(&self) -> Severity {
        self.severity
    }
    pub fn source(&self) -> &str {
        self.source.as_str()
    }
    pub fn title(&self) -> &str {
        self.title.as_str()
    }
    pub fn evidence(&self) -> &str {
        self.evidence.as_str()
    }

    pub(crate) fn stamp_source(&mut self, source: &str) {
        self.source = Outbound::new(source);
    }

    pub(crate) fn add_location(&mut self, start_line: usize, end_line: usize) {
        let loc = Location {
            start_line,
            end_line,
        };
        if !self.locations.contains(&loc) {
            self.locations.push(loc);
            self.locations.sort_unstable();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_snapshot(
        id: FindingId,
        file: PathBuf,
        locations: Vec<Location>,
        rule: String,
        severity: Severity,
        source: String,
        title: String,
        evidence: String,
    ) -> Option<Self> {
        if locations.is_empty() {
            return None;
        }
        Some(Self::assemble(
            id, file, locations, &rule, severity, &source, &title, &evidence,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(
        id: FindingId,
        file: PathBuf,
        locations: Vec<Location>,
        rule: &str,
        severity: Severity,
        source: &str,
        title: &str,
        evidence: &str,
    ) -> Self {
        let file = PathBuf::from(Outbound::new(&file.to_string_lossy()).as_str());
        let evidence = if evidence_is_verbatim(rule) {
            Outbound::secret(evidence)
        } else {
            Outbound::new(evidence)
        };
        Self {
            id,
            file,
            locations,
            rule: Outbound::new(rule),
            severity,
            source: Outbound::new(source),
            title: Outbound::new(title),
            evidence,
        }
    }
}

impl std::fmt::Debug for Finding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Finding")
            .field("id", &self.id)
            .field("file", &self.file)
            .field("locations", &self.locations)
            .field("rule", &self.rule)
            .field("severity", &self.severity)
            .field("source", &self.source)
            .field("title", &self.title)
            .field("evidence", &self.evidence)
            .finish()
    }
}

impl Finding {
    /// Build a finding only if `file` exists, is inside `root`, and the line
    /// range is inside it.
    pub fn grounded(root: &Path, draft: Draft<'_>) -> Option<Self> {
        let file = canonical_file(root, &draft.file)?;
        let absolute = root.join(&file);
        let contents = read_source(&absolute)?;
        Self::grounded_on(root, draft, contents.lines().count())
    }

    /// Ground with a precomputed line count so a detector that already
    /// indexed the file does not rescan it per match.
    pub(crate) fn grounded_on(root: &Path, draft: Draft<'_>, lines: usize) -> Option<Self> {
        if draft.start_line == 0 || draft.end_line < draft.start_line {
            return None;
        }
        if draft.end_line > lines {
            return None;
        }
        let file = canonical_file(root, &draft.file)?;
        Some(Self::assemble(
            FindingId::of(draft.rule, &file, Some(draft.start_line), draft.raw_excerpt),
            file,
            vec![Location {
                start_line: draft.start_line,
                end_line: draft.end_line,
            }],
            draft.rule,
            draft.severity,
            draft.source,
            draft.title,
            draft.raw_excerpt,
        ))
    }
}

/// Collapse findings that share an id into one, keeping every location.
/// The v1 ledger did last-wins on duplicate ids; with the line dropped for
/// secrets that would forget a real place. One problem, many positions.
pub(crate) fn coalesce(findings: Vec<Finding>) -> Vec<Finding> {
    let mut order: Vec<String> = Vec::new();
    let mut by_id: std::collections::HashMap<String, Finding> = std::collections::HashMap::new();
    for finding in findings {
        let key = finding.id().as_str().to_string();
        if let Some(existing) = by_id.get_mut(&key) {
            for loc in finding.locations {
                existing.add_location(loc.start_line, loc.end_line);
            }
        } else {
            order.push(key.clone());
            by_id.insert(key, finding);
        }
    }
    order
        .into_iter()
        .filter_map(|key| by_id.remove(&key))
        .collect()
}

const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;

pub(crate) fn read_source(path: &Path) -> Option<String> {
    // metadata-then-read is a TOCTOU window (declared not-done). A file that
    // changes or vanishes between the two must skip, never panic, never fail
    // the whole review. Skip counting lives at the polis-backend walk, because
    // two cheap detectors would otherwise double-count the same vanished file.
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
    fn two_copies_of_the_same_secret_share_an_id() {
        let file = Path::new("src/auth.rs");
        let token = "same-token-twice";
        assert_eq!(
            FindingId::of("secret", file, Some(1), token),
            FindingId::of("secret", file, Some(4), token),
            "byte-identical secrets in one file are one finding"
        );
    }

    #[test]
    #[cfg(windows)]
    fn windows_path_case_is_folded() {
        let evidence = "fn foo exceeds the cyclomatic threshold";
        assert_eq!(
            FindingId::of("complexity", Path::new(r"src\A.rs"), Some(1), evidence),
            FindingId::of("complexity", Path::new("src/a.rs"), Some(1), evidence),
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

    #[test]
    fn a_file_outside_the_root_is_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        let excerpt = tokens::aws_example();
        std::fs::write(outside.path().join("x.rs"), format!("{excerpt}\n")).expect("write");
        assert!(
            Finding::grounded(
                temp.path(),
                draft(outside.path().join("x.rs"), 1, 1, &excerpt),
            )
            .is_none(),
            "a path that escapes the root must not become a finding"
        );
    }

    #[test]
    fn a_rule_id_is_matched_without_regard_to_case() {
        let file = Path::new("src/a.rs");
        let evidence = "fn foo exceeds the cyclomatic threshold";
        assert_eq!(
            FindingId::of("complexity", file, Some(10), evidence),
            FindingId::of("COMPLEXITY", file, Some(10), evidence),
        );
        assert_eq!(
            FindingId::of("secret", file, Some(1), "Aa"),
            FindingId::of("SECRET", file, Some(1), "Aa"),
        );
    }

    #[test]
    fn the_public_api_cannot_hold_a_raw_secret() {
        let secret = tokens::aws_access_token();
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("a.rs"), format!("let x = \"{secret}\";\n"))
            .expect("write");
        let finding = Finding::grounded(
            temp.path(),
            Draft {
                file: PathBuf::from("a.rs"),
                start_line: 1,
                end_line: 1,
                rule: "secret",
                severity: Severity::Inferno,
                source: "secrets",
                title: &format!("found {secret}"),
                raw_excerpt: &secret,
            },
        )
        .expect("grounded");
        let dump = format!("{finding:?}");
        assert!(
            !dump.contains(&secret),
            "Finding Debug carried a raw secret: {dump}"
        );
        assert!(!finding.title().contains(&secret));
        assert!(!finding.evidence().contains(&secret));
        let payload = crate::exchange::encode_ledger(&finding);
        let smuggled = payload.replace("[redacted-secret]", &secret);
        let back = crate::exchange::decode_ledger(&smuggled).expect("decode");
        assert!(
            !back.evidence().contains(&secret) && !format!("{back:?}").contains(&secret),
            "from_snapshot reconstituted a raw secret"
        );
    }
}
