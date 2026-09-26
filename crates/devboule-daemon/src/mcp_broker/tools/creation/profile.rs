//! Resolving the profile a creation names, and the profile list it serves.

use serde_json::{json, Value};

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
    pub(in crate::mcp_broker) mode: String,
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
    Ok(ResolvedProfile {
        id: profile.id.clone(),
        name: profile.name.clone(),
        provider: profile.provider.clone(),
        model: profile.model.clone(),
        mode: profile.mode_id.clone(),
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
/// own dictionary answering for the mode the profile names.
pub(in crate::mcp_broker) fn predicted_unattended(
    provider: &str,
    mode_id: &str,
) -> devboule_protocol::UnattendedState {
    crate::peer_policy::unattended_mode(
        crate::provider_catalog::session_kind_for(provider),
        Some(mode_id),
    )
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
    let document = store.document();
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
                "unattended": predicted_unattended(&profile.provider, &profile.mode_id),
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
