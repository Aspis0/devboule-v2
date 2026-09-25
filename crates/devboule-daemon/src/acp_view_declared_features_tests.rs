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
        2,
        "both choices survive, the empty one included — see          a_declared_empty_choice_survives_the_parse"
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
    let modes_json = json!({
        "currentModeId": "default",
        "availableModes": [
            {"id": "default", "name": "Manual"},
            {"id": "acceptEdits", "name": "Accept edits"}
        ]
    });
    let named = with_mode_option(
        &modes_json,
        json!({"id": "mode", "type": "select", "category": "mode", "currentValue": "default",
               "options": [{"value": "default", "name": "Manual"},
                           {"value": "acceptEdits", "name": "Accept edits"}]}),
    );
    let redressed = with_mode_option(
        &modes_json,
        json!({"id": "behaviour", "type": "select", "currentValue": "default",
               "options": [{"value": "default", "name": "Manual"},
                           {"value": "acceptEdits", "name": "Accept edits"}]}),
    );
    let empty_position = with_mode_option(
        &modes_json,
        json!({"id": "mode-with-default", "type": "select", "currentValue": "default",
               "options": [{"value": "", "name": "Provider default"},
                           {"value": "default", "name": "Manual"},
                           {"value": "acceptEdits", "name": "Accept edits"}]}),
    );
    let a_real_feature = with_mode_option(
        &modes_json,
        json!({"id": "fast", "type": "select", "currentValue": "off",
               "options": [{"value": "on", "name": "On"}, {"value": "off", "name": "Off"}]}),
    );
    // The modes view the same handshake would hand the read: parsing the block
    // here rather than passing `None` is the point of the assertion.
    let modes = modes_from_standard(&serde_json::json!({ "modes": modes_json }))
        .expect("the block above is a standard modes view");
    assert!(
        declared_features_from_options(&named, &[], Some(&modes)).is_empty(),
        "the mode list under the `mode` category and matching values is the mode selector"
    );
    assert!(
        declared_features_from_options(&redressed, &[], Some(&modes)).is_empty(),
        "so is the same values under another name, once the modes block is known"
    );
    assert!(
        declared_features_from_options(&empty_position, &[], Some(&modes)).is_empty(),
        "an empty default choice does not make the mode selector a second feature control"
    );
    let kept = declared_features_from_options(&a_real_feature, &[], Some(&modes));
    assert_eq!(
        kept.iter()
            .map(|feature| feature.id.as_str())
            .collect::<Vec<_>>(),
        ["fast"],
        "a dial that is not the mode list survives"
    );
    // The category alone never excludes. An agent is free to label an
    // independent dial `mode` — the ACP spec says the category "MUST NOT be
    // required for correctness" — and treating the word as the test hid a real
    // control and let `prune` delete its stored value on save.
    let mislabelled = with_mode_option(
        &modes_json,
        json!({"id": "verbosity", "type": "select", "category": "mode", "currentValue": "short",
               "options": [{"value": "short", "name": "Short"},
                           {"value": "long", "name": "Long"}]}),
    );
    let kept_mislabelled = declared_features_from_options(&mislabelled, &[], Some(&modes));
    assert_eq!(
        kept_mislabelled
            .iter()
            .map(|feature| feature.id.as_str())
            .collect::<Vec<_>>(),
        ["verbosity"],
        "a `mode`-category option whose values are not the modes is a feature: {kept_mislabelled:?}"
    );
    // Without the standard block there is no value-set to compare against, so
    // nothing is excluded on the mode question at all: the option stays a
    // feature the daemon can set with a real verb.
    let blind = json!({"sessionId": "s", "configOptions": [
        {"id": "behaviour", "type": "select", "currentValue": "default",
         "options": [{"value": "default", "name": "Manual"}]}]});
    assert_eq!(
        declared_features_from_options(&blind, &[], None).len(),
        1,
        "no modes block, no value-set to match"
    );
}

/// An empty-string choice is a position the agent declared, not an absence.
/// Paseo relabels it (`emptyOptionLabel`) rather than deleting it, and this
/// parser must keep it: `value_fits` reads the declared list, so a dropped
/// choice would prune on save a value the provider genuinely accepts.
#[test]
fn a_declared_empty_choice_survives_the_parse() {
    let result = options(json!([
        {"id": "context", "type": "select", "name": "Context", "currentValue": "",
         "options": [{"value": "", "name": "None"}, {"value": "full", "name": "Full"}]}
    ]));
    let declared = declared_features_from_options(&result, &[], None);
    assert_eq!(
        declared.len(),
        1,
        "the empty value is a choice: {declared:?}"
    );
    assert_eq!(
        declared[0]
            .options
            .iter()
            .map(|option| option.id.as_str())
            .collect::<Vec<_>>(),
        ["", "full"],
        "{:?}",
        declared[0].options
    );
    assert_eq!(declared[0].options[0].label, "None", "its own label");
}

