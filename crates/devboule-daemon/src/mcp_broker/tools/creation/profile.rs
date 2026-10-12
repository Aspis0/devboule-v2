//! Resolving the profile a creation names, and the profile list it serves.

use devboule_protocol::SessionKind;
use serde_json::{json, Value};

use super::posture::{agent_child_mode, ModeNote};
use super::request::CreationChoice;
use crate::design_profile;

/// The name a profile's labels and the creation record are keyed on: the
/// catalog's own spelling of the name, trimmed exactly as the store trims it.
/// The comparison form of the name a creation asked for and of every stored
/// name it is matched against: NFC, because canonically equivalent spellings
/// are one name to a human reading the list, and trimmed, exactly as the
/// store canonicalises. Names are stored as typed; only the match is
/// normalised.
fn profile_name_key(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    name.trim().nfc().collect::<String>()
}

/// One profile, resolved for one creation: what the store said at the moment of
/// the call and nothing that was cached.
#[derive(Debug)]
pub(in crate::mcp_broker) struct ResolvedProfile {
    /// The profile's identity, which is what the session records.
    pub(in crate::mcp_broker) id: String,
    /// The name the human ticked, which is what the card and the creator's
    /// transcript show.
    pub(in crate::mcp_broker) name: String,
    pub(in crate::mcp_broker) provider: String,
    /// The provider's own model id, exactly as saved. Carried because the card
    /// states what the human is being asked to approve, and because nothing may
    /// substitute it.
    pub(in crate::mcp_broker) model: String,
    /// The provider serving that model, when the profile stores the pair.
    /// `None` is what older builds wrote: the spawn path falls back to the
    /// bare id, exactly as it reads those profiles today.
    pub(in crate::mcp_broker) model_provider: Option<String>,
    pub(in crate::mcp_broker) mode: String,
    /// What the creator is told about the mode the child started in, when it is not the mode asked for.
    pub(in crate::mcp_broker) mode_note: Option<ModeNote>,
    /// The provider's thinking option, exactly as saved.
    pub(in crate::mcp_broker) thinking_option_id: Option<String>,
    /// The provider's feature values, exactly as saved.
    pub(in crate::mcp_broker) features: serde_json::Map<String, Value>,
    /// The profile's spawn prompt, exactly as the store canonicalised it —
    /// trimmed, empty means none. Read at the moment of the call: the card
    /// names it and the creation carries it, so an edit to the profile after
    /// the creation began changes neither.
    pub(in crate::mcp_broker) spawn_prompt: String,
    pub(in crate::mcp_broker) overlay: crate::provider_catalog::ToolOverlay,
}

/// Resolve the profile a creation named, out of the profiles the human ticked.
///
/// Every refusal here is one of §2's sentences, and they are in the order that
/// keeps the answers honest:
///
/// 1. **The list is read now.** `AgentProfilesStore::document` is asked on every
///    call and nothing is cached per session, so a profile the human enabled or
///    un-ticked while an agent was reading `devboule_list_profiles` is answered
///    by the list as it stands when the creation is attempted.
/// 2. **No ticked profile at all** is refused before the requested name is even
///    looked at, and the sentence names **no** profile. That is not politeness:
///    a refusal that said "the profile *X* exists but is not enabled" would tell
///    a caller what it is not allowed to see, and the list an agent reads is the
///    enabled set and nothing else.
/// 3. **A name that is unknown or unticked** gets the sentence that sends the
///    caller to the list, which is where the answer is.
/// 4. **A name two ticked profiles share** is refused rather than resolved.
///    The store refuses such a document outright (`check_document`), so this
///    arm is the belt behind that rule: a resolution that cannot name which
///    profile it means must not pick one, and "the first one" would be picking
///    a provider the human did not name.
pub(in crate::mcp_broker) fn resolve_profile(
    store: &crate::agent_profiles::AgentProfilesStore,
    requested: &str,
) -> Result<ResolvedProfile, String> {
    let document = store.document();
    let enabled: Vec<&devboule_protocol::AgentProfile> = document
        .profiles
        .iter()
        .filter(|profile| profile.enabled_for_agents)
        .collect();
    if enabled.is_empty() {
        return Err("no profile is enabled for agents".to_string());
    }
    let wanted = profile_name_key(requested);
    let matching: Vec<&devboule_protocol::AgentProfile> = enabled
        .into_iter()
        .filter(|profile| profile_name_key(&profile.name) == wanted)
        .collect();
    let profile = match matching.as_slice() {
        [] => return Err("unknown profile; call devboule_list_profiles".to_string()),
        [one] => *one,
        many => return Err(format!("more than one profile is called {}", many[0].name)),
    };
    if crate::provider_catalog::catalog_provider_id_for(
        &crate::session::catalog_registry(),
        &profile.provider,
    )
    .is_none()
    {
        return Err(format!(
            "profile {} uses a provider that is not installed; pick another provider for it in Settings",
            profile.name
        ));
    }
    Ok(ResolvedProfile {
        id: profile.id.clone(),
        name: profile.name.clone(),
        provider: profile.provider.clone(),
        model: profile.model.clone(),
        model_provider: profile.model_provider.clone(),
        mode: profile.mode_id.clone(),
        mode_note: None,
        thinking_option_id: profile.thinking_option_id.clone(),
        features: profile.features.clone(),
        spawn_prompt: profile.spawn_prompt.clone(),
        // The profile's own deny list, applied on top of the provider's stored
        // policy — the same two places a preset's overlay was applied. The store
        // has already refused a name outside the broker's table, so this can
        // only ever remove a tool the broker serves.
        overlay: crate::provider_catalog::ToolOverlay::from_profile_names(&profile.tool_overlay),
    })
}

