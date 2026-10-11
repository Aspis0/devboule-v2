//! A declined plan: the refusal goes to the agent as its own turn, plan mode
//! stays on, and the agent asks what to change. Only a refusal starts that
//! turn; a steer or a cancel leaves the session where the person put it.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use devboule_protocol::{SessionEvent, UserMessageAuthor};

use super::super::permission_broker::HostDecision;
use super::plan_card_test_support::{capturing_runtime, published, working_stdin};

fn plan_mode_state() -> Arc<crate::codex_view::CodexState> {
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
    state
}

fn prompt_for(state: &Arc<crate::codex_view::CodexState>) -> super::CodexStaticPrompt {
    super::CodexStaticPrompt::new(
        working_stdin(),
        Arc::new(AtomicU64::new(1)),
        Arc::clone(state),
        super::empty_commands(),
    )
}

/// The agent-authored turn texts the decision published, in order.
fn agent_turns(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentUserMessage {
                text,
                author: UserMessageAuthor::Agent,
                ..
            } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_declined_plan_sends_the_refusal_and_the_agent_asks() {
    let state = plan_mode_state();
    let (runtime, conn) = capturing_runtime();
    prompt_for(&state).answer_plan(
        HostDecision::Deny,
        "## Steps

- Build",
        &runtime,
    );

    // The person sees the outcome, not the instruction the agent receives.
    let turns = agent_turns(&published(&conn));
    assert_eq!(
        turns,
        vec!["Plan rejected".to_string()],
        "the transcript shows one muted outcome line, agent-authored"
    );
    let instruction = super::rejected_plan_prompt();
    assert!(
        instruction.contains("did not approve the plan")
            && instruction.contains("Do not implement"),
        "the agent is told the plan was refused and not to implement it: {instruction}"
    );
    assert!(
        instruction.contains("what they want changed"),
        "the agent is asked to ask what to change: {instruction}"
    );
    assert!(
        state.plan_mode_enabled(),
        "a declined plan keeps the session in plan mode"
    );
}

#[test]
fn a_steer_or_a_cancel_starts_no_turn() {
    for decision in [HostDecision::Cancelled, HostDecision::Timeout] {
        let state = plan_mode_state();
        let (runtime, conn) = capturing_runtime();
        prompt_for(&state).answer_plan(decision, "## Steps\n\n- Build", &runtime);
        assert_eq!(
            agent_turns(&published(&conn)),
            Vec::<String>::new(),
            "no turn is started for a card that was not refused"
        );
        assert!(state.plan_mode_enabled(), "plan mode is untouched");
    }
}
