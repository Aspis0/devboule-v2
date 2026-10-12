//! The built-in Design profile: the one profile the daemon always offers, unless
//! a row of the person's own holds the name. It names no provider of its own; a
//! Design agent runs on the provider of the agent that asked for it.

use devboule_protocol::{AgentProfile, AgentProfilesDocument};
use unicode_normalization::UnicodeNormalization;

use crate::provider_catalog::DESIGN_DENIED;

pub(crate) const DESIGN_PROFILE_NAME: &str = "Design";
pub(crate) const DESIGN_PROFILE_ID: &str = "design-builtin";

/// What an agent reads to decide to route a mockup here.
const DESIGN_NOTE: &str = "Mockups and page designs. One agent per mockup — start two for two directions. The result appears on the Design canvas; it writes no files.";

/// The contract the app's own Design runs give the model (`agentHost.ts`),
/// without the doctrine, which does not fit the spawn-prompt cap. The canvas
/// reads the last ```html block of the final reply, so the brief asks for it there.
pub(crate) const DESIGN_SPAWN_PROMPT: &str = concat!(
    "When you produce visual output, include a self-contained HTML fragment that renders the generated design.\n",
    "Put it in a single fenced ```html code block. Use inline CSS for all styling.\n",
    "Scripts will not run, so do not rely on JavaScript — use only HTML and CSS.\n",
    "If you produce more than one block, only the last one is used.\n",
    "Reply with the block and one sentence. Do not write files, run commands or take screenshots.",
);

pub(crate) fn denied_names() -> Vec<String> {
    DESIGN_DENIED.iter().map(|name| name.to_string()).collect()
}

/// The built-in row as the list and the settings panel read it. Its provider,
/// model and mode are empty: they are the calling agent's, not the row's.
pub(crate) fn row() -> AgentProfile {
    AgentProfile {
        id: DESIGN_PROFILE_ID.to_string(),
        name: DESIGN_PROFILE_NAME.to_string(),
        icon: None,
        note: DESIGN_NOTE.to_string(),
        spawn_prompt: DESIGN_SPAWN_PROMPT.to_string(),
        provider: String::new(),
        model: String::new(),
        model_provider: None,
        mode_id: String::new(),
        thinking_option_id: None,
        features: serde_json::Map::new(),
        tool_overlay: denied_names(),
        enabled_for_agents: true,
        idle_close_minutes: None,
    }
}

/// The document a reader sees: the person's rows, then the built-in, unless one
/// of the person's rows holds the name. Enabled or not, that row is the answer.
pub(crate) fn with_builtin(mut document: AgentProfilesDocument) -> AgentProfilesDocument {
    if !document
        .profiles
        .iter()
        .any(|profile| is_design_name(&profile.name))
    {
        document.profiles.push(row());
    }
    document
}

/// Whether a creation names the built-in: the name, with no row of the
/// person's holding it.
pub(crate) fn is_unstored_design(profiles: &[AgentProfile], requested: &str) -> bool {
    is_design_name(requested) && !profiles.iter().any(|profile| is_design_name(&profile.name))
}

/// The built-in row as the panel echoes it back unchanged: enabled, and with no
/// provider. The store does not keep it; only a row the person changed is kept.
pub(crate) fn is_echoed(profile: &AgentProfile) -> bool {
    profile.id == DESIGN_PROFILE_ID && profile.enabled_for_agents && profile.provider.is_empty()
}

/// A row that only switches the built-in off. It names no provider, model or
/// mode, because it holds nothing to run on.
pub(crate) fn is_switched_off(profile: &AgentProfile) -> bool {
    profile.id == DESIGN_PROFILE_ID && !profile.enabled_for_agents && profile.provider.is_empty()
}

fn is_design_name(name: &str) -> bool {
    name.trim().nfc().collect::<String>() == DESIGN_PROFILE_NAME
}
