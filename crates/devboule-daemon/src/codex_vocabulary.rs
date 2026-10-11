//! Codex's vocabulary before any agent runs: its models, read from the model
//! cache Codex keeps under its home, and its permission modes, from the
//! daemon's own mode table.

use std::path::Path;

use devboule_protocol::{
    SessionModel, SessionModelEffort, VocabularyModels, VocabularyModes, VocabularyOrigin,
    VocabularyState,
};
use serde_json::Value;

const MODELS_CACHE_FILE: &str = "models_cache.json";

/// Codex's models and modes. A missing or unreadable model cache leaves the
/// models axis absent; the modes are the daemon's table and always present.
pub(crate) fn codex_axes() -> (VocabularyModels, VocabularyModes) {
    codex_axes_in(&crate::codex_command_catalog::resolve_home())
}

fn codex_axes_in(home: &Path) -> (VocabularyModels, VocabularyModes) {
    let models = match cached_models(&home.join(MODELS_CACHE_FILE)) {
        Some(models) if !models.is_empty() => VocabularyModels::new(
            VocabularyState::Present,
            Some(VocabularyOrigin::Provider),
            models,
        )
        .expect("a present models axis always carries its origin and items"),
        _ => crate::provider_vocabulary::absent_axes().0,
    };
    let modes = VocabularyModes::new(
        VocabularyState::Present,
        Some(VocabularyOrigin::Daemon),
        crate::codex_view::available_mode_views(),
    )
    .expect("a present modes axis always carries its origin");
    (models, modes)
}

/// The listed models of the cache, in Codex's own priority order. A file that
/// is missing, unreadable or not the expected shape answers `None`: nothing is
/// guessed from it.
fn cached_models(path: &Path) -> Option<Vec<SessionModel>> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let mut listed: Vec<(i64, SessionModel)> = value
        .get("models")?
        .as_array()?
        .iter()
        .filter(|entry| entry.get("visibility").and_then(Value::as_str) == Some("list"))
        .filter(|entry| entry.get("supported_in_api").and_then(Value::as_bool) != Some(false))
        .filter_map(|entry| {
            let priority = entry
                .get("priority")
                .and_then(Value::as_i64)
                .unwrap_or(i64::MAX);
            Some((priority, model_from_entry(entry)?))
        })
        .collect();
    listed.sort_by_key(|(priority, _)| *priority);
    Some(listed.into_iter().map(|(_, model)| model).collect())
}

fn model_from_entry(entry: &Value) -> Option<SessionModel> {
    let model_id = entry.get("slug")?.as_str()?.to_string();
    let default_effort = entry.get("default_reasoning_level").and_then(Value::as_str);
    let efforts = entry
        .get("supported_reasoning_levels")?
        .as_array()?
        .iter()
        .filter_map(|level| {
            let id = level.get("effort")?.as_str()?.to_string();
            Some(SessionModelEffort {
                label: id.clone(),
                default: Some(Some(id.as_str()) == default_effort),
                description: level
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                id,
            })
        })
        .collect();
    Some(SessionModel {
        name: entry
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or(&model_id)
            .to_string(),
        description: entry
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        model_id,
        provider_id: None,
        context_tokens: None,
        current_effort: None,
        efforts: Some(efforts),
        // The cache names no input modalities, so an image goes as a path line.
        accepts_images: false,
    })
}

#[cfg(test)]
mod tests {
    use super::codex_axes_in;
    use devboule_protocol::{VocabularyOrigin, VocabularyState};

    fn home_with_cache(tag: &str, body: &str) -> std::path::PathBuf {
        let home = crate::test_dirs::test_temp_dir(&format!("devboule-codex-vocabulary-{tag}"));
        std::fs::create_dir_all(&home).expect("codex home");
        std::fs::write(home.join("models_cache.json"), body).expect("model cache");
        home
    }

    const CACHE: &str = r#"{
        "models": [
            {
                "slug": "gpt-b", "display_name": "GPT-B", "description": "second",
                "default_reasoning_level": "high", "visibility": "list",
                "supported_in_api": true, "priority": 2,
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "fast"},
                    {"effort": "high", "description": "deep"}
                ]
            },
            {
                "slug": "gpt-a", "display_name": "GPT-A", "description": "first",
                "default_reasoning_level": "low", "visibility": "list",
                "supported_in_api": true, "priority": 1,
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "fast"},
                    {"effort": "xhigh", "description": "most"}
                ]
            },
            {
                "slug": "hidden", "display_name": "Hidden", "description": "hidden",
                "default_reasoning_level": "low", "visibility": "hide",
                "supported_in_api": true, "priority": 0,
                "supported_reasoning_levels": []
            }
        ]
    }"#;

    #[test]
    fn listed_models_come_from_the_cache_in_its_priority_order() {
        let home = home_with_cache("listed", CACHE);
        let (models, _) = codex_axes_in(&home);

        assert_eq!(models.state, VocabularyState::Present);
        assert_eq!(models.origin, Some(VocabularyOrigin::Provider));
        let ids: Vec<&str> = models.items.iter().map(|m| m.model_id.as_str()).collect();
        assert_eq!(ids, ["gpt-a", "gpt-b"], "a hidden model is not offered");
        let first = &models.items[0];
        assert_eq!(first.name, "GPT-A");
        let efforts = first.efforts.as_ref().expect("efforts");
        let effort_ids: Vec<&str> = efforts.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(effort_ids, ["low", "xhigh"]);
        assert_eq!(efforts[0].default, Some(true), "the cache's default effort");
        assert_eq!(efforts[1].default, Some(false));
    }

    #[test]
    fn a_missing_cache_leaves_models_absent_and_modes_present() {
        let home = crate::test_dirs::test_temp_dir("devboule-codex-vocabulary-missing");
        std::fs::create_dir_all(&home).expect("codex home");
        let (models, modes) = codex_axes_in(&home);

        assert_eq!(models.state, VocabularyState::Absent);
        assert_eq!(modes.state, VocabularyState::Present);
        assert_eq!(modes.origin, Some(VocabularyOrigin::Daemon));
        let ids: Vec<&str> = modes.items.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["read-only", "auto", "auto-review", "full-access"]);
    }

    #[test]
    fn a_cache_that_is_not_the_expected_shape_leaves_models_absent() {
        let home = home_with_cache("shape", r#"{"models": "not a list"}"#);
        let (models, _) = codex_axes_in(&home);

        assert_eq!(models.state, VocabularyState::Absent);
    }
}
