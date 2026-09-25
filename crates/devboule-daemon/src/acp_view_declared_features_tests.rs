//! The ACP feature declaration read: what a `configOptions` array says a
//! provider offers, and what the daemon keeps out of it.
//!
//! The tests here are the ones that can go red without a process: the read is a
//! pure function of the agent's own reply, so every rule about which options
//! become a control is checkable on a fixture. The spawn that obtains the reply
//! is proved in `tests/acp_sessions.rs`.

use serde_json::json;

use super::{
    declared_features_from_options, declared_surfaces_from_options, merge_handshake_manifest,
    modes_from_standard,
};

fn options(array: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "sessionId": "s", "configOptions": array })
}

fn model_and_effort() -> serde_json::Value {
    options(json!([
        {
            "id": "model", "type": "select", "category": "model", "currentValue": "m1",
            "options": [{"value": "m1", "name": "One"}, {"value": "m2", "name": "Two"}]
        },
        {
            "id": "effort", "type": "select", "category": "thought_level", "currentValue": "high",
            "options": [{"value": "low", "name": "Low"}, {"value": "high", "name": "High"}]
        }
    ]))
}

/// Step 1's rule, the whole reason the parse keeps a third list: the model and
/// effort selectors are already the profile's `model` and `thinkingOptionId`
/// fields. A second control over the same value would be two sources for one
/// setting, free to disagree with the card.
#[test]
fn the_model_and_effort_options_are_not_features() {
    let declared =
        declared_features_from_options(&model_and_effort(), &[Some("model"), Some("effort")], None);
    assert!(
        declared.is_empty(),
        "two declared switches are zero features: {declared:?}"
    );
}

/// A vendor's own dial is the case the old parse threw away. Its declared `id`
/// becomes the stored key and the `configId` the wire takes, so no mapping
/// table sits between the control a human drew and the frame the agent reads.
#[test]
fn a_third_select_option_becomes_a_select_feature() {
    let result = options(json!([
        {"id": "model", "type": "select", "category": "model", "currentValue": "m1",
         "options": [{"value": "m1", "name": "One"}]},
        {"id": "fast", "type": "select", "name": "Fast responses", "currentValue": "on",
         "options": [{"value": "on", "name": "On"}, {"value": "off", "name": "Off"}]}
    ]));
    let declared = declared_features_from_options(&result, &[Some("model"), None], None);
    assert_eq!(
        declared.len(),
        1,
        "one feature beside the switch: {declared:?}"
    );
    let feature = &declared[0];
    assert_eq!(feature.id, "fast");
    assert_eq!(feature.label, "Fast responses");
    assert_eq!(
        feature.control,
        devboule_protocol::VocabularyFeatureControl::Select
    );
    assert_eq!(
        feature
            .options
            .iter()
            .map(|o| o.id.as_str())
            .collect::<Vec<_>>(),
        ["on", "off"],
        "the agent's own choice values, in its own order"
    );
    // Every row here is the provider's: the daemon did not author any of it.
    assert_eq!(
        feature.author,
        devboule_protocol::VocabularyOrigin::Provider
    );
}

/// The surfaces are the same list reduced to what a delivery validates a
/// choice against. Read through a separate function they would be a second
/// answer about one array, free to disagree with the form's.
#[test]
fn the_surface_list_and_the_declaration_list_agree_by_construction() {
    let result = model_and_effort();
    let declared = declared_features_from_options(&result, &[Some("model"), Some("effort")], None);
    let surfaces = declared_surfaces_from_options(&result, &[Some("model"), Some("effort")], None);
    assert_eq!(surfaces.len(), declared.len());

    let result = options(json!([
        {"id": "engine", "type": "select", "currentValue": "a",
         "options": [{"value": "a", "name": "A"}, {"value": "b", "name": "B"}]}
    ]));
    let declared = declared_features_from_options(&result, &[None, None], None);
    let surfaces = declared_surfaces_from_options(&result, &[None, None], None);
    assert_eq!(surfaces.len(), 1);
    assert_eq!(surfaces[0].id, declared[0].id);
    assert_eq!(
        surfaces[0].values,
        declared[0]
            .options
            .iter()
            .map(|option| option.id.clone())
            .collect::<Vec<_>>()
    );
}