/// The move tool's profile resolution (slice 5b §2 check 3, Pass A): the one
/// resolver, [`resolve_profile`], plus the third refusal §1.2 demands.
///
/// `resolve_profile` is the create surface's resolver and deliberately
/// conflates unticked with unknown — the list an agent reads is the ticked set
/// and nothing else, so the sentence that sends the caller back to the list is
/// the honest one there. The move surface is bound to the three-state
/// discipline instead: a name the store holds but the human has not ticked is
/// its own refusal, distinct from a name nobody ever wrote. Only the sentence
/// is refined: the matching, the ambiguity refusal and the read-now rule all
/// stay `resolve_profile`'s, so the two surfaces cannot drift into a second
/// resolver with a second ambiguity answer.
pub(in crate::mcp_broker) fn resolve_profile_for_move(
    store: &crate::agent_profiles::AgentProfilesStore,
    requested: &str,
) -> Result<crate::session::ChildProfileFacts, String> {
    match resolve_profile(store, requested) {
        Ok(profile) => Ok(crate::session::ChildProfileFacts {
            profile_id: profile.id,
            mode_id: profile.mode,
            model: profile.model,
            model_provider: profile.model_provider,
            thinking_option_id: profile.thinking_option_id,
        }),
        Err(message) => {
            let wanted = profile_name_key(requested);
            let unticked = store.document().profiles.iter().any(|profile| {
                profile_name_key(&profile.name) == wanted && !profile.enabled_for_agents
            });
            if unticked {
                Err(format!(
                    "the profile '{wanted}' exists but the human has not enabled it for agents; only a ticked profile can be moved onto"
                ))
            } else {
                Err(message)
            }
        }
    }
}

/// The per-profile **prediction** the list and the card serve (F6): the same
/// tri-state the child's birth will derive, judged before the child exists
/// from the profile's mode and the family that mode would be delivered in.
/// No session exists yet, so nothing here is observed — it is the delivery's
/// own dictionary answering for the mode the profile names. A mode that
/// answers its own prompts without the broker — Claude's `auto` classifier —
/// predicts `Yes` like a broker-answered mode does: the tick over it stands,
/// and the card must name the mode that answers rather than claim it asks.
pub(in crate::mcp_broker) fn predicted_unattended(
    provider: &str,
    mode_id: &str,
) -> devboule_protocol::UnattendedState {
    let kind = crate::provider_catalog::session_kind_for(provider);
    if crate::provider_catalog::mode_self_answers(Some(kind.clone()), mode_id) {
        return devboule_protocol::UnattendedState::Yes;
    }
    crate::peer_policy::unattended_mode(kind, Some(mode_id))
}

