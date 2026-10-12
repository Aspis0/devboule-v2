//! Codex's vocabulary before any agent runs: the models a short-lived Codex
//! app-server lists, or failing that Codex's model cache while it is fresh,
//! and its permission modes from the daemon's own mode table.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use devboule_protocol::{VocabularyModels, VocabularyModes, VocabularyOrigin, VocabularyState};
use serde_json::{json, Value};

use crate::codex_catalog_source::CodexCatalogSource;
use crate::codex_model_catalog::{catalog_from_listing, configured_defaults};
use crate::session::codex_catalog_probe::{cli_version, read_catalog, CodexCatalogRead};

const MODELS_CACHE_FILE: &str = "models_cache.json";
/// A cache older than this is not offered: it may list models Codex has since retired.
const CACHE_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// Codex's models and modes. Without a live answer and without a fresh cache
/// the models axis is absent; the modes are the daemon's table and always present.
pub(crate) fn codex_axes(source: &CodexCatalogSource) -> (VocabularyModels, VocabularyModes) {
    let home = crate::codex_command_catalog::resolve_home();
    let disk = disk_cache_read(&home, SystemTime::now());
    models_and_modes(source.read(Arc::new(read_catalog), disk), cli_version())
}

fn models_and_modes(
    read: Option<CodexCatalogRead>,
    version: Option<[u64; 3]>,
) -> (VocabularyModels, VocabularyModes) {
    let catalog =
        read.map(|read| catalog_from_listing(&read.rows, &configured_defaults(&read.config)));
    let models = match catalog {
        Some(catalog) if !catalog.models.is_empty() => VocabularyModels::new(
            VocabularyState::Present,
            Some(VocabularyOrigin::Provider),
            catalog.models,
        )
        .expect("a present models axis always carries its origin and items")
        .with_default_id(catalog.default_id),
        _ => crate::provider_vocabulary::absent_axes().0,
    };
    let modes = VocabularyModes::new(
        VocabularyState::Present,
        Some(VocabularyOrigin::Daemon),
        crate::codex_view::available_mode_views(version),
    )
    .expect("a present modes axis always carries its origin")
    .with_default_id(Some(
        crate::codex_view::default_mode_id(version).to_string(),
    ));
    (models, modes)
}