/// A switch the daemon could not identify contributes nothing to the exclusion,
/// and must not swallow a real feature: `None` in the slot means "no option was
/// claimed by this switch", not "no option may be offered".
#[test]
fn an_unidentified_switch_excludes_nothing() {
    let result = options(json!([
        {"id": "engine", "type": "select", "currentValue": "a",
         "options": [{"value": "a", "name": "A"}]}
    ]));
    let declared = declared_features_from_options(&result, &[None, None], None);
    assert_eq!(declared.len(), 1, "no switch claimed it: {declared:?}");
    // And the same array with the option claimed as the model switch is empty.
    let claimed = declared_features_from_options(&result, &[Some("engine"), None], None);
    assert!(
        claimed.is_empty(),
        "claimed by the model switch: {claimed:?}"
    );
}

/// Junk a provider emits is dropped row by row, never fatal to the handshake:
/// an unnamed dial cannot be stored or set, a dial with no choices cannot be
/// drawn, a second row with one id is the agent contradicting itself, and a
/// non-select type is not a feature at all.
#[test]
fn an_unusable_option_contributes_no_row_and_fails_nothing() {
    let result = options(json!([
        {"type": "select", "options": [{"value": "a", "name": "A"}]},
        {"id": "empty", "type": "select", "options": []},
        {"id": "", "type": "select", "options": [{"value": "a", "name": "A"}]},
        {"id": "dupe", "type": "select", "options": [
            {"value": "a", "name": "A"}, {"value": "a", "name": "Again"}
        ]},
        {"id": "dupe", "type": "select", "options": [{"value": "z", "name": "Z"}]},
        {"id": "bool", "type": "boolean"},
        {"id": "choices", "type": "select", "options": [
            {"value": "a", "name": "A"}, {"value": "", "name": "No value"}
        ]}
    ]));
    let declared = declared_features_from_options(&result, &[None, None], None);
    let ids = declared
        .iter()
        .map(|feature| feature.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["dupe", "choices"], "the first of each: {declared:?}");
    assert_eq!(
        declared[1].options.len(),
        1,
        "a choice with no value is dropped"
    );
}

/// A reply with no `configOptions` at all is not a provider that offers nothing:
/// the read answers with an empty list and the axis stays honest about which of
/// the two it was, because the state comes from the caller's own tick-plus-list
/// rule and not from this function.
#[test]
fn a_reply_without_config_options_declares_nothing() {
    let empty = serde_json::json!({"sessionId": "s"});
    assert!(declared_features_from_options(&empty, &[None, None], None).is_empty());
    let not_an_array = serde_json::json!({"configOptions": "model"});
    assert!(declared_features_from_options(&not_an_array, &[None, None], None).is_empty());
}

/// The declarations are a by-product of the one handshake parse, on the same
/// footing as the switch shape and for the same reason: a second reader of the
/// raw array could disagree with the manifest about which option is which.
#[test]
fn the_handshake_parse_carries_the_declaration_alongside_the_manifest() {
    // Three options, not two: with a model selector and one other select, the
    // effort reader's own single-leftover fallback claims the second one (the
    // same conservative hint `model_and_effort` relies on), so a fixture that
    // wants a third surface to stay a feature has to give the effort dial an id
    // of its own to find.
    let session = options(json!([
        {"id": "model", "type": "select", "category": "model", "currentValue": "m1",
         "options": [{"value": "m1", "name": "One"}]},
        {"id": "effort", "type": "select", "category": "thought_level", "currentValue": "high",
         "options": [{"value": "high", "name": "High"}]},
        {"id": "web_search", "type": "select", "name": "Web search", "currentValue": "off",
         "options": [{"value": "on", "name": "On"}, {"value": "off", "name": "Off"}]}
    ]));
    let handshake = merge_handshake_manifest(
        &serde_json::json!({"protocolVersion": 1}),
        &session,
        Some("trae".to_string()),
    );
    let ids = handshake
        .declared_features
        .iter()
        .map(|feature| feature.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        ["web_search"],
        "the model option is a switch, not a feature"
    );
    assert_eq!(
        handshake
            .declared_surfaces
            .iter()
            .map(|surface| surface.id.as_str())
            .collect::<Vec<_>>(),
        ids,
        "and the delivery reads the same list the form does"
    );
    // The manifest still describes the model: reading a third list out of the
    // array must not cost the session the two it already had.
    assert!(handshake.shape.is_some(), "the switch shape survives");
}

