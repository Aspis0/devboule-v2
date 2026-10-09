//! Pi model identity: a model is its provider and its id together. The same id
//! served by two providers stays two models in the catalog and on the wire, and
//! a bare id names a model only when exactly one provider serves it.

use super::{catalog_from_responses, manifest_from_catalog, PiCatalog, PiLookup};
use devboule_protocol::{SessionEvent, SessionModel};
use serde_json::json;

fn models_response(models: serde_json::Value) -> serde_json::Value {
    json!({"data": {"models": models}})
}

fn state_on(id: &str, provider: &str) -> serde_json::Value {
    json!({"data": {"model": {"id": id, "provider": provider}, "thinkingLevel": "off"}})
}

fn no_levels() -> serde_json::Value {
    json!({"data": {"levels": []}})
}

fn catalog(current: (&str, &str), models: serde_json::Value) -> PiCatalog {
    catalog_from_responses(
        &state_on(current.0, current.1),
        &models_response(models),
        &no_levels(),
    )
    .expect("the fixture catalog builds")
}

/// `nemotron` is served by two providers; the current model is the second.
fn nemotron_twice() -> PiCatalog {
    catalog(
        ("nemotron", "opencode-go"),
        json!([
            {"id": "nemotron", "name": "Nemotron (free)", "provider": "openrouter"},
            {"id": "nemotron", "name": "Nemotron", "provider": "opencode-go"},
        ]),
    )
}

fn manifest_parts(catalog: &PiCatalog) -> (Option<String>, Vec<SessionModel>) {
    match manifest_from_catalog(catalog, "ask") {
        SessionEvent::SessionManifest {
            current_model_id,
            models,
            ..
        } => (current_model_id, models),
        other => panic!("expected a session manifest, got {other:?}"),
    }
}

#[test]
fn one_id_under_two_providers_is_two_listed_models() {
    let (current, models) = manifest_parts(&nemotron_twice());
    let listed: Vec<(&str, Option<&str>, &str)> = models
        .iter()
        .map(|model| {
            (
                model.model_id.as_str(),
                model.provider_id.as_deref(),
                model.name.as_str(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("opencode-go/nemotron", Some("opencode-go"), "Nemotron"),
            ("openrouter/nemotron", Some("openrouter"), "Nemotron (free)"),
        ],
        "each provider keeps its own row, with its own name, in key order"
    );
    assert_eq!(
        current.as_deref(),
        Some("opencode-go/nemotron"),
        "the current model is named by provider and id"
    );
}

#[test]
fn a_bare_id_held_by_two_providers_names_neither() {
    assert!(
        matches!(nemotron_twice().lookup("nemotron"), PiLookup::Ambiguous),
        "a bare id shared by two providers must not pick one of them"
    );
}

#[test]
fn a_key_names_exactly_its_own_provider() {
    match nemotron_twice().lookup("openrouter/nemotron") {
        PiLookup::Found(model) => assert_eq!(model.provider.as_deref(), Some("openrouter")),
        _ => panic!("the provider-qualified key must resolve to its own model"),
    }
}

#[test]
fn a_bare_id_held_by_one_provider_still_resolves() {
    let solo = catalog(
        ("minimax-m3", "opencode-go"),
        json!([{"id": "minimax-m3", "name": "MiniMax-M3", "provider": "opencode-go"}]),
    );
    match solo.lookup("minimax-m3") {
        PiLookup::Found(model) => assert_eq!(model.provider.as_deref(), Some("opencode-go")),
        _ => panic!("a profile names a bare id, and one provider serving it is no ambiguity"),
    }
}

#[test]
fn an_unknown_name_is_missing_not_ambiguous() {
    assert!(matches!(nemotron_twice().lookup("nope"), PiLookup::Missing));
}

#[test]
fn a_state_spelling_the_list_lacks_takes_the_listed_provider() {
    // get_state says "opencode" for a model the list files under "opencode-go".
    let divergent = catalog(
        ("minimax-m3", "opencode"),
        json!([{"id": "minimax-m3", "name": "MiniMax-M3", "provider": "opencode-go"}]),
    );
    assert_eq!(
        divergent.current_key().as_deref(),
        Some("opencode-go/minimax-m3"),
        "the running model must key to the row the list publishes"
    );
    let (current, models) = manifest_parts(&divergent);
    assert_eq!(current.as_deref(), Some("opencode-go/minimax-m3"));
    assert!(
        models
            .iter()
            .any(|model| Some(model.model_id.as_str()) == current.as_deref()),
        "the manifest's current model must be one of its listed models"
    );
}

#[test]
fn an_unmatched_state_spelling_with_two_rows_is_not_guessed() {
    let unmatched = catalog(
        ("nemotron", "opencode"),
        json!([
            {"id": "nemotron", "name": "Nemotron (free)", "provider": "openrouter"},
            {"id": "nemotron", "name": "Nemotron", "provider": "opencode-go"},
        ]),
    );
    assert_eq!(
        unmatched.current_key().as_deref(),
        Some("opencode/nemotron"),
        "two rows hold the id, so neither is the running model and none is picked"
    );
}