/// The rows of Codex's model cache, when the file is recent and well-formed. The
/// cache names no configured default, so a draft answered from it leaves the model unset.
pub(crate) fn disk_cache_read(home: &Path, now: SystemTime) -> Option<CodexCatalogRead> {
    let path = home.join(MODELS_CACHE_FILE);
    let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
    if !now
        .duration_since(modified)
        .is_ok_and(|age| age <= CACHE_MAX_AGE)
    {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    Some(CodexCatalogRead {
        rows: cache_rows(&value)?,
        config: Value::Null,
    })
}

/// The cache's listed models as `model/list` rows, in its priority order.
fn cache_rows(value: &Value) -> Option<Vec<Value>> {
    let mut listed: Vec<(i64, Value)> = value
        .get("models")?
        .as_array()?
        .iter()
        .filter(|entry| entry.get("visibility").and_then(Value::as_str) == Some("list"))
        .filter(|entry| entry.get("supported_in_api").and_then(Value::as_bool) != Some(false))
        .map(|entry| {
            let priority = entry
                .get("priority")
                .and_then(Value::as_i64)
                .unwrap_or(i64::MAX);
            (priority, listing_row(entry))
        })
        .collect();
    listed.sort_by_key(|(priority, _)| *priority);
    Some(listed.into_iter().map(|(_, row)| row).collect())
}

fn listing_row(entry: &Value) -> Value {
    let levels: Vec<Value> = entry
        .get("supported_reasoning_levels")
        .and_then(Value::as_array)
        .map(|levels| {
            levels
                .iter()
                .map(|level| {
                    json!({
                        "reasoningEffort": level.get("effort"),
                        "description": level.get("description"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "model": entry.get("slug"),
        "displayName": entry.get("display_name"),
        "description": entry.get("description"),
        "defaultReasoningEffort": entry.get("default_reasoning_level"),
        "supportedReasoningEfforts": levels,
    })
}

#[cfg(test)]
mod tests {
    use super::{disk_cache_read, models_and_modes, CACHE_MAX_AGE};
    use crate::session::codex_catalog_probe::CodexCatalogRead;
    use devboule_protocol::{VocabularyOrigin, VocabularyState};
    use serde_json::{json, Value};
    use std::time::SystemTime;

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
    fn a_live_answer_lists_its_models_with_the_configured_default() {
        let probe = CodexCatalogRead {
            rows: vec![
                json!({"model": "gpt-a", "displayName": "GPT-A", "isDefault": true,
                       "defaultReasoningEffort": "low",
                       "supportedReasoningEfforts": [{"reasoningEffort": "low"}]}),
                json!({"model": "gpt-b", "displayName": "GPT-B", "isDefault": false,
                       "defaultReasoningEffort": "low",
                       "supportedReasoningEfforts": [{"reasoningEffort": "low"}, {"reasoningEffort": "xhigh"}]}),
            ],
            config: json!({"config": {"model": "gpt-b", "model_reasoning_effort": "xhigh"}}),
        };
        let (models, modes) = models_and_modes(Some(probe), None);

        assert_eq!(models.state, VocabularyState::Present);
        assert_eq!(models.origin, Some(VocabularyOrigin::Provider));
        assert_eq!(models.default_id.as_deref(), Some("gpt-b"));
        assert_eq!(modes.default_id.as_deref(), Some("auto"));
    }

    #[test]
    fn a_fresh_cache_gives_the_models_with_no_default() {
        let home = home_with_cache("fresh", CACHE);
        let (models, _) = models_and_modes(disk_cache_read(&home, SystemTime::now()), None);

        assert_eq!(models.state, VocabularyState::Present);
        assert_eq!(models.origin, Some(VocabularyOrigin::Provider));
        let ids: Vec<&str> = models.items.iter().map(|m| m.model_id.as_str()).collect();
        assert_eq!(ids, ["gpt-a", "gpt-b"], "a hidden model is not offered");
        assert_eq!(models.default_id, None, "the draft leaves the model unset");
        let efforts = models.items[0].efforts.as_ref().expect("efforts");
        assert_eq!(efforts[0].label, "Low");
        assert_eq!(efforts[1].label, "Extra high");
    }

    #[test]
    fn a_stale_cache_is_not_read() {
        let home = home_with_cache("stale", CACHE);
        let file = std::fs::File::options()
            .write(true)
            .open(home.join("models_cache.json"))
            .expect("cache file");
        let month_old = SystemTime::now() - CACHE_MAX_AGE * 2;
        file.set_modified(month_old).expect("age the cache");

        assert!(disk_cache_read(&home, SystemTime::now()).is_none());
    }

    #[test]
    fn no_answer_and_no_cache_leave_models_absent_and_modes_present() {
        let (models, modes) = models_and_modes(None, Some([0, 162, 0]));

        assert_eq!(models.state, VocabularyState::Absent);
        assert_eq!(modes.state, VocabularyState::Present);
        assert_eq!(modes.origin, Some(VocabularyOrigin::Daemon));
        let ids: Vec<&str> = modes.items.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["read-only", "auto", "auto-review", "full-access"]);
    }

    #[test]
    fn auto_review_is_offered_and_defaulted_only_from_the_release_that_routes_it() {
        let older = models_and_modes(None, Some([0, 114, 9]));
        let ids: Vec<&str> = older.1.items.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["read-only", "auto", "full-access"]);
        assert_eq!(older.1.default_id.as_deref(), Some("auto"));

        let newer = models_and_modes(None, Some([0, 115, 0]));
        assert!(newer.1.items.iter().any(|m| m.id == "auto-review"));
        assert_eq!(newer.1.default_id.as_deref(), Some("auto-review"));
    }

    #[test]
    fn a_cache_that_is_not_the_expected_shape_leaves_models_absent() {
        let home = home_with_cache("shape", r#"{"models": "not a list"}"#);
        let (models, _) = models_and_modes(disk_cache_read(&home, SystemTime::now()), None);

        assert_eq!(models.state, VocabularyState::Absent);
    }

    #[test]
    fn a_probe_with_no_rows_leaves_models_absent() {
        let probe = CodexCatalogRead {
            rows: Vec::new(),
            config: Value::Null,
        };
        let (models, _) = models_and_modes(Some(probe), None);

        assert_eq!(models.state, VocabularyState::Absent);
    }
}
