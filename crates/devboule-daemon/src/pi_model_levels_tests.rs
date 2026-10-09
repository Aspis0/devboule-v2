//! What pi's model list says about each model that is not the one running:
//! its thinking levels (read the way pi reads them, `getSupportedThinkingLevels`)
//! and whether it takes image input.

use devboule_protocol::SessionEvent;

/// The catalog for `models` with `current` as the running model.
fn catalog_with_current(current: &str, models: &str) -> super::PiCatalog {
    let state = serde_json::json!({
        "data": {"model": {"id": current, "provider": "p"}, "thinkingLevel": "off"}
    });
    let models: serde_json::Value = serde_json::from_str(models).expect("captured models");
    let levels = serde_json::json!({"data": {"levels": ["off"]}});
    super::catalog_from_responses(&state, &models, &levels).expect("catalog")
}

fn level_ids(catalog: &super::PiCatalog, model_id: &str) -> Option<Vec<String>> {
    catalog
        .models
        .get(model_id)?
        .efforts
        .as_ref()
        .map(|efforts| efforts.iter().map(|effort| effort.id.clone()).collect())
}

#[test]
fn reasoning_model_without_a_level_map_offers_the_levels_pi_offers() {
    // The captured MiMo-V2.6-Flash entry reasons and carries no level map:
    // pi offers off through high, and not xhigh or max, which need a mapping.
    let catalog = catalog_with_current(
        "current",
        r#"{"data":{"models":[
{"id":"current","name":"Current","provider":"p","reasoning":true},
{"id":"xiaomi/mimo-v2.6-flash","name":"Xiaomi: MiMo-V2.6-Flash","provider":"openrouter","reasoning":true,"input":["text","image"]}
]}}"#,
    );
    assert_eq!(
        level_ids(&catalog, "xiaomi/mimo-v2.6-flash"),
        Some(
            ["off", "minimal", "low", "medium", "high"]
                .map(String::from)
                .to_vec()
        )
    );
}

#[test]
fn a_level_map_nulls_levels_out_and_names_xhigh() {
    let catalog = catalog_with_current(
        "current",
        r#"{"data":{"models":[
{"id":"current","name":"Current","provider":"p","reasoning":true},
{"id":"mapped","name":"Mapped","provider":"p","reasoning":true,"thinkingLevelMap":{"minimal":null,"xhigh":"max"}}
]}}"#,
    );
    assert_eq!(
        level_ids(&catalog, "mapped"),
        Some(
            ["off", "low", "medium", "high", "xhigh"]
                .map(String::from)
                .to_vec()
        )
    );
}

#[test]
fn a_model_that_does_not_reason_offers_no_control() {
    let catalog = catalog_with_current(
        "current",
        r#"{"data":{"models":[
{"id":"current","name":"Current","provider":"p","reasoning":true},
{"id":"plain","name":"Plain","provider":"p","reasoning":false}
]}}"#,
    );
    assert_eq!(level_ids(&catalog, "plain"), None);
}

#[test]
fn a_model_takes_images_only_when_pi_lists_image_input() {
    // The manifest tells the composer which models take image blocks: pi's
    // own `input` list decides, and a model whose list omits `image` does not.
    let catalog = catalog_with_current(
        "current",
        r#"{"data":{"models":[
{"id":"current","name":"Current","provider":"p","reasoning":true,"input":["text"]},
{"id":"vision","name":"Vision","provider":"p","reasoning":true,"input":["text","image"]},
{"id":"text-only","name":"Text only","provider":"p","reasoning":true,"input":["text"]}
]}}"#,
    );
    let SessionEvent::SessionManifest { models, .. } =
        super::manifest_from_catalog(&catalog, "bypass")
    else {
        panic!("a catalog builds a manifest");
    };
    let accepts = |id: &str| {
        models
            .iter()
            .find(|model| model.model_id == id)
            .map(|model| model.accepts_images)
    };
    assert_eq!(accepts("vision"), Some(true));
    assert_eq!(accepts("text-only"), Some(false));
}

#[test]
fn a_switch_publishes_the_levels_pi_answered_with() {
    // The catalog estimated the new model's levels before the switch; pi's
    // answer after the switch is the list the manifest must publish.
    let mut catalog = catalog_with_current(
        "current",
        r#"{"data":{"models":[
{"id":"current","name":"Current","provider":"p","reasoning":true},
{"id":"mimo","name":"MiMo","provider":"openrouter","reasoning":true}
]}}"#,
    );
    catalog.record_switch(
        "mimo",
        "openrouter".to_string(),
        vec!["off".to_string(), "high".to_string()],
        Some("high".to_string()),
    );
    let SessionEvent::SessionManifest { models, .. } =
        super::manifest_from_catalog(&catalog, "bypass")
    else {
        panic!("a catalog builds a manifest");
    };
    let mimo = models
        .iter()
        .find(|model| model.model_id == "mimo")
        .expect("mimo");
    assert_eq!(
        level_ids(&catalog, "mimo"),
        Some(vec!["off".to_string(), "high".to_string()])
    );
    assert_eq!(mimo.current_effort.as_deref(), Some("high"));
}