/// The session's **mode selector** is not a feature either, and the fixture is
/// the shape a real agent ships: the captured `claude-agent-acp` frame carries
/// `modes` as a standard block *and* a `select` config option with the same
/// values. A profile stores its mode in `modeId` and the daemon switches it with
/// `session/set_mode`, so a second control over those values would be two sources
/// for one setting — reached by two verbs, one of them silently last.
///
/// The exclusion is by value-set and not by the id `"mode"`, because `category`
/// is advisory: an agent that names the option `behaviour` and lists the same
/// modes is offering the same selector, and it must be excluded the same way.
#[test]
fn the_mode_selector_is_excluded_by_its_values_and_not_only_its_name() {
    let modes = json!({
        "currentModeId": "default",
        "availableModes": [
            {"id": "default", "name": "Manual"},
            {"id": "acceptEdits", "name": "Accept edits"}
        ]
    });
    let named = with_mode_option(
        &modes,
        json!({"id": "mode", "type": "select", "category": "mode", "currentValue": "default",
               "options": [{"value": "default", "name": "Manual"},
                           {"value": "acceptEdits", "name": "Accept edits"}]}),
    );
    let redressed = with_mode_option(
        &modes,
        json!({"id": "behaviour", "type": "select", "currentValue": "default",
               "options": [{"value": "default", "name": "Manual"},
                           {"value": "acceptEdits", "name": "Accept edits"}]}),
    );
    let a_real_feature = with_mode_option(
        &modes,
        json!({"id": "fast", "type": "select", "currentValue": "off",
               "options": [{"value": "on", "name": "On"}, {"value": "off", "name": "Off"}]}),
    );
    // The modes view the same handshake would hand the read: parsing the block
    // here rather than passing `None` is the point of the assertion.
    let modes = modes_from_standard(&serde_json::json!({ "modes": modes }))
        .expect("the block above is a standard modes view");
    assert!(
        declared_features_from_options(&named, &[], Some(&modes)).is_empty(),
        "a category-tagged mode option is the mode selector"
    );
    assert!(
        declared_features_from_options(&redressed, &[], Some(&modes)).is_empty(),
        "so is the same values under another name, once the modes block is known"
    );
    let kept = declared_features_from_options(&a_real_feature, &[], Some(&modes));
    assert_eq!(
        kept.iter()
            .map(|feature| feature.id.as_str())
            .collect::<Vec<_>>(),
        ["fast"],
        "a dial that is not the mode list survives"
    );
    // Without the standard block there is no value-set to compare against, so
    // only the advisory category excludes. That is the honest limit of a
    // by-values rule, and the reason the rule is a backstop and not a rewrite:
    // an option the daemon cannot prove is the mode selector stays a feature it
    // can set, and `session/set_config_option` is a real verb for it.
    let blind = json!({"sessionId": "s", "configOptions": [
        {"id": "behaviour", "type": "select", "currentValue": "default",
         "options": [{"value": "default", "name": "Manual"}]}]});
    assert_eq!(
        declared_features_from_options(&blind, &[], Some(&modes)).len(),
        1,
        "no modes block, no value-set to match"
    );
}

fn with_mode_option(modes: &serde_json::Value, option: serde_json::Value) -> serde_json::Value {
    json!({
        "sessionId": "s",
        "modes": modes,
        "configOptions": [option]
    })
}
