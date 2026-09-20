//! Refused-spawn coverage, moved whole out of `session_tests.rs` lines
//! 1510-1862: the journal row an ordinary spawn failure must end before the
//! refusal returns and its non-blocking variant, the two creation-time
//! profile refusals, and the pre-card tick refusal the clients' own tick
//! rules bound. Every line below is byte-identical to its text there, apart
//! from this header; `ticked_features` travels with the tests that use it.

use super::*;

/// The refused spawn's journal row is ended **by the time the refusal
/// returns** (the R2a audit's F8): the row was written Live before the
/// spawn, and an end left to a fire-and-forget thread is an end a daemon
/// death in that window undoes — the row would come back `status=live`
/// and resurrect a phantom recovered session. The spawn here fails on a
/// program that does not exist, the most ordinary spawn failure there
/// is.
#[test]
fn a_refused_spawn_ends_its_journal_row_before_the_refusal_is_returned() {
    let state = ServerState::new("refused-row-ends".to_string());
    let owner = OwnerId::new("local", "test").expect("owner");
    let command = PtyCommand::new(
        "definitely-not-a-real-program-xyz",
        Vec::new(),
        std::env::temp_dir(),
        Vec::new(),
    );
    let meta = SessionCreateMeta::default();
    state
        .sessions
        .create_with_provider_env(
            &state,
            &owner,
            None,
            SessionKind::Terminal,
            None,
            crate::profile_delivery::ProfileDelivery::for_request(None),
            Some(command),
            &None,
            None,
            &meta,
        )
        .expect_err("a nonexistent program refuses the spawn");

    // The end is async (throwaway thread, like the resume path), so poll
    // until it lands: the refusal must not leave a Live row behind.
    let journal = state
        .sessions
        .journal
        .as_ref()
        .expect("the test state has a journal");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let rows = journal.list().expect("journal rows");
        let row = rows
            .iter()
            .find(|row| row.title == "Terminal")
            .expect("the refused spawn's row");
        if matches!(row.status, crate::journal::PersistStatus::Ended) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the refused spawn's row never ended: {:?}",
            row.status
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// J2: a failed create must not stall the dispatch thread on the journal.
/// The resume path already states the rule (unbounded 5 ms busy-loop, no
/// timeout) and uses a throwaway thread; the create failure paths must do
/// the same. This pins the async shape via the spawn road (the MCP road
/// shares the same blocking call and gets the same fix): Live immediately
/// after the refusal, Ended once the queue drains.
#[test]
fn a_refused_spawn_ends_its_row_async_without_blocking_the_caller() {
    let state = ServerState::new("refused-row-async".to_string());
    let owner = OwnerId::new("local", "test").expect("owner");
    let command = PtyCommand::new(
        "definitely-not-a-real-program-xyz",
        Vec::new(),
        std::env::temp_dir(),
        Vec::new(),
    );
    let meta = SessionCreateMeta::default();
    state
        .sessions
        .create_with_provider_env(
            &state,
            &owner,
            None,
            SessionKind::Terminal,
            None,
            crate::profile_delivery::ProfileDelivery::for_request(None),
            Some(command),
            &None,
            None,
            &meta,
        )
        .expect_err("a nonexistent program refuses the spawn");
    let journal = state.sessions.journal.as_ref().expect("journal");
    let rows = journal.list().expect("journal rows");
    let row = rows
        .iter()
        .find(|row| row.title == "Terminal")
        .expect("the refused spawn's row");
    assert!(
        matches!(row.status, crate::journal::PersistStatus::Live),
        "the end is async, so the row is still Live when the refusal returns: {:?}",
        row.status
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let rows = journal.list().expect("journal rows");
        let row = rows
            .iter()
            .find(|row| row.title == "Terminal")
            .expect("the refused spawn's row");
        if matches!(row.status, crate::journal::PersistStatus::Ended) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the async end never landed"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// The health recorder's class line (the R2a audit's F6): a refusal the
/// profile alone decides — an unknown model or mode, an `autoAccept`
/// contradiction, an agent refusing the delivered switch — is
/// `InvalidRequest` and says nothing about the provider; a provider or
/// pipe failure is any other code and does. Three saved profiles with a
/// tick over an asking mode must not read as three unhealthy providers.
#[test]
fn a_profile_refusal_does_not_read_as_provider_health() {
    assert!(!spawn_failure_is_provider_health(&WireError::new(
        ErrorCode::InvalidRequest,
        "Claude model 'x' is not among the models this Claude publishes; the creation is refused rather than started on a different model",
    )));
    assert!(!spawn_failure_is_provider_health(&WireError::new(
        ErrorCode::InvalidRequest,
        "the profile asks Claude to approve its own permission prompts and also to start in mode 'default', which asks the human; the two contradict, so the creation is refused",
    )));
    assert!(!spawn_failure_is_provider_health(&WireError::new(
        ErrorCode::InvalidRequest,
        "the agent refused the delivered model 'stub-model-new' the card promised, so the creation is refused rather than started on a different model: ACP request failed (-32602): unknown model",
    )));
    assert!(spawn_failure_is_provider_health(&WireError::new(
        ErrorCode::Io,
        "ACP stdio failed: broken pipe",
    )));
    assert!(spawn_failure_is_provider_health(&WireError::new(
        ErrorCode::Io,
        "Pi permission extension not active.",
    )));
}

fn ticked_features() -> serde_json::Map<String, serde_json::Value> {
    serde_json::json!({ "autoAccept": true })
        .as_object()
        .expect("object")
        .to_owned()
}

/// The crossing the re-audit's P1 found missing: the pre-card gate and
/// the clients' own spawn-time tick rules, asserted against each other
/// over the daemon's mode vocabulary plus the provider-authored ids the
/// audit named. The invariant that convicts the old gate is exact — a
/// pair the pre-card gate refuses must be a pair the client that will
/// speak for the child also refuses — and for Claude and pi, whose tick
/// rule is the daemon's own, the two verdicts must agree outright.
#[test]
fn the_pre_card_tick_refusal_never_exceeds_what_the_clients_refuse_at_spawn() {
    use crate::provider_catalog::{judge_auto_accept_tick, AutoAcceptTick};
    let features = ticked_features();
    let modes = [
        "bypass",
        "auto_accept",
        "bypassPermissions",
        "default",
        "ask",
        "plan",
        "acceptEdits",
        "auto",
        "full-access",
        "auto-review",
    ];
    for (provider, spawn_refuses) in [
        (
            "claude",
            super::claude_client::tick_contradicts
                as fn(&crate::profile_delivery::ProfileDelivery) -> bool,
        ),
        ("pi", super::pi_client::tick_contradicts),
        ("codex", super::codex_client::tick_contradicts),
    ] {
        for mode in modes {
            let delivery = crate::profile_delivery::ProfileDelivery::for_child(
                mode,
                "some-model",
                None,
                &features,
            );
            let pre_card_refuses =
                judge_auto_accept_tick(provider, mode, &features) == AutoAcceptTick::Contradicts;
            let spawn_refuses = spawn_refuses(&delivery);
            assert!(
                !pre_card_refuses || spawn_refuses,
                "{provider} {mode}: the pre-card gate refuses a pair the client accepts at spawn"
            );
            // The gate is exact where the rule is the daemon's own: a
            // silent gate over a refused pair would move the
            // contradiction behind the consent card (the R2a audit's F7).
            if provider != "codex" {
                assert_eq!(
                    pre_card_refuses, spawn_refuses,
                    "{provider} {mode}: the gate and the client disagree"
                );
            }
        }
    }
    // The conviction itself, spelled: `full-access` + tick is accepted
    // by Codex's own rule and is `NotOursToJudge` pre-card — never
    // refused by a table that did not author it.
    let delivery = crate::profile_delivery::ProfileDelivery::for_child(
        "full-access",
        "some-model",
        None,
        &features,
    );
    assert!(!super::codex_client::tick_contradicts(&delivery));
    assert_eq!(
        judge_auto_accept_tick("codex", "full-access", &features),
        AutoAcceptTick::NotOursToJudge
    );
}

/// The convention the F6 classifier rests on, asserted against the
/// **producers** and not hand-built errors (the re-audit's P3-3): every
/// creation-time refusal a client can make from the profile alone is
/// `InvalidRequest`, so `spawn_failure_is_provider_health` reads false
/// for it. A client that reclassified one of these as `Io` would flip
/// the health recording for every profile mistake, and this is the test
/// that goes red.
#[test]
fn every_clients_creation_time_profile_refusal_is_invalid_request() {
    let refusal_is_not_provider_health = |error: WireError, what: &str| {
        assert_eq!(
            error.code,
            ErrorCode::InvalidRequest,
            "{what} must be a profile refusal, not provider health: {error:?}"
        );
        assert!(
            !spawn_failure_is_provider_health(&error),
            "{what} must not read as provider health: {error:?}"
        );
    };
    // pi: an unknown mode, and the tick over an asking mode.
    refusal_is_not_provider_health(
        super::pi_client::validate_delivery(&crate::profile_delivery::ProfileDelivery::for_child(
            "no-such-mode",
            "m",
            None,
            &serde_json::Map::new(),
        ))
        .expect_err("unknown pi mode"),
        "pi unknown mode",
    );
    refusal_is_not_provider_health(
        super::pi_client::validate_delivery(&crate::profile_delivery::ProfileDelivery::for_child(
            "ask",
            "m",
            None,
            &ticked_features(),
        ))
        .expect_err("pi tick over ask"),
        "pi tick over an asking mode",
    );
    // Codex: an unknown mode, and the tick over an on-request mode.
    refusal_is_not_provider_health(
        super::codex_client::validate_delivery(
            &crate::profile_delivery::ProfileDelivery::for_child(
                "no-such-mode",
                "m",
                None,
                &serde_json::Map::new(),
            ),
        )
        .expect_err("unknown codex mode"),
        "codex unknown mode",
    );
    refusal_is_not_provider_health(
        super::codex_client::validate_delivery(
            &crate::profile_delivery::ProfileDelivery::for_child(
                "auto",
                "m",
                None,
                &ticked_features(),
            ),
        )
        .expect_err("codex tick over auto"),
        "codex tick over an on-request mode",
    );
    // Claude: the tick over the default mode, and — on a derived but
    // empty catalog — a model with no vocabulary to be judged against.
    refusal_is_not_provider_health(
        super::claude_client::validate_delivery(
            &crate::claude_catalog::ClaudeCatalogSnapshot::derived(Vec::new()),
            &crate::profile_delivery::ProfileDelivery::for_child(
                "default",
                "m",
                None,
                &ticked_features(),
            ),
        )
        .expect_err("claude tick over default"),
        "claude tick over an asking mode",
    );
    refusal_is_not_provider_health(
        super::claude_client::validate_delivery(
            &crate::claude_catalog::ClaudeCatalogSnapshot::derived(Vec::new()),
            &crate::profile_delivery::ProfileDelivery::for_child(
                "default",
                "some-model",
                None,
                &serde_json::Map::new(),
            ),
        )
        .expect_err("claude model over an empty catalog"),
        "claude model absence",
    );
    // ACP: the model axis's absence sentence — an agent that declares no
    // surface at all — and its mismatch sentence against a declared one.
    refusal_is_not_provider_health(
        super::acp_client::validate_acp_model_choice(
            &crate::acp_view::SwitchControlShape {
                vendor: None,
                config: None,
            },
            "some-model",
        )
        .expect_err("acp model with no declared surface"),
        "acp model axis absence",
    );
    refusal_is_not_provider_health(
        super::acp_client::validate_acp_model_choice(
            &crate::acp_view::SwitchControlShape {
                vendor: Some(crate::acp_view::VendorSwitchSurface {
                    values: vec!["other-model".to_string()],
                    values_by_model: Vec::new(),
                }),
                config: None,
            },
            "some-model",
        )
        .expect_err("acp model outside the declared values"),
        "acp model axis mismatch",
    );
}
