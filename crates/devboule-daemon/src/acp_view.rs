//! Derive a UI view from an inbound ACP JSON-RPC envelope.
//!
//! The envelope is the source of truth. This module never mutates it: callers
//! journal the original object and, separately, publish the derived view.

use std::path::Path;

use crate::tool_paths::relativize_tool_path;
use devboule_protocol::{
    AvailableCommandView, SessionEvent, SessionModeStateView, SessionModeView, SessionModel,
    SessionModelEffort, ToolLocation, TurnUsage,
};

/// Kind of a JSON-RPC line. Requests carry a method *and* an id; treating a
/// request as a response is how the previous dispatcher went mute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AcpLineKind {
    Request { method: String },
    Notification { method: String },
    Response,
}

pub(crate) fn classify_line(value: &serde_json::Value) -> Option<AcpLineKind> {
    let method = value.get("method").and_then(serde_json::Value::as_str);
    let has_id = value.get("id").is_some();
    match (method, has_id) {
        (Some(method), true) => Some(AcpLineKind::Request {
            method: method.to_string(),
        }),
        (Some(method), false) => Some(AcpLineKind::Notification {
            method: method.to_string(),
        }),
        (None, true) => Some(AcpLineKind::Response),
        (None, false) => None,
    }
}

/// Derive the UI view from an inbound envelope. `None` means we do not model
/// this message yet; the envelope is still the source of truth and must be
/// kept.
pub(crate) fn view_from_envelope(
    value: &serde_json::Value,
    expected_session_id: &str,
) -> Option<SessionEvent> {
    view_from_envelope_in(value, expected_session_id, None)
}

pub(crate) fn view_from_envelope_in(
    value: &serde_json::Value,
    expected_session_id: &str,
    cwd: Option<&Path>,
) -> Option<SessionEvent> {
    if value.get("method").and_then(serde_json::Value::as_str) == Some("session/update") {
        return view_from_session_update(value, expected_session_id, cwd);
    }
    if value.get("method").and_then(serde_json::Value::as_str) == Some("_x.ai/models/update") {
        let params = value.get("params")?;
        return session_manifest_from_models_update(params, None);
    }
    if classify_line(value) == Some(AcpLineKind::Response) {
        return view_from_prompt_response(value, expected_session_id);
    }
    None
}

