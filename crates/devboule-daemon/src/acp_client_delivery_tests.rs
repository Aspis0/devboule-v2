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
