//! The parsed design-craft corpus: the manifest and the sections.
//!
//! One responsibility: read the embedded files into the structures the
//! composer works on. The manifest is the one shared config the app's pure
//! skill modules import too; the sections parse with the same front-matter
//! rule, and the strict check reports what the tolerant runtime skips.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use super::design_craft_corpus;

#[derive(Debug, serde::Deserialize)]
pub(crate) struct ManifestWeights {
    pub(crate) title: f64,
    pub(crate) description: f64,
    pub(crate) body: f64,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManifestBm25 {
    pub(crate) k1: f64,
    pub(crate) b: f64,
    pub(crate) fallback_threshold: f64,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct ManifestCeilings {
    pub(crate) doctrine: usize,
    pub(crate) section: usize,
    // Read by the strict check, which only the tests call.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) description: usize,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct ManifestDelimiters {
    pub(crate) begin: String,
    pub(crate) end: String,
    pub(crate) disclaimer: String,
    pub(crate) restatement: String,
    pub(crate) removed: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Manifest {
    pub(crate) priority: Vec<String>,
    pub(crate) scoped: HashMap<String, String>,
    pub(crate) weights: ManifestWeights,
    pub(crate) bm25: ManifestBm25,
    pub(crate) stopwords: Vec<String>,
    pub(crate) min_token_length: usize,
    pub(crate) ceilings: ManifestCeilings,
    pub(crate) section_separator: String,
    pub(crate) truncation_notice: String,
    pub(crate) baseline: Vec<String>,
    pub(crate) delimiters: ManifestDelimiters,
}

static MANIFEST: OnceLock<Option<Manifest>> = OnceLock::new();

/// The shared config, parsed once. `None` only when the embedded manifest
/// itself is broken — the corpus tests pin it valid, and the composer
/// degrades to no doctrine rather than refusing a creation over it.
pub(crate) fn manifest() -> Option<&'static Manifest> {
    MANIFEST
        .get_or_init(|| serde_json::from_str(design_craft_corpus::MANIFEST_JSON).ok())
        .as_ref()
}

/// What TypeScript's `string.length` counts: UTF-16 code units, not bytes
/// and not scalar values. Every ceiling and budget comparison uses this, so
/// an emoji counts two here exactly as it does there.
pub(crate) fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

#[derive(Debug, Clone)]
pub(crate) struct SkillSection {
    pub(crate) slug: String,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) body: String,
    pub(crate) requires: Vec<String>,
    // Retained for the strict check, which only the tests call: the
    // tolerant runtime ignores unknown keys by design.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) unknown_keys: Vec<String>,
}

fn is_valid_slug(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn parse_requires(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "[]" {
        return Vec::new();
    }
    let mut inner = trimmed;
    if let Some(rest) = inner.strip_prefix('[') {
        inner = rest;
    }
    if let Some(rest) = inner.strip_suffix(']') {
        inner = rest;
    }
    let inner = inner.trim();
    if inner.is_empty() {
        return Vec::new();
    }
    inner
        .split(',')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

fn filename_stem(path: &str) -> Option<&str> {
    path.rsplit('/').next()?.split('.').next()
}

/// One skill file's text into a section, with the same failures the app's
/// loader reports: malformed fences, an invalid slug, a slug that does not
/// match the filename, a doubled key, a missing field.
pub(crate) fn parse_skill_file(path: &str, text: &str) -> Result<SkillSection, String> {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() < 2 || lines[0].trim() != "---" {
        return Err("malformed_front_matter: Missing opening ---".to_string());
    }
    let mut end = None;
    for (index, line) in lines.iter().enumerate().skip(1) {
        if line.trim() == "---" {
            end = Some(index);
            break;
        }
    }
    let end = match end {
        Some(index) => index,
        None => {
            return Err("malformed_front_matter: Missing closing ---".to_string());
        }
    };
    let body = lines[end + 1..].join("\n").trim().to_string();

    let mut slug = None;
    let mut title = None;
    let mut description = None;
    let mut unknown_keys = Vec::new();
    let mut requires = None;
    let mut seen = HashSet::new();
    for line in &lines[1..end] {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let colon = match trimmed.find(':') {
            Some(index) => index,
            None => {
                return Err(format!(
                    "malformed_front_matter: a front-matter value must stay on one line: {trimmed}"
                ));
            }
        };
        let key = trimmed[..colon].trim();
        let value = trimmed[colon + 1..].trim();
        if !seen.insert(key.to_string()) {
            return Err(format!("duplicate_key: {key}"));
        }
        match key {
            "slug" => {
                if !is_valid_slug(value) {
                    return Err(format!("invalid_slug: {value}"));
                }
                slug = Some(value.to_string());
            }
            "title" => title = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            "requires" => requires = Some(parse_requires(value)),
            _ => unknown_keys.push(key.to_string()),
        }
    }

    let slug = match slug {
        Some(slug) => slug,
        None => return Err("malformed_front_matter: Missing slug field".to_string()),
    };
    let title = match title {
        Some(title) => title,
        None => return Err("malformed_front_matter: Missing title field".to_string()),
    };
    let description = match description {
        Some(description) => description,
        None => {
            return Err("malformed_front_matter: Missing description field".to_string());
        }
    };
    if let Some(stem) = filename_stem(path) {
        if slug != stem {
            return Err(format!("slug_filename_mismatch: {slug} is not {stem}"));
        }
    }
    Ok(SkillSection {
        slug,
        title,
        description,
        body,
        requires: requires.unwrap_or_default(),
        unknown_keys,
    })
}

/// The sections the chooser may route over for a page: the embedded files in
/// manifest priority order (an unlisted file sorts last, alphabetically, so
/// it cannot displace a listed one), minus the sections scoped to another
/// output shape. Malformed files are skipped, exactly as the tolerant
/// runtime does; the strict check below is what fails on them.
pub(crate) fn page_index(manifest: &Manifest) -> Vec<SkillSection> {
    let mut sections = Vec::new();
    let mut seen = HashSet::new();
    for source in design_craft_corpus::EMBEDDED_SOURCES {
        let Ok(section) = parse_skill_file(source.path, source.text) else {
            continue;
        };
        if !seen.insert(section.slug.clone()) {
            continue;
        }
        sections.push(section);
    }
    let order: HashMap<&str, usize> = manifest
        .priority
        .iter()
        .enumerate()
        .map(|(index, slug)| (slug.as_str(), index))
        .collect();
    sections.sort_by(|left, right| {
        match (
            order.get(left.slug.as_str()),
            order.get(right.slug.as_str()),
        ) {
            (Some(left), Some(right)) => left.cmp(right),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.slug.cmp(&right.slug),
        }
    });
    sections.retain(|section| match manifest.scoped.get(section.slug.as_str()) {
        Some(scope) => scope == "page",
        None => true,
    });
    sections
}

/// The strict first-party check over the embedded files: every problem the
/// tolerant runtime would silently skip — malformed fences, slug and
/// filename mismatches, doubled slugs, empty or over-ceiling bodies,
/// over-long descriptions, unknown keys, dangling `requires`, dependency
/// cycles. Empty means the corpus ships. Called by the corpus tests; the
/// send path stays tolerant by design.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn validate_corpus() -> Vec<String> {
    let mut problems = Vec::new();
    let mut sections = Vec::new();
    let mut seen: HashMap<String, &str> = HashMap::new();
    for source in design_craft_corpus::EMBEDDED_SOURCES {
        match parse_skill_file(source.path, source.text) {
            Ok(section) => {
                if seen.insert(section.slug.clone(), source.path).is_some() {
                    problems.push(format!("{}: duplicate slug {}", source.path, section.slug));
                } else {
                    sections.push(section);
                }
            }
            Err(error) => problems.push(format!("{}: {error}", source.path)),
        }
    }
    let Some(manifest) = manifest() else {
        problems.push("the embedded manifest does not parse".to_string());
        return problems;
    };
    for section in &sections {
        if section.body.is_empty() {
            problems.push(format!("{}: empty body", section.slug));
        } else if utf16_len(&section.body) > manifest.ceilings.section {
            problems.push(format!(
                "{}: body {} chars over the {} ceiling",
                section.slug,
                utf16_len(&section.body),
                manifest.ceilings.section
            ));
        }
        if utf16_len(&section.description) > manifest.ceilings.description {
            problems.push(format!(
                "{}: description {} chars over the {} ceiling",
                section.slug,
                utf16_len(&section.description),
                manifest.ceilings.description
            ));
        }
        for key in &section.unknown_keys {
            problems.push(format!("{}: unknown front-matter key {key}", section.slug));
        }
    }
    let known: HashSet<&str> = sections.iter().map(|s| s.slug.as_str()).collect();
    for section in &sections {
        for dep in &section.requires {
            if !known.contains(dep.as_str()) {
                problems.push(format!("{}: unknown requires slug {dep}", section.slug));
            }
        }
    }
    let requested: Vec<String> = sections.iter().map(|s| s.slug.clone()).collect();
    if let Err(error) = super::design_craft_resolve::resolve_sections(&requested, &sections) {
        problems.push(format!("dependency graph: {error:?}"));
    }
    problems
}
