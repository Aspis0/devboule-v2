//! In-process detector driven by a rule file, not by regexes in this module.
//!
//! Secret patterns come from the vendored gitleaks TOML. Extra location rules
//! use the same loader. Adding a rule is editing data.

use std::path::Path;

use crate::detector::{Context, Cost, Detector};
use crate::error::Error;
use crate::finding::{Draft, Finding};
use crate::ruleset::{shannon_entropy, CompiledRule, Ruleset};

pub struct Secrets {
    rules: Ruleset,
}

impl Default for Secrets {
    fn default() -> Self {
        Self {
            rules: Ruleset::builtin().expect("shipped gitleaks.toml must parse"),
        }
    }
}

impl Secrets {
    #[cfg(test)]
    pub fn from_toml(toml: &str) -> Result<Self, Error> {
        Ok(Self {
            rules: Ruleset::parse(toml)?,
        })
    }
}

impl Detector for Secrets {
    fn id(&self) -> &'static str {
        "secrets"
    }

    fn cost(&self) -> Cost {
        Cost::Cheap
    }

    fn scan(&self, ctx: &Context<'_>) -> Result<Vec<Finding>, Error> {
        let mut findings = Vec::new();
        for relative in ctx.files {
            let absolute = if relative.is_absolute() {
                relative.clone()
            } else {
                ctx.root.join(relative)
            };
            let Some(text) = crate::finding::read_source(&absolute) else {
                continue;
            };
            findings.extend(scan_file(&self.rules, ctx.root, relative, &absolute, &text));
        }
        Ok(findings)
    }
}

fn scan_file(
    rules: &Ruleset,
    root: &Path,
    relative: &Path,
    absolute: &Path,
    text: &str,
) -> Vec<Finding> {
    let path_text = slashy(&absolute.to_string_lossy());
    let relative_text = slashy(&relative.to_string_lossy());
    if path_is_skipped(rules, &path_text, &relative_text) {
        return Vec::new();
    }
    let lower_file = text.to_lowercase();
    let line_index = LineIndex::new(text);
    let mut findings = Vec::new();
    for rule in &rules.rules {
        if rule
            .skip_paths
            .iter()
            .any(|skip| skip.is_match(&path_text) || skip.is_match(&relative_text))
        {
            continue;
        }
        if let Some(path_filter) = &rule.path {
            if !path_filter.is_match(&path_text) && !path_filter.is_match(&relative_text) {
                continue;
            }
        }
        if !rule.keywords.is_empty() && !rule.keywords.iter().any(|word| lower_file.contains(word))
        {
            continue;
        }
        for capture in rule.pattern.captures_iter(text) {
            let full_match = capture.get(0).map(|all| all.as_str()).unwrap_or("");
            let excerpt = excerpt_from(&capture, rule);
            let start = capture.get(0).map(|span| span.start()).unwrap_or(0);
            if is_allowlisted(
                rules,
                rule,
                excerpt,
                full_match,
                line_index.enclosing(text, start),
            ) {
                continue;
            }
            if let Some(minimum) = rule.entropy {
                if shannon_entropy(excerpt) < minimum {
                    continue;
                }
            }
            let end = capture
                .get(0)
                .map(|span| span.end().saturating_sub(1).max(start))
                .unwrap_or(start);
            if let Some(finding) = Finding::grounded_on(
                root,
                Draft {
                    file: relative.to_path_buf(),
                    start_line: line_index.line(start),
                    end_line: line_index.line(end),
                    rule: &rule.id,
                    severity: rule.severity,
                    source: "secrets",
                    title: &rule.title,
                    raw_excerpt: excerpt,
                },
                line_index.line_count(),
            ) {
                findings.push(finding);
            }
        }
    }
    crate::finding::coalesce(findings)
}

fn slashy(path: &str) -> String {
    path.replace('\\', "/")
}

fn path_is_skipped(rules: &Ruleset, absolute: &str, relative: &str) -> bool {
    rules
        .skip_paths
        .iter()
        .any(|skip| skip.is_match(absolute) || skip.is_match(relative))
}

fn is_allowlisted(
    rules: &Ruleset,
    rule: &CompiledRule,
    excerpt: &str,
    full_match: &str,
    line: &str,
) -> bool {
    if rule.skip.iter().any(|skip| skip.is_match(excerpt)) {
        return true;
    }
    if rule
        .skip_match
        .iter()
        .any(|skip| skip.is_match(full_match) || skip.is_match(excerpt) || skip.is_match(line))
    {
        return true;
    }
    if rules.skip_secrets.iter().any(|skip| skip.is_match(excerpt)) {
        return true;
    }
    let lower = excerpt.to_lowercase();
    rule.stopwords.iter().any(|word| lower.contains(word))
        || rules.stopwords.iter().any(|word| lower.contains(word))
}

