//! Codex's model catalog as the pre-agent draft reads it: the rows `model/list`
//! answers, the defaults `config/read` names, and the model a new agent starts on.

use devboule_protocol::{SessionModel, SessionModelEffort};
use serde_json::Value;

/// The model and effort the person's Codex config selects.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ConfiguredDefaults {
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
}

/// The models a draft offers, and the one a new agent starts on. `default_id`
/// is `None` when the configured model is not in the list and no listed model
/// is Codex's default: the draft then leaves the model unset.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CodexModelCatalog {
    pub(crate) models: Vec<SessionModel>,
    pub(crate) default_id: Option<String>,
}

/// The defaults in a `config/read` answer, or none when the answer has no config.
pub(crate) fn configured_defaults(config_read: &Value) -> ConfiguredDefaults {
    let config = config_read.get("config");
    let text = |key: &str| {
        config
            .and_then(|config| config.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    ConfiguredDefaults {
        model: text("model"),
        effort: text("model_reasoning_effort"),
    }
}

/// The catalog from one `model/list` listing, in the order Codex answers it.
pub(crate) fn catalog_from_listing(
    rows: &[Value],
    configured: &ConfiguredDefaults,
) -> CodexModelCatalog {
    let mut models: Vec<(bool, SessionModel, Option<String>)> = Vec::new();
    for row in rows {
        let Some(model) = model_from_row(row) else {
            continue;
        };
        if models
            .iter()
            .any(|(_, listed, _)| listed.model_id == model.0.model_id)
        {
            continue;
        }
        models.push((
            row.get("isDefault") == Some(&Value::Bool(true)),
            model.0,
            model.1,
        ));
    }
    let default_index = match &configured.model {
        Some(configured_model) => models
            .iter()
            .position(|(_, model, _)| &model.model_id == configured_model),
        None => models.iter().position(|(is_default, _, _)| *is_default),
    };
    let default_id = default_index.map(|index| models[index].1.model_id.clone());
    let default_effort = default_index.and_then(|index| {
        let (_, model, default_effort) = &models[index];
        let efforts = model.efforts.as_deref().unwrap_or_default();
        let listed = |effort: &str| efforts.iter().any(|choice| choice.id == effort);
        configured
            .effort
            .clone()
            .filter(|effort| listed(effort))
            .or_else(|| default_effort.clone().filter(|effort| listed(effort)))
    });
    let models = models
        .into_iter()
        .enumerate()
        .map(|(index, (_, mut model, _))| {
            if Some(index) == default_index {
                if let Some(effort) = &default_effort {
                    for choice in model.efforts.iter_mut().flatten() {
                        choice.default = Some(&choice.id == effort);
                    }
                }
            }
            model
        })
        .collect();
    CodexModelCatalog { models, default_id }
}

/// One listing row as a model, with the default effort Codex names for it.
/// `None` for a row with no id or a hidden one.
fn model_from_row(row: &Value) -> Option<(SessionModel, Option<String>)> {
    if row.get("hidden") == Some(&Value::Bool(true)) {
        return None;
    }
    let model_id = text(row.get("model")).or_else(|| text(row.get("id")))?;
    let name = text(row.get("displayName")).unwrap_or_else(|| model_id.clone());
    let default_effort = text(row.get("defaultReasoningEffort"));
    let efforts = row
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .map(|levels| levels.iter().filter_map(effort_choice).collect())
        .unwrap_or_default();
    let model = SessionModel {
        model_id,
        name,
        provider_id: None,
        description: text(row.get("description")),
        context_tokens: None,
        current_effort: None,
        efforts: Some(efforts),
        // The listing names no input modalities, so an image goes as a path line.
        accepts_images: false,
    };
    Some((model, default_effort))
}

fn effort_choice(level: &Value) -> Option<SessionModelEffort> {
    let id = text(level.get("reasoningEffort"))?;
    Some(SessionModelEffort {
        label: effort_label(&id),
        default: None,
        description: text(level.get("description")),
        id,
    })
}

/// The word a person reads for an effort id: the spellings Codex uses for
/// its two irregular ones, the rest capitalised.
fn effort_label(id: &str) -> String {
    match id {
        "xhigh" => "Extra high".to_string(),
        "minimal" => "Minimal".to_string(),
        _ => {
            let mut chars = id.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        }
    }
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::{catalog_from_listing, configured_defaults, ConfiguredDefaults};
    use serde_json::{json, Value};

    fn row(id: &str, default: bool, efforts: &[&str], default_effort: &str) -> Value {
        json!({
            "model": id,
            "displayName": format!("Model {id}"),
            "isDefault": default,
            "defaultReasoningEffort": default_effort,
            "supportedReasoningEfforts": efforts
                .iter()
                .map(|effort| json!({"reasoningEffort": effort, "description": "d"}))
                .collect::<Vec<_>>(),
        })
    }

    fn default_efforts(model: &devboule_protocol::SessionModel) -> Vec<(String, Option<bool>)> {
        model
            .efforts
            .as_ref()
            .expect("efforts")
            .iter()
            .map(|choice| (choice.id.clone(), choice.default))
            .collect()
    }

    #[test]
    fn hidden_rows_are_not_offered_and_the_listing_order_stands() {
        let rows = [
            row("gpt-b", false, &["low"], "low"),
            json!({"model": "gpt-h", "hidden": true, "supportedReasoningEfforts": []}),
            row("gpt-a", false, &["low"], "low"),
        ];
        let catalog = catalog_from_listing(&rows, &ConfiguredDefaults::default());

        let ids: Vec<&str> = catalog.models.iter().map(|m| m.model_id.as_str()).collect();
        assert_eq!(ids, ["gpt-b", "gpt-a"]);
        assert_eq!(catalog.models[0].name, "Model gpt-b");
    }

    #[test]
    fn a_configured_model_is_the_default_with_its_configured_effort() {
        let rows = [
            row("gpt-a", true, &["low", "high"], "low"),
            row("gpt-b", false, &["low", "xhigh"], "low"),
        ];
        let configured = ConfiguredDefaults {
            model: Some("gpt-b".to_string()),
            effort: Some("xhigh".to_string()),
        };
        let catalog = catalog_from_listing(&rows, &configured);

        assert_eq!(catalog.default_id.as_deref(), Some("gpt-b"));
        let configured_model = &catalog.models[1];
        assert_eq!(
            default_efforts(configured_model),
            [
                ("low".to_string(), Some(false)),
                ("xhigh".to_string(), Some(true))
            ]
        );
    }

    #[test]
    fn a_configured_model_the_listing_omits_leaves_no_default_and_no_first_row() {
        let rows = [row("gpt-a", true, &["low"], "low")];
        let configured = ConfiguredDefaults {
            model: Some("gpt-gone".to_string()),
            effort: None,
        };
        let catalog = catalog_from_listing(&rows, &configured);

        assert_eq!(catalog.default_id, None);
        assert!(default_efforts(&catalog.models[0])
            .iter()
            .all(|(_, d)| *d != Some(true)));
    }

    #[test]
    fn without_configuration_the_listing_default_is_the_default() {
        let rows = [
            row("gpt-a", false, &["low"], "low"),
            row("gpt-b", true, &["low", "high"], "high"),
        ];
        let catalog = catalog_from_listing(&rows, &ConfiguredDefaults::default());

        assert_eq!(catalog.default_id.as_deref(), Some("gpt-b"));
        assert_eq!(
            default_efforts(&catalog.models[1]),
            [
                ("low".to_string(), Some(false)),
                ("high".to_string(), Some(true))
            ]
        );
    }

    #[test]
    fn a_model_without_an_effort_list_is_kept_with_none() {
        let rows = [json!({"model": "gpt-plain", "displayName": "Plain"})];
        let catalog = catalog_from_listing(&rows, &ConfiguredDefaults::default());

        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.models[0].efforts, Some(Vec::new()));
        assert_eq!(catalog.default_id, None);
    }

    #[test]
    fn effort_ids_are_read_as_their_labels() {
        let rows = [row("gpt-a", true, &["minimal", "low", "xhigh"], "low")];
        let catalog = catalog_from_listing(&rows, &ConfiguredDefaults::default());

        let labels: Vec<&str> = catalog.models[0]
            .efforts
            .as_ref()
            .expect("efforts")
            .iter()
            .map(|choice| choice.label.as_str())
            .collect();
        assert_eq!(labels, ["Minimal", "Low", "Extra high"]);
        let wire_id = catalog.models[0].efforts.as_ref().expect("efforts")[2]
            .id
            .as_str();
        assert_eq!(wire_id, "xhigh");
    }

    #[test]
    fn the_config_read_answer_names_the_model_and_effort() {
        let answer = json!({"config": {"model": " gpt-b ", "model_reasoning_effort": "high"}});

        assert_eq!(
            configured_defaults(&answer),
            ConfiguredDefaults {
                model: Some("gpt-b".to_string()),
                effort: Some("high".to_string()),
            }
        );
        assert_eq!(
            configured_defaults(&Value::Null),
            ConfiguredDefaults::default()
        );
    }
}