fn view_from_session_update(
    value: &serde_json::Value,
    expected_session_id: &str,
    cwd: Option<&Path>,
) -> Option<SessionEvent> {
    let params = value.get("params")?;
    if !expected_session_id.is_empty()
        && params.get("sessionId").and_then(serde_json::Value::as_str) != Some(expected_session_id)
    {
        return None;
    }
    let update = params.get("update")?;
    let message_id = update
        .get("messageId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    match update
        .get("sessionUpdate")
        .and_then(serde_json::Value::as_str)
    {
        Some("user_message_chunk") => {
            let text = text_from_content(update.get("content"))?;
            Some(SessionEvent::AgentUserMessage {
                message_id,
                text: text.to_string(),
            })
        }
        Some("agent_thought_chunk") => {
            let text = text_from_content(update.get("content"))?;
            Some(SessionEvent::AgentThought {
                message_id,
                text: text.to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            })
        }
        Some("agent_message_chunk") => {
            let text = text_from_content(update.get("content"))?;
            Some(SessionEvent::AgentMessage {
                message_id,
                text: text.to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            })
        }
        Some("available_commands_update") => {
            let commands = commands_from_update(update)?;
            Some(SessionEvent::AvailableCommands { commands })
        }
        Some("tool_call") => Some(SessionEvent::AgentToolCall {
            tool_call_id: update
                .get("toolCallId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            title: update
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            status: update
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("in_progress")
                .to_string(),
            kind: update
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            locations: locations_from_value(update.get("locations"), cwd, false),
            subagent_type: None,
            parent_tool_use_id: None,
            spawn_depth: None,
        }),
        Some("tool_call_update") => {
            let text = text_from_content(update.get("content")).map(str::to_string);
            Some(SessionEvent::AgentToolUpdate {
                tool_call_id: update
                    .get("toolCallId")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                status: update
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                text,
                title: update
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                kind: update
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                locations: locations_from_value(update.get("locations"), cwd, true),
                parent_tool_use_id: None,
                spawn_depth: None,
            })
        }
        _ => None,
    }
}

fn view_from_prompt_response(
    value: &serde_json::Value,
    expected_session_id: &str,
) -> Option<SessionEvent> {
    let result = value.get("result")?;
    let stop_reason = result
        .get("stopReason")
        .and_then(serde_json::Value::as_str)?
        .to_string();
    let meta = result.get("_meta");
    if let Some(session_id) = meta
        .and_then(|meta| meta.get("sessionId"))
        .and_then(serde_json::Value::as_str)
    {
        if session_id != expected_session_id {
            return None;
        }
    }
    let model_id = meta
        .and_then(|meta| meta.get("modelId"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let usage = meta.and_then(usage_from_meta).or_else(|| {
        result.get("usage").and_then(|usage| {
            usage_from_meta(usage).or_else(|| {
                let usage = TurnUsage {
                    input_tokens: usage.get("inputTokens").and_then(serde_json::Value::as_u64),
                    output_tokens: usage
                        .get("outputTokens")
                        .and_then(serde_json::Value::as_u64),
                    total_tokens: usage.get("totalTokens").and_then(serde_json::Value::as_u64),
                    thought_tokens: usage
                        .get("thoughtTokens")
                        .and_then(serde_json::Value::as_u64),
                };
                if usage.input_tokens.is_none()
                    && usage.output_tokens.is_none()
                    && usage.total_tokens.is_none()
                    && usage.thought_tokens.is_none()
                {
                    None
                } else {
                    Some(usage)
                }
            })
        })
    });
    Some(SessionEvent::AgentFinished {
        stop_reason,
        model_id,
        usage,
    })
}

fn text_from_content(content: Option<&serde_json::Value>) -> Option<&str> {
    let content = content?;
    if content.get("type").and_then(serde_json::Value::as_str) != Some("text") {
        return None;
    }
    content.get("text").and_then(serde_json::Value::as_str)
}

fn usage_from_meta(meta: &serde_json::Value) -> Option<TurnUsage> {
    let input_tokens = meta.get("inputTokens").and_then(serde_json::Value::as_u64);
    let output_tokens = meta.get("outputTokens").and_then(serde_json::Value::as_u64);
    let total_tokens = meta.get("totalTokens").and_then(serde_json::Value::as_u64);
    let thought_tokens = meta
        .get("thoughtTokens")
        .or_else(|| meta.get("reasoningTokens"))
        .and_then(serde_json::Value::as_u64);
    if input_tokens.is_none()
        && output_tokens.is_none()
        && total_tokens.is_none()
        && thought_tokens.is_none()
    {
        return None;
    }
    Some(TurnUsage {
        input_tokens,
        output_tokens,
        total_tokens,
        thought_tokens,
    })
}

fn commands_from_update(update: &serde_json::Value) -> Option<Vec<AvailableCommandView>> {
    let commands = update.get("availableCommands")?.as_array()?;
    Some(
        commands
            .iter()
            .filter_map(|command| {
                let name = command.get("name")?.as_str()?.to_string();
                let description = command.get("description")?.as_str()?.to_string();
                let hint = command
                    .get("input")
                    .and_then(|input| input.get("hint"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                Some(AvailableCommandView {
                    name,
                    description,
                    hint,
                })
            })
            .collect(),
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn session_manifest_from_initialize(
    result: &serde_json::Value,
    provider_id: Option<String>,
) -> Option<SessionEvent> {
    let state = result
        .get("_meta")
        .and_then(|meta| meta.get("modelState"))?;
    manifest_from_vendor_models(state, provider_id, None)
}

// Parses the manifest shape grok sends in `session/new` responses (measured:
// `models.currentModelId` + `availableModels`). Production consumes the
// initialize/_meta and models/update paths today; `session/load` (reattach)
// is the intended production caller.
#[cfg(test)]
pub(crate) fn session_manifest_from_new_session(
    result: &serde_json::Value,
    provider_id: Option<String>,
) -> Option<SessionEvent> {
    let modes = modes_from_standard(result).or_else(|| Some(synthesized_modes()));
    let models = result
        .get("models")
        .and_then(|value| manifest_from_vendor_models(value, provider_id.clone(), modes.clone()));
    if models.is_some() {
        return models;
    }
    modes.map(|modes| SessionEvent::SessionManifest {
        provider_id,
        current_model_id: None,
        models: Vec::new(),
        modes: Some(modes),
    })
}

pub(crate) fn session_manifest_from_models_update(
    params: &serde_json::Value,
    provider_id: Option<String>,
) -> Option<SessionEvent> {
    manifest_from_vendor_models(params, provider_id, None)
}

/// A switch surface declared by one ACP control. Model and effort are tracked
/// independently because a peer may expose a vendor model catalog and a
/// config-option effort selector at the same time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SwitchControlShape {
    pub vendor: Option<VendorSwitchSurface>,
    pub config: Option<ConfigOptionSurface>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VendorSwitchSurface {
    pub values: Vec<String>,
    /// Vendor effort choices are model-specific; the empty list for model
    /// controls is intentional.
    pub values_by_model: Vec<(String, Vec<String>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfigOptionSurface {
    pub id: String,
    pub values: Vec<String>,
}

/// Which surfaces the handshake parse found for each control. When both are
/// present, the client uses `config` first and retains `vendor` as the
/// error-driven legacy fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelSwitchShape {
    pub model: SwitchControlShape,
    pub effort: SwitchControlShape,
}

impl ModelSwitchShape {
    pub(crate) fn has_any_surface(&self) -> bool {
        self.model.vendor.is_some()
            || self.model.config.is_some()
            || self.effort.vendor.is_some()
            || self.effort.config.is_some()
    }
}

/// Adds a vendor surface discovered by a parsed models manifest without
/// discarding an already-declared config surface (for example, a vendor model
/// catalog paired with a config-only thought-level selector).
pub(crate) fn add_vendor_surface(
    mut shape: Option<ModelSwitchShape>,
    event: &SessionEvent,
) -> Option<ModelSwitchShape> {
    let SessionEvent::SessionManifest { models, .. } = event else {
        return shape;
    };
    let model_values = models
        .iter()
        .map(|model| model.model_id.clone())
        .collect::<Vec<_>>();
    let effort_values_by_model = models
        .iter()
        .filter_map(|model| {
            let efforts = model.efforts.as_ref()?;
            Some((
                model.model_id.clone(),
                efforts
                    .iter()
                    .map(|effort| effort.id.clone())
                    .collect::<Vec<_>>(),
            ))
        })
        .collect::<Vec<_>>();
    if model_values.is_empty() && effort_values_by_model.is_empty() {
        return shape;
    }
    let vendor_model = VendorSwitchSurface {
        values: model_values,
        values_by_model: Vec::new(),
    };
    let vendor_effort = VendorSwitchSurface {
        values: effort_values_by_model
            .iter()
            .flat_map(|(_, values)| values.iter().cloned())
            .collect(),
        values_by_model: effort_values_by_model,
    };
    let shape_ref = shape.get_or_insert(ModelSwitchShape {
        model: SwitchControlShape {
            vendor: None,
            config: None,
        },
        effort: SwitchControlShape {
            vendor: None,
            config: None,
        },
    });
    shape_ref.model.vendor = Some(vendor_model);
    shape_ref.effort.vendor = Some(vendor_effort);
    shape
}

/// What the handshake parse produced: the manifest to publish plus the shape
/// that produced it. The shape is a BY-PRODUCT of this parse — there is no
/// second reader of the raw bytes that could disagree with the manifest
/// (the audit's finding that a separate sniff recorded `VendorModels` for a
/// hybrid frame whose manifest was built from `configOptions` is closed by
/// construction).
#[derive(Debug, Default)]
pub(crate) struct HandshakeManifest {
    /// The manifest to publish, if any: a model catalog, or a modes-only
    /// manifest when the agent declared modes but no model shape.
    pub event: Option<SessionEvent>,
    /// Which switch verb the shape that produced the manifest speaks. `None`
    /// means no model shape was parsed: switching must fail with an explicit
    /// unsupported-shape error, never fall through to a guessed verb.
    pub shape: Option<ModelSwitchShape>,
}

/// Parses the handshake pair into the manifest to publish plus the switch
/// shape that produced it. Used for BOTH `session/new` and `session/load`, so
/// a reattached session derives its shape exactly like a fresh one. If the
/// reply carries neither shape, the shape is `None`: the switch path must
/// then fail loudly, and the next attributable manifest event (e.g. a
/// vendor-shaped `_x.ai/models/update` push) may fill it in — re-deriving on
/// evidence, never guessing.
pub(crate) fn merge_handshake_manifest(
    initialize_result: &serde_json::Value,
    new_session_result: &serde_json::Value,
    provider_id: Option<String>,
) -> HandshakeManifest {
    let modes = modes_from_standard(new_session_result).or_else(|| Some(synthesized_modes()));
    // Each parser returns both the view and the control surface it discovered.
    // That keeps the shape a by-product of the parse, including hybrids; no
    // second raw-byte sniff can disagree with the manifest-producing parse.
    let session_vendor = new_session_result
        .get("models")
        .and_then(|value| vendor_catalog_from_models(value, provider_id.clone(), modes.clone()));
    let config =
        catalog_from_config_options(new_session_result, provider_id.clone(), modes.clone(), None);
    let config_effort = config_effort_surface_from_result(
        new_session_result,
        None,
        config
            .as_ref()
            .map(|catalog| catalog.model_option_id.as_str()),
    );
    let initialize_vendor = initialize_result
        .get("_meta")
        .and_then(|meta| meta.get("modelState"))
        .and_then(|value| vendor_catalog_from_models(value, provider_id.clone(), modes.clone()));
    let vendor = session_vendor.or(initialize_vendor);

    let shape = ModelSwitchShape {
        model: SwitchControlShape {
            vendor: vendor.as_ref().map(|catalog| VendorSwitchSurface {
                values: catalog.model_values.clone(),
                values_by_model: Vec::new(),
            }),
            config: config.as_ref().map(|catalog| ConfigOptionSurface {
                id: catalog.model_option_id.clone(),
                values: catalog.model_values.clone(),
            }),
        },
        effort: SwitchControlShape {
            vendor: vendor.as_ref().map(|catalog| VendorSwitchSurface {
                values: catalog
                    .effort_values_by_model
                    .iter()
                    .flat_map(|(_, values)| values.iter().cloned())
                    .collect(),
                values_by_model: catalog.effort_values_by_model.clone(),
            }),
            config: config
                .as_ref()
                .and_then(|catalog| catalog.effort_option_id.as_ref())
                .map(|id| ConfigOptionSurface {
                    id: id.clone(),
                    values: config
                        .as_ref()
                        .map(|catalog| catalog.effort_values.clone())
                        .unwrap_or_default(),
                })
                .or(config_effort),
        },
    };
    let event = config
        .as_ref()
        .map(|catalog| catalog.manifest.clone())
        .or_else(|| vendor.map(|catalog| catalog.manifest));

    // Modes only, or nothing at all: publish the modes-only manifest (so
    // "this agent offers no model list" stays distinct from "no agent"),
    // but record no switch shape.
    HandshakeManifest {
        event: event.or_else(|| {
            modes.map(|modes| SessionEvent::SessionManifest {
                provider_id,
                current_model_id: None,
                models: Vec::new(),
                modes: Some(modes),
            })
        }),
        shape: shape.has_any_surface().then_some(shape),
    }
}

fn manifest_from_vendor_models(
    value: &serde_json::Value,
    provider_id: Option<String>,
    modes: Option<SessionModeStateView>,
) -> Option<SessionEvent> {
    vendor_catalog_from_models(value, provider_id, modes).map(|catalog| catalog.manifest)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VendorCatalog {
    manifest: SessionEvent,
    model_values: Vec<String>,
    effort_values_by_model: Vec<(String, Vec<String>)>,
}

fn vendor_catalog_from_models(
    value: &serde_json::Value,
    provider_id: Option<String>,
    modes: Option<SessionModeStateView>,
) -> Option<VendorCatalog> {
    let available = value.get("availableModels")?.as_array()?;
    let current_model_id = value
        .get("currentModelId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let models: Vec<SessionModel> = available
        .iter()
        .filter_map(session_model_from_vendor)
        .collect();
    // An empty model list with no current model is not a model surface. Modes
    // ride on the manifest built by the caller's fallback, so their presence
    // must not promote an empty catalog into a vendor switch shape.
    if models.is_empty() && current_model_id.is_none() {
        return None;
    }
    let model_values = models.iter().map(|model| model.model_id.clone()).collect();
    let effort_values_by_model = models
        .iter()
        .filter_map(|model| {
            let efforts = model.efforts.as_ref()?;
            if efforts.is_empty() {
                return None;
            }
            Some((
                model.model_id.clone(),
                efforts.iter().map(|effort| effort.id.clone()).collect(),
            ))
        })
        .collect();
    Some(VendorCatalog {
        manifest: SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes,
        },
        model_values,
        effort_values_by_model,
    })
}

/// A model catalog parsed from the ACP v2 `configOptions` surface, plus the
/// config-option ids the agent actually declared (the switch must send
/// those, not hard-coded constants).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigCatalog {
    pub manifest: SessionEvent,
    pub model_option_id: String,
    pub effort_option_id: Option<String>,
    pub model_values: Vec<String>,
    pub effort_values: Vec<String>,
}

/// Parses the ACP v1 `configOptions` surface into the model catalog.
///
/// Measured on `@agentclientprotocol/claude-agent-acp@0.76.0` over raw stdio
/// (2026-09-09): its `session/new` result carries neither `models`/`availableModels`
/// nor `initialize` `_meta.modelState`. The model list arrives as a `select`
/// session config option, the active model as that option's `currentValue`,
/// and the reasoning-effort catalog as a sibling select option. The effort
/// option is session-level; we only know the CURRENT model's live effort, so
/// `efforts`/`current_effort` are attached to the current model only instead
/// of being copied onto models we cannot vouch for.
///
/// `known_ids` carries the option ids a previous parse of the same session
/// recorded; they are tried first so a reply from the same agent is matched
/// by what it declared before, with discovery as the fallback. The returned
/// ids are the ones actually used, so the switcher sends what the agent
/// declared rather than a constant.
pub(crate) fn catalog_from_config_options(
    result: &serde_json::Value,
    provider_id: Option<String>,
    modes: Option<SessionModeStateView>,
    known_ids: Option<(&str, Option<&str>)>,
) -> Option<ConfigCatalog> {
    let options = result.get("configOptions")?.as_array()?;
    let model_option = find_select_option(
        options,
        known_ids.map(|(model, _)| model),
        &["model", "model_id", "modelId"],
        &["model", "model_config"],
        &["thought_level"],
        None,
        true,
    )?;
    let effort_option = find_select_option(
        options,
        known_ids.and_then(|(_, effort)| effort),
        &["effort", "reasoning_effort", "thinking"],
        &["thought_level"],
        &["model", "model_config"],
        model_option.get("id").and_then(serde_json::Value::as_str),
        true,
    );
    let model_surface = config_surface_from_option(model_option);
    let effort_surface = effort_option.map(config_surface_from_option);
    let current_model_id = model_option
        .get("currentValue")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let mut models: Vec<SessionModel> = Vec::new();
    for entry in model_option
        .get("options")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(model_id) = entry.get("value").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if model_id.is_empty() {
            continue;
        }
        // Duplicate ids would produce duplicate React keys for a controlled
        // select; the first declaration wins.
        if models.iter().any(|model| model.model_id == model_id) {
            continue;
        }
        models.push(SessionModel {
            name: entry
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(model_id)
                .to_string(),
            model_id: model_id.to_string(),
            description: entry
                .get("description")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            // Attached per-model below, from the effort config option.
            context_tokens: None,
            current_effort: None,
            efforts: None,
        });
    }
    if models.is_empty() {
        return None;
    }
    // The current selection must be one of the offered options: a catalog
    // whose `currentValue` is absent from the list would render a controlled
    // select with a value its options do not contain.
    if let Some(current) = &current_model_id {
        if !models.iter().any(|model| &model.model_id == current) {
            return None;
        }
    }
    // The effort option describes the current model's live effort levels
    // ("Available effort levels for this model", measured). Attach them only
    // where we know they apply: the selected model.
    let effort_info = effort_option.as_ref();
    if let (Some(current_model_id), Some(effort_option)) = (current_model_id.as_ref(), effort_info)
    {
        let current_effort = effort_option
            .get("currentValue")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let mut efforts: Vec<SessionModelEffort> = Vec::new();
        for entry in effort_option
            .get("options")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(id) = entry.get("value").and_then(serde_json::Value::as_str) else {
                continue;
            };
            if id.is_empty() || efforts.iter().any(|effort| effort.id == id) {
                continue;
            }
            efforts.push(SessionModelEffort {
                label: entry
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(id)
                    .to_string(),
                id: id.to_string(),
                description: entry
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                default: None,
            });
        }
        // An effort selection that is absent from the offered effort list is
        // not information we can vouch for: keep the model catalog, but do
        // not attach effort knowledge from a malformed option.
        let effort_listed = current_effort
            .as_ref()
            .is_none_or(|effort| efforts.iter().any(|listed| &listed.id == effort));
        if effort_listed && (!efforts.is_empty() || current_effort.is_some()) {
            for model in models.iter_mut() {
                if model.model_id == *current_model_id {
                    model.current_effort = current_effort.clone();
                    if !efforts.is_empty() {
                        model.efforts = Some(efforts.clone());
                    }
                }
            }
        }
    }
    Some(ConfigCatalog {
        manifest: SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes,
        },
        model_option_id: model_surface.id,
        effort_option_id: effort_surface.as_ref().map(|option| option.id.clone()),
        model_values: model_surface.values,
        effort_values: effort_surface
            .map(|option| option.values)
            .unwrap_or_default(),
    })
}

fn config_effort_surface_from_result(
    result: &serde_json::Value,
    known_id: Option<&str>,
    excluded_id: Option<&str>,
) -> Option<ConfigOptionSurface> {
    let options = result.get("configOptions")?.as_array()?;
    let option = find_select_option(
        options,
        known_id,
        &["effort", "reasoning_effort", "thinking"],
        &["thought_level"],
        &["model", "model_config"],
        excluded_id,
        true,
    )?;
    Some(config_surface_from_option(option))
}

/// Finds a `select`-shaped session config option: the known id from a
/// previous parse of the same session first, then the canonical id, then the
/// advisory category. The ACP schema says: "This is intended to help Clients
/// distinguish broadly common selectors (e.g. model selector vs session mode
/// selector vs thought/reasoning level) for UX purposes ... It MUST NOT be
/// required for correctness." If those signals are absent, a role-shaped id
/// or a single unclaimed select is a conformant fallback. The caller always
/// sends the option's declared id, never a hard-coded category name.
fn find_select_option<'a>(
    options: &'a [serde_json::Value],
    known_id: Option<&str>,
    canonical_ids: &[&str],
    categories: &[&str],
    excluded_categories: &[&str],
    excluded_id: Option<&str>,
    allow_single_fallback: bool,
) -> Option<&'a serde_json::Value> {
    let is_select = |option: &serde_json::Value| {
        option.get("type").and_then(serde_json::Value::as_str) == Some("select")
            && !excluded_categories.iter().any(|category| {
                option.get("category").and_then(serde_json::Value::as_str) == Some(*category)
            })
            && option.get("id").and_then(serde_json::Value::as_str) != excluded_id
    };
    if let Some(known) = known_id {
        if let Some(option) = options.iter().find(|option| {
            option.get("id").and_then(serde_json::Value::as_str) == Some(known) && is_select(option)
        }) {
            return Some(option);
        }
    }
    if let Some(option) = options.iter().find(|option| {
        canonical_ids
            .iter()
            .any(|id| option.get("id").and_then(serde_json::Value::as_str) == Some(*id))
            && is_select(option)
    }) {
        return Some(option);
    }
    if let Some(option) = options.iter().find(|option| {
        categories.iter().any(|category| {
            option.get("category").and_then(serde_json::Value::as_str) == Some(*category)
        }) && is_select(option)
    }) {
        return Some(option);
    }
    let role_hint = |option: &serde_json::Value| {
        let id = option
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        if canonical_ids.first() == Some(&"model") {
            ["model", "engine", "profile"]
                .iter()
                .any(|hint| id.contains(hint))
        } else {
            ["effort", "reason", "think", "thought"]
                .iter()
                .any(|hint| id.contains(hint))
        }
    };
    if let Some(option) = options
        .iter()
        .find(|option| is_select(option) && role_hint(option))
    {
        return Some(option);
    }
    if allow_single_fallback {
        let mut selects = options.iter().filter(|option| {
            if !is_select(option) {
                return false;
            }
            // A vendor catalog may be paired with one config-only effort
            // selector. When its category is absent or unknown, do not
            // mistake that selector for the model merely because it is the
            // only select-shaped option. Category is advisory, so this
            // conservative role hint is the fallback's only exclusion.
            if canonical_ids.first() == Some(&"model") {
                let id = option
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_ascii_lowercase();
                return !["effort", "reason", "think", "thought"]
                    .iter()
                    .any(|hint| id.contains(hint));
            }
            true
        });
        let option = selects.next()?;
        if selects.next().is_none() {
            return Some(option);
        }
    }
    None
}

fn config_surface_from_option(option: &serde_json::Value) -> ConfigOptionSurface {
    let id = option
        .get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut values = Vec::new();
    for value in option
        .get("options")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("value").and_then(serde_json::Value::as_str))
    {
        if !value.is_empty() && !values.iter().any(|known| known == value) {
            values.push(value.to_string());
        }
    }
    ConfigOptionSurface { id, values }
}

fn session_model_from_vendor(value: &serde_json::Value) -> Option<SessionModel> {
    let model_id = value.get("modelId")?.as_str()?.to_string();
    let name = value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&model_id)
        .to_string();
    let description = value
        .get("description")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let meta = value.get("_meta");
    let context_tokens = meta
        .and_then(|meta| meta.get("totalContextTokens"))
        .and_then(serde_json::Value::as_u64);
    let supports_effort = meta
        .and_then(|meta| meta.get("supportsReasoningEffort"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let efforts = if supports_effort {
        meta.and_then(|meta| meta.get("reasoningEfforts"))
            .and_then(serde_json::Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(session_effort_from_vendor)
                    .collect::<Vec<_>>()
            })
            .filter(|entries| !entries.is_empty())
    } else {
        None
    };
    let current_effort = if supports_effort {
        meta.and_then(|meta| meta.get("reasoningEffort"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    } else {
        None
    };
    Some(SessionModel {
        model_id,
        name,
        description,
        context_tokens,
        current_effort,
        efforts,
    })
}

fn session_effort_from_vendor(value: &serde_json::Value) -> Option<SessionModelEffort> {
    Some(SessionModelEffort {
        id: value.get("id")?.as_str()?.to_string(),
        label: value
            .get("label")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| {
                value
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
            })
            .to_string(),
        description: value
            .get("description")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        default: value.get("default").and_then(serde_json::Value::as_bool),
    })
}

fn locations_from_value(
    value: Option<&serde_json::Value>,
    cwd: Option<&Path>,
    empty_is_replace: bool,
) -> Option<Vec<ToolLocation>> {
    let value = value?;
    let entries = value.as_array()?;
    if entries.is_empty() {
        return if empty_is_replace {
            Some(Vec::new())
        } else {
            None
        };
    }
    Some(
        entries
            .iter()
            .filter_map(|location| {
                let path = location.get("path").and_then(serde_json::Value::as_str)?;
                let line = location
                    .get("line")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok());
                Some(ToolLocation {
                    path: relativize_tool_path(path, cwd),
                    line,
                })
            })
            .collect(),
    )
}

fn modes_from_standard(result: &serde_json::Value) -> Option<SessionModeStateView> {
    let modes = result.get("modes")?;
    let current_mode_id = modes
        .get("currentModeId")
        .and_then(serde_json::Value::as_str)?
        .to_string();
    let available = modes.get("availableModes")?.as_array()?;
    let available_modes: Vec<SessionModeView> = available
        .iter()
        .filter_map(|mode| {
            Some(SessionModeView {
                id: mode.get("id")?.as_str()?.to_string(),
                name: mode.get("name")?.as_str()?.to_string(),
                description: mode
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect();
    if available_modes.is_empty() {
        return None;
    }
    Some(SessionModeStateView {
        current_mode_id,
        available_modes,
    })
}

pub(crate) fn has_standard_modes(result: &serde_json::Value) -> bool {
    modes_from_standard(result).is_some()
}

fn synthesized_modes() -> SessionModeStateView {
    SessionModeStateView {
        current_mode_id: "ask".to_string(),
        available_modes: vec![
            SessionModeView {
                id: "ask".to_string(),
                name: "Always ask".to_string(),
                description: Some("Permission prompts are shown to you".to_string()),
            },
            SessionModeView {
                id: "auto_accept".to_string(),
                name: "Auto accept".to_string(),
                description: Some("Permission prompts are approved automatically".to_string()),
            },
        ],
    }
}

pub(crate) fn current_mode_id_from_update(
    value: &serde_json::Value,
    expected_session_id: &str,
) -> Option<String> {
    let params = value.get("params")?;
    if !expected_session_id.is_empty()
        && params.get("sessionId").and_then(serde_json::Value::as_str) != Some(expected_session_id)
    {
        return None;
    }
    let update = params.get("update")?;
    if update
        .get("sessionUpdate")
        .and_then(serde_json::Value::as_str)
        != Some("current_mode_update")
    {
        return None;
    }
    update
        .get("modeId")
        .and_then(serde_json::Value::as_str)
        .filter(|mode_id| !mode_id.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::{
        catalog_from_config_options, classify_line, current_mode_id_from_update,
        merge_handshake_manifest, session_manifest_from_initialize,
        session_manifest_from_models_update, session_manifest_from_new_session, view_from_envelope,
        view_from_envelope_in, AcpLineKind,
    };
    use devboule_protocol::SessionEvent;

    const GROK_CAPTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/wire/grok-v1.jsonl"
    ));
    const QWEN_CAPTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/wire/qwen-v1.jsonl"
    ));

    fn measured_raw(capture: &str, needle: &str) -> serde_json::Value {
        capture
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter_map(|entry| {
                entry
                    .get("raw")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .find(|raw| raw.contains(needle))
            .map(|raw| parse(&raw))
            .unwrap_or_else(|| panic!("missing measured line containing {needle}"))
    }

    // Reconstructed from recon/probes/grok-acp-fullcaps.txt (2026-09-04).
    // The probe file truncates long lines; these objects keep the measured
    // field names and the token-sized chunking.
    const SESSION: &str = "01a06c70-ea2b-7882-ad27-aae8188fc243";

    fn parse(line: &str) -> serde_json::Value {
        serde_json::from_str(line).expect("probe json")
    }

    #[test]
    fn request_with_id_is_not_classified_as_a_response() {
        let line = parse(
            r#"{"jsonrpc":"2.0","id":9,"method":"terminal/create","params":{"sessionId":"s","command":"echo"}}"#,
        );
        assert_eq!(
            classify_line(&line),
            Some(AcpLineKind::Request {
                method: "terminal/create".to_string()
            })
        );
    }

    #[test]
    fn probe_user_message_chunk_becomes_user_view() {
        let line = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"Reply with exactly one word: PONG"},"_meta":{"modelId":"grok-4.6","promptIndex":0}}}}"#,
        );
        let view = view_from_envelope(&line, SESSION).expect("user chunk is modeled");
        assert_eq!(
            view,
            SessionEvent::AgentUserMessage {
                message_id: None,
                text: "Reply with exactly one word: PONG".to_string(),
            }
        );
        assert_eq!(
            line["params"]["update"]["sessionUpdate"], "user_message_chunk",
            "derivation must not consume the envelope"
        );
    }

    #[test]
    fn probe_thought_chunks_stay_one_event_per_token() {
        let first = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"The"}}}}"#,
        );
        let second = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":" user"}}}}"#,
        );
        let a = view_from_envelope(&first, SESSION).expect("thought chunk");
        let b = view_from_envelope(&second, SESSION).expect("thought chunk");
        assert_eq!(
            a,
            SessionEvent::AgentThought {
                message_id: None,
                text: "The".to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            }
        );
        assert_eq!(
            b,
            SessionEvent::AgentThought {
                message_id: None,
                text: " user".to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            }
        );
    }

    #[test]
    fn probe_message_chunks_stay_one_event_per_token() {
        let first = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"P"}}}}"#,
        );
        let second = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"ONG"}}}}"#,
        );
        let a = view_from_envelope(&first, SESSION).expect("message chunk");
        let b = view_from_envelope(&second, SESSION).expect("message chunk");
        assert_eq!(
            a,
            SessionEvent::AgentMessage {
                message_id: None,
                text: "P".to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            }
        );
        assert_eq!(
            b,
            SessionEvent::AgentMessage {
                message_id: None,
                text: "ONG".to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            }
        );
    }

    #[test]
    fn probe_available_commands_update_lists_slash_commands() {
        let line = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"compact","description":"Compress conversation history to save context window","input":{"hint":"optional context"}}]}}}"#,
        );
        match view_from_envelope(&line, SESSION) {
            Some(SessionEvent::AvailableCommands { commands }) => {
                assert_eq!(commands.len(), 1);
                assert_eq!(commands[0].name, "compact");
                assert_eq!(
                    commands[0].description,
                    "Compress conversation history to save context window"
                );
                assert_eq!(commands[0].hint.as_deref(), Some("optional context"));
            }
            other => panic!("expected available commands, got {other:?}"),
        }
    }

    #[test]
    fn probe_prompt_result_carries_stop_reason_model_and_usage() {
        let line = parse(
            r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn","_meta":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","modelId":"grok-4.6","inputTokens":18883,"outputTokens":43,"totalTokens":18926}}}"#,
        );
        match view_from_envelope(&line, SESSION) {
            Some(SessionEvent::AgentFinished {
                stop_reason,
                model_id,
                usage,
                ..
            }) => {
                assert_eq!(stop_reason, "end_turn");
                assert_eq!(model_id.as_deref(), Some("grok-4.6"));
                let usage = usage.expect("usage from _meta");
                assert_eq!(usage.input_tokens, Some(18883));
                assert_eq!(usage.output_tokens, Some(43));
                assert_eq!(usage.total_tokens, Some(18926));
            }
            other => panic!("expected agent finished, got {other:?}"),
        }
    }

    #[test]
    fn xai_extension_has_no_view_and_keeps_the_envelope() {
        let line = parse(
            r#"{"jsonrpc":"2.0","method":"_x.ai/mcp/servers_updated","params":{"mcpServers":[]}}"#,
        );
        assert!(view_from_envelope(&line, SESSION).is_none());
        assert_eq!(line["method"], "_x.ai/mcp/servers_updated");
        assert_eq!(
            classify_line(&line),
            Some(AcpLineKind::Notification {
                method: "_x.ai/mcp/servers_updated".to_string()
            })
        );
    }

    #[test]
    fn foreign_session_update_is_ignored() {
        let line = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"other","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"no"}}}}"#,
        );
        assert!(view_from_envelope(&line, SESSION).is_none());
    }

    /// A shape demonstration on SYNTHETIC input, not a measurement.
    ///
    /// The fixtures below are padded on purpose so the ratio is visible at a
    /// glance; the byte counts this produces describe the fixtures and nothing
    /// else. Its previous name claimed it measured the probe, and those numbers
    /// reached a public commit message before anyone recomputed them.
    ///
    /// The real turn, counted line by line from
    /// `recon/probes/grok-acp-fullcaps.txt`: 56 inbound lines, 92,586 bytes, of
    /// which 64,263 -- 69% -- are three dumps of the slash-command catalogue.
    /// Thought fragments are 15,692 bytes over 34 lines, about 8x their view.
    /// Written up in `recon/M6-completion-plan.md` section 12.
    #[test]
    fn an_envelope_costs_more_than_the_view_derived_from_it() {
        fn envelope_len(value: &serde_json::Value) -> usize {
            serde_json::to_vec(value).expect("envelope").len()
        }
        fn view_len(value: &serde_json::Value) -> usize {
            view_from_envelope(value, SESSION)
                .and_then(|view| serde_json::to_vec(&view).ok())
                .map(|bytes| bytes.len())
                .unwrap_or(0)
        }

        // Thought-chunk envelopes in recon/probes/grok-acp-fullcaps.txt are
        // annotated ~459 bytes because grok attaches _meta per token.
        let thought = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": SESSION,
                "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "content": {"type": "text", "text": "The"},
                    "_meta": {
                        "totalTokens": 1687,
                        "eventId": "01a06c70-ea2b-7882-ad27-aae8188fc243-3",
                        "agentTimestampMs": 1788525739000u64,
                        "pad": "x".repeat(220)
                    }
                }
            }
        });
        let user = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"Reply with exactly one word: PONG"}}}}"#,
        );
        let message = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"PONG"}}}}"#,
        );
        let mut commands = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": SESSION,
                "update": {
                    "sessionUpdate": "available_commands_update",
                    "availableCommands": [{
                        "name": "compact",
                        "description": "Compress conversation history to save context window",
                        "input": {"hint": "optional context"}
                    }]
                }
            }
        });
        commands["params"]["update"]["availableCommands"][0]["description"] =
            serde_json::Value::String("x".repeat(20_800));
        let xai = parse(
            r#"{"jsonrpc":"2.0","method":"_x.ai/sessions/changed","params":{"upserted":[{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","modelId":"grok-4.6"}]}}"#,
        );
        let finished = parse(
            r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn","_meta":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","modelId":"grok-4.6","inputTokens":19016,"outputTokens":41,"totalTokens":19057}}}"#,
        );

        let mut envelopes = Vec::new();
        envelopes.push(("user", user));
        for _ in 0..32 {
            envelopes.push(("thought", thought.clone()));
        }
        envelopes.push(("message", message));
        envelopes.push(("commands", commands.clone()));
        envelopes.push(("commands", commands));
        for _ in 0..6 {
            envelopes.push(("xai", xai.clone()));
        }
        envelopes.push(("finished", finished));

        let mut envelope_bytes = 0usize;
        let mut view_bytes = 0usize;
        let mut modeled = 0usize;
        let mut raw_only = 0usize;
        let mut thought_envelope = 0usize;
        let mut thought_view = 0usize;
        let mut command_envelope = 0usize;
        let mut command_view = 0usize;
        for (kind, value) in &envelopes {
            let e = envelope_len(value);
            let v = view_len(value);
            envelope_bytes += e;
            view_bytes += v;
            match *kind {
                "thought" => {
                    thought_envelope += e;
                    thought_view += v;
                }
                "commands" => {
                    command_envelope += e;
                    command_view += v;
                }
                _ => {}
            }
            if v == 0 {
                raw_only += 1;
                assert_eq!(*kind, "xai");
            } else {
                modeled += 1;
            }
        }
        eprintln!(
            "grok-like one-word turn: n={} envelope={envelope_bytes} view={view_bytes} ratio={:.1}x modeled={modeled} raw_only={raw_only} thoughts 32× envelope={thought_envelope} view={thought_view} ratio={:.1}x commands 2× envelope={command_envelope} view={command_view}",
            envelopes.len(),
            envelope_bytes as f64 / view_bytes.max(1) as f64,
            thought_envelope as f64 / thought_view.max(1) as f64
        );
        assert!(envelope_bytes > view_bytes);
        assert_eq!(envelopes.len(), 1 + 32 + 1 + 2 + 6 + 1);
    }

    // Reconstructed from recon/probes/acp-handshake.txt (2026-09-04) initialize
    // result._meta.modelState / session/new.result.models. Truncated probe
    // lines keep these field names and the grok-4.6 vs grok-4.5 effort split.
    const GROK_MODELS: &str = r#"{
        "currentModelId": "grok-4.6",
        "availableModels": [
            {
                "modelId": "grok-4.6",
                "name": "Grok 4.6",
                "description": "SpaceXAI's latest frontier model",
                "_meta": {
                    "totalContextTokens": 500000,
                    "agentType": "grok-build-plan",
                    "supportsReasoningEffort": true,
                    "reasoningEffort": "xhigh",
                    "reasoningEfforts": [
                        {"id": "xhigh", "value": "xhigh", "label": "Extra High Effort", "description": "Highest effort and reasoning level", "default": false},
                        {"id": "high", "value": "high", "label": "High Effort", "description": "Higher implementation quality with extensive reasoning", "default": true},
                        {"id": "medium", "value": "medium", "label": "Medium Effort", "description": "Balanced effort with standard implementation and testing", "default": false},
                        {"id": "low", "value": "low", "label": "Low Effort", "description": "Quick, fast implementations", "default": false}
                    ]
                }
            },
            {
                "modelId": "grok-4.5",
                "name": "Grok 4.5",
                "_meta": {
                    "totalContextTokens": 500000,
                    "agentType": "grok-build-plan",
                    "supportsReasoningEffort": true,
                    "reasoningEffort": "high",
                    "reasoningEfforts": [
                        {"id": "high", "value": "high", "label": "High Effort", "description": "Highest implementation quality with extensive reasoning", "default": true},
                        {"id": "medium", "value": "medium", "label": "Medium Effort", "description": "Balanced effort with standard implementation and testing", "default": false},
                        {"id": "low", "value": "low", "label": "Low Effort", "description": "Quick, fast implementations", "default": false}
                    ]
                }
            }
        ]
    }"#;

    fn grok_session_new() -> serde_json::Value {
        parse(&format!(
            r#"{{"sessionId":"{SESSION}","models":{GROK_MODELS}}}"#
        ))
    }

    fn merge_manifest_event(
        initialize: &serde_json::Value,
        new_session: &serde_json::Value,
        provider_id: Option<&str>,
    ) -> SessionEvent {
        merge_handshake_manifest(initialize, new_session, provider_id.map(str::to_string))
            .event
            .expect("handshake must produce a manifest")
    }

    #[test]
    fn grok_session_new_models_become_a_session_manifest() {
        let result = grok_session_new();
        let SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes,
        } = merge_manifest_event(&serde_json::Value::Null, &result, Some("grok"))
        else {
            panic!("expected SessionManifest");
        };
        assert_eq!(provider_id.as_deref(), Some("grok"));
        assert_eq!(current_model_id.as_deref(), Some("grok-4.6"));
        let modes = modes.expect("grok synthesized modes");
        assert_eq!(modes.current_mode_id, "ask");
        assert_eq!(modes.available_modes[1].id, "auto_accept");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].model_id, "grok-4.6");
        assert_eq!(models[0].context_tokens, Some(500_000));
        assert_eq!(models[0].current_effort.as_deref(), Some("xhigh"));
        let grok46_ids: Vec<&str> = models[0]
            .efforts
            .as_ref()
            .expect("grok-4.6 declares efforts")
            .iter()
            .map(|effort| effort.id.as_str())
            .collect();
        assert!(grok46_ids.contains(&"xhigh"));
        let grok45_ids: Vec<&str> = models[1]
            .efforts
            .as_ref()
            .expect("grok-4.5 declares efforts")
            .iter()
            .map(|effort| effort.id.as_str())
            .collect();
        assert!(!grok45_ids.contains(&"xhigh"));
    }

    #[test]
    fn measured_grok_session_new_without_modes_gets_permission_modes() {
        let result = measured_raw(GROK_CAPTURE, r#""id":2,"result""#)["result"].clone();
        let SessionEvent::SessionManifest { modes, .. } =
            session_manifest_from_new_session(&result, Some("grok".to_string()))
                .expect("measured grok session/new")
        else {
            panic!("expected SessionManifest");
        };
        let modes = modes.expect("synthesized grok modes");
        assert_eq!(modes.current_mode_id, "ask");
        assert_eq!(
            modes.available_modes[0].description.as_deref(),
            Some("Permission prompts are shown to you")
        );
        assert_eq!(modes.available_modes[1].id, "auto_accept");
        assert_eq!(
            modes.available_modes[1].description.as_deref(),
            Some("Permission prompts are approved automatically")
        );
    }

    #[test]
    fn measured_qwen_mode_switch_and_private_notification_are_tolerated() {
        let result = measured_raw(QWEN_CAPTURE, r#""id":2,"result""#)["result"].clone();
        let SessionEvent::SessionManifest { modes, .. } =
            session_manifest_from_new_session(&result, Some("qwen".to_string()))
                .expect("measured qwen session/new")
        else {
            panic!("expected SessionManifest");
        };
        let modes = modes.expect("qwen modes");
        assert_eq!(modes.current_mode_id, "auto");
        assert!(modes.available_modes.iter().any(|mode| mode.id == "plan"));

        let request = measured_raw(QWEN_CAPTURE, r#""method":"session/set_mode""#);
        assert_eq!(request["params"]["sessionId"], result["sessionId"]);
        assert_eq!(request["params"]["modeId"], "plan");
        let response = measured_raw(QWEN_CAPTURE, r#""id":3,"result":{}"#);
        assert_eq!(response["result"], serde_json::json!({}));

        let private = measured_raw(QWEN_CAPTURE, "qwen/notify/session/mode-update");
        assert!(view_from_envelope(&private, result["sessionId"].as_str().unwrap()).is_none());

        let standard = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": result["sessionId"],
                "update": {"sessionUpdate": "current_mode_update", "modeId": "plan"}
            }
        });
        assert_eq!(
            current_mode_id_from_update(&standard, result["sessionId"].as_str().unwrap()),
            Some("plan".to_string())
        );
    }

    #[test]
    fn modes_only_session_result_keeps_the_manifest_but_names_no_switch_shape() {
        // The reattach scenario: a session/load reply that answers
        // `{sessionId, modes}` only. The modes-only manifest is published
        // (so "this agent offers no model list" stays distinct from "no
        // agent"), but the switch shape is None: a click must fail loudly,
        // never fall through to a guessed verb.
        let result = parse(
            r#"{"sessionId":"s1","modes":{"currentModeId":"ask","availableModes":[{"id":"ask","name":"Always ask","description":"Ask before every tool call."},{"id":"acceptEdits","name":"Accept edits"}]}}"#,
        );
        let handshake = merge_handshake_manifest(&serde_json::Value::Null, &result, None);
        let SessionEvent::SessionManifest { modes, models, .. } =
            handshake.event.expect("modes-only manifest")
        else {
            panic!("expected SessionManifest");
        };
        assert!(models.is_empty());
        let modes = modes.expect("modes");
        assert_eq!(modes.current_mode_id, "ask");
        assert_eq!(modes.available_modes.len(), 2);
        assert_eq!(modes.available_modes[0].name, "Always ask");
        assert_eq!(modes.available_modes[1].id, "acceptEdits");
        assert_eq!(handshake.shape, None);
    }

    #[test]
    fn handshake_prefers_session_new_models_over_initialize_meta() {
        let initialize = parse(
            r#"{"_meta":{"modelState":{"currentModelId":"stale","availableModels":[{"modelId":"stale","name":"Stale"}]}}}"#,
        );
        let new_session = grok_session_new();
        let SessionEvent::SessionManifest {
            current_model_id, ..
        } = merge_manifest_event(&initialize, &new_session, Some("grok"))
        else {
            panic!("expected SessionManifest");
        };
        assert_eq!(current_model_id.as_deref(), Some("grok-4.6"));
    }

    // Verbatim raw-stdio capture of `@agentclientprotocol/claude-agent-acp@0.76.0`
    // (2026-09-09, probe kept out-of-band under %TEMP%/acp-probe). The initialize
    // request used Devboule's own advertised params (fs read/write + terminal,
    // clientInfo "devboule"); no prompt was ever sent. The agent's `session/new`
    // result carries the model catalog as a `configOptions` select entry — not
    // as `models`/`availableModels` — which is why the manifest used to come
    // out with an empty model list.
    const CLAUDE_ACP_076_INITIALIZE: &str =
        include_str!("../fixtures/acp-claude-076-initialize.json");
    const CLAUDE_ACP_076_SESSION_NEW: &str =
        include_str!("../fixtures/acp-claude-076-session-new.json");

    fn claude_acp_076_frames() -> (serde_json::Value, serde_json::Value) {
        let initialize: serde_json::Value =
            serde_json::from_str(CLAUDE_ACP_076_INITIALIZE).expect("verbatim initialize frame");
        let new_session: serde_json::Value =
            serde_json::from_str(CLAUDE_ACP_076_SESSION_NEW).expect("verbatim session/new frame");
        (initialize, new_session)
    }

    #[test]
    fn claude_acp_config_options_become_a_model_catalog() {
        let (initialize, new_session) = claude_acp_076_frames();

        // Nothing model-shaped hides in the initialize result: the catalog must
        // come from session/new alone.
        assert!(session_manifest_from_initialize(&initialize["result"], None).is_none());

        let handshake = merge_handshake_manifest(
            &initialize["result"],
            &new_session["result"],
            Some("claude-acp".to_string()),
        );
        // The shape is a by-product of the parse that produced the manifest:
        // the declared option ids ride along, so the switch sends what the
        // agent declared instead of a hard-coded "model"/"effort".
        let shape = handshake.shape.as_ref().expect("switch shape");
        assert_eq!(
            shape.model.config.as_ref().map(|option| option.id.as_str()),
            Some("model")
        );
        assert_eq!(
            shape
                .effort
                .config
                .as_ref()
                .map(|option| option.id.as_str()),
            Some("effort")
        );
        let (provider_id, current_model_id, models, modes) = match handshake
            .event
            .expect("configOptions must become a manifest")
        {
            SessionEvent::SessionManifest {
                provider_id,
                current_model_id,
                models,
                modes,
            } => (provider_id, current_model_id, models, modes),
            other => panic!("expected SessionManifest, got {other:?}"),
        };
        assert_eq!(provider_id.as_deref(), Some("claude-acp"));
        assert_eq!(current_model_id.as_deref(), Some("opus[1m]"));
        let ids: Vec<&str> = models.iter().map(|model| model.model_id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "default",
                "opus[1m]",
                "claude-fable-5-1[1m]",
                "sonnet",
                "haiku"
            ]
        );
        let current = models
            .iter()
            .find(|model| model.model_id == "opus[1m]")
            .expect("current model in catalog");
        assert_eq!(current.name, "Opus 5");
        assert_eq!(current.current_effort.as_deref(), Some("xhigh"));
        let efforts = current.efforts.as_ref().expect("effort catalog");
        let effort_ids: Vec<&str> = efforts.iter().map(|effort| effort.id.as_str()).collect();
        assert_eq!(
            effort_ids,
            vec!["default", "low", "medium", "high", "xhigh", "max"]
        );
        // The effort option is session-level knowledge scoped to the selected
        // model: other entries must not inherit it.
        let other = models
            .iter()
            .find(|model| model.model_id == "sonnet")
            .expect("sonnet in catalog");
        assert!(other.current_effort.is_none() && other.efforts.is_none());
        // Standard modes ride along in the same result and must survive.
        let modes = modes.expect("measured modes");
        assert_eq!(modes.current_mode_id, "default");
        let mode_ids: Vec<&str> = modes
            .available_modes
            .iter()
            .map(|mode| mode.id.as_str())
            .collect();
        assert_eq!(
            mode_ids,
            vec![
                "default",
                "acceptEdits",
                "plan",
                "auto",
                "bypassPermissions"
            ]
        );
    }

    #[test]
    fn a_reattach_reply_with_the_full_config_surface_yields_the_config_shape() {
        // A session/load reply carrying the full config surface derives the
        // same shape as session/new. (The integration test drives the real
        // reattach through a daemon restart; this pins the parse.)
        let (_, new_session) = claude_acp_076_frames();
        let handshake =
            merge_handshake_manifest(&serde_json::Value::Null, &new_session["result"], None);
        let shape = handshake.shape.as_ref().expect("switch shape");
        assert_eq!(
            shape.model.config.as_ref().map(|option| option.id.as_str()),
            Some("model")
        );
        assert_eq!(
            shape
                .effort
                .config
                .as_ref()
                .map(|option| option.id.as_str()),
            Some("effort")
        );
        let SessionEvent::SessionManifest {
            current_model_id,
            models,
            ..
        } = handshake.event.expect("configOptions manifest")
        else {
            panic!("expected SessionManifest");
        };
        assert_eq!(current_model_id.as_deref(), Some("opus[1m]"));
        assert_eq!(models.len(), 5);
        assert_eq!(models[4].model_id, "haiku");
        assert_eq!(models[4].name, "Haiku 4.5");
    }

    #[test]
    fn config_options_without_a_model_option_still_stay_absent() {
        // A configOptions surface with no model entry must not fabricate an
        // empty-model manifest on its own; the modes-only fallback owns that.
        let result = parse(
            r#"{"configOptions":[{"id":"effort","category":"thought_level","type":"select","currentValue":"high","options":[{"value":"high","name":"High"}]}]}"#,
        );
        assert!(catalog_from_config_options(&result, None, None, None).is_none());
    }

    #[test]
    fn the_shape_comes_from_the_parse_that_produced_the_manifest() {
        // grok declares the legacy vendor surface.
        let grok = merge_handshake_manifest(
            &serde_json::Value::Null,
            &grok_session_new(),
            Some("grok".to_string()),
        );
        let grok_shape = grok.shape.as_ref().expect("grok switch shape");
        assert!(grok_shape.model.vendor.is_some());
        assert!(grok_shape.model.config.is_none());
        // Neither shape: modes only, no switch shape.
        let neither = parse(
            r#"{"sessionId":"s","modes":{"currentModeId":"ask","availableModes":[{"id":"ask","name":"Always ask"}]}}"#,
        );
        let handshake = merge_handshake_manifest(&serde_json::Value::Null, &neither, None);
        assert_eq!(handshake.shape, None);
        // Both shapes, both populated: configOptions is primary but the
        // vendor surface remains available as the error-driven fallback.
        let both = parse(&format!(
            r#"{{"sessionId":"{SESSION}","models":{{"availableModels":[{{"modelId":"m"}}]}},"configOptions":[{{"id":"model","type":"select","currentValue":"m","options":[{{"value":"m"}}]}}]}}"#
        ));
        let handshake = merge_handshake_manifest(&serde_json::Value::Null, &both, None);
        let shape = handshake.shape.as_ref().expect("hybrid switch shape");
        assert!(shape.model.config.is_some());
        assert!(shape.model.vendor.is_some());
        // The audit's hybrid case: an EMPTY vendor array plus a populated
        // configOptions model select. The vendor parse returns None on an
        // empty catalog, so the config parse produces the manifest — and the
        // shape MUST say ConfigOptions, not VendorModels.
        let hybrid = parse(
            r#"{"sessionId":"s","models":{"availableModels":[]},"configOptions":[{"id":"model","category":"model","type":"select","currentValue":"m","options":[{"value":"m","name":"M"}]}]}"#,
        );
        let handshake = merge_handshake_manifest(&serde_json::Value::Null, &hybrid, None);
        let shape = handshake.shape.as_ref().expect("config switch shape");
        assert_eq!(
            shape.model.config.as_ref().map(|option| option.id.as_str()),
            Some("model")
        );
        assert!(shape.model.vendor.is_none());
    }

    #[test]
    fn the_switch_sends_the_option_id_the_agent_declared_not_a_constant() {
        // Detection must also work when one option omits category and the
        // other uses an unknown category; category is advisory. The
        // discovered ids must be carried through so the switch names options
        // the agent actually offered.
        let result = parse(
            r#"{"configOptions":[{"id":"engine","type":"select","currentValue":"v2","options":[{"value":"v2","name":"Engine v2"},{"value":"v1","name":"Engine v1"}]},{"id":"thinking","category":"future_reasoning_selector","type":"select","currentValue":"deep","options":[{"value":"deep","name":"Deep"}]}]}"#,
        );
        let catalog = catalog_from_config_options(&result, None, None, None)
            .expect("categoryless and unknown-category options parse");
        assert_eq!(catalog.model_option_id, "engine");
        assert_eq!(catalog.effort_option_id.as_deref(), Some("thinking"));
        let SessionEvent::SessionManifest {
            current_model_id, ..
        } = &catalog.manifest
        else {
            panic!("expected SessionManifest");
        };
        assert_eq!(current_model_id.as_deref(), Some("v2"));
        // A reply re-parsed with the recorded ids matches by them first.
        let reparsed =
            catalog_from_config_options(&result, None, None, Some(("engine", Some("thinking"))))
                .expect("known ids re-parse");
        assert_eq!(reparsed.model_option_id, "engine");
    }

    #[test]
    fn a_current_value_absent_from_the_offered_options_is_rejected_and_duplicates_fold() {
        // A current selection the options list does not contain would render
        // a controlled select whose value is absent from its options.
        let ghost = parse(
            r#"{"configOptions":[{"id":"model","category":"model","type":"select","currentValue":"ghost","options":[{"value":"real","name":"Real"}]}]}"#,
        );
        assert!(catalog_from_config_options(&ghost, None, None, None).is_none());
        // Duplicate model ids survive as one entry (the first declaration
        // wins) instead of producing duplicate React keys.
        let dupes = parse(
            r#"{"configOptions":[{"id":"model","category":"model","type":"select","currentValue":"a","options":[{"value":"a","name":"A"},{"value":"a","name":"A again"},{"value":"b","name":"B"}]}]}"#,
        );
        let catalog =
            catalog_from_config_options(&dupes, None, None, None).expect("duplicate ids fold");
        let SessionEvent::SessionManifest { models, .. } = catalog.manifest else {
            panic!("expected SessionManifest");
        };
        let ids: Vec<&str> = models.iter().map(|model| model.model_id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    // Wire shape measured from a live journal:
    // recon/probes/grok-xai-models-update.txt. The two `_x.ai/models/update`
    // envelopes there match GROK_MODELS above.
    #[test]
    fn xai_models_update_replaces_the_manifest() {
        let params = parse(GROK_MODELS);
        let SessionEvent::SessionManifest {
            current_model_id,
            models,
            ..
        } = session_manifest_from_models_update(&params, Some("grok".to_string()))
            .expect("models update must parse")
        else {
            panic!("expected SessionManifest");
        };
        assert_eq!(current_model_id.as_deref(), Some("grok-4.6"));
        assert_eq!(models[0].current_effort.as_deref(), Some("xhigh"));

        let envelope = parse(&format!(
            r#"{{"jsonrpc":"2.0","method":"_x.ai/models/update","params":{GROK_MODELS}}}"#
        ));
        let view = view_from_envelope(&envelope, SESSION).expect("models update is modeled");
        let SessionEvent::SessionManifest {
            current_model_id, ..
        } = view
        else {
            panic!("models update must become SessionManifest, got {view:?}");
        };
        assert_eq!(current_model_id.as_deref(), Some("grok-4.6"));
    }

    #[test]
    fn tool_call_forwards_kind_and_relativized_locations() {
        let cwd = std::path::Path::new(r"C:\work");
        let line = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"tool_call","toolCallId":"call-1","title":"Read lib.rs","status":"pending","kind":"read","locations":[{"path":"C:\\work\\src\\lib.rs","line":12}]}}}"#,
        );
        match view_from_envelope_in(&line, SESSION, Some(cwd)) {
            Some(SessionEvent::AgentToolCall {
                tool_call_id,
                kind,
                locations,
                ..
            }) => {
                assert_eq!(tool_call_id, "call-1");
                assert_eq!(kind.as_deref(), Some("read"));
                let locations = locations.expect("locations");
                assert_eq!(locations.len(), 1);
                assert_eq!(
                    locations[0].path,
                    std::path::PathBuf::from("src")
                        .join("lib.rs")
                        .to_string_lossy()
                );
                assert_eq!(locations[0].line, Some(12));
            }
            other => panic!("expected tool call with locations, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_update_locations_replace_and_are_not_merged() {
        let cwd = std::path::Path::new(r"C:\work");
        let line = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"completed","kind":"edit","locations":[{"path":"C:\\work\\src\\main.rs"}]}}}"#,
        );
        match view_from_envelope_in(&line, SESSION, Some(cwd)) {
            Some(SessionEvent::AgentToolUpdate {
                kind,
                locations,
                status,
                ..
            }) => {
                assert_eq!(status.as_deref(), Some("completed"));
                assert_eq!(kind.as_deref(), Some("edit"));
                let locations = locations.expect("replaced locations");
                assert_eq!(locations.len(), 1);
                assert_eq!(
                    locations[0].path,
                    std::path::PathBuf::from("src")
                        .join("main.rs")
                        .to_string_lossy()
                );
                assert!(locations[0].line.is_none());
            }
            other => panic!("expected tool update with replaced locations, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_update_forwards_title_when_present() {
        let line = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"in_progress","title":"cargo test"}}}"#,
        );
        match view_from_envelope(&line, SESSION) {
            Some(SessionEvent::AgentToolUpdate { title, status, .. }) => {
                assert_eq!(status.as_deref(), Some("in_progress"));
                assert_eq!(title.as_deref(), Some("cargo test"));
            }
            other => panic!("expected tool update with title, got {other:?}"),
        }
        let untitled = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"tool_call_update","toolCallId":"call-1"}}}"#,
        );
        match view_from_envelope(&untitled, SESSION) {
            Some(SessionEvent::AgentToolUpdate { title, .. }) => {
                assert!(title.is_none());
            }
            other => panic!("expected tool update without title, got {other:?}"),
        }
        let empty = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"tool_call_update","toolCallId":"call-1","title":""}}}"#,
        );
        match view_from_envelope(&empty, SESSION) {
            Some(SessionEvent::AgentToolUpdate { title, .. }) => {
                assert!(title.is_none(), "empty title must not overwrite the row");
            }
            other => panic!("expected tool update with empty title, got {other:?}"),
        }
    }
}