/// What one creation runs, resolved from its choice: a profile the human ticked
/// (see [`resolve_profile`]), or the provider and settings the caller named.
///
/// A direct creation has no stored profile, so its id is empty and it carries no
/// features, no spawn prompt and no model provider; the provider and model are
/// the caller's, and the spawn path validates them against what the provider
/// offers exactly as it does a profile's. A preset supplies the mode and the tool
/// overlay of its cell for that provider; without one the caller's mode stands,
/// and without that the child takes its creator's posture (`creator`, the
/// creator's family and current mode).
pub(in crate::mcp_broker) fn resolve_creation(
    store: &crate::agent_profiles::AgentProfilesStore,
    choice: &CreationChoice,
    creator: Option<(&SessionKind, &str)>,
    caller_provider: Option<&str>,
) -> Result<ResolvedProfile, String> {
    let (provider, model, mode, effort, preset) = match choice {
        CreationChoice::Profile(name) => {
            if design_profile::is_unstored_design(&store.document().profiles, name) {
                return resolve_design(creator, caller_provider);
            }
            return resolve_profile(store, name);
        }
        CreationChoice::Direct {
            provider,
            model,
            mode,
            effort,
            preset,
        } => (provider, model, mode, effort, preset),
    };
    let catalog_id = crate::provider_catalog::catalog_provider_id_for(
        &crate::session::catalog_registry(),
        provider,
    )
    .ok_or_else(|| format!("provider {provider} is not installed"))?;
    let (requested, overlay) = match preset {
        Some(preset_id) => {
            let (_, cell) = crate::provider_catalog::resolve_agent_preset(preset_id, &catalog_id)?;
            (Some(cell.mode), cell.overlay)
        }
        None => (mode.as_deref(), crate::provider_catalog::ToolOverlay::NONE),
    };
    let child = crate::provider_catalog::session_kind_for(&catalog_id);
    let chosen = agent_child_mode(&child, requested, creator)?;
    Ok(ResolvedProfile {
        id: String::new(),
        name: match preset {
            Some(preset_id) => format!("{catalog_id} {preset_id}"),
            None => format!(
                "{catalog_id} {}",
                model.as_deref().unwrap_or("default model")
            ),
        },
        provider: catalog_id,
        model: model.clone().unwrap_or_default(),
        model_provider: None,
        mode: chosen.mode,
        mode_note: chosen.note,
        thinking_option_id: effort.clone(),
        features: serde_json::Map::new(),
        spawn_prompt: String::new(),
        overlay,
    })
}

/// The built-in Design profile's resolution: the calling agent's provider, its
/// own default model (empty: the provider answers), its worker mode, and the
/// Design deny list. A caller that names no provider cannot run it.
fn resolve_design(
    creator: Option<(&SessionKind, &str)>,
    caller_provider: Option<&str>,
) -> Result<ResolvedProfile, String> {
    let caller = caller_provider.ok_or_else(|| {
        "the Design profile runs on the provider of the agent that creates it, and this caller names none"
            .to_string()
    })?;
    let catalog_id = crate::provider_catalog::catalog_provider_id_for(
        &crate::session::catalog_registry(),
        caller,
    )
    .ok_or_else(|| format!("provider {caller} is not installed"))?;
    let requested = crate::provider_catalog::resolve_agent_preset(
        crate::provider_catalog::AGENT_PRESET_WORKER,
        &catalog_id,
    )
    .ok()
    .map(|(_, cell)| cell.mode);
    let child = crate::provider_catalog::session_kind_for(&catalog_id);
    let chosen = agent_child_mode(&child, requested, creator)?;
    Ok(ResolvedProfile {
        id: design_profile::DESIGN_PROFILE_ID.to_string(),
        name: design_profile::DESIGN_PROFILE_NAME.to_string(),
        provider: catalog_id,
        model: String::new(),
        model_provider: None,
        mode: chosen.mode,
        mode_note: chosen.note,
        thinking_option_id: None,
        features: serde_json::Map::new(),
        spawn_prompt: design_profile::DESIGN_SPAWN_PROMPT.to_string(),
        overlay: crate::provider_catalog::ToolOverlay::from_profile_names(
            &design_profile::denied_names(),
        ),
    })
}

