//! Matched composition parity: the daemon agrees with the app.
//!
//! One responsibility: prove the Rust port answers exactly as the
//! production TypeScript helpers did when the checked-in fixtures were
//! generated — same ordered slugs, same fallback, same dropped sections,
//! same composed text and UTF-16 counts — plus the shared edge behavior
//! (tolerant skips, delimiter neutralization, UTF-16 budgets).

use sha2::Digest;

use crate::design_craft::{manifest, page_index, utf16_len};
use crate::design_craft_compose::{
    build_skill_block, child_preamble, embed_doctrine_block, match_skill_choice,
    max_automatic_skill_sections,
};

const FIXTURES_JSON: &str = include_str!("../../../src/features/design/designCraftFixtures.json");

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureCorpusEntry {
    slug: String,
    title: String,
    description: String,
    body_sha256: String,
    body_utf16_len: usize,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureCase {
    name: String,
    prompt: String,
    slugs: Vec<String>,
    fallback: bool,
    dropped: Vec<String>,
    text: String,
    text_sha256: String,
    text_utf16_len: usize,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureEmbed {
    input: String,
    output: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixtures {
    version: u32,
    corpus: Vec<FixtureCorpusEntry>,
    cases: Vec<FixtureCase>,
    embed_sample: FixtureEmbed,
}

fn fixtures() -> Fixtures {
    serde_json::from_str(FIXTURES_JSON).expect("the conformance fixtures parse")
}

fn sha256_hex(text: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

fn embedded_sources() -> Vec<(&'static str, &'static str)> {
    crate::design_craft_corpus::EMBEDDED_SOURCES
        .iter()
        .map(|s| (s.path, s.text))
        .collect()
}

#[test]
fn design_craft_manifest_parses_with_a_sane_budget() {
    let manifest = manifest().expect("the embedded manifest parses");
    assert_eq!(manifest.ceilings.doctrine, 12000);
    assert_eq!(manifest.ceilings.section, 2500);
    assert_eq!(manifest.ceilings.description, 300);
    assert_eq!(manifest.baseline, vec!["anti-ai-slop".to_string()]);
    // Four worst-case sections fit under the composed ceiling and five do
    // not: the budget both sides derive, pinned so a corpus change that
    // moves it fails loudly instead of halving every selection silently.
    assert_eq!(max_automatic_skill_sections(manifest), 4);
}

#[test]
fn design_craft_embedded_sections_match_the_fixture_corpus() {
    let fixtures = fixtures();
    assert_eq!(fixtures.version, 1);
    let manifest = manifest().expect("the embedded manifest parses");
    let index = page_index(manifest);
    // The slide deck section is scoped away from the page corpus on both
    // sides; the manual catalogue keeps the whole list, which the page
    // narrowing test in the app suite pins.
    assert!(!index.iter().any(|s| s.slug == "slides"));
    let expected: Vec<String> = manifest
        .priority
        .iter()
        .filter(|slug| !manifest.scoped.contains_key(slug.as_str()))
        .cloned()
        .collect();
    assert_eq!(
        index.iter().map(|s| s.slug.clone()).collect::<Vec<_>>(),
        expected,
        "the page index is the priority minus the scoped sections"
    );
    assert_eq!(
        fixtures
            .corpus
            .iter()
            .map(|c| c.slug.clone())
            .collect::<Vec<_>>(),
        manifest.priority,
        "the fixture corpus travels in priority order"
    );
    // Page plus the scoped deck section is the whole catalogue the
    // fixtures pin: content parity covers every file, routing narrows it.
    let mut catalogue = page_index(manifest);
    for source in crate::design_craft_corpus::EMBEDDED_SOURCES {
        if source.path == "craft/slides.md" {
            catalogue.push(
                crate::design_craft::parse_skill_file(source.path, source.text)
                    .expect("the slides section parses"),
            );
        }
    }
    for entry in &fixtures.corpus {
        let section = catalogue
            .iter()
            .find(|s| s.slug == entry.slug)
            .unwrap_or_else(|| panic!("fixture slug {} is embedded", entry.slug));
        assert_eq!(section.title, entry.title, "title of {}", entry.slug);
        assert_eq!(
            section.description, entry.description,
            "description of {}",
            entry.slug
        );
        assert_eq!(
            sha256_hex(&section.body),
            entry.body_sha256,
            "body bytes of {}",
            entry.slug
        );
        assert_eq!(
            utf16_len(&section.body),
            entry.body_utf16_len,
            "body length of {}",
            entry.slug
        );
    }
}

#[test]
fn design_craft_matched_cases_match_the_fixtures() {
    let fixtures = fixtures();
    assert!(!fixtures.cases.is_empty(), "the fixed prompt set travels");
    let manifest = manifest().expect("the embedded manifest parses");
    let sources = embedded_sources();
    for case in &fixtures.cases {
        let choice = match_skill_choice(&case.prompt);
        assert_eq!(choice.slugs, case.slugs, "slugs for {}", case.name);
        assert_eq!(choice.fallback, case.fallback, "fallback for {}", case.name);
        let block = build_skill_block(&sources, &choice.slugs, manifest);
        assert_eq!(block.dropped, case.dropped, "dropped for {}", case.name);
        assert_eq!(block.text, case.text, "composed text for {}", case.name);
        assert_eq!(
            block.total_chars,
            utf16_len(&block.text),
            "total_chars counts UTF-16 units for {}",
            case.name
        );
        assert_eq!(
            block.total_chars, case.text_utf16_len,
            "length for {}",
            case.name
        );
        assert_eq!(
            sha256_hex(&block.text),
            case.text_sha256,
            "fingerprint for {}",
            case.name
        );
    }
    // The budget property the derivation exists for: every matched head
    // fits the ceiling, while the weak-query fallback carries the notice
    // that its tail was cut.
    for case in &fixtures.cases {
        if case.fallback {
            assert!(
                case.text.contains(&manifest.truncation_notice),
                "fallback case {} names its cut tail",
                case.name
            );
        } else {
            assert!(
                case.text_utf16_len <= manifest.ceilings.doctrine,
                "matched case {} fits the ceiling",
                case.name
            );
            assert!(
                case.slugs
                    .first()
                    .is_some_and(|slug| slug == "anti-ai-slop"),
                "matched case {} leads with the baseline",
                case.name
            );
        }
    }
}

#[test]
fn design_craft_embed_sample_matches() {
    let fixtures = fixtures();
    let manifest = manifest().expect("the embedded manifest parses");
    assert_eq!(
        embed_doctrine_block(&fixtures.embed_sample.input, manifest),
        fixtures.embed_sample.output,
        "framing and neutralization agree"
    );
}

fn synthetic_section(
    slug: &str,
    title: &str,
    body: &str,
    requires: &str,
) -> (&'static str, String) {
    // Leaked once per test run: synthetic sources need a stable address and
    // the suite never frees them, which is what `Box::leak` is for.
    let path: &'static str = Box::leak(format!("craft/{slug}.md").into_boxed_str());
    let text = format!(
        "---\nslug: {slug}\ntitle: {title}\ndescription: {title} rules.\nrequires: {requires}\n---\n\n{body}\n"
    );
    (path, text)
}

#[test]
fn design_craft_unknown_slug_names_itself() {
    let (path, text) = synthetic_section("alpha", "Alpha", "Alpha body.", "[]");
    let section =
        crate::design_craft::parse_skill_file(path, &text).expect("the synthetic section parses");
    match crate::design_craft_resolve::resolve_sections(&["nope".to_string()], &[section]) {
        Err(crate::design_craft_resolve::ResolveError::UnknownSlug(slug)) => {
            assert_eq!(slug, "nope")
        }
        other => panic!("an unknown slug refuses by name: {other:?}"),
    }
}

#[test]
fn design_craft_unknown_slug_composes_nothing() {
    let manifest = manifest().expect("the embedded manifest parses");
    let (path, text) = synthetic_section("alpha", "Alpha", "Alpha body.", "[]");
    let block = build_skill_block(&[(path, text.as_str())], &["nope".to_string()], manifest);
    assert!(block.text.is_empty() && block.dropped.is_empty());
}

#[test]
fn design_craft_malformed_source_is_skipped() {
    let manifest = manifest().expect("the embedded manifest parses");
    let (good_path, good_text) = synthetic_section("alpha", "Alpha", "Alpha body.", "[]");
    let bad = ("craft/broken.md", "no fences here\n");
    let block = build_skill_block(
        &[(bad.0, bad.1), (good_path, good_text.as_str())],
        &["alpha".to_string()],
        manifest,
    );
    assert!(block.text.contains("## Alpha"));
    assert!(block.dropped.is_empty());
}

#[test]
fn design_craft_cycle_drops_its_members_and_keeps_the_rest() {
    let manifest = manifest().expect("the embedded manifest parses");
    let (a_path, a_text) = synthetic_section("aaa", "Aaa", "Aaa body.", "[bbb]");
    let (b_path, b_text) = synthetic_section("bbb", "Bbb", "Bbb body.", "[aaa]");
    let (c_path, c_text) = synthetic_section("ccc", "Ccc", "Ccc body.", "[]");
    let block = build_skill_block(
        &[
            (a_path, a_text.as_str()),
            (b_path, b_text.as_str()),
            (c_path, c_text.as_str()),
        ],
        &["aaa".to_string(), "bbb".to_string(), "ccc".to_string()],
        manifest,
    );
    assert!(block.text.contains("## Ccc"), "the clean section survives");
    assert!(!block.text.contains("## Aaa") && !block.text.contains("## Bbb"));
}

#[test]
fn design_craft_embedded_delimiters_are_neutralized() {
    let manifest = manifest().expect("the embedded manifest parses");
    let forged = format!(
        "Smuggled {} and {} markers.",
        manifest.delimiters.begin, manifest.delimiters.end
    );
    let embedded = embed_doctrine_block(&forged, manifest);
    assert_eq!(
        embedded.matches(manifest.delimiters.begin.as_str()).count(),
        1,
        "one genuine opening delimiter"
    );
    assert_eq!(
        embedded.matches(manifest.delimiters.end.as_str()).count(),
        1,
        "one genuine closing delimiter"
    );
    assert!(embedded.contains(manifest.delimiters.removed.as_str()));
    assert!(embedded.contains(&manifest.delimiters.disclaimer));
}

#[test]
fn design_craft_ceiling_counts_utf16_units_not_chars() {
    let manifest = manifest().expect("the embedded manifest parses");
    // One astral character is one `char` and two UTF-16 units: a body of
    // 100 of them is 100 chars long and 200 units, and the composer must
    // report the units TypeScript's `length` would.
    let astral = "𝄞".repeat(100);
    let (path, text) = synthetic_section("uni", "Uni", &astral, "[]");
    let block = build_skill_block(&[(path, text.as_str())], &["uni".to_string()], manifest);
    assert_eq!(block.total_chars, utf16_len(&block.text));
    assert_eq!(block.total_chars, block.text.chars().count() + 100);

    // The drop decision reads units too: `aaa` (9,000 units) plus `bbb`
    // (2,000 chars, 4,000 units) is 11,000 chars but 13,000 units, over the
    // 12,000 ceiling — so `bbb` drops where a char count would keep it.
    let (a_path, a_text) = synthetic_section("aaa", "Aaa", &"a".repeat(9000), "[]");
    let (b_path, b_text) = synthetic_section("bbb", "Bbb", &"𝄞".repeat(2000), "[]");
    let block = build_skill_block(
        &[(a_path, a_text.as_str()), (b_path, b_text.as_str())],
        &["aaa".to_string(), "bbb".to_string()],
        manifest,
    );
    assert_eq!(block.dropped, vec!["bbb".to_string()]);
    assert!(block.text.contains(&manifest.truncation_notice));
}

#[test]
fn design_craft_non_design_preamble_is_untouched() {
    let lead_in = "[devboule: untrusted content]\nthe creator's header";
    assert_eq!(
        child_preamble(false, "AGENT_PREAMBLE", lead_in, "Draft the landing page."),
        "AGENT_PREAMBLE\n\n[devboule: untrusted content]\nthe creator's header"
    );
}
