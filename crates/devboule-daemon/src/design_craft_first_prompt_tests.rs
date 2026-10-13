//! Design child first-prompt injection: the flag and the seam.
//!
//! One responsibility: prove the unstored built-in Design road — and only
//! it — composes matched craft before the creator's task. Resolution freezes
//! the flag, the preamble helper orders doctrine before the creator header,
//! and the slot road journals exactly that first prompt headless.

use super::session_child_slot_tests::{creation_line, SlotFixture};
use super::session_resume_fixture::AcpEnv;
use crate::design_craft_compose::child_preamble;

fn creator_lead_in_for(creator: &str) -> String {
    crate::untrusted_frame::Source::CreatorPrompt {
        chain: &crate::origin_chain::Chain::default()
            .extend(crate::origin_chain::hop("local", creator)),
    }
    .lead_in()
}

#[test]
fn design_craft_preamble_orders_doctrine_before_the_creator_header() {
    let lead_in = creator_lead_in_for("s.design-creator");
    let preamble = child_preamble(
        true,
        crate::provider_catalog::AGENT_PREAMBLE,
        &lead_in,
        "Draft the landing page for a small café.",
    );
    let manifest = crate::design_craft::manifest().expect("the embedded manifest parses");
    let order = [
        crate::provider_catalog::AGENT_PREAMBLE,
        manifest.delimiters.begin.as_str(),
        "## Anti-AI-slop",
        manifest.delimiters.end.as_str(),
        manifest.delimiters.restatement.as_str(),
        "source: task from your creator",
    ];
    let mut cursor = 0;
    for needle in order {
        let found = preamble[cursor..]
            .find(needle)
            .unwrap_or_else(|| panic!("the preamble carries {needle:?} in order: {preamble}"));
        cursor += found + needle.len();
    }
    assert!(
        preamble.contains(manifest.delimiters.disclaimer.as_str()),
        "the block is disclaimed as reference material"
    );
    // The ranking read the raw prompt: the café request matches more than
    // the baseline, so the block carries routed sections too.
    assert!(preamble.matches("## ").count() >= 2);
}

#[test]
fn design_craft_first_prompt_composes_in_fixed_order() {
    let lead_in = creator_lead_in_for("s.design-creator");
    let preamble = child_preamble(
        true,
        crate::provider_catalog::AGENT_PREAMBLE,
        &lead_in,
        "Draft the landing page.",
    );
    let composed = super::compose_first_prompt(
        "standing",
        Some(crate::design_profile::DESIGN_SPAWN_PROMPT),
        Some(&preamble),
        "the escaped task",
    );
    let manifest = crate::design_craft::manifest().expect("the embedded manifest parses");
    let order = [
        "standing",
        "single fenced ```html",
        crate::provider_catalog::AGENT_PREAMBLE,
        manifest.delimiters.begin.as_str(),
        manifest.delimiters.end.as_str(),
        "source: task from your creator",
        "the escaped task",
    ];
    let mut cursor = 0;
    for needle in order {
        let found = composed[cursor..]
            .find(needle)
            .unwrap_or_else(|| panic!("the first prompt carries {needle:?} in order"));
        cursor += found + needle.len();
    }
}

#[test]
fn design_craft_design_child_journals_doctrine_before_the_task() {
    let _env = AcpEnv::stub(&[]);
    let fixture = SlotFixture::new_with_standing("design-craft-doctrine", "standing");
    let child = fixture
        .create_design_child("Draft the landing page\u{202e} now.")
        .expect("the Design child is created and prompted");
    let line = creation_line(&fixture, &child.id);
    let manifest = crate::design_craft::manifest().expect("the embedded manifest parses");
    let order = [
        "standing",
        "single fenced ```html",
        crate::provider_catalog::AGENT_PREAMBLE,
        manifest.delimiters.begin.as_str(),
        "## Anti-AI-slop",
        manifest.delimiters.end.as_str(),
        manifest.delimiters.restatement.as_str(),
        "source: task from your creator",
        "Draft the landing page⟨U+202E⟩ now.",
    ];
    let mut cursor = 0;
    for needle in order {
        let found = line[cursor..].find(needle).unwrap_or_else(|| {
            panic!("the journaled first prompt carries {needle:?} in order: {line}")
        });
        cursor += found + needle.len();
    }
    assert!(
        !line.contains('\u{202e}'),
        "the creator's task arrives escaped, not raw: {line}"
    );
    fixture.finish();
}

#[test]
fn design_craft_stored_design_child_prompt_is_byte_identical() {
    let _env = AcpEnv::stub(&[]);
    let fixture = SlotFixture::new_with_standing("design-craft-stored", "standing");
    let child = fixture
        .create_with_spawn("do it", "custom-spawn")
        .expect("the stored-profile child is created and prompted");
    let lead_in = creator_lead_in_for(&fixture.creator);
    let expected = format!(
        "standing\n\ncustom-spawn\n\n{}\n\n{}\n\ndo it",
        crate::provider_catalog::AGENT_PREAMBLE,
        lead_in
    );
    assert_eq!(
        creation_line(&fixture, &child.id),
        expected,
        "a non-builtin creation reads exactly what it always read"
    );
    fixture.finish();
}