struct LineIndex {
    starts: Vec<usize>,
    count: usize,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(index + 1);
            }
        }
        Self {
            starts,
            count: text.lines().count(),
        }
    }

    fn line_count(&self) -> usize {
        self.count
    }

    fn line(&self, byte: usize) -> usize {
        self.starts.partition_point(|&start| start <= byte).max(1)
    }

    fn enclosing<'a>(&self, text: &'a str, byte: usize) -> &'a str {
        let line = self.line(byte);
        let start = self
            .starts
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or(0);
        let end = self.starts.get(line).copied().unwrap_or(text.len());
        let end = end.min(text.len()).max(start);
        text.get(start..end).unwrap_or("").trim_end_matches('\n')
    }
}

fn excerpt_from<'a>(capture: &'a regex::Captures<'a>, rule: &CompiledRule) -> &'a str {
    if let Some(index) = rule.secret_group {
        if let Some(group) = capture.get(index) {
            return group.as_str();
        }
    }
    if let Some(group) = capture.get(1) {
        return group.as_str();
    }
    capture.get(0).map(|all| all.as_str()).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detector::Context;
    use crate::finding::FindingId;
    use crate::tokens;
    use std::path::PathBuf;

    fn assert_assembled_matches_rule(id: &str, sample: &str) {
        let set = crate::ruleset::Ruleset::builtin().expect("shipped ruleset");
        let rule = set
            .rules
            .iter()
            .find(|rule| rule.id == id)
            .unwrap_or_else(|| panic!("gitleaks rule {id} missing from the compiled set"));
        assert!(
            rule.pattern.is_match(sample),
            "assembled fixture no longer matches gitleaks {id}"
        );
    }

    fn write(root: &Path, relative: &str, body: &str) -> PathBuf {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, body).expect("write");
        PathBuf::from(relative)
    }

    #[test]
    fn an_aws_key_is_a_finding_and_the_evidence_is_redacted() {
        let aws = tokens::aws_access_token();
        assert_assembled_matches_rule("aws-access-token", &aws);
        let temp = tempfile::tempdir().expect("tempdir");
        let file = write(
            temp.path(),
            "src/auth.rs",
            &format!("const KEY: &str = \"{aws}\";\n"),
        );
        let files = [file];
        let findings = Secrets::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("scan");
        let hit = findings
            .iter()
            .find(|finding| finding.rule() == "aws-access-token")
            .expect("gitleaks aws-access-token should fire");
        assert_eq!(hit.start_line(), 1);
        assert!(
            !hit.evidence().contains(&aws),
            "evidence quoted the secret: {}",
            hit.evidence()
        );
    }

    #[test]
    fn shifting_the_lines_keeps_the_id() {
        let aws = tokens::aws_access_token();
        assert_assembled_matches_rule("aws-access-token", &aws);
        let body = format!("const KEY: &str = \"{aws}\";\n");
        let temp = tempfile::tempdir().expect("tempdir");
        let high = write(temp.path(), "src/auth.rs", &body);
        let first = Secrets::default()
            .scan(&Context::new(temp.path(), std::slice::from_ref(&high)))
            .expect("scan")
            .into_iter()
            .find(|finding| finding.rule() == "aws-access-token")
            .expect("a finding");
        std::fs::write(temp.path().join("src/auth.rs"), format!("\n\n\n{body}")).expect("write");
        let second = Secrets::default()
            .scan(&Context::new(temp.path(), &[high]))
            .expect("scan")
            .into_iter()
            .find(|finding| finding.rule() == "aws-access-token")
            .expect("a finding after the shift");
        assert_eq!(
            first.id(),
            second.id(),
            "three blank lines resurrected a fire"
        );
        assert_eq!(second.start_line(), 4);
        assert_ne!(
            first.id(),
            &FindingId::of(
                "aws-access-token",
                Path::new("src/auth.rs"),
                Some(4),
                "different"
            ),
        );
    }

    #[test]
    fn two_copies_of_the_same_secret_are_one_finding() {
        let aws = tokens::aws_access_token();
        assert_assembled_matches_rule("aws-access-token", &aws);
        let temp = tempfile::tempdir().expect("tempdir");
        let file = write(
            temp.path(),
            "src/auth.rs",
            &format!("const A: &str = \"{aws}\";\nconst B: &str = \"{aws}\";\n"),
        );
        let files = [file];
        let hits: Vec<_> = Secrets::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("scan")
            .into_iter()
            .filter(|finding| finding.rule() == "aws-access-token")
            .collect();
        assert_eq!(hits.len(), 1, "one secret, one finding: {hits:?}");
        assert_eq!(hits[0].locations().len(), 2, "both places kept: {hits:?}");
        assert_eq!(hits[0].locations()[0].start_line(), 1);
        assert_eq!(hits[0].locations()[1].start_line(), 2);
    }

    #[test]
    fn a_hardcoded_windows_path_is_a_finding() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = write(
            temp.path(),
            "src/config.rs",
            "let db = \"C:\\\\Users\\\\me\\\\project.db\";\n",
        );
        let files = [file];
        let findings = Secrets::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("scan");
        assert!(
            findings
                .iter()
                .any(|finding| finding.rule() == "path.absolute"),
            "hardcoded absolute path was not reported: {findings:?}"
        );
    }

    #[test]
    fn a_localhost_url_is_a_finding() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = write(
            temp.path(),
            "src/client.rs",
            "const API: &str = \"http://localhost:8080/v1\";\n",
        );
        let files = [file];
        let findings = Secrets::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("scan");
        assert!(
            findings
                .iter()
                .any(|finding| finding.rule() == "url.localhost"),
            "localhost URL was not reported: {findings:?}"
        );
    }

    #[test]
    fn a_rule_file_can_add_a_pattern_without_new_rust() {
        let toml = r#"
[[rules]]
id = "todo.fixme"
regex = "FIXME"
description = "A leftover FIXME"
severity = "smoke"
"#;
        let detector = Secrets::from_toml(toml).expect("parse");
        let temp = tempfile::tempdir().expect("tempdir");
        let file = write(temp.path(), "src/lib.rs", "fn f() { /* FIXME later */ }\n");
        let files = [file];
        let findings = detector
            .scan(&Context::new(temp.path(), &files))
            .expect("scan");
        assert!(
            findings
                .iter()
                .any(|finding| finding.rule() == "todo.fixme"),
            "a rule that lives in the file was ignored: {findings:?}"
        );
    }

    #[test]
    fn a_gitleaks_example_allowlist_is_honoured() {
        let example = tokens::aws_example();
        assert_assembled_matches_rule("aws-access-token", &example);
        let temp = tempfile::tempdir().expect("tempdir");
        let file = write(
            temp.path(),
            "src/auth.rs",
            &format!("const KEY: &str = \"{example}\";\n"),
        );
        let files = [file];
        let findings = Secrets::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("scan");
        assert!(
            findings
                .iter()
                .all(|finding| finding.rule() != "aws-access-token"),
            "gitleaks '.+EXAMPLE$' allowlist was ignored: {findings:?}"
        );
    }

    #[test]
    fn a_pem_private_key_spans_lines_and_still_fires() {
        let pem = tokens::rsa_private_key_pem();
        assert_assembled_matches_rule("private-key", &pem);
        let temp = tempfile::tempdir().expect("tempdir");
        let file = write(temp.path(), "src/key.pem", &pem);
        let files = [file];
        let findings = Secrets::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("scan");
        assert!(
            findings
                .iter()
                .any(|finding| finding.rule() == "private-key"),
            "multiline gitleaks rule was dropped by a line-by-line scan: {findings:?}"
        );
        let pem = findings
            .iter()
            .find(|finding| finding.rule() == "private-key")
            .expect("pem");
        assert!(
            !pem.evidence().contains("BEGIN") && !pem.evidence().contains("MIIE"),
            "PEM leaked into evidence: {}",
            pem.evidence()
        );
    }

    #[test]
    fn a_gitleaks_secret_that_oracle_core_would_miss_is_still_redacted() {
        let openai = tokens::openai_api_key();
        assert_assembled_matches_rule("openai-api-key", &openai);
        let temp = tempfile::tempdir().expect("tempdir");
        let file = write(
            temp.path(),
            "src/llm.rs",
            &format!("const K: &str = \"{openai}\";\n"),
        );
        let files = [file];
        let findings = Secrets::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("scan");
        let hit = findings
            .iter()
            .find(|finding| finding.rule() == "openai-api-key")
            .expect("openai-api-key should fire");
        assert!(
            !hit.evidence().contains(&openai) && !hit.evidence().contains("sk-"),
            "gitleaks excerpt leaked: {}",
            hit.evidence()
        );
        assert!(hit.evidence().contains("[redacted-secret]"));
    }

    #[test]
    fn a_binary_file_is_not_scanned() {
        let aws = tokens::aws_access_token();
        let temp = tempfile::tempdir().expect("tempdir");
        let mut body = format!("const KEY: &str = \"{aws}\";\n").into_bytes();
        body.push(0);
        let path = temp.path().join("src/auth.rs");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, body).expect("write");
        let files = [PathBuf::from("src/auth.rs")];
        let findings = Secrets::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("scan");
        assert!(
            findings.is_empty(),
            "a file with a NUL was scanned as text: {findings:?}"
        );
    }

    #[test]
    fn a_clean_file_is_silent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = write(
            temp.path(),
            "src/lib.rs",
            "fn add(a: i32, b: i32) -> i32 { a + b }\n",
        );
        let files = [file];
        let findings = Secrets::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("scan");
        assert!(findings.is_empty(), "clean code produced {findings:?}");
    }

    #[test]
    fn a_file_that_vanishes_mid_scan_is_skipped_not_a_failed_review() {
        let aws = tokens::aws_access_token();
        assert_assembled_matches_rule("aws-access-token", &aws);
        let temp = tempfile::tempdir().expect("tempdir");
        let kept = write(
            temp.path(),
            "src/auth.rs",
            &format!("const KEY: &str = \"{aws}\";\n"),
        );
        let files = [kept, PathBuf::from("src/gone.rs")];
        let findings = Secrets::default()
            .scan(&Context::new(temp.path(), &files))
            .expect("a vanished neighbour must not fail the detector");
        assert!(
            findings
                .iter()
                .any(|finding| finding.rule() == "aws-access-token"),
            "kept secret was lost because a neighbour vanished: {findings:?}"
        );
    }
}
