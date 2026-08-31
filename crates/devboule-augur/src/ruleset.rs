//! Rule files as data. Gitleaks TOML is the secret ruleset; extra location
//! rules use the same loader (`[[rules]]` with `id`, `regex`, optional
//! `entropy`, `keywords`, `secretGroup`, `path`).

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::{Regex, RegexBuilder};

use crate::error::Error;
use crate::finding::Severity;

const GITLEAKS_TOML: &str = include_str!("../vendor/gitleaks/gitleaks.toml");
const LOCATIONS_TOML: &str = include_str!("../rules/locations.toml");

static GITLEAKS_IDS: OnceLock<HashSet<String>> = OnceLock::new();

pub fn is_gitleaks_rule(rule: &str) -> bool {
    GITLEAKS_IDS
        .get_or_init(|| {
            parse_toml(GITLEAKS_TOML)
                .expect("shipped gitleaks.toml must parse")
                .rules
                .into_iter()
                .map(|rule| rule.id)
                .collect()
        })
        .contains(rule)
}

#[derive(Debug, serde::Deserialize)]
struct File {
    #[serde(default)]
    rules: Vec<RawRule>,
    #[serde(default, rename = "rule")]
    rule: Vec<RawRule>,
    #[serde(default)]
    allowlist: Option<RawAllowlist>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct RawAllowlist {
    #[serde(default)]
    regexes: Vec<String>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    stopwords: Vec<String>,
    #[serde(default, rename = "regexTarget")]
    regex_target: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct RawRule {
    id: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    regex: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    entropy: Option<f64>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default, rename = "secretGroup")]
    secret_group: Option<usize>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    allowlist: Option<RawAllowlist>,
    #[serde(default)]
    allowlists: Vec<RawAllowlist>,
}

#[derive(Clone)]
pub struct CompiledRule {
    pub id: String,
    pub title: String,
    pub severity: Severity,
    pub pattern: Regex,
    pub entropy: Option<f64>,
    pub keywords: Vec<String>,
    pub secret_group: Option<usize>,
    pub path: Option<Regex>,
    pub skip: Vec<Regex>,
    pub skip_match: Vec<Regex>,
    pub skip_paths: Vec<Regex>,
    pub stopwords: Vec<String>,
}

#[derive(Clone)]
pub struct Ruleset {
    pub rules: Vec<CompiledRule>,
    pub skip_paths: Vec<Regex>,
    pub skip_secrets: Vec<Regex>,
    pub stopwords: Vec<String>,
}

impl Ruleset {
    pub fn builtin() -> Result<Self, Error> {
        static CACHE: OnceLock<Ruleset> = OnceLock::new();
        let cached = CACHE.get_or_init(|| {
            let mut set =
                parse_and_compile(GITLEAKS_TOML).expect("shipped gitleaks.toml must parse");
            let extra =
                parse_and_compile(LOCATIONS_TOML).expect("shipped locations.toml must parse");
            set.rules.extend(extra.rules);
            set
        });
        Ok(cached.clone())
    }

    #[cfg(test)]
    pub fn parse(toml: &str) -> Result<Self, Error> {
        parse_and_compile(toml)
    }
}

fn parse_toml(toml: &str) -> Result<File, Error> {
    toml::from_str(toml).map_err(|error| Error::Rules(error.to_string()))
}