/// One `devboule_list_profiles` call (`create-from-profile`).
///
/// The ticked profiles, in the human's stored order and never sorted, as
/// `{name, note, provider, model, mode, unattended}`, the last a tri-state
/// **prediction** (`"yes" | "no" | "unknown"`) whose meaning the tool's
/// description spells out for the caller. Nothing else is served:
/// not the id (a caller names a profile by its name, and the id is the daemon's
/// key for the session it records), not a profile the human did not tick, and
/// not the standing instructions — those are not a profile's business to read.
///
/// `note` is verbatim and never truncated. It is the only thing a model has to
/// route work with, so a truncated note is a different instruction, not a
/// shorter display of the same one.
pub(in crate::mcp_broker) fn list_profiles(
    store: &crate::agent_profiles::AgentProfilesStore,
    id: &Value,
) -> Value {
    let document = design_profile::with_builtin(store.document());
    let profiles: Vec<Value> = document
        .profiles
        .iter()
        .filter(|profile| profile.enabled_for_agents)
        .map(|profile| {
            json!({
                "name": profile.name,
                "note": profile.note,
                "provider": profile.provider,
                "model": profile.model,
                "mode": profile.mode_id,
                // The prediction, not a promise: the same tri-state the
                // child's birth will derive, judged from the mode and the
                // family that mode would be delivered in. `unknown` is a
                // value here — an agent picking a profile *because it will
                // not ask* must get `unknown`, never a false `no`.
                // The built-in names no provider, so no answer about one is predicted.
                "unattended": if profile.provider.is_empty() {
                    devboule_protocol::UnattendedState::Unknown
                } else {
                    predicted_unattended(&profile.provider, &profile.mode_id)
                },
            })
        })
        .collect();
    let document = json!({ "profiles": profiles });
    let text = serde_json::to_string(&document)
        .unwrap_or_else(|error| format!("Could not encode the profile list: {error}"));
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": text}],
            "structuredContent": document,
            "isError": false,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::resolve_creation;
    use crate::agent_profiles::AgentProfilesStore;
    use crate::mcp_broker::tools::creation::request::CreationChoice;
    use devboule_protocol::SessionKind;

    fn direct(provider: &str, mode: Option<&str>) -> CreationChoice {
        CreationChoice::Direct {
            provider: provider.to_string(),
            model: Some("model-a".to_string()),
            mode: mode.map(str::to_string),
            effort: None,
            preset: None,
        }
    }

    fn store() -> AgentProfilesStore {
        AgentProfilesStore::load(&crate::test_dirs::test_temp_dir(
            "devboule-creation-profile",
        ))
    }

    /// A child named no mode takes its creator's posture, spelled for its own provider.
    #[test]
    fn a_child_without_a_mode_takes_the_creators_posture() {
        let resolved = resolve_creation(
            &store(),
            &direct("claude", None),
            Some((&SessionKind::Codex, "auto-review")),
            None,
        )
        .expect("the posture carries over");
        assert_eq!(resolved.mode, "auto");
    }

    /// A named mode that does not ask stands, even under a wider creator.
    #[test]
    fn a_named_mode_that_does_not_ask_stands_under_a_wider_creator() {
        let resolved = resolve_creation(
            &store(),
            &direct("claude", Some("acceptEdits")),
            Some((&SessionKind::Codex, "full-access")),
            None,
        )
        .expect("the named mode");
        assert_eq!(resolved.mode, "acceptEdits");
        assert_eq!(resolved.mode_note, None);
    }

    /// An agent's child cannot run in the asking mode the agent names: it takes
    /// its creator's posture, and the replaced mode is reported.
    #[test]
    fn an_agent_child_cannot_ask_for_default_under_a_reviewed_creator() {
        let resolved = resolve_creation(
            &store(),
            &direct("claude", Some("default")),
            Some((&SessionKind::Codex, "auto-review")),
            None,
        )
        .expect("the posture is taken");
        assert_eq!(resolved.mode, "auto");
        assert_eq!(
            resolved.mode_note,
            Some(super::super::posture::ModeNote::Replaced(
                "default".to_string()
            ))
        );
    }

    /// A preset's asking mode is replaced the same way; the preset's tool overlay stays.
    #[test]
    fn a_preset_child_takes_the_creators_posture() {
        let choice = CreationChoice::Direct {
            provider: "claude".to_string(),
            model: None,
            mode: None,
            effort: None,
            preset: Some("worker".to_string()),
        };
        let resolved = resolve_creation(
            &store(),
            &choice,
            Some((&SessionKind::Codex, "auto-review")),
            None,
        )
        .expect("the preset resolves under the posture");
        assert_eq!(resolved.mode, "auto");
    }

    /// Nothing to inherit from: the caller is told to name a mode.
    #[test]
    fn a_child_with_no_mode_and_no_creator_mode_is_refused() {
        let refused = resolve_creation(&store(), &direct("claude", None), None, None)
            .expect_err("no posture to take");
        assert!(refused.contains("mode is required"), "{refused}");
    }
}

#[cfg(test)]
mod design_tests {
    use super::resolve_creation;
    use crate::agent_profiles::AgentProfilesStore;
    use crate::mcp_broker::tools::creation::request::CreationChoice;
    use devboule_protocol::{AgentProfile, AgentProfilesDocument, SessionKind};
    use serde_json::json;

