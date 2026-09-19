//! Tests for the Codex client's delivery gate: which profile a prompt may reach.

use super::{
    mode_answers_own_prompts, seed_model_and_effort, turn_start_params_for_prompt,
    validate_delivery, CodexState, ProfileDelivery,
};
use crate::codex_view::catalog_from_response;
use devboule_protocol::SessionEvent;
use std::sync::Arc;

fn catalog() -> crate::codex_view::CodexCatalog {
    let frame = serde_json::json!({
        "data": [{
            "id": "gpt-5.1",
            "displayName": "GPT 5.1",
            "isDefault": true,
            "supportedReasoningEfforts": [
                {"reasoningEffort": "high"},
                {"reasoningEffort": "low"}
            ],
            "defaultReasoningEffort": "high",
        }]
    });
    catalog_from_response(&frame).expect("catalog")
}

/// The seed is the delivery: the first `turn/start` reads its model and
/// effort from the state the profile seeded, so a seed that silently
/// no-ops would put a child on the app-server's default model while the
/// card named another.
#[test]
fn a_seeded_codex_delivery_reaches_the_first_turn_params() {
    let state = Arc::new(CodexState::new("thread".to_string(), catalog(), "auto"));
    let mut delivery = ProfileDelivery::none();
    delivery.model_id = Some("gpt-5.1".to_string());
    delivery.thinking_option_id = Some("low".to_string());
    seed_model_and_effort(&state, &delivery).expect("seeded");

    let params = turn_start_params_for_prompt(&state, "report your result", &[]);
    assert_eq!(
        params["model"], "gpt-5.1",
        "the first turn runs the profile's model"
    );
    assert_eq!(
        params["effort"], "low",
        "the first turn runs the profile's effort"
    );
}

fn delivery(mode: &str, auto_accept: bool) -> ProfileDelivery {
    let mut delivery = ProfileDelivery::for_request(Some(mode.to_string()));
    delivery.auto_accept = auto_accept;
    delivery
}

/// The daemon's broker answers the provider-agnostic ids it owns;
/// `full-access` is this client's own knob, whose approval policy is
/// `never` — the provider never asks anybody. Both admit an
/// `autoAccept` tick; everything else asks the human.
///
/// The broker half is **walked, not hand-listed** (the R2a audit's F9):
/// the test iterates the table itself, so a fourth id added to
/// `auto_answered_modes` is asserted to answer here the moment it
/// exists, and a codex-side predicate change is caught against whatever
/// the table holds.
#[test]
fn codex_auto_answer_modes_are_the_broker_list_plus_full_access() {
    assert!(
        mode_answers_own_prompts("full-access"),
        "approvalPolicy never"
    );
    for mode_id in crate::provider_catalog::auto_answered_modes() {
        assert!(
            mode_answers_own_prompts(mode_id),
            "route A: {mode_id} answers its own prompts"
        );
    }
    assert!(!mode_answers_own_prompts("auto"), "on-request asks");
    assert!(!mode_answers_own_prompts("read-only"), "on-request asks");
    assert!(
        !mode_answers_own_prompts("auto-review"),
        "eligible is not all: on-request requests may still reach the human"
    );
}

/// The predicate reads the row, not a name it spells itself: walked over
/// every mode the manifest presents, it agrees with the table's own
/// `unattended` answer. A new `Yes` row added to `CODEX_MODES` is
/// answered here the moment it exists — the old `mode_id ==
/// "full-access"` shape would go red on exactly that row, refusing a
/// pair with a rationale false about it.
#[test]
fn auto_answer_agrees_with_the_table_on_every_presented_row() {
    let state = Arc::new(CodexState::new("thread".to_string(), catalog(), "auto"));
    let SessionEvent::SessionManifest {
        modes: Some(modes), ..
    } = state.manifest()
    else {
        panic!("the Codex manifest carries modes");
    };
    assert!(
        modes
            .available_modes
            .iter()
            .any(|mode| mode.id == "full-access"),
        "the walked table still carries the Yes row this pins"
    );
    for mode in &modes.available_modes {
        assert_eq!(
            mode_answers_own_prompts(&mode.id),
            crate::codex_view::unattended_answer(Some(mode.id.as_str()))
                == devboule_protocol::UnattendedState::Yes,
            "{}: the predicate answers what the row's marker says",
            mode.id
        );
    }
}

/// The contradiction is refused at creation: a tick over a mode that
/// asks the human names two ways to run and delivers neither.
#[test]
fn a_codex_auto_accept_tick_over_an_asking_mode_is_refused() {
    validate_delivery(&delivery("full-access", true)).expect("never asks");
    validate_delivery(&delivery("auto", false)).expect("asking mode, no tick");
    let error = validate_delivery(&delivery("auto", true))
        .expect_err("a tick over an asking mode is the contradiction");
    assert!(
        error.message.contains("contradict"),
        "the refusal names both halves: {}",
        error.message
    );
    assert!(
        error.message.contains("mode 'auto'"),
        "the refusal names the delivered mode: {}",
        error.message
    );
}