fn parse_and_compile(toml: &str) -> Result<Ruleset, Error> {
    let mut file = parse_toml(toml)?;
    file.rules.append(&mut file.rule);
    let global = file.allowlist.unwrap_or_default();
    let skip_paths = compile_regexes(global.paths);
    let skip_secrets = compile_regexes(global.regexes);
    let stopwords = lowercase(global.stopwords);
    let mut rules = Vec::new();
    for raw in file.rules {
        let Some(pattern_src) = raw.regex.or(raw.pattern) else {
            continue;
        };
        let Some(pattern) = compile_re(&pattern_src) else {
            continue;
        };
        let path = match raw.path {
            Some(src) => match compile_re(&src) {
                Some(regex) => Some(regex),
                None => continue,
            },
            None => None,
        };
        let severity = match raw.severity.as_deref() {
            Some("smoke") => Severity::Smoke,
            Some("fire") => Severity::Fire,
            Some("inferno") => Severity::Inferno,
            _ => Severity::Inferno,
        };
        let title = raw
            .title
            .or(raw.description)
            .unwrap_or_else(|| raw.id.clone());
        let mut skip = Vec::new();
        let mut skip_match = Vec::new();
        let mut rule_skip_paths = Vec::new();
        let mut rule_stopwords = Vec::new();
        let mut allowlists = raw.allowlists;
        if let Some(one) = raw.allowlist {
            allowlists.push(one);
        }
        for allow in allowlists {
            let compiled = compile_regexes(allow.regexes);
            match allow.regex_target.as_deref() {
                Some("match") | Some("line") => skip_match.extend(compiled),
                _ => skip.extend(compiled),
            }
            rule_skip_paths.extend(compile_regexes(allow.paths));
            rule_stopwords.extend(lowercase(allow.stopwords));
        }
        rules.push(CompiledRule {
            id: raw.id,
            title,
            severity,
            pattern,
            entropy: raw.entropy,
            keywords: lowercase(raw.keywords),
            secret_group: raw.secret_group,
            path,
            skip,
            skip_match,
            skip_paths: rule_skip_paths,
            stopwords: rule_stopwords,
        });
    }
    Ok(Ruleset {
        rules,
        skip_paths,
        skip_secrets,
        stopwords,
    })
}

fn compile_regexes(sources: Vec<String>) -> Vec<Regex> {
    sources
        .into_iter()
        .filter_map(|src| compile_re(&src))
        .collect()
}

/// Prefer RE2-like ASCII classes (gitleaks) and fall back to the crate
/// default. `unicode(false)` rejects `[\s\S]` and `[^\s]` which private-key
/// and the location rules use; dropping those rules is worse than `\w`
/// matching a non-ASCII letter.
fn compile_re(src: &str) -> Option<Regex> {
    RegexBuilder::new(src)
        .unicode(false)
        .build()
        .or_else(|_| Regex::new(src))
        .ok()
}

fn lowercase(words: Vec<String>) -> Vec<String> {
    words.into_iter().map(|word| word.to_lowercase()).collect()
}

pub fn shannon_entropy(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for &byte in text.as_bytes() {
        counts[byte as usize] += 1;
    }
    let len = text.len() as f64;
    counts
        .iter()
        .filter(|count| **count > 0)
        .map(|count| {
            let probability = f64::from(*count) / len;
            -probability * probability.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vendored_gitleaks_file_loads_as_data() {
        let set = Ruleset::builtin().expect("shipped gitleaks.toml must parse");
        let ids: Vec<&str> = set.rules.iter().map(|rule| rule.id.as_str()).collect();
        for needed in [
            "aws-access-token",
            "gcp-api-key",
            "github-pat",
            "slack-bot-token",
            "stripe-access-token",
            "private-key",
        ] {
            assert!(
                ids.contains(&needed),
                "gitleaks rule {needed} did not compile into the detector: {ids:?}"
            );
        }
        assert!(
            is_gitleaks_rule("aws-access-token"),
            "identity choke point must recognise gitleaks ids"
        );
        assert!(!is_gitleaks_rule("unused_variables"));
        // 195 gitleaks ids + 3 location rules. A silent compile drop of a
        // core rule is a miss, not a green test.
        assert!(
            set.rules.len() >= 150,
            "too many gitleaks regexes failed to compile in Rust: {}",
            set.rules.len()
        );
        let aws = set
            .rules
            .iter()
            .find(|rule| rule.id == "aws-access-token")
            .expect("aws");
        let example = crate::tokens::aws_example();
        assert!(
            aws.skip.iter().any(|skip| skip.is_match(&example)),
            "gitleaks EXAMPLE allowlist did not compile onto aws-access-token"
        );
    }

    #[test]
    fn a_go_regex_that_rust_cannot_compile_is_skipped_not_fatal() {
        let toml = r#"
[[rules]]
id = "broken"
regex = "(?P<oops>"
[[rules]]
id = "ok"
regex = "FIXME"
"#;
        let set = Ruleset::parse(toml).expect("parse");
        assert_eq!(set.rules.len(), 1);
        assert_eq!(set.rules[0].id, "ok");
    }
}
