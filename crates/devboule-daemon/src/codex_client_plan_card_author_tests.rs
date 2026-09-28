//! The implementation prompt's journal author: the daemon composes the line,
//! so it is the agent's own, never the person's words.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use devboule_protocol::SessionEvent;

use super::plan_card_test_support::{capturing_runtime, published, working_stdin};

#[test]
fn the_implementation_prompt_is_journalled_as_the_agents_own() {
    let state = Arc::new(crate::codex_view::CodexState::new(
        "thread".to_string(),
        crate::codex_view::catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog"),
        "auto",
    ));
    state
        .set_collaboration_modes(&serde_json::json!({
            "data": [
                { "name": "Plan", "mode": "plan" },
                { "name": "Auto", "mode": "auto" }
            ]
        }))
        .expect("modes");
    state.set_plan_mode(true).expect("plan enabled");
    let (runtime, conn) = capturing_runtime();
    let stdin = working_stdin();
    let prompt = super::CodexStaticPrompt::new(
        Arc::clone(&stdin),
        Arc::new(AtomicU64::new(1)),
        Arc::clone(&state),
        super::empty_commands(),
    );
    prompt.send_approved_plan("## Steps\n\n- Build", &runtime);

    let events = published(&conn);
    let author = events.iter().find_map(|event| match event {
        SessionEvent::AgentUserMessage { author, .. } => Some(author),
        _ => None,
    });
    assert_eq!(
        author,
        Some(&devboule_protocol::UserMessageAuthor::Agent),
        "the daemon-composed implementation prompt is the agent's own, never the person's words"
    );
}