    fn design_choice() -> CreationChoice {
        CreationChoice::Profile("Design".to_string())
    }

    fn store(name: &str, profiles: Vec<AgentProfile>) -> AgentProfilesStore {
        let store = AgentProfilesStore::load(&crate::test_dirs::test_temp_dir(name));
        store
            .set(AgentProfilesDocument {
                profiles,
                standing_instructions: String::new(),
            })
            .expect("the store admits the document");
        store
    }

    fn person_design_row(provider: &str, enabled: bool) -> AgentProfile {
        AgentProfile {
            id: "p-design".to_string(),
            name: "Design".to_string(),
            icon: None,
            note: String::new(),
            spawn_prompt: String::new(),
            provider: provider.to_string(),
            model: "m-1".to_string(),
            model_provider: None,
            mode_id: "ask".to_string(),
            thinking_option_id: None,
            features: serde_json::Map::new(),
            tool_overlay: Vec::new(),
            enabled_for_agents: enabled,
            idle_close_minutes: None,
        }
    }

    /// With no row of the person's own, Design runs on the calling agent's
    /// provider, with the provider's own default model and the Design deny list.
    #[test]
    fn the_builtin_design_profile_runs_on_the_callers_codex_provider() {
        let resolved = resolve_creation(
            &store("devboule-design-builtin-codex", Vec::new()),
            &design_choice(),
            Some((&SessionKind::Codex, "auto")),
            Some("codex"),
        )
        .expect("the built-in Design resolves for a codex caller");
        assert_eq!(resolved.provider, "codex");
        assert_eq!(resolved.mode, "auto");
        assert!(
            resolved.model.is_empty(),
            "the provider's default model answers"
        );
        for name in crate::provider_catalog::DESIGN_DENIED {
            assert!(!resolved.overlay.allows(name), "{name} must be denied");
        }
    }

    #[test]
    fn the_builtin_design_profile_runs_on_the_callers_claude_provider() {
        let resolved = resolve_creation(
            &store("devboule-design-builtin-claude", Vec::new()),
            &design_choice(),
            Some((&SessionKind::Claude, "default")),
            Some("claude"),
        )
        .expect("the built-in Design resolves for a claude caller");
        assert_eq!(resolved.provider, "claude");
        assert_eq!(resolved.mode, "default");
    }

    #[test]
    fn the_builtin_design_profile_is_refused_without_a_caller_provider() {
        let refused = resolve_creation(
            &store("devboule-design-builtin-none", Vec::new()),
            &design_choice(),
            Some((&SessionKind::Codex, "auto")),
            None,
        )
        .expect_err("no provider to run on");
        assert!(refused.contains("provider"), "{refused}");
    }

    /// A Design row the person enabled overrides the built-in: their provider wins.
    #[test]
    fn a_design_row_of_the_persons_own_overrides_the_builtin() {
        let resolved = resolve_creation(
            &store(
                "devboule-design-override",
                vec![person_design_row("pi", true)],
            ),
            &design_choice(),
            Some((&SessionKind::Codex, "auto")),
            Some("codex"),
        )
        .expect("the person's Design resolves");
        assert_eq!(resolved.provider, "pi");
        assert_eq!(resolved.mode, "ask");
    }

    /// A Design row the person switched off hides the built-in too: it is not usable.
    #[test]
    fn a_switched_off_design_row_hides_the_builtin_from_creation() {
        let mut coder = person_design_row("codex", true);
        coder.id = "p-coder".to_string();
        coder.name = "Coder".to_string();
        let refused = resolve_creation(
            &store(
                "devboule-design-off",
                vec![coder, person_design_row("pi", false)],
            ),
            &design_choice(),
            Some((&SessionKind::Codex, "auto")),
            Some("codex"),
        )
        .expect_err("switched off");
        assert!(refused.contains("unknown profile"), "{refused}");
    }

    /// The listing an agent reads offers the built-in with its note, and makes
    /// no prediction about a provider it does not name.
    #[test]
    fn the_listing_offers_the_builtin_design_profile() {
        let answer = super::list_profiles(&store("devboule-design-listing", Vec::new()), &json!(9));
        let profiles = answer["result"]["structuredContent"]["profiles"]
            .as_array()
            .expect("the list");
        let design = profiles
            .iter()
            .find(|profile| profile["name"] == "Design")
            .expect("Design is listed");
        assert_eq!(design["unattended"], "unknown");
        assert!(design["note"]
            .as_str()
            .unwrap_or_default()
            .contains("Mockups"));
    }
}
