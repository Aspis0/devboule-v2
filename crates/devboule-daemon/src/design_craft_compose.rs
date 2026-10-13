//! Matched doctrine composition for Design children.
//!
//! One responsibility: turn the ranked head into the framed doctrine block
//! — budget composition, matched selection, delimiters, and the preamble
//! the send reads — so a delegated child reads what an app-UI run for the
//! same prompt reads. Counts are UTF-16 units throughout, because the
//! ceilings being matched are TypeScript `length`s.

use std::collections::HashSet;

use super::design_craft::{manifest, page_index, utf16_len, Manifest, SkillSection};
use super::design_craft_rank::rank_slugs_for_query;
use super::design_craft_resolve::{resolve_sections, ResolveError};

pub(crate) struct ComposedBlock {
    pub(crate) text: String,
    // The conformance contract reads what the send path does not need:
    // which sections the budget cut, and the length in UTF-16 units.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) dropped: Vec<String>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) total_chars: usize,
}

fn fit_sections(
    order: &[&SkillSection],
    ceiling: usize,
    separator: &str,
) -> (Vec<String>, Vec<String>) {
    let mut parts = Vec::new();
    let mut dropped = Vec::new();
    for section in order {
        let block = format!("## {}\n\n{}", section.title, section.body);
        let candidate = if parts.is_empty() {
            block.clone()
        } else {
            format!("{}{}{}", parts.join(separator), separator, block)
        };
        if utf16_len(&candidate) > ceiling {
            if parts.is_empty() {
                parts.push(block);
            } else {
                dropped.push(section.slug.clone());
            }
        } else {
            parts.push(block);
        }
    }
    (parts, dropped)
}

/// The ordered sections into one block under the ceiling. What does not fit
/// is dropped from the end and named, and a dropped tail reserves room for
/// the notice that the block is incomplete. A lone section that alone
/// exceeds the ceiling is still included — a coherent section beats an
/// empty block, and only explicitly cut sections are named as dropped.
pub(crate) fn compose_sections(order: &[&SkillSection], manifest: &Manifest) -> ComposedBlock {
    let ceiling = manifest.ceilings.doctrine;
    let separator = manifest.section_separator.as_str();
    let (parts, dropped) = fit_sections(order, ceiling, separator);
    if dropped.is_empty() {
        let text = parts.join(separator);
        let total_chars = utf16_len(&text);
        return ComposedBlock {
            text,
            dropped,
            total_chars,
        };
    }
    let reserved = ceiling
        .saturating_sub(utf16_len(separator) + utf16_len(manifest.truncation_notice.as_str()));
    let (parts, dropped) = fit_sections(order, reserved, separator);
    let mut blocks = parts;
    blocks.push(manifest.truncation_notice.clone());
    let text = blocks.join(separator);
    let total_chars = utf16_len(&text);
    ComposedBlock {
        text,
        dropped,
        total_chars,
    }
}

fn empty_block() -> ComposedBlock {
    ComposedBlock {
        text: String::new(),
        dropped: Vec::new(),
        total_chars: 0,
    }
}

/// The tolerant runtime: malformed files are skipped, a dependency cycle
/// drops its members and keeps the rest, an unknown slug composes nothing.
/// The strict check is what fails on first-party problems instead.
pub(crate) fn build_skill_block(
    sources: &[(&str, &str)],
    requested: &[String],
    manifest: &Manifest,
) -> ComposedBlock {
    if requested.is_empty() {
        return empty_block();
    }
    let mut sections = Vec::new();
    let mut seen = HashSet::new();
    for (path, text) in sources {
        let Ok(section) = super::design_craft::parse_skill_file(path, text) else {
            continue;
        };
        if !seen.insert(section.slug.clone()) {
            continue;
        }
        sections.push(section);
    }
    if sections.is_empty() {
        return empty_block();
    }
    let mut current_sections = sections;
    let mut current_requested: Vec<String> = requested.to_vec();
    let bound = current_sections.len();
    for _ in 0..bound {
        match resolve_sections(&current_requested, &current_sections) {
            Ok(order) => return compose_sections(&order, manifest),
            Err(ResolveError::UnknownSlug(_)) => return empty_block(),
            Err(ResolveError::Cycle(path)) => {
                let exclude: HashSet<&str> = path.iter().map(|s| s.as_str()).collect();
                let before = current_sections.len();
                let next_sections: Vec<SkillSection> = std::mem::take(&mut current_sections)
                    .into_iter()
                    .filter(|s| !exclude.contains(s.slug.as_str()))
                    .collect();
                let next_requested: Vec<String> = std::mem::take(&mut current_requested)
                    .into_iter()
                    .filter(|s| !exclude.contains(s.as_str()))
                    .collect();
                if next_sections.len() == before {
                    return empty_block();
                }
                if next_requested.is_empty() {
                    return empty_block();
                }
                current_sections = next_sections;
                current_requested = next_requested;
            }
        }
    }
    empty_block()
}

