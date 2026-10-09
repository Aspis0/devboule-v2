//! A profile whose provider the catalog no longer publishes stays in the file:
//! it loads, it saves back unchanged, and the only edit it takes is a change of
//! provider.

use super::*;

fn profile(id: &str, name: &str, provider: &str) -> AgentProfile {
    AgentProfile {
        id: id.to_string(),
        name: name.to_string(),
        icon: None,
        note: String::new(),
        spawn_prompt: String::new(),
        provider: provider.to_string(),
        model: "a-model".to_string(),
        model_provider: None,
        mode_id: "default".to_string(),
        thinking_option_id: None,
        features: serde_json::Map::new(),
        tool_overlay: Vec::new(),
        enabled_for_agents: false,
        idle_close_minutes: None,
    }
}

fn document(profiles: Vec<AgentProfile>) -> AgentProfilesDocument {
    AgentProfilesDocument {
        profiles,
        standing_instructions: "Stay brief.".to_string(),
    }
}

/// Writes two rows through `set`, then swaps the provider of the second on
/// disk: the state a removed user provider leaves behind. Returns the bytes
/// the file holds after the swap.
fn seeded_with_removed_provider(dir: &Path) -> Vec<u8> {
    let store = AgentProfilesStore::load(dir);
    store
        .set(document(vec![
            profile("p-good", "Good", "claude"),
            profile("p-gone", "Gone", "claude"),
        ]))
        .expect("the seed is admitted while claude is published");
    let path = dir.join(PROFILES_FILE);
    let text = std::fs::read_to_string(&path).expect("read the seed");
    let row = text.find("\"name\": \"Gone\"").expect("the Gone row");
    let swap = row
        + text[row..]
            .find("\"provider\": \"claude\"")
            .expect("its provider");
    let swapped = "\"provider\": \"claude\"".len();
    let edited = format!(
        "{}\"provider\": \"does-not-exist\"{}",
        &text[..swap],
        &text[swap + swapped..]
    );
    std::fs::write(&path, edited.as_bytes()).expect("write the removed provider");
    edited.into_bytes()
}

#[test]
fn a_removed_provider_row_loads_and_an_unchanged_save_writes_it_back_byte_for_byte() {
    let dir = crate::test_dirs::test_temp_dir("devboule-retained-profiles");
    let before = seeded_with_removed_provider(&dir);
    let store = AgentProfilesStore::load(&dir);
    let loaded = store.document();
    assert_eq!(
        loaded.profiles.len(),
        2,
        "the removed row is kept, not dropped at load"
    );
    assert_eq!(loaded.profiles[1].provider, "does-not-exist");
    store.set(loaded).expect("an unchanged save is admitted");
    assert_eq!(
        std::fs::read(dir.join(PROFILES_FILE)).expect("read"),
        before,
        "saving the document back rewrites nothing"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saving_another_profile_keeps_the_removed_row_as_it_was() {
    let dir = crate::test_dirs::test_temp_dir("devboule-retained-profiles");
    seeded_with_removed_provider(&dir);
    let store = AgentProfilesStore::load(&dir);
    let mut edited = store.document();
    let gone_before = edited.profiles[1].clone();
    edited.profiles[0].name = "Renamed".to_string();
    store
        .set(edited)
        .expect("a save of the other profile is admitted");
    let reopened = AgentProfilesStore::load(&dir).document();
    assert_eq!(reopened.profiles[0].name, "Renamed");
    assert_eq!(reopened.profiles[1], gone_before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_removed_row_takes_only_a_change_of_provider() {
    let dir = crate::test_dirs::test_temp_dir("devboule-retained-profiles");
    seeded_with_removed_provider(&dir);
    let store = AgentProfilesStore::load(&dir);
    let mut renamed = store.document();
    renamed.profiles[1].name = "Gone, renamed".to_string();
    let error = store
        .set(renamed)
        .expect_err("keeping the removed provider admits no other edit");
    assert!(error.to_string().contains("not installed"), "{error}");
    let mut repicked = store.document();
    repicked.profiles[1].provider = "claude".to_string();
    store
        .set(repicked)
        .expect("picking a published provider is admitted");
    assert_eq!(store.document().profiles[1].provider, "claude");
    let _ = std::fs::remove_dir_all(&dir);
}
