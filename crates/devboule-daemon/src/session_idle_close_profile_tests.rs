//! The profile's half of the idle-close timer (D5): the switch that means
//! never, a custom value honoured, and a settings edit reaching a child that
//! is already running — the sweep reads the profile at every pass instead of
//! the minutes the child was born under.

use super::session_idle_close_tests::{
    armed, idle_state, linked_child, linked_creator, minutes, shut_down,
};
use super::tests::test_owner;
use super::*;

/// A linked child on a profile that names `minutes` (`None` is the profile
/// saying nothing, which the sweep reads as the default).
fn profile_child(
    state: &Arc<ServerState>,
    label: &str,
    owner: &OwnerId,
    creator: &str,
    minutes: Option<u32>,
) -> String {
    let registry = &state.sessions;
    let profile_id = format!("p-{label}");
    state
        .agent_profiles
        .set(devboule_protocol::AgentProfilesDocument {
            profiles: vec![devboule_protocol::AgentProfile {
                id: profile_id.clone(),
                name: format!("Profile {label}"),
                icon: None,
                note: String::new(),
                spawn_prompt: String::new(),
                provider: "claude".to_string(),
                model: "claude-opus-4-6".to_string(),
                mode_id: "default".to_string(),
                thinking_option_id: None,
                features: serde_json::Map::new(),
                tool_overlay: Vec::new(),
                enabled_for_agents: true,
                idle_close_minutes: minutes,
            }],
            standing_instructions: String::new(),
        })
        .expect("the store admits this document");
    let child = linked_child(registry, &format!("idle-{label}-child"), owner, creator);
    {
        let mut map = registry.inner.lock().expect("registry");
        let live = map
            .get_mut(&child)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("live entry");
        live.metadata.profile_id = Some(profile_id);
    }
    child
}

#[test]
fn a_profile_that_says_never_never_closes() {
    let (state, dir) = idle_state("never");
    let registry = &state.sessions;
    let owner = test_owner("idle-never-user", "idle-never-client");
    let creator = "idle-never-creator";
    linked_creator(registry, creator, &owner);
    let child = profile_child(&state, "never", &owner, creator, Some(0));

    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(
        armed(registry, &child),
        None,
        "the switch off is not a spell that runs very slowly"
    );
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + Duration::from_secs(60 * 60 * 24 * 400)),
        0
    );
    assert!(
        registry
            .inner
            .lock()
            .expect("registry")
            .contains_key(&child),
        "a year of idleness closes nothing"
    );
    shut_down(&state, &dir);
}

#[test]
fn a_custom_idle_close_value_is_honoured() {
    let (state, dir) = idle_state("custom");
    let registry = &state.sessions;
    let owner = test_owner("idle-custom-user", "idle-custom-client");
    let creator = "idle-custom-creator";
    linked_creator(registry, creator, &owner);
    let child = profile_child(&state, "custom", &owner, creator, Some(2));

    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(armed(registry, &child), Some(start));
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(1)),
        0
    );
    assert!(
        registry
            .inner
            .lock()
            .expect("registry")
            .contains_key(&child),
        "one minute of a two-minute timer"
    );
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(2)),
        1
    );
    assert!(!registry
        .inner
        .lock()
        .expect("registry")
        .contains_key(&child));
    shut_down(&state, &dir);
}

#[test]
fn a_settings_edit_reaches_a_child_already_running() {
    let (state, dir) = idle_state("edit");
    let registry = &state.sessions;
    let owner = test_owner("idle-edit-user", "idle-edit-client");
    let creator = "idle-edit-creator";
    linked_creator(registry, creator, &owner);
    let child = profile_child(&state, "edit", &owner, creator, None);

    // The default: a profile that says nothing runs the thirty-minute clock.
    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(armed(registry, &child), Some(start));

    // The human lowers the timer while the child is running: the sweep reads
    // the profile again rather than the minutes the child was born under.
    state
        .agent_profiles
        .set(devboule_protocol::AgentProfilesDocument {
            profiles: vec![devboule_protocol::AgentProfile {
                id: "p-edit".to_string(),
                name: "Profile edit".to_string(),
                icon: None,
                note: String::new(),
                spawn_prompt: String::new(),
                provider: "claude".to_string(),
                model: "claude-opus-4-6".to_string(),
                mode_id: "default".to_string(),
                thinking_option_id: None,
                features: serde_json::Map::new(),
                tool_overlay: Vec::new(),
                enabled_for_agents: true,
                idle_close_minutes: Some(1),
            }],
            standing_instructions: String::new(),
        })
        .expect("the store admits this document");

    assert_eq!(
        registry.sweep_idle_close_children(&state, start + minutes(1)),
        1,
        "the edit landed on a child that was already running"
    );
    assert!(!registry
        .inner
        .lock()
        .expect("registry")
        .contains_key(&child));
    shut_down(&state, &dir);
}