/// The daemon's mode-constraint key is reserved. A provider select that claims
/// it would put two controls on one stored value, and the child would receive
/// neither: the delivery reads `autoAccept` as the mode tick and skips it as a
/// feature. Renaming it would send the agent a `configId` it never declared, so
/// the row is dropped instead.
#[test]
fn a_provider_option_named_auto_accept_is_dropped_and_the_rest_survive() {
    let result = options(json!([
        {"id": "autoAccept", "type": "select", "name": "Auto accept",
         "currentValue": "on",
         "options": [{"value": "on", "name": "On"}, {"value": "off", "name": "Off"}]},
        {"id": "fast", "type": "select", "name": "Fast", "currentValue": "off",
         "options": [{"value": "on", "name": "On"}, {"value": "off", "name": "Off"}]}
    ]));
    let declared = declared_features_from_options(&result, &[], None);
    assert_eq!(
        declared
            .iter()
            .map(|feature| feature.id.as_str())
            .collect::<Vec<_>>(),
        ["fast"],
        "the reserved key goes, the agent's own dial stays: {declared:?}"
    );
}

/// A repeated choice gives one select two options with the same value: React
/// keys collide and the stored value names neither. The base parser promised
/// order-preserving de-duplication and used `Vec::dedup_by`, which compares
/// neighbours only — so `A, B, A` survived it with both `A` rows.
#[test]
fn a_repeated_choice_is_dropped_whole_list_and_the_agent_order_survives() {
    let result = options(json!([
        {"id": "engine", "type": "select", "name": "Engine", "currentValue": "a",
         "options": [
            {"value": "a", "name": "First A"},
            {"value": "b", "name": "B"},
            {"value": "a", "name": "Second A"},
            {"value": "c", "name": "C"},
            {"value": "b", "name": "Another B"}
         ]}
    ]));
    let declared = declared_features_from_options(&result, &[], None);
    let options_of_engine = &declared[0].options;
    assert_eq!(
        options_of_engine
            .iter()
            .map(|option| option.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"],
        "each value once, in the agent's order: {options_of_engine:?}"
    );
    assert_eq!(
        options_of_engine[1].label, "B",
        "the first label for a repeated value wins"
    );
}

fn with_mode_option(modes: &serde_json::Value, option: serde_json::Value) -> serde_json::Value {
    json!({
        "sessionId": "s",
        "modes": modes,
        "configOptions": [option]
    })
}

/// The probe's cleanup question, read from the agent's own answer:
/// `session/close` is sent only when the agent advertises it. Advertising is
/// tri-state in this file's own style for prompt capabilities, but here both
/// `false` and an absent field mean the same operational thing — do not send a
/// request the agent has not said it answers — so the read collapses them and
/// the doc says why.
#[test]
fn the_close_capability_is_read_from_the_initialize_result() {
    use super::close_session_advertised;
    let parse = |raw: &str| serde_json::from_str::<serde_json::Value>(raw).expect("json");
    assert!(close_session_advertised(&parse(
        r#"{"protocolVersion":1,"agentCapabilities":{"sessionCapabilities":{"close":true}}}"#
    )));
    assert!(
        !close_session_advertised(&parse(
            r#"{"protocolVersion":1,"agentCapabilities":{"sessionCapabilities":{"close":false}}}"#
        )),
        "an explicit refusal is not an advertisement"
    );
    assert!(
        !close_session_advertised(&parse(r#"{"protocolVersion":1}"#)),
        "no capabilities at all is no close"
    );
    assert!(
        !close_session_advertised(&parse(
            r#"{"protocolVersion":1,"agentCapabilities":{"sessionCapabilities":{"list":true}}}"#
        )),
        "a neighbouring capability is not this one"
    );
    // The handshake carries the answer beside the declarations, so the probe
    // never reads `initialize` a second time with its own rule.
    let handshake = merge_handshake_manifest(
        &parse(
            r#"{"protocolVersion":1,"agentCapabilities":{"sessionCapabilities":{"close":true}}}"#,
        ),
        &options(json!([])),
        Some("trae".to_string()),
    );
    assert!(handshake.close_session_advertised);
}
