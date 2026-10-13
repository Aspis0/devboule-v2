//! Embedded corpus integrity: the files, the manifest, the notices.
//!
//! One responsibility: prove the embedded corpus IS the frontend's corpus —
//! same file set, same manifest order, a clean strict check, and the
//! provenance notices traveling with it — so a drift between the two sides
//! fails here instead of reaching a child.

use std::collections::HashSet;

use crate::design_craft::{manifest, validate_corpus};

const VERSION_TEXT: &str = include_str!("../../../src/features/design/craft/VERSION");
const LICENSE_TEXT: &str = include_str!("../../../src/features/design/craft/LICENSE.refero_skill");

fn craft_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../src/features/design/craft")
}

#[test]
fn design_craft_corpus_passes_the_strict_first_party_check() {
    assert!(
        validate_corpus().is_empty(),
        "first-party problems: {:?}",
        validate_corpus()
    );
}

#[test]
fn design_craft_embedded_files_are_the_craft_directory() {
    let mut on_disk: HashSet<String> = HashSet::new();
    let entries = std::fs::read_dir(craft_dir()).expect("the craft directory lists");
    for entry in entries {
        let name = entry.expect("a directory entry").file_name();
        let name = name.to_string_lossy().into_owned();
        if name.ends_with(".md") {
            on_disk.insert(format!("craft/{name}"));
        }
    }
    let embedded: HashSet<String> = crate::design_craft_corpus::EMBEDDED_SOURCES
        .iter()
        .map(|s| s.path.to_string())
        .collect();
    assert_eq!(
        embedded, on_disk,
        "an added or removed craft file fails here, not in a child"
    );
    assert!(craft_dir().join("VERSION").is_file());
    assert!(craft_dir().join("LICENSE.refero_skill").is_file());
}

#[test]
fn design_craft_priority_lists_every_embedded_section() {
    let manifest = manifest().expect("the embedded manifest parses");
    let embedded: HashSet<&str> = crate::design_craft_corpus::EMBEDDED_SOURCES
        .iter()
        .map(|s| {
            s.path
                .rsplit('/')
                .next()
                .expect("an embedded path")
                .split('.')
                .next()
                .expect("a filename stem")
        })
        .collect();
    let priority: HashSet<&str> = manifest.priority.iter().map(|s| s.as_str()).collect();
    assert_eq!(
        priority, embedded,
        "a section without a priority slot sorts last silently — list it"
    );
}

#[test]
fn design_craft_provenance_travels_with_the_corpus() {
    assert!(
        VERSION_TEXT.contains("Refero"),
        "the provenance record names its first origin"
    );
    assert!(
        VERSION_TEXT.contains("open-design"),
        "the provenance record names its second origin"
    );
    assert!(
        LICENSE_TEXT.contains("Permission is hereby granted"),
        "the MIT grant travels with the derived sections"
    );
}

#[test]
fn design_craft_manifest_stopwords_are_sorted_and_live() {
    let manifest = manifest().expect("the embedded manifest parses");
    assert!(!manifest.stopwords.is_empty());
    let mut sorted = manifest.stopwords.clone();
    sorted.sort();
    assert_eq!(
        manifest.stopwords, sorted,
        "the ranker contract keeps the list sorted"
    );
    assert!(manifest.stopwords.contains(&"the".to_string()));
    assert!(manifest.min_token_length >= 1);
}
