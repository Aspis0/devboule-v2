//! Tests for the ACP client's model and effort choices.

use super::{validate_acp_effort_choice, validate_acp_model_choice};
use crate::acp_view::{ConfigOptionSurface, SwitchControlShape, VendorSwitchSurface};

fn vendor_surface(values: &[&str]) -> SwitchControlShape {
    SwitchControlShape {
        vendor: Some(VendorSwitchSurface {
            values: values.iter().map(|value| value.to_string()).collect(),
            values_by_model: Vec::new(),
        }),
        config: None,
    }
}

fn config_surface(id: &str, values: &[&str]) -> SwitchControlShape {
    SwitchControlShape {
        vendor: None,
        config: Some(ConfigOptionSurface {
            id: id.to_string(),
            values: values.iter().map(|value| value.to_string()).collect(),
        }),
    }
}

fn no_surface() -> SwitchControlShape {
    SwitchControlShape {
        vendor: None,
        config: None,
    }
}

/// Absence of any declared surface is one sentence; an id outside the
/// declared values is the other. An agent that declares no values at all
/// is not judgeable, and the check refuses nothing.
#[test]
fn acp_model_absence_and_mismatch_are_two_distinct_refusals() {
    let error = validate_acp_model_choice(&no_surface(), "stub-model")
        .expect_err("no surface cannot deliver a model");
    assert!(
        error.message.contains("declares no model switch surface"),
        "the absence sentence: {}",
        error.message
    );

    let shape = vendor_surface(&["stub-model", "stub-model-new"]);
    let error = validate_acp_model_choice(&shape, "stub-bogus")
        .expect_err("an undeclared model id must be refused");
    assert!(
        error.message.contains("is not among the model values"),
        "the mismatch sentence: {}",
        error.message
    );
    assert!(
        !error.message.contains("declares no model switch surface"),
        "the two sentences must stay distinct: {}",
        error.message
    );

    // A surface with no declared values: the agent's own answer is the
    // confirmation, so nothing is refused here.
    validate_acp_model_choice(&vendor_surface(&[]), "stub-model")
        .expect("unjudgeable, not refused");
    validate_acp_model_choice(&config_surface("model", &["m1"]), "m1").expect("declared");
}

#[test]
fn acp_thinking_absence_and_mismatch_are_two_distinct_refusals() {
    let error = validate_acp_effort_choice(&no_surface(), Some("stub-model"), "high")
        .expect_err("no surface cannot deliver a thinking option");
    assert!(
        error
            .message
            .contains("declares no thinking-option surface"),
        "the absence sentence: {}",
        error.message
    );

    // Vendor effort values are per model: only the delivered model's own
    // declared values judge the choice.
    let mut shape = vendor_surface(&["stub-model"]);
    shape.vendor.as_mut().expect("vendor").values_by_model =
        vec![("stub-model".to_string(), vec!["high".to_string()])];
    let error = validate_acp_effort_choice(&shape, Some("stub-model"), "bogus")
        .expect_err("an undeclared effort must be refused");
    assert!(
        error.message.contains("is not among the thinking options"),
        "the mismatch sentence: {}",
        error.message
    );
    // A different model declared nothing, so the choice is unjudgeable.
    validate_acp_effort_choice(&shape, Some("stub-model-new"), "high")
        .expect("the delivered model declared no values");
    // No model delivered: the per-model judgment cannot run.
    validate_acp_effort_choice(&shape, None, "high").expect("unjudgeable, not refused");

    // The config-option surface judges from the option's own values.
    let config = config_surface("thought-level", &["low", "high"]);
    validate_acp_effort_choice(&config, Some("stub-model"), "low").expect("declared");
    let error = validate_acp_effort_choice(&config, Some("stub-model"), "bogus")
        .expect_err("an undeclared config value must be refused");
    assert!(
        error.message.contains("is not among the thinking options"),
        "{}",
        error.message
    );
}