pub(crate) struct SkillChoice {
    pub(crate) slugs: Vec<String>,
    // Whether the priority order stood in: reported beside the result on
    // the app side, and pinned by the conformance fixtures here.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fallback: bool,
}

/// How many sections the budget takes: derived from the worst-case block,
/// exactly as the app derives it — four fit under the composed ceiling and
/// five do not.
pub(crate) fn max_automatic_skill_sections(manifest: &Manifest) -> usize {
    let widest_title = page_index(manifest)
        .iter()
        .map(|s| utf16_len(s.title.as_str()))
        .max()
        .unwrap_or(0);
    let block = manifest.ceilings.section + utf16_len("## ") + widest_title + utf16_len("\n\n");
    let with_separator = block + utf16_len(manifest.section_separator.as_str());
    (manifest.ceilings.doctrine + utf16_len(manifest.section_separator.as_str())) / with_separator
}

/// Matched selection for a delegated child: the ranker orders the page
/// corpus for the raw prompt, the baseline is prepended, the head is capped
/// by the shared budget. A weak query falls back to the priority order and
/// lets the composed budget keep its head. A broken manifest degrades to an
/// empty choice — no doctrine — rather than refusing the creation.
pub(crate) fn match_skill_choice(prompt: &str) -> SkillChoice {
    let Some(manifest) = manifest() else {
        return SkillChoice {
            slugs: Vec::new(),
            fallback: true,
        };
    };
    let index = page_index(manifest);
    let (ranking, fallback) = rank_slugs_for_query(prompt, &index, manifest);
    if fallback {
        return SkillChoice {
            slugs: index.into_iter().map(|s| s.slug).collect(),
            fallback: true,
        };
    }
    let baseline: HashSet<&str> = manifest.baseline.iter().map(|s| s.as_str()).collect();
    let routed_budget =
        max_automatic_skill_sections(manifest).saturating_sub(manifest.baseline.len());
    let routed: Vec<String> = ranking
        .into_iter()
        .filter(|slug| !baseline.contains(slug.as_str()))
        .take(routed_budget)
        .collect();
    let known: HashSet<&str> = index.iter().map(|s| s.slug.as_str()).collect();
    let mut seen = HashSet::new();
    let mut applied = Vec::new();
    for slug in manifest.baseline.iter().chain(routed.iter()) {
        if !known.contains(slug.as_str()) || !seen.insert(slug.clone()) {
            continue;
        }
        applied.push(slug.clone());
    }
    SkillChoice {
        slugs: applied,
        fallback: false,
    }
}

/// The framed block: the delimiters, the disclaimer that the block is
/// reference material rather than a request, and the neutralization of a
/// delimiter the corpus itself smuggles in. Empty composes to empty, so a
/// caller that has nothing to frame says nothing.
pub(crate) fn embed_doctrine_block(composed: &str, manifest: &Manifest) -> String {
    if composed.is_empty() {
        return String::new();
    }
    let neutralized = composed
        .replace(
            manifest.delimiters.begin.as_str(),
            manifest.delimiters.removed.as_str(),
        )
        .replace(
            manifest.delimiters.end.as_str(),
            manifest.delimiters.removed.as_str(),
        );
    [
        manifest.delimiters.begin.as_str(),
        manifest.delimiters.disclaimer.as_str(),
        neutralized.as_str(),
        manifest.delimiters.end.as_str(),
    ]
    .join("\n\n")
}

/// The daemon-authored preamble for one creation's first send. A non-Design
/// creation reads exactly what it always read; a Design child's preamble
/// carries the matched doctrine — framed, disclaimed and restated — between
/// the daemon's own preamble and the creator's header, never as the
/// creator's words and never in the spawn field the card prints.
pub(crate) fn child_preamble(
    is_design_craft: bool,
    agent_preamble: &str,
    creator_lead_in: &str,
    raw_prompt: &str,
) -> String {
    if !is_design_craft {
        return format!("{agent_preamble}\n\n{creator_lead_in}");
    }
    let choice = match_skill_choice(raw_prompt);
    let sources: Vec<(&str, &str)> = super::design_craft_corpus::EMBEDDED_SOURCES
        .iter()
        .map(|s| (s.path, s.text))
        .collect();
    let Some(manifest) = manifest() else {
        return format!("{agent_preamble}\n\n{creator_lead_in}");
    };
    let block = build_skill_block(&sources, &choice.slugs, manifest);
    let doctrine = embed_doctrine_block(&block.text, manifest);
    if doctrine.is_empty() {
        return format!("{agent_preamble}\n\n{creator_lead_in}");
    }
    format!(
        "{agent_preamble}\n\n{doctrine}\n\n{}\n\n{creator_lead_in}",
        manifest.delimiters.restatement
    )
}