/// The ACP arm of the delivery rule, judged against what this agent declared —
/// the pair the create road puts on `session/set_config_option`, and each of the
/// three ways it can be undeliverable.
///
/// Nothing here is skipped quietly. A stored key the agent does not declare may
/// well have been declared by the version of the agent that dressed the form, and
/// a child started without it is a card that promised a configuration nobody
/// chose; the refusal is the honest answer, and a re-save drops the key.
#[test]
fn a_declared_feature_becomes_one_config_option_frame_per_value() {
    use super::declared_feature_frames;
    use crate::acp_view::ConfigOptionSurface;
    use crate::profile_delivery::delivered_features;
    use serde_json::json;

    let values = |ids: &[&str]| ids.iter().map(|id| (*id).to_string()).collect::<Vec<_>>();
    let declared = vec![
        ConfigOptionSurface {
            id: "engine".to_string(),
            values: values(&["m1", "m2"]),
        },
        ConfigOptionSurface {
            id: "fast".to_string(),
            values: values(&["on", "off"]),
        },
    ];
    let stored = |pairs: &[(&str, serde_json::Value)]| {
        let mut map = serde_json::Map::new();
        for (key, value) in pairs {
            map.insert((*key).to_string(), value.clone());
        }
        delivered_features(&map)
    };

    // The ordinary case: two declared dials, two frames, in the profile's order.
    let frames = declared_feature_frames(
        &declared,
        &stored(&[("engine", json!("m2")), ("fast", json!("on"))]),
    )
    .expect("both are declared");
    assert_eq!(
        frames,
        vec![
            ("engine".to_string(), "m2".to_string()),
            ("fast".to_string(), "on".to_string())
        ],
        "the declared option id is the configId and the stored value is the wire value"
    );

    // A key the agent never declared: refused, and the key is named.
    let error = declared_feature_frames(&declared, &stored(&[("gone", json!("x"))]))
        .expect_err("an undeclared option is refused");
    assert!(
        error.message.contains("gone") && error.message.contains("no config option"),
        "the refusal names the key: {}",
        error.message
    );

    // A value outside the declared choices: refused, with both named.
    let error = declared_feature_frames(&declared, &stored(&[("engine", json!("m3"))]))
        .expect_err("an unoffered choice is refused");
    assert!(
        error.message.contains("engine") && error.message.contains("m3"),
        "the refusal names the option and the choice it does not offer: {}",
        error.message
    );

    // A type the control could not produce. `delivered_features` is the reader
    // that keeps this reachable at all: a boolean on a select still has a
    // variant, so the frame judgement is where it is refused, not the read.
    let error = declared_feature_frames(&declared, &stored(&[("engine", json!(true))]))
        .expect_err("a toggle value on a select is refused");
    assert!(
        error.message.contains("engine"),
        "the refusal names the key: {}",
        error.message
    );

    // The daemon's own tick is never a config option and never reaches here: it
    // is the mode constraint, carried by the field beside the list.
    assert!(
        stored(&[("autoAccept", json!(true))]).is_empty(),
        "the tick is not a delivered feature"
    );
}

/// The stored map as the child receives it: the tick read as a constraint and
/// left out, every other key as a value, and a value no control produces dropped
/// at the read — so the card cannot print one the wire cannot carry.
#[test]
fn the_delivery_reads_each_stored_value_by_its_control() {
    use crate::profile_delivery::{delivered_features, DeliveredFeature};
    use serde_json::json;

    let map = json!({
        "autoAccept": true,
        "engine": "m1",
        "fast": false,
        "count": 3,
        "nested": {"a": 1},
    })
    .as_object()
    .expect("map")
    .clone();
    let delivered = delivered_features(&map);
    assert_eq!(
        delivered
            .iter()
            .map(|feature| feature.id())
            .collect::<Vec<_>>(),
        ["engine", "fast"],
        "the two typable keys, and never the tick: {delivered:?}"
    );
    assert_eq!(
        delivered[0],
        DeliveredFeature::Choice {
            id: "engine".to_string(),
            value: "m1".to_string()
        }
    );
    assert_eq!(
        delivered[0].printed(),
        "m1",
        "a select is printed with the choice it stores"
    );
    assert_eq!(
        delivered[1].printed(),
        "false",
        "an off toggle is printed as off, not hidden"
    );
}
