//! Tests for the MCP broker: provider launchability, session wiring and the tool surface.

use super::*;
use crate::provider_catalog::{MCP_CREATE_AGENT_TOOL, MCP_ROSTER_TOOL, MCP_SEND_MESSAGE_TOOL};
use std::net::Shutdown;
use std::sync::mpsc;

/// A user-declared row carries its own argv, so it is launchable without
/// being on PATH — `resolve_named` reads the live registry before the
/// PATH/CDN walk. Asking PATH alone refused every user provider on the
/// MCP create-from-profile road while the wire create road spawned it:
/// one provider, two answers, depending on which door the caller came in.
#[test]
fn a_user_row_is_launchable_even_though_it_is_not_on_path() {
    let rows = crate::user_providers::parse_providers_document(
        br#"{"launchable-agent": {"extends": "acp", "command": ["/bin/launchable"]}}"#,
        &crate::session::native_family_ids(),
    )
    .expect("a valid row");
    let gate = crate::user_providers::lock_rows_state();
    crate::session::apply_user_rows(rows);

    assert!(
        crate::provider_catalog::find_available("launchable-agent").is_none(),
        "it is not on PATH: that is what makes this test mean something"
    );
    assert!(
        provider_is_launchable("launchable-agent"),
        "a live user row is launchable through its own argv"
    );

    crate::session::apply_user_rows(std::collections::BTreeMap::new());
    drop(gate);
    assert!(
        !provider_is_launchable("launchable-agent"),
        "and once the row is retired it is not launchable again"
    );
}

#[test]
fn tools_state_tri_state_and_single_computation_point() {
    // Closed-table walk: three variants, three distinct wire strings,
    // each string parsing back to exactly one variant.
    let states = [
        ToolsState::Hosted,
        ToolsState::Unavailable,
        ToolsState::Unverified,
    ];
    let words: Vec<&str> = states.iter().map(|state| state.as_str()).collect();
    assert_eq!(words.len(), 3);
    assert!(words.iter().all(|word| !word.is_empty()));
    let mut sorted = words.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), 3, "each ToolsState must serialise distinctly");
    for state in states {
        assert_eq!(tools_state_from_str(state.as_str()), Some(state));
    }
    assert_eq!(tools_state_from_str(""), None);
    assert_eq!(tools_state_from_str("HOSTED"), None);
    // The forbidden combination the type exists to name: a pi/Codex
    // session WITH a bearer but WITHOUT verification reads Unverified —
    // never Hosted, never Unavailable. Built through the single
    // computation point directly (registration state as booleans).
    for kind in [
        SessionKind::Acp,
        SessionKind::Claude,
        SessionKind::Pi,
        SessionKind::Codex,
    ] {
        assert_eq!(
            compute_tools_state(&kind, true, false),
            ToolsState::Unverified,
            "registered-but-unverified must be Unverified for {kind:?}"
        );
        assert_eq!(
            compute_tools_state(&kind, true, true),
            ToolsState::Hosted,
            "registered-and-verified must be Hosted for {kind:?}"
        );
        // The inverse forbidden state: no registration and no bearer
        // reads Unavailable, never Unverified.
        assert_eq!(
            compute_tools_state(&kind, false, false),
            ToolsState::Unavailable,
            "unregistered must be Unavailable for {kind:?}"
        );
        assert_eq!(
            compute_tools_state(&kind, false, true),
            ToolsState::Unavailable,
            "verification without registration is still Unavailable for {kind:?}"
        );
    }
    // Storage starts unknown-as-absent: a fresh runtime reads
    // Unavailable until registration flips it (S8).
    let runtime = crate::session::SessionRuntime::new();
    assert_eq!(runtime.tools_state(), ToolsState::Unavailable);
    runtime.set_tools_state(ToolsState::Unverified);
    assert_eq!(runtime.tools_state(), ToolsState::Unverified);
}

fn s2_session(kind: devboule_protocol::SessionKind) -> devboule_protocol::Session {
    devboule_protocol::Session {
        id: "s.s2.1".to_string(),
        workspace_id: None,
        cwd: None,
        kind,
        title: "Agent".to_string(),
        provider: None,
        peer_session_id: None,
        state: devboule_protocol::SessionState::Live { generation: 1 },
        elapsed_ms: Some(0),
        created_at_ms: 1,
        origin: devboule_protocol::SessionOrigin::local(),
        display_name: Some("builder".to_string()),
        created_by: Some("s.s2.0".to_string()),
        profile_id: None,
        context_id: Some("s.s2.0".to_string()),
        unattended: devboule_protocol::UnattendedState::No,
        labels: Default::default(),
        resumable: false,
    }
}

#[test]
fn bind_without_registration_is_a_noop() {
    // S8 bind-split safety: `bind_runtime` without a row touches nothing —
    // no bearer, no URL, no state flip. This is what makes the else-branch
    // bind production-identical without a row. (The registered half is
    // wired by the S9 road test, which drives a minted carrier live.)
    let state = ServerState::new("mcp-bind-noop".to_string());
    let runtime = Arc::new(crate::session::SessionRuntime::new());
    state.mcp.bind_runtime("s.nobody.9", &runtime);
    assert_eq!(runtime.tools_state(), ToolsState::Unavailable);
}

#[test]
fn registration_is_a_fact_the_surfaces_read() {
    // S9: the flipped gate admits every agent kind — Acp AND Codex rows
    // exist; unknown ids do not. (In parts 1–2 this same test pinned the
    // closed gate with Codex → None; the flip is the pass.)
    let state = ServerState::new("mcp-registered-fact".to_string());
    let owner = owner("mcp-user-reg", "mcp-client-reg");
    assert!(!state.mcp.is_registered("s.nobody.1"));
    let _guard = state
        .mcp
        .register("s.reg.1", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    assert!(state.mcp.is_registered("s.reg.1"));
    let codex = state
        .mcp
        .register_with_provider(
            "s.codex.1",
            &owner,
            &SessionKind::Codex,
            Some("codex"),
            AgentLineage::root(),
        )
        .expect("the gate answers")
        .expect("S9 mints for Codex");
    assert!(state.mcp.is_registered("s.codex.1"));
    drop(codex);
    assert!(
        !state.mcp.is_registered("s.codex.1"),
        "dropping the guard revokes the bearer"
    );
}

/// Pass 2c: nobody spells the MCP question as a list of kinds any more,
/// anywhere. The answers live in the provider impls (`Provider::hosts_mcp`,
/// `Provider::mcp_gates_first_prompt`) and the broker's two predicates are
/// shims answering through the registry; the zero counts below pin that no
/// file reintroduces an inline kind gate, and the walk pins the exact
/// answers through the shim (shim -> registry -> impl), including the
/// deliberate narrowness: pi/Codex host but never block a first prompt.
/// (Needles are concatenated so this very test does not match itself.)
#[test]
fn mcp_predicates_are_provider_facts_not_kind_lists() {
    let two = ["SessionKind::Acp ", "| SessionKind::Claude"].concat();
    let four_tail = ["| SessionKind::Pi ", "| SessionKind::Codex"].concat();
    let sources = [
        include_str!("mcp_broker.rs"),
        include_str!("session.rs"),
        include_str!("provider.rs"),
        // The session runtime's siblings: the walk has to follow the code out of
        // session.rs, or a gate site that moves file stops being covered.
        include_str!("session_children.rs"),
        include_str!("session_envelopes.rs"),
        include_str!("session_spawn.rs"),
        include_str!("session_workspaces.rs"),
        include_str!("session_messaging.rs"),
        include_str!("session_items.rs"),
        // The two halves carved out of `session_items.rs` by its seam split:
        // same walk rule as the siblings above. The registry-state half
        // carries the `SessionKind` placeholder that
        // `AgentCreator::may_create_sessions` builds its probe from, so it is
        // exactly the kind of site this walk exists to follow.
        include_str!("session_prompt_planning.rs"),
        include_str!("session_registry_state.rs"),
        // The resume road's phases, carved out of `session.rs` by the C4
        // slice: same walk rule as the siblings above.
        include_str!("session_resume.rs"),
    ];
    let mut narrow = 0;
    let mut wide = 0;
    for source in sources {
        for line in source.lines() {
            if line.contains(two.as_str()) {
                if line.contains(four_tail.as_str()) {
                    wide += 1;
                } else {
                    narrow += 1;
                }
            }
        }
    }
    assert_eq!(wide, 0, "no file spells the four-kind MCP gate any more");
    assert_eq!(
        narrow, 0,
        "no file spells the two-kind wait rule any more; gate sites call, never spell"
    );
    for (kind, hosts, gates) in [
        (SessionKind::Acp, true, true),
        (SessionKind::Claude, true, true),
        (SessionKind::Pi, true, false),
        (SessionKind::Codex, true, false),
        (SessionKind::Terminal, false, false),
    ] {
        assert_eq!(hosts_mcp(&kind), hosts, "hosts_mcp for {kind:?}");
        assert_eq!(
            mcp_gates_first_prompt(&kind),
            gates,
            "mcp_gates_first_prompt for {kind:?}"
        );
    }
}

#[test]
fn phase0_gate_names_the_decision_it_makes() {
    // The guard is still the branch that decides. Post-S9 only Terminal
    // takes `Ok(None)`; every agent kind registers. The sentence it logs
    // names the session id, the kind and provider, and the state
    // `unavailable` with the reason. `eprintln!` output cannot be captured
    // in a unit test, so the test pins the sentenced string the guard
    // emits (stated adaptation); the emission itself is verified by reading
    // the daemon log on a Terminal create.
    let line = phase0_gate_log("s.term.1", &SessionKind::Terminal, None);
    assert!(line.contains("s.term.1"), "names the session: {line}");
    assert!(line.contains("Terminal"), "names the kind: {line}");
    assert!(
        line.contains("unavailable"),
        "never renders the unknown as the benign state: {line}"
    );
    let state = ServerState::new("mcp-phase0".to_string());
    let owner = owner("mcp-user-phase0", "mcp-client-phase0");
    let terminal = state
        .mcp
        .register_with_provider(
            "s.phase0.1",
            &owner,
            &SessionKind::Terminal,
            None,
            AgentLineage::root(),
        )
        .expect("the gate answers");
    assert!(terminal.is_none(), "Terminal hosts no broker, still");
    for kind in [
        SessionKind::Acp,
        SessionKind::Claude,
        SessionKind::Pi,
        SessionKind::Codex,
    ] {
        assert!(hosts_mcp(&kind), "S9: every agent kind hosts: {kind:?}");
    }
    assert!(!hosts_mcp(&SessionKind::Terminal));
}

#[test]
fn creation_card_promises_tools_honestly_per_provider() {
    // S9: every agent family hosts a carrier, so every card promises
    // Hosted with the precedence rule's phrase. (In parts 1–2 pi/codex
    // promised Unavailable with the no-tools sentence; the flip retires
    // that branch — the sentence helpers below stay for result/roster.)
    // Forbidden state: a card without the verification promise.
    for provider in ["pi", "codex", "claude", "gemini", "grok", "qwen"] {
        assert_eq!(
            card_tools_for_provider(provider),
            ToolsState::Hosted,
            "{provider} children host tools"
        );
    }
    let unavailable = card_tools_sentence(ToolsState::Unavailable);
    assert!(
        unavailable.contains("without Devboule tools"),
        "pi/Codex card carries the no-tools sentence"
    );
    assert!(
        unavailable.contains("cannot create, message or list agents"),
        "the sentence states the consequence: {unavailable}"
    );
    let hosted = card_tools_sentence(ToolsState::Hosted);
    assert!(
        hosted.contains("will be verified at start"),
        "card promises verification, never bare has-tools: {hosted}"
    );
    assert!(
        !hosted.contains("without Devboule tools"),
        "hosted card never carries the no-tools sentence: {hosted}"
    );
}

#[test]
fn creation_result_reports_verification_per_state() {
    // Result-shape test across the three states: the `tools` word rides
    // every result, and unavailable/unverified add the model-readable
    // sentence while hosted adds none. Forbidden state: a result claiming
    // hosted for a session with no bearer (call `_for_tools` with Hosted
    // for a Pi-kind session — the wrapper below would never produce it).
    let id = json!(7);
    for (state, word, sentence) in [
        (ToolsState::Hosted, "hosted", None),
        (ToolsState::Unverified, "unverified", Some("unverified")),
        (
            ToolsState::Unavailable,
            "unavailable",
            Some("without Devboule tools"),
        ),
    ] {
        let session = s2_session(SessionKind::Acp);
        let result = created_result_for_tools(&id, &session, state);
        let structured = &result["result"]["structuredContent"];
        assert_eq!(
            structured["tools"], word,
            "every result carries the tools word"
        );
        let text = result["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        match sentence {
            Some(needle) => assert!(
                text.contains(needle),
                "{word} result carries its sentence: {text}"
            ),
            None => assert!(
                !text.contains("without Devboule tools") && !text.contains("unverified"),
                "hosted result adds no tools sentence: {text}"
            ),
        }
    }
    // S8: the wrapper reads the registration FACT, not the kind — a Codex
    // child with a minted carrier reads unverified (establishing), one
    // without reads unavailable. Production-identical while the gate holds.
    let unregistered = created_result(&id, &s2_session(SessionKind::Codex), false);
    assert_eq!(
        unregistered["result"]["structuredContent"]["tools"],
        "unavailable"
    );
    let registered = created_result(&id, &s2_session(SessionKind::Codex), true);
    assert_eq!(
        registered["result"]["structuredContent"]["tools"],
        "unverified"
    );
    let acp = created_result(&id, &s2_session(SessionKind::Acp), true);
    assert_eq!(acp["result"]["structuredContent"]["tools"], "unverified");
}

#[test]
fn protected_bytes_write_is_mode_narrow_and_atomic() {
    // S4 DACL-order test: the primitive creates narrow and stays narrow.
    // On unix the assertion is the 0o600 mode bit; on Windows the DACL call
    // runs before the first byte (code inspection) and the test pins content
    // + cleanup. Mutation: drop the 0o600 mode (unix) → this test red.
    let dir = std::env::temp_dir().join(format!("devboule-s4-write-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("carrier.json");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("tmp"));
    crate::atomic::write_protected_bytes(&path, b"{\"a\":1}").expect("protected write");
    assert_eq!(
        std::fs::read(&path).expect("read back").as_slice(),
        b"{\"a\":1}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "carrier files are owner-only (actual {mode:o})"
        );
    }
    // The temp name is the writer's own: a second write replaces atomically
    // via rename (create_new guards the temp, not the target).
    crate::atomic::write_protected_bytes(&path, b"{}").expect("atomic replace");
    assert_eq!(std::fs::read(&path).expect("read back").as_slice(), b"{}");
    // Missing parent is created, missing grandparent chain included.
    let nested = dir.join("sub").join("deep.txt");
    write_protected_str(&nested, "hello").expect("nested write");
    assert_eq!(std::fs::read_to_string(&nested).expect("read"), "hello");
    // JSON wrapper round-trips through the same primitive.
    let json_path = dir.join("roundtrip.json");
    write_protected_json(&json_path, &json!({"x": [1, 2]})).expect("json write");
    let back: Value =
        serde_json::from_slice(&std::fs::read(&json_path).expect("read")).expect("parse");
    assert_eq!(back, json!({"x": [1, 2]}));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sweep_removes_legacy_codex_homes_and_keeps_strangers() {
    // S4 sweep test: stale Claude configs, pi permission/bridge files and
    // their temps go, plus legacy owned Codex home trees (the `-c` carrier
    // writes no home; the sweep only ever sees leftovers from older builds);
    // a non-matching file — and a non-matching dir — stay. Forbidden states:
    // an orphan bridge file surviving teardown, an orphan legacy home tree
    // surviving it (leave either → red).
    let dir = std::env::temp_dir().join(format!("devboule-s4-sweep-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    for name in [
        "devboule-mcp-abc.json",
        "devboule-mcp-abc.tmp",
        "devboule-pi-permissions-7.ts",
        "devboule-pi-permissions-7.tmp",
        "devboule-pi-bridge-9.ts",
        "devboule-pi-bridge-9.tmp",
    ] {
        std::fs::write(dir.join(name), b"orphan").expect("plant orphan");
    }
    let home = dir.join("devboule-codex-home-9");
    std::fs::create_dir_all(home.join("state")).expect("plant orphan home tree");
    std::fs::write(home.join("config.toml"), b"orphan").expect("plant orphan config");
    std::fs::write(home.join("state").join("sqlite"), b"orphan").expect("plant orphan state");
    std::fs::write(dir.join("notes.txt"), b"mine").expect("plant stranger");
    std::fs::create_dir_all(dir.join("someone-elses-dir")).expect("plant stranger dir");
    std::fs::write(dir.join("devboule-mcp-abc.json.bak"), b"bak").expect("plant bak");
    cleanup_stale_configs(&dir).expect("sweep");
    for name in [
        "devboule-mcp-abc.json",
        "devboule-mcp-abc.tmp",
        "devboule-pi-permissions-7.ts",
        "devboule-pi-permissions-7.tmp",
        "devboule-pi-bridge-9.ts",
        "devboule-pi-bridge-9.tmp",
    ] {
        assert!(!dir.join(name).exists(), "orphan {name} is swept");
    }
    assert!(dir.join("notes.txt").exists(), "strangers are kept");
    assert!(
        dir.join("someone-elses-dir").is_dir(),
        "stranger dirs are kept"
    );
    assert!(
        !dir.join("devboule-codex-home-9").exists(),
        "orphan legacy Codex home trees are swept"
    );
    assert!(
        dir.join("devboule-mcp-abc.json.bak").exists(),
        ".bak is not our temp suffix and is kept"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn redaction_covers_bearer_and_url_but_not_env_names() {
    // S4 redaction test: bearer+url vanish from errors/logs for the new
    // carriers too; env names (`DEVBOULE_MCP_TOKEN`, `DEVBOULE_MCP_URL`)
    // carry no secret bytes themselves and pass through.
    let config = McpLaunchConfig {
        url: "http://127.0.0.1:4567/mcp".to_string(),
        bearer: "secret-bearer-xyz".to_string(),
        claude_config_path: None,
    };
    let error = format!(
        "bridge dial {} with Bearer {} failed",
        config.url,
        config.bearer()
    );
    let redacted = config.redact_text(&error);
    assert!(
        !redacted.contains("secret-bearer-xyz"),
        "bearer redacted: {redacted}"
    );
    assert!(!redacted.contains("4567"), "endpoint redacted: {redacted}");
    let argv = "pi --mode rpc -e bridge.ts with DEVBOULE_MCP_TOKEN and DEVBOULE_MCP_URL";
    assert_eq!(config.redact_text(argv), argv, "env names are not secrets");
    // S8: the same cover for the Codex launch line — the URL rides a `-c`
    // override on argv, the bearer value stays in the env, and the env-var
    // names themselves are not secrets.
    let codex_argv = format!(
        "codex app-server -c mcp_servers.devboule.url=\"{}\" with Bearer {} and {}",
        config.url,
        config.bearer(),
        crate::mcp_broker::MCP_TOKEN_ENV
    );
    let redacted = config.redact_text(&codex_argv);
    assert!(
        !redacted.contains("secret-bearer-xyz"),
        "bearer redacted: {redacted}"
    );
    assert!(!redacted.contains("4567"), "endpoint redacted: {redacted}");
    assert!(
        redacted.contains("DEVBOULE_MCP_TOKEN"),
        "names pass through"
    );
}

#[test]
fn roster_entries_carry_the_tools_word() {
    // Roster test with mixed states (and, since S8, mixed kinds — the word
    // is kind-blind by construction): every entry carries `tools`.
    // Forbidden state: one entry with the field dropped (remove the field
    // in the fixture → red).
    for kind in [SessionKind::Acp, SessionKind::Codex] {
        for state in [
            ToolsState::Hosted,
            ToolsState::Unavailable,
            ToolsState::Unverified,
        ] {
            let runtime = crate::session::SessionRuntime::new();
            runtime.set_tools_state(state);
            let value = agent_value(&s2_session(kind.clone()), &runtime, 1);
            assert_eq!(
                value["tools"],
                state.as_str(),
                "every roster entry carries its tools word"
            );
        }
    }
}

#[test]
fn agent_activity_tool_serves_one_agents_metadata() {
    let state = ServerState::new("mcp-activity".to_string());
    let stranger_owner = owner("mcp-stranger-user", "mcp-stranger-client");
    let owner = owner("mcp-activity-user", "mcp-activity-client");
    crate::session::insert_test_live_agent(&state.sessions, "activity-caller", owner.clone());
    let child =
        crate::session::insert_test_live_agent(&state.sessions, "activity-child", owner.clone());
    child.publish_agent_event(
        SessionEvent::AgentThought {
            message_id: None,
            text: "thinking".to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,
        },
        None,
    );
    let guard = state
        .mcp
        .register("activity-caller", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("caller MCP guard");
    let token = state.mcp.test_token("activity-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let listed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let listed_body = response_json(&listed);
    let tools = listed_body["result"]["tools"].as_array().expect("tools");
    let activity = tools
        .iter()
        .find(|tool| tool["name"] == crate::provider_catalog::MCP_ACTIVITY_TOOL)
        .expect("activity is served");
    assert_eq!(activity["inputSchema"]["required"], json!(["session"]));
    let response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"activity-child"}}}"#,
    );
    let body = response_json(&response);
    assert_eq!(body["result"]["isError"], false);
    let doc = &body["result"]["structuredContent"];
    assert_eq!(doc["sessionId"], "activity-child");
    assert_eq!(doc["activity"], "idle");
    let recent = doc["recent"].as_array().expect("recent");
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0]["kind"], "agent_thought");
    assert!(
        recent[0].get("text").is_none(),
        "kinds only, never transcript text"
    );
    assert!(doc.get("summary").is_none());
    let missing = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"activity-missing"}}}"#,
    );
    assert_eq!(response_json(&missing)["error"]["code"], -32602);
    let bogus = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"activity-child","bogus":1}}}"#,
    );
    assert_eq!(response_json(&bogus)["error"]["code"], -32602);
    // A stranger's session is the same refusal as a missing one: the
    // daemon cannot and must not say which.
    crate::session::insert_test_live_agent(&state.sessions, "activity-stranger", stranger_owner);
    let stranger = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"activity-stranger"}}}"#,
    );
    assert_eq!(response_json(&stranger)["error"]["code"], -32602);
    // Every test session is titled "Agent": naming the title refuses
    // with the remedy instead of silently reading the lowest id.
    let vague = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"Agent"}}}"#,
    );
    let vague_body = response_json(&vague);
    assert_eq!(vague_body["error"]["code"], -32602);
    assert!(
        vague_body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("use the session id")),
        "ambiguity names the remedy: {}",
        vague_body["error"]["message"]
    );
    // The limit is honored and capped: 0 reads state only, a huge
    // number stops at the cap.
    for _ in 0..55 {
        child.publish_agent_event(
            SessionEvent::AgentThought {
                message_id: None,
                text: "thinking".to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
            None,
        );
    }
    let capped = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"activity-child","limit":5000}}}"#,
    );
    assert_eq!(
        response_json(&capped)["result"]["structuredContent"]["recent"]
            .as_array()
            .expect("recent")
            .len(),
        crate::agent_activity::ACTIVITY_MAX_LIMIT
    );
    let state_only = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"activity-child","limit":0}}}"#,
    );
    assert!(
        response_json(&state_only)["result"]["structuredContent"]["recent"]
            .as_array()
            .expect("recent")
            .is_empty()
    );
    drop(guard);
    drop(server);
}

#[test]
fn a_stored_policy_can_take_the_activity_tool_away() {
    // The catalog promises it: supervision is disableable, unlike the
    // roster and the profile list. A disabled tool is refused before
    // anything is touched, and vanishes from tools/list.
    let state = ServerState::new("mcp-activity-policy".to_string());
    let owner = owner("mcp-activity-policy-user", "mcp-activity-policy-client");
    crate::session::insert_test_live_agent(&state.sessions, "policy-caller", owner.clone());
    let guard = state
        .mcp
        .register_with_provider(
            "policy-caller",
            &owner,
            &SessionKind::Acp,
            Some("claude"),
            AgentLineage::root(),
        )
        .expect("registration")
        .expect("MCP guard");
    state
        .tool_policy
        .set(
            "claude",
            Some(true),
            vec![crate::provider_catalog::MCP_ACTIVITY_TOOL.to_string()],
        )
        .expect("policy");
    let token = state.mcp.test_token("policy-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let listed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let listed_body = response_json(&listed);
    let names: Vec<&str> = listed_body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(!names.contains(&crate::provider_catalog::MCP_ACTIVITY_TOOL));
    let refused = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"policy-caller"}}}"#,
    );
    let refused_body = response_json(&refused);
    assert_eq!(refused_body.pointer("/error/code"), Some(&json!(-32601)));
    assert_eq!(
        refused_body.pointer("/error/message"),
        Some(&json!("Tool disabled by policy"))
    );
    drop(guard);
    drop(server);
}

#[test]
fn the_end_tools_stop_and_close_a_callers_own_children_only() {
    // The destructive pair end to end: served, scoped to the caller's
    // own children, and refused for the caller's parent, itself, and an
    // invented id — with the sentences the sessions layer owns.
    let state = ServerState::new("mcp-end".to_string());
    let owner = owner("mcp-end-user", "mcp-end-client");
    crate::session::insert_test_live_agent(&state.sessions, "end-parent", owner.clone());
    crate::session::insert_test_child_agent(
        &state.sessions,
        "end-caller",
        owner.clone(),
        "end-parent",
    );
    crate::session::insert_test_child_agent(
        &state.sessions,
        "end-child",
        owner.clone(),
        "end-caller",
    );
    let guard = state
        .mcp
        .register("end-caller", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("caller MCP guard");
    let token = state.mcp.test_token("end-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let call = |name: &str, arguments: &str| {
        http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            &format!(
                r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
            ),
        )
    };
    let sentence = |reply: &str| {
        response_json(reply)["result"]["content"][0]["text"]
            .as_str()
            .expect("the refusal sentence")
            .to_string()
    };
    let listed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let tools = response_json(&listed)["result"]["tools"]
        .as_array()
        .expect("tools")
        .to_vec();
    for name in [
        crate::provider_catalog::MCP_STOP_AGENT_TOOL,
        crate::provider_catalog::MCP_CLOSE_AGENT_TOOL,
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == json!(name))
            .unwrap_or_else(|| panic!("{name} is served"));
        assert_eq!(tool["inputSchema"]["required"], json!(["session"]));
    }
    // The green stop: the child stops, the row stays.
    let stopped = response_json(&call(
        crate::provider_catalog::MCP_STOP_AGENT_TOOL,
        r#"{"session":"end-child"}"#,
    ));
    assert_eq!(stopped["result"]["isError"], json!(false));
    assert_eq!(stopped["result"]["content"][0]["text"], json!("stopped"));
    let live_ids = || {
        state
            .sessions
            .live_agent_entries(&owner)
            .expect("entries")
            .into_iter()
            .map(|entry| entry.session.id)
            .collect::<Vec<_>>()
    };
    assert!(live_ids().iter().any(|id| id == "end-child"));
    // The parent, itself, and an invented id: refused either way, and
    // the invented one is indistinguishable from the parent.
    assert!(sentence(&call(
        crate::provider_catalog::MCP_STOP_AGENT_TOOL,
        r#"{"session":"end-parent"}"#,
    ))
    .contains("none of your live children"));
    assert!(sentence(&call(
        crate::provider_catalog::MCP_STOP_AGENT_TOOL,
        r#"{"session":"end-caller"}"#,
    ))
    .contains("not its own child"));
    assert!(sentence(&call(
        crate::provider_catalog::MCP_STOP_AGENT_TOOL,
        r#"{"session":"end-invented"}"#,
    ))
    .contains("none of your live children"));
    // A missing argument is a protocol error before any scope runs.
    let bogus = response_json(&call(
        crate::provider_catalog::MCP_STOP_AGENT_TOOL,
        r#"{"bogus":1}"#,
    ));
    assert_eq!(bogus["error"]["code"], json!(-32602));
    // The green close: the row goes, everything else stays.
    let closed = response_json(&call(
        crate::provider_catalog::MCP_CLOSE_AGENT_TOOL,
        r#"{"session":"end-child"}"#,
    ));
    assert_eq!(closed["result"]["isError"], json!(false));
    assert_eq!(closed["result"]["content"][0]["text"], json!("closed"));
    let remaining = live_ids();
    assert!(!remaining.iter().any(|id| id == "end-child"));
    assert!(remaining.iter().any(|id| id == "end-caller"));
    assert!(remaining.iter().any(|id| id == "end-parent"));
    drop(guard);
    drop(server);
}

#[test]
fn a_stored_policy_can_take_the_end_tools_away() {
    // The catalog promises it: the destructive pair is supervision,
    // disableable like the send tool, unlike the roster and the profile
    // list. A disabled tool is refused before anything is touched.
    let state = ServerState::new("mcp-end-policy".to_string());
    let owner = owner("mcp-end-policy-user", "mcp-end-policy-client");
    crate::session::insert_test_live_agent(&state.sessions, "end-policy-caller", owner.clone());
    let guard = state
        .mcp
        .register_with_provider(
            "end-policy-caller",
            &owner,
            &SessionKind::Acp,
            Some("claude"),
            AgentLineage::root(),
        )
        .expect("registration")
        .expect("MCP guard");
    state
        .tool_policy
        .set(
            "claude",
            Some(true),
            vec![
                crate::provider_catalog::MCP_STOP_AGENT_TOOL.to_string(),
                crate::provider_catalog::MCP_CLOSE_AGENT_TOOL.to_string(),
            ],
        )
        .expect("policy");
    let token = state.mcp.test_token("end-policy-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let listed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let listed_body = response_json(&listed);
    let names: Vec<&str> = listed_body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(!names.contains(&crate::provider_catalog::MCP_STOP_AGENT_TOOL));
    assert!(!names.contains(&crate::provider_catalog::MCP_CLOSE_AGENT_TOOL));
    let refused = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_stop_agent","arguments":{"session":"end-policy-caller"}}}"#,
    );
    assert_eq!(
        response_json(&refused).pointer("/error/code"),
        Some(&json!(-32601))
    );
    drop(guard);
    drop(server);
}

#[test]
fn a_restored_overlay_hides_and_refuses_both_denied_tools() {
    // The two gates a resumed lineage feeds, on the exact functions the
    // broker calls: `enabled_tool_list` for tools/list,
    // `tool_call_refusal` for tools/call. One tool proving one gate
    // does not prove the restriction.
    use crate::provider_catalog::{
        MCP_ACTIVITY_TOOL, MCP_CREATE_AGENT_TOOL, MCP_ROSTER_TOOL, MCP_SEND_MESSAGE_TOOL,
    };
    let overlay = crate::provider_catalog::ToolOverlay::from_profile_names(&[
        MCP_SEND_MESSAGE_TOOL.to_string(),
        MCP_CREATE_AGENT_TOOL.to_string(),
    ]);
    let names: Vec<String> = enabled_tool_list(
        crate::provider_catalog::MCP_BROKER_TOOLS,
        None,
        overlay.clone(),
    )
    .into_iter()
    .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
    .collect();
    assert!(
        !names.iter().any(|name| name == MCP_SEND_MESSAGE_TOOL),
        "send is hidden from the list: {names:?}"
    );
    assert!(
        !names.iter().any(|name| name == MCP_CREATE_AGENT_TOOL),
        "create is hidden from the list: {names:?}"
    );
    assert!(
        names.iter().any(|name| name == MCP_ROSTER_TOOL)
            && names.iter().any(|name| name == MCP_ACTIVITY_TOOL),
        "the rest is still served: {names:?}"
    );
    assert_eq!(
        tool_call_refusal(None, &overlay, MCP_SEND_MESSAGE_TOOL),
        Some("Tool disabled by policy")
    );
    assert_eq!(
        tool_call_refusal(None, &overlay, MCP_CREATE_AGENT_TOOL),
        Some("Tool disabled by policy")
    );
    assert_eq!(tool_call_refusal(None, &overlay, MCP_ROSTER_TOOL), None);
}

#[test]
fn a_journal_row_restriction_reaches_the_broker_registration() {
    // From journal bytes to broker gates through every production
    // function on the wiring path: row → resumed_lineage →
    // register_with_provider → HTTP tools/list + tools/call. It does not
    // execute resume()'s call site (which needs a live provider for the
    // respawn) nor the respawn itself: reverting that one line escapes
    // this test, and only the live e2e battery covers it.
    use crate::provider_catalog::{MCP_CREATE_AGENT_TOOL, MCP_SEND_MESSAGE_TOOL};
    let dir = std::env::temp_dir().join(format!("devboule-overlay-wire-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let journal = crate::journal::Journal::open(&dir.join("journal.db")).expect("journal");
    let mut record = crate::journal::new_session_record(
        "wire-child",
        "wire-user",
        None,
        SessionKind::Acp,
        "Agent",
    );
    record.created_by = Some("wire-creator".to_string());
    record.overlay = Some(crate::provider_catalog::ToolOverlay::from_profile_names(&[
        MCP_SEND_MESSAGE_TOOL.to_string(),
        MCP_CREATE_AGENT_TOOL.to_string(),
    ]));
    record.depth = Some(1);
    journal.create_session(record).expect("birth row");
    journal.shutdown();
    // The restart: a new journal on the same file.
    let journal = crate::journal::Journal::open(&dir.join("journal.db")).expect("reopen");
    let row = journal
        .list()
        .expect("list")
        .into_iter()
        .find(|row| row.id == "wire-child")
        .expect("the birth row survived");
    let lineage = crate::session::SessionRegistry::resumed_lineage(Some(&row))
        .expect("readable row restores");
    assert_eq!(lineage.depth, 1);
    // Register the way resume() does, then ask over HTTP like a child
    // would: both denied tools stay hidden and refused.
    let state = ServerState::new("mcp-overlay-wire".to_string());
    let owner = owner("wire-user", "wire-client");
    crate::session::insert_test_live_agent(&state.sessions, "wire-child", owner.clone());
    let guard = state
        .mcp
        .register_with_provider("wire-child", &owner, &SessionKind::Acp, None, lineage)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("wire-child").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let listed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let listed_body = response_json(&listed);
    let names: Vec<&str> = listed_body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(!names.contains(&MCP_SEND_MESSAGE_TOOL));
    assert!(!names.contains(&MCP_CREATE_AGENT_TOOL));
    for (id, tool) in [(2, MCP_SEND_MESSAGE_TOOL), (3, MCP_CREATE_AGENT_TOOL)] {
        let call = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            &format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{tool}","arguments":{{}}}}}}"#
            ),
        );
        let body = response_json(&call);
        assert_eq!(
            body.pointer("/error/code"),
            Some(&json!(-32601)),
            "{tool} refused"
        );
        assert_eq!(
            body.pointer("/error/message"),
            Some(&json!("Tool disabled by policy"))
        );
    }
    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(server);
    drop(guard);
    drop(state);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

#[test]
fn pi_bridge_fetch_hygiene_against_the_real_broker() {
    // S5/Q1 measurement: the bridge's exact header set against the REAL broker,
    // raw bytes. Dual Accept takes the JSON branch (not SSE framing); the
    // `notifications/initialized` second call is 202-empty (success without a
    // result — never parsed, never failed); RPC errors ride HTTP 200 (a `res.ok`
    // branch would read refusals as success); chunked is refused; no bearer is
    // 401. The Node-`fetch`-sends-`Content-Length` half is spike-measured +
    // template-pinned (string bodies); this pins the broker half it speaks to.
    use std::net::Shutdown;
    fn raw_post(url: &str, headers: &[(&str, &str)], body: &str) -> String {
        let endpoint = url
            .strip_prefix("http://")
            .expect("loopback URL")
            .split('/')
            .next()
            .expect("loopback endpoint")
            .to_string();
        let mut stream = TcpStream::connect(endpoint).expect("MCP listener");
        let mut request = format!(
            "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str(&format!("\r\n{body}"));
        stream.write_all(request.as_bytes()).expect("MCP request");
        stream.shutdown(Shutdown::Write).expect("request shutdown");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("MCP response");
        String::from_utf8(response).expect("HTTP response")
    }
    fn split_response(response: &str) -> (&str, &str) {
        response.split_once("\r\n\r\n").expect("HTTP response body")
    }
    let state = ServerState::new("mcp-bridge-hygiene".to_string());
    let owner = owner("mcp-user-hygiene", "mcp-client-hygiene");
    let _guard = state
        .mcp
        .register("hygiene", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("hygiene").expect("token");
    // The door resolves callers from the registry row: give the session one
    // (a local row, like a person-started session), or every `tools/call`
    // below is refused as `Absent` before anything is touched.
    crate::session::insert_test_live_agent(&state.sessions, "hygiene", owner.clone());
    let server = state.mcp.start(&state).expect("MCP server");
    let url = state.mcp.url.clone();
    let bearer = format!("Bearer {token}");
    // The bridge's exact header set: JSON body, dual Accept, Bearer.
    let headers_ref: Vec<(&str, &str)> = vec![
        ("Content-Type", "application/json"),
        ("Accept", "application/json, text/event-stream"),
        ("Authorization", bearer.as_str()),
    ];
    // initialize → 200 JSON (raw body opens with `{`: the JSON branch, not
    // SSE framing — the Q1 dual-Accept verdict).
    let init = raw_post(
        &url,
        &headers_ref,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"devboule-pi-bridge","version":"1"}}}"#,
    );
    let (head, body) = split_response(&init);
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "initialize status: {head}"
    );
    assert!(head.contains("application/json"), "JSON branch: {head}");
    assert!(body.starts_with('{'), "raw JSON body, no event framing");
    let reply: Value = serde_json::from_str(body).expect("initialize reply");
    assert_eq!(reply["result"]["serverInfo"]["name"], "devboule");
    // notifications/initialized → 202 with an empty body: success with no
    // result. The bridge's `mcpNotify` never parses it.
    let notified = raw_post(
        &url,
        &headers_ref,
        r#"{"jsonrpc":"2.0","id":2,"method":"notifications/initialized"}"#,
    );
    let (head, body) = split_response(&notified);
    assert!(
        head.starts_with("HTTP/1.1 202"),
        "notification status: {head}"
    );
    assert!(body.is_empty(), "202 carries no body");
    // tools/list over the same headers → the seven tools as JSON.
    let listed = raw_post(
        &url,
        &headers_ref,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
    );
    let (head, body) = split_response(&listed);
    assert!(head.starts_with("HTTP/1.1 200"), "list status: {head}");
    let reply: Value = serde_json::from_str(body).expect("list reply");
    let names: Vec<&str> = reply["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(names.len(), crate::provider_catalog::MCP_BROKER_TOOLS.len());
    for (name, _) in crate::provider_catalog::MCP_BROKER_TOOLS {
        assert!(names.contains(name), "broker serves {name}");
    }
    // The trap the bridge avoids: SSE-only Accept gets one-event framing.
    let sse_headers: Vec<(&str, &str)> = vec![
        ("Content-Type", "application/json"),
        ("Accept", "text/event-stream"),
        ("Authorization", &bearer),
    ];
    let sse = raw_post(
        &url,
        &sse_headers,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#,
    );
    let (head, body) = split_response(&sse);
    assert!(head.starts_with("HTTP/1.1 200"), "sse status: {head}");
    assert!(
        body.starts_with("event: message\ndata: "),
        "SSE-only gets event framing the bridge avoids by sending dual Accept"
    );
    // RPC errors ride HTTP 200: an unknown tool is a 200 with an error
    // payload, never an HTTP error — a `res.ok` branch reads it as success.
    let unknown = raw_post(
        &url,
        &headers_ref,
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"no_such_tool","arguments":{}}}"#,
    );
    let (head, body) = split_response(&unknown);
    assert!(head.starts_with("HTTP/1.1 200"), "rpc error status: {head}");
    let reply: Value = serde_json::from_str(body).expect("error reply");
    assert_eq!(
        reply["error"]["code"],
        serde_json::json!(-32601),
        "rpc error code"
    );
    // A served call answers a result on the same 200.
    let roster = raw_post(
        &url,
        &headers_ref,
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"devboule_list_agents","arguments":{}}}"#,
    );
    let (head, body) = split_response(&roster);
    assert!(head.starts_with("HTTP/1.1 200"), "call status: {head}");
    let reply: Value = serde_json::from_str(body).expect("call reply");
    assert!(reply["result"]["structuredContent"]["agents"].is_array());
    // No bearer is 401 before anything is touched.
    let bare: Vec<(&str, &str)> = vec![
        ("Content-Type", "application/json"),
        ("Accept", "application/json, text/event-stream"),
    ];
    let denied = raw_post(
        &url,
        &bare,
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#,
    );
    assert!(denied.starts_with("HTTP/1.1 401"), "bearer required");
    drop(server);
}

fn owner(user: &str, client: &str) -> OwnerId {
    OwnerId::new(user, client).expect("owner")
}

fn endpoint(url: &str) -> String {
    url.strip_prefix("http://")
        .expect("loopback URL")
        .split('/')
        .next()
        .expect("loopback endpoint")
        .to_string()
}

fn http_request(url: &str, authorization: Option<&str>, body: &str) -> String {
    let mut stream = TcpStream::connect(endpoint(url)).expect("MCP listener");
    let authorization = authorization
        .map(|value| format!("Authorization: {value}\r\n"))
        .unwrap_or_default();
    let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{authorization}\r\n{body}",
            body.len()
        );
    stream.write_all(request.as_bytes()).expect("MCP request");
    stream.shutdown(Shutdown::Write).expect("request shutdown");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("MCP response");
    String::from_utf8(response).expect("HTTP response")
}

fn response_json(response: &str) -> Value {
    serde_json::from_str(
        response
            .split_once("\r\n\r\n")
            .expect("HTTP response body")
            .1,
    )
    .expect("JSON response")
}

#[test]
fn loopback_requests_require_the_session_bearer() {
    let state = ServerState::new("mcp-auth".to_string());
    let first_owner = owner("mcp-user-first", "mcp-client-first");
    let second_owner = owner("mcp-user-second", "mcp-client-second");
    let first_guard = state
        .mcp
        .register("first", &first_owner, &SessionKind::Acp)
        .expect("first registration")
        .expect("first MCP guard");
    let second_guard = state
        .mcp
        .register("second", &second_owner, &SessionKind::Acp)
        .expect("second registration")
        .expect("second MCP guard");
    let first_token = state.mcp.test_token("first").expect("first token");
    let second_token = state.mcp.test_token("second").expect("second token");
    assert_eq!(
        state
            .mcp
            .authenticate(Some(&format!("Bearer {first_token}")))
            .expect("first auth")
            .owner,
        first_owner
    );
    assert_eq!(
        state
            .mcp
            .authenticate(Some(&format!("Bearer {second_token}")))
            .expect("second auth")
            .owner,
        second_owner
    );
    assert!(state.mcp.authenticate(None).is_none());
    assert!(state
        .mcp
        .authenticate(Some("Bearer token-from-another-session"))
        .is_none());

    let server = state.mcp.start(&state).expect("MCP server");
    let url = state.mcp.url.clone();
    let no_bearer = http_request(
        &url,
        None,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    assert!(no_bearer.starts_with("HTTP/1.1 401"));
    let wrong_bearer = http_request(
        &url,
        Some("Bearer token-from-another-session"),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    );
    assert!(wrong_bearer.starts_with("HTTP/1.1 401"));
    drop(first_guard);
    let revoked = http_request(
        &url,
        Some(&format!("Bearer {first_token}")),
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
    );
    assert!(revoked.starts_with("HTTP/1.1 401"));
    drop(second_guard);
    drop(server);
}

#[test]
fn connection_cap_is_enforced_before_client_spawn() {
    let state = ServerState::new("mcp-connection-cap".to_string());
    let permits = (0..MAX_MCP_CONNECTIONS)
        .map(|_| {
            assert!(state.mcp.try_acquire_connection());
            ConnectionPermit {
                broker: Arc::clone(&state.mcp),
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(permits.len(), MAX_MCP_CONNECTIONS);
    assert!(!state.mcp.try_acquire_connection());
    drop(permits);
    assert!(state.mcp.try_acquire_connection());
    let permit = ConnectionPermit {
        broker: Arc::clone(&state.mcp),
    };
    drop(permit);
}

#[test]
fn listener_rejects_overflow_and_admits_a_client_after_preauth_expiry() {
    let state = ServerState::new("mcp-listener-cap".to_string());
    let owner = owner("mcp-listener-user", "mcp-listener-client");
    let guard = state
        .mcp
        .register("listener-session", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("listener-session").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let url = state.mcp.url.clone();
    let mut flood = Vec::with_capacity(MAX_MCP_CONNECTIONS);
    for _ in 0..MAX_MCP_CONNECTIONS {
        let mut stream = TcpStream::connect(endpoint(&url)).expect("flood connection");
        stream.write_all(b"GET ").expect("flood request prefix");
        flood.push(stream);
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    while state.mcp.active_connections.load(Ordering::Acquire) < MAX_MCP_CONNECTIONS
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        state.mcp.active_connections.load(Ordering::Acquire),
        MAX_MCP_CONNECTIONS
    );

    let mut overflow = TcpStream::connect(endpoint(&url)).expect("overflow connection");
    overflow
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("overflow read timeout");
    overflow
            .write_all(
                format!(
                    "POST /mcp HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\n\r\n"
                )
                .as_bytes(),
            )
            .expect("overflow request");
    overflow
        .shutdown(Shutdown::Write)
        .expect("overflow shutdown");
    thread::sleep(Duration::from_millis(100));
    let mut overflow_response = Vec::new();
    let overflow_result = overflow.read_to_end(&mut overflow_response);
    let overflow_closed = match &overflow_result {
        Ok(_) => true,
        Err(error) => matches!(
            error.kind(),
            io::ErrorKind::ConnectionAborted
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::TimedOut
        ),
    };
    assert!(
            overflow_response.is_empty() && overflow_closed,
            "overflow connection must be dropped before dispatch: {overflow_result:?} {overflow_response:?}"
        );

    let deadline = Instant::now() + Duration::from_secs(5);
    while state.mcp.active_connections.load(Ordering::Acquire) != 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(state.mcp.active_connections.load(Ordering::Acquire), 0);
    let response = http_request(
        &url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    assert!(response.starts_with("HTTP/1.1 200"));
    drop(flood);
    drop(server);
    drop(guard);
}

#[test]
fn accept_resource_errors_are_retried_but_permanent_errors_stop() {
    let transient_kinds = [
        io::ErrorKind::WouldBlock,
        io::ErrorKind::Interrupted,
        io::ErrorKind::ConnectionAborted,
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::TimedOut,
    ];
    for kind in transient_kinds {
        assert!(is_transient_accept_error(&io::Error::from(kind)));
    }
    for code in [12, 23, 24, 105, 10024, 10055] {
        assert!(is_transient_accept_error(&io::Error::from_raw_os_error(
            code
        )));
    }
    assert!(!is_transient_accept_error(&io::Error::from(
        io::ErrorKind::BrokenPipe,
    )));
    assert!(!is_transient_accept_error(&io::Error::from_raw_os_error(
        12345
    )));
}

#[test]
fn get_stream_has_a_bounded_lifetime() {
    let state = ServerState::new("mcp-get-lifetime".to_string());
    let owner = owner("mcp-get-user", "mcp-get-client");
    let guard = state
        .mcp
        .register("get-session", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("get-session").expect("token");
    let server = state
        .mcp
        .start_with_get_lifetime(&state, Duration::from_millis(10))
        .expect("MCP server");
    let mut client = TcpStream::connect(endpoint(&state.mcp.url)).expect("test client");
    client
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("client read timeout");
    client
        .write_all(
            format!(
                "GET /mcp HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\r\n"
            )
            .as_bytes(),
        )
        .expect("GET request");
    let mut response = Vec::new();
    client.read_to_end(&mut response).expect("GET response");
    let response = String::from_utf8(response).expect("GET response text");
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("Content-Type: text/event-stream"));
    drop(server);
    drop(guard);
}

#[test]
fn http_bearer_owns_the_filtered_roster_not_tool_arguments() {
    let state = ServerState::new("mcp-arguments".to_string());
    let caller = owner("mcp-argument-user-a", "mcp-argument-client-a");
    let other = owner("mcp-argument-user-b", "mcp-argument-client-b");
    crate::session::insert_test_live_agent(&state.sessions, "agent-a", caller.clone());
    crate::session::insert_test_live_agent(&state.sessions, "agent-b", other.clone());
    let caller_guard = state
        .mcp
        .register("agent-a", &caller, &SessionKind::Acp)
        .expect("registration")
        .expect("caller MCP guard");
    let other_guard = state
        .mcp
        .register("agent-b", &other, &SessionKind::Acp)
        .expect("registration")
        .expect("other MCP guard");
    let caller_token = state.mcp.test_token("agent-a").expect("caller token");
    let server = state.mcp.start(&state).expect("MCP server");
    let response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {caller_token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_agents","arguments":{"agent_id":"agent-b"}}}"#,
    );
    assert!(response.starts_with("HTTP/1.1 200"));
    let body = response_json(&response);
    let agents = body
        .pointer("/result/structuredContent/agents")
        .and_then(Value::as_array)
        .expect("agent roster");
    assert!(agents.iter().any(|agent| agent["id"] == "agent-a"));
    assert!(!agents.iter().any(|agent| agent["id"] == "agent-b"));
    let other_token = state.mcp.test_token("agent-b").expect("other token");
    let other_response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {other_token}")),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_list_agents"}}"#,
    );
    let other_body = response_json(&other_response);
    let other_agents = other_body
        .pointer("/result/structuredContent/agents")
        .and_then(Value::as_array)
        .expect("other agent roster");
    assert!(!other_agents.iter().any(|agent| agent["id"] == "agent-a"));
    assert!(other_agents.iter().any(|agent| agent["id"] == "agent-b"));
    let send_response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {caller_token}")),
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"devboule_send_message","arguments":{"to_agent":"agent-missing","text":"hello"}}}"#,
    );
    let send_body = response_json(&send_response);
    assert_eq!(send_body["error"]["code"], -32602);
    drop(caller_guard);
    drop(other_guard);
    drop(server);
}

/// The brief's second refusal, measured rather than assumed.
///
/// A connection owned by a session's Bearer is an MCP connection, and its
/// only channel into the daemon is a tool *name*: the broker runs in this
/// process and never carries a pipe frame, so there is no
/// `AgentProfilesGet`/`AgentProfilesSet` a bearer could send. What a bearer
/// can try is a tool named after the store, and this is what it gets.
#[test]
fn a_bearers_tool_name_cannot_reach_the_agent_profile_store() {
    let state = ServerState::new("mcp-agent-profiles".to_string());
    let owner = owner("mcp-profile-user", "mcp-profile-client");
    // The caller is a live local session: the tool door resolves every
    // bearer to its registry row, and a rowless registration is refused.
    crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
    let guard = state
        .mcp
        .register("session", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("session").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    // What is forbidden, exactly: any name in the bearer's closed table
    // that carries the profile-store vocabulary, because a tool name is a
    // bearer's only channel into the daemon — the store's own RPCs
    // (`AgentProfilesGet`/`AgentProfilesSet`) travel a different surface
    // and cannot be reached from here. Two deliberate exceptions, each
    // with its own authority, and nothing else: `devboule_list_profiles`,
    // the read-only ticked list the design serves; and
    // `devboule_set_agent_profile`, which reads the store through the same
    // resolver the create tool uses and writes only a child's own row —
    // its authority is the `created_by` link, never the name. Every other
    // `profile` spelling must fail this assertion, including one-letter
    // neighbours of the allowed names such as `devboule_agent_profile_get`,
    // which a substring deny on `agent_profiles`/`set_profile` used to
    // wave through.
    for (name, _) in crate::provider_catalog::MCP_BROKER_TOOLS {
        assert!(
            name == &crate::provider_catalog::MCP_LIST_PROFILES_TOOL
                || name == &crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL
                || !name.contains("profile"),
            "the broker's closed table must not reach the profile store: \
                 {name} carries the profile vocabulary and is neither of the two \
                 allowed tools (the read-only list, the created_by-gated move)"
        );
    }

    for (index, name) in [
        "devboule_agent_profiles",
        "devboule_set_profile",
        "agent_profiles_set",
        "devboule_agent_profile_get",
    ]
    .iter()
    .enumerate()
    {
        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            &format!(
                r#"{{"jsonrpc":"2.0","id":{},"method":"tools/call","params":{{"name":"{name}"}}}}"#,
                index + 1
            ),
        );
        let body = response_json(&response);
        assert_eq!(body["error"]["code"], -32601, "{name}: {body}");
        assert_eq!(body["error"]["message"], "Unknown tool", "{name}: {body}");
    }

    // The delegation switch is refused the same way, on the same grounds:
    // its RPCs (`DelegationGet`/`DelegationSet`) travel the app's wire and
    // cannot be reached from a bearer, so the only attack is a tool named
    // after it. `delegat` catches every delegation spelling; `grant`
    // catches the vocabulary a per-session or per-creator allow would
    // reach for, and this slice deliberately has no tool by that name —
    // the only thing that answers a card is `devboule_answer_permission`,
    // which matches neither word because it reads the pending table, not
    // the switch.
    for (name, _) in crate::provider_catalog::MCP_BROKER_TOOLS {
        assert!(
            !name.contains("delegat") && !name.contains("grant"),
            "the broker's closed table must not reach the delegation switch: \
                 {name} carries the switch's vocabulary"
        );
    }

    drop(guard);
    drop(server);
}

/// C8, at the two doors an agent's answer has: the schema the model
/// reads offers `allow_once` and `deny` and nothing else, and the arm
/// itself refuses any other outcome string before it looks at a card.
#[test]
fn the_answer_tool_offers_allow_once_or_deny_and_nothing_else() {
    let state = ServerState::new("mcp-answer-c8".to_string());
    let owner = owner("mcp-answer-user", "mcp-answer-client");
    // The caller is a live local session: the tool door resolves every
    // bearer to its registry row, and a rowless registration is refused.
    crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
    let guard = state
        .mcp
        .register("session", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("session").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let body = response_json(&response);
    let schema = body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == crate::provider_catalog::MCP_ANSWER_PERMISSION_TOOL)
        .expect("the answer tool is listed")["inputSchema"]
        .clone();
    assert_eq!(
        schema["properties"]["outcome"]["enum"],
        serde_json::json!(["allow_once", "deny"]),
        "the closed outcome table, at the schema: {schema}"
    );

    // The arm refuses a durable allow before any card is consulted:
    // allow_always is not representable from an agent, ever.
    for (id, arguments, why) in [
        (
            2,
            r#"{"cardId":"card-1","outcome":"allow_always"}"#,
            "allow_always must be refused at the door",
        ),
        (
            3,
            r#"{"cardId":"card-1"}"#,
            "a missing outcome is the caller's mistake",
        ),
        (
            4,
            r#"{"outcome":"deny"}"#,
            "a missing cardId is the caller's mistake",
        ),
    ] {
        let message = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"devboule_answer_permission","arguments":{arguments}}}}}"#
        );
        let response = http_request(&state.mcp.url, Some(&format!("Bearer {token}")), &message);
        let body = response_json(&response);
        assert_eq!(body["error"]["code"], -32602, "{why}: {body}");
        assert!(
            !serde_json::to_string(&body).unwrap().contains("pending"),
            "{why}: the refusal says nothing about any card: {body}"
        );
    }

    drop(guard);
    drop(server);
}

/// C12, on the answer side: a delegated answer audited with its actor
/// session, and a refused answer audited as denied. The rows name the
/// actor, never the card's contents.
#[test]
fn an_answer_through_the_tool_is_audited_with_its_actor() {
    let state = ServerState::new("mcp-answer-audit".to_string());
    let owner = owner("mcp-answer-audit-user", "mcp-answer-client");
    // The state's own store: the registry already holds it, attached at
    // construction, and the OnceLock keeps the first.
    let store = Arc::clone(&state.delegation);
    store.set(true).expect("set on");

    let creator = "s.creator.1".to_string();
    let child = "s.creator.1.child".to_string();
    crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
    state
        .sessions
        .insert_test_child(&child, owner.clone(), &creator);

    let guard = state
        .mcp
        .register(&creator, &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token(&creator).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let runtime = Arc::new(crate::session::SessionRuntime::new());
    runtime.require_mcp();
    state.mcp.bind_runtime(&creator, &runtime);
    state.sessions.test_park_card(&child, "card-audit");

    let message = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_answer_permission","arguments":{"cardId":"card-audit","outcome":"deny"}}}"#;
    let response = http_request(&state.mcp.url, Some(&format!("Bearer {token}")), message);
    let body = response_json(&response);
    assert_eq!(body["result"]["isError"], false, "{body}");

    // The refusal side: with the switch off, the same answer is audited
    // as denied — and the card stays pending for the human.
    store.set(false).expect("set off");
    state.sessions.test_park_card(&child, "card-off");
    let message = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_answer_permission","arguments":{"cardId":"card-off","outcome":"deny"}}}"#;
    let response = http_request(&state.mcp.url, Some(&format!("Bearer {token}")), message);
    let body = response_json(&response);
    assert_eq!(body["result"]["isError"], true, "{body}");

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    let connection =
        rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
    let mut statement = connection
        .prepare("SELECT action, session_id, outcome FROM audit ORDER BY id")
        .expect("prepare");
    let rows: Vec<(String, Option<String>, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query")
        .map(Result::unwrap)
        .collect();
    assert!(
        rows.contains(&(
            "devboule_answer_permission".to_string(),
            Some(creator.clone()),
            "ok".to_string()
        )),
        "the accepted answer names its actor session: {rows:?}"
    );
    assert!(
        rows.contains(&(
            "devboule_answer_permission".to_string(),
            Some(creator.clone()),
            "denied".to_string()
        )),
        "the refused answer is audited as denied: {rows:?}"
    );

    drop(guard);
    drop(server);
}

// -----------------------------------------------------------------------
// Pass A: `devboule_set_agent_profile` — a creator moves its own live
// child onto a ticked profile (slice 5b §2).
// -----------------------------------------------------------------------

/// The move tool is listed with the closed schema it documents, and the
/// arm demands both arguments at the door.
#[test]
fn the_move_tool_is_listed_with_a_closed_schema_and_demands_both_arguments() {
    let state = ServerState::new("mcp-move-schema".to_string());
    let owner = owner("mcp-move-user", "mcp-move-client");
    // The caller is a live local session: the tool door resolves every
    // bearer to its registry row, and a rowless registration is refused.
    crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
    let guard = state
        .mcp
        .register("session", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("session").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let body = response_json(&response);
    let schema = body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL)
        .expect("the move tool is listed")["inputSchema"]
        .clone();
    assert_eq!(
        schema["required"],
        serde_json::json!(["session", "profile"]),
        "both arguments, required: {schema}"
    );
    assert_eq!(schema["additionalProperties"], false, "{schema}");
    assert_eq!(schema["properties"]["session"]["type"], "string");
    assert_eq!(schema["properties"]["profile"]["type"], "string");

    // Missing arguments are the caller's mistake, refused at the door.
    for (id, arguments, why) in [
        (2, r#"{}"#, "neither argument arrived"),
        (
            3,
            r#"{"session":"child-1"}"#,
            "a missing profile is the caller's mistake",
        ),
        (
            4,
            r#"{"profile":"Solo"}"#,
            "a missing session is the caller's mistake",
        ),
    ] {
        let message = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"devboule_set_agent_profile","arguments":{arguments}}}}}"#
        );
        let response = http_request(&state.mcp.url, Some(&format!("Bearer {token}")), &message);
        let body = response_json(&response);
        assert_eq!(body["error"]["code"], -32602, "{why}: {body}");
        assert_eq!(
            body["error"]["message"], "session and profile are required",
            "{why}: {body}"
        );
    }

    drop(guard);
    drop(server);
}

/// §1.2, at the move surface: unknown, ambiguous and unticked are three
/// refusals with three sentences — and the tick is read at the moment of
/// the call, never from a list cached earlier.
#[test]
fn the_move_resolver_distinguishes_unknown_ambiguous_and_unticked_and_reads_now() {
    let store = profile_store(document(
        vec![
            profile(
                "Solo",
                "p-1",
                "claude",
                "default",
                serde_json::json!({}),
                &[],
                true,
            ),
            profile(
                "Ticked off",
                "p-2",
                "claude",
                "default",
                serde_json::json!({}),
                &[],
                false,
            ),
            profile(
                "Dup",
                "p-3",
                "claude",
                "default",
                serde_json::json!({}),
                &[],
                true,
            ),
            profile(
                "Dup",
                "p-4",
                "claude",
                "default",
                serde_json::json!({}),
                &[],
                true,
            ),
        ],
        "",
    ));
    let facts = resolve_profile_for_move(&store, "Solo").expect("ticked");
    assert_eq!(facts.profile_id, "p-1");
    assert_eq!(facts.mode_id, "default");
    assert_eq!(facts.thinking_option_id.as_deref(), Some("high"));

    let error = resolve_profile_for_move(&store, "Ghost").expect_err("unknown");
    assert!(error.contains("unknown profile"), "{error}");

    let error = resolve_profile_for_move(&store, "Ticked off").expect_err("unticked");
    assert!(
        error.contains("has not enabled it for agents"),
        "unticked is its own sentence, not unknown's: {error}"
    );

    let error = resolve_profile_for_move(&store, "Dup").expect_err("ambiguous");
    assert!(error.contains("more than one profile is called"), "{error}");

    // The read-now rule: un-tick Solo and the next ask is refused unticked.
    // A resolver that cached the ticked list would still answer Ok here.
    store
        .set(
            serde_json::from_value(document(
                vec![profile(
                    "Solo",
                    "p-1",
                    "claude",
                    "default",
                    serde_json::json!({}),
                    &[],
                    false,
                )],
                "",
            ))
            .expect("the document"),
        )
        .expect("the un-ticked document is admitted");
    let error = resolve_profile_for_move(&store, "Solo").expect_err("read at the call");
    assert!(error.contains("has not enabled it for agents"), "{error}");
}

/// Pass A's audit: a move through the tool names its actor session, and a
/// refused move is audited as denied — the answer arm's shape, on the move
/// surface.
#[test]
fn a_move_through_the_tool_is_audited_with_its_actor() {
    let state = ServerState::new("mcp-move-audit".to_string());
    let owner = owner("mcp-move-audit-user", "mcp-move-client");
    state
        .agent_profiles
        .set(
            serde_json::from_value(document(
                vec![profile(
                    "Solo",
                    "profile-solo",
                    "claude",
                    "bypassPermissions",
                    serde_json::json!({}),
                    &[],
                    true,
                )],
                "",
            ))
            .expect("the document"),
        )
        .expect("the store admits this document");
    let creator = "s.mover.1".to_string();
    let child = "s.mover.1.child".to_string();
    crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
    state.sessions.insert_test_move_child(
        &child,
        owner.clone(),
        &creator,
        "Worker",
        &["bypassPermissions"],
        Some("model-a"),
        false,
    );

    let guard = state
        .mcp
        .register(&creator, &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token(&creator).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let message = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_set_agent_profile","arguments":{"session":"Worker","profile":"Solo"}}}"#;
    let response = http_request(&state.mcp.url, Some(&format!("Bearer {token}")), message);
    let body = response_json(&response);
    assert_eq!(body["result"]["isError"], false, "{body}");
    assert_eq!(
        body["result"]["structuredContent"]["state"], "moved",
        "{body}"
    );

    // The refusal side: an unknown profile is refused, the child untouched,
    // and the refusal is audited as denied.
    let message = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_set_agent_profile","arguments":{"session":"Worker","profile":"Ghost"}}}"#;
    let response = http_request(&state.mcp.url, Some(&format!("Bearer {token}")), message);
    let body = response_json(&response);
    assert_eq!(body["result"]["isError"], true, "{body}");

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    let connection =
        rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
    let mut statement = connection
        .prepare("SELECT action, session_id, outcome FROM audit ORDER BY id")
        .expect("prepare");
    let rows: Vec<(String, Option<String>, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query")
        .map(Result::unwrap)
        .collect();
    assert!(
        rows.contains(&(
            crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL.to_string(),
            Some(creator.clone()),
            "ok".to_string()
        )),
        "the move names its actor session: {rows:?}"
    );
    assert!(
        rows.contains(&(
            crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL.to_string(),
            Some(creator.clone()),
            "denied".to_string()
        )),
        "the refused move is audited as denied: {rows:?}"
    );

    drop(guard);
    drop(server);
}

// ------------------------------------------------------------------
// P0 — the broker asks where its caller came from, once, at the door.
// ------------------------------------------------------------------

/// A `peers` row the door's capability reads can see.
fn peer_row(device_id: &str, caps: &[&str]) -> crate::journal::PeerRecord {
    crate::journal::PeerRecord {
        device_id: device_id.to_string(),
        display_name: "Peer".to_string(),
        role: "client".to_string(),
        public_key: vec![7u8; 32],
        paired_by_user: None,
        binding_kind: "tailnet".to_string(),
        binding_stable_id: Some("npeer".to_string()),
        binding_node_name: None,
        binding_login_name: None,
        address: "100.64.0.2:47831".to_string(),
        paired_at: 1,
        revoked_at: None,
        caps: caps.iter().map(|cap| cap.to_string()).collect(),
    }
}

/// The door is a no-op for the person at this machine: every served tool
/// passes, and unknown names fall through to the broker's own arm. This is
/// the local case that must not regress — same behaviour, same sentences.
#[test]
fn local_callers_pass_the_door_for_every_tool() {
    let caller = McpCaller::Local;
    for (name, _) in crate::provider_catalog::MCP_BROKER_TOOLS {
        assert!(
            mcp_peer_door(&caller, Some(name), &json!(1)).is_none(),
            "{name}: a local caller is never judged"
        );
    }
    assert!(mcp_peer_door(&caller, Some("devboule_no_such_tool"), &json!(1)).is_none());
    assert!(mcp_peer_door(&caller, None, &json!(1)).is_none());
}

/// The discovery tool answers from this daemon's own rows, scoped to the
/// calling session's own user. Its schema lives in its own
/// `enabled_tool_list` arm, and the assertions below pin the shape that
/// arm claims, so a default-arm change cannot silently reshape it.
#[test]
fn the_devices_tool_answers_scoped_from_this_daemons_rows() {
    let state = ServerState::new("mcp-devices-tool".to_string());
    let owner = owner("S-1-5-21-devtool", "mcp-devices-client");
    crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
    state
        .peer_upsert(crate::journal::PeerRecord {
            device_id: "dev-mine".to_string(),
            display_name: "Work laptop".to_string(),
            role: "daemon".to_string(),
            public_key: vec![7u8; 32],
            paired_by_user: Some(owner.user.clone()),
            binding_kind: "tailnet".to_string(),
            binding_stable_id: Some("nstable".to_string()),
            binding_node_name: None,
            binding_login_name: None,
            address: "100.64.0.2:47831".to_string(),
            paired_at: 1,
            revoked_at: None,
            caps: vec![crate::peer_policy::CAP_VIEW.to_string()],
        })
        .expect("peer row");
    let guard = state
        .mcp
        .register("session", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("session").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let listed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let listed_body = response_json(&listed);
    let tool = listed_body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == crate::provider_catalog::MCP_LIST_DEVICES_TOOL)
        .expect("the devices tool is listed")
        .clone();
    assert_eq!(tool["inputSchema"]["type"], "object");
    assert_eq!(
        tool["inputSchema"]["properties"],
        serde_json::json!({}),
        "the tool takes no arguments, and the schema says so"
    );

    let call = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_list_devices","arguments":{}}}"#,
    );
    let body = response_json(&call);
    assert_eq!(body["result"]["isError"], false, "{body}");
    let devices = body["result"]["structuredContent"]["devices"]
        .as_array()
        .expect("devices array");
    assert_eq!(devices.len(), 1, "{body}");
    assert_eq!(devices[0]["deviceId"], "dev-mine");
    assert_eq!(devices[0]["displayName"], "Work laptop");
    assert_eq!(devices[0]["role"], "daemon");
    assert_eq!(devices[0]["online"], false);

    drop(guard);
    drop(server);
}

/// The one-dial roster tool, over the real broker: its schema demands
/// `deviceId`, a device outside the calling session's own rows refuses
/// by name — absent is never an empty roster — and the refusal carries a
/// sentence, not a debug string.
#[test]
fn the_peer_agents_tool_refuses_an_unknown_device_by_name() {
    let state = ServerState::new("mcp-peer-agents".to_string());
    let owner = owner("S-1-5-21-peeragents", "mcp-peer-agents-client");
    crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
    let guard = state
        .mcp
        .register("session", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("session").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let listed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let listed_body = response_json(&listed);
    let tool = listed_body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL)
        .expect("the peer agents tool is listed")
        .clone();
    assert_eq!(
        tool["inputSchema"]["required"],
        serde_json::json!(["deviceId"])
    );

    let missing = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_list_peer_agents","arguments":{}}}"#,
    );
    assert_eq!(
        response_json(&missing).pointer("/error/message"),
        Some(&json!("deviceId is required"))
    );

    let unknown = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"devboule_list_peer_agents","arguments":{"deviceId":"dev-nowhere"}}}"#,
    );
    let body = response_json(&unknown);
    assert_eq!(body.pointer("/error/code"), Some(&json!(-32602)), "{body}");
    let sentence = body
        .pointer("/error/message")
        .and_then(Value::as_str)
        .expect("a sentence");
    assert!(
        sentence.contains("No paired device named 'dev-nowhere'"),
        "{sentence}"
    );
    assert!(
        sentence.contains("devboule_list_devices"),
        "the sentence names the discovery tool: {sentence}"
    );

    // The refused call names its actor in the audit table: this is the
    // tool that dials other machines, so even its refusals are facts.
    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    let connection =
        rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
    let mut statement = connection
        .prepare("SELECT action, session_id, outcome FROM audit ORDER BY id")
        .expect("prepare");
    let rows: Vec<(String, Option<String>, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query")
        .map(Result::unwrap)
        .collect();
    assert!(
        rows.contains(&(
            crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL.to_string(),
            Some("session".to_string()),
            "denied".to_string()
        )),
        "the refused roster call is audited as denied: {rows:?}"
    );

    drop(guard);
    drop(server);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// A roster call that goes out is audited with its actor: the tool opens
/// an outbound connection to another machine and comes back with that
/// machine's roster, which is exactly what the audit table exists to
/// remember.
#[test]
fn a_roster_call_that_dials_is_audited_with_its_actor() {
    let keypair = snow::Builder::new(
        crate::peer_transport::PEER_NOISE_PATTERN
            .parse()
            .expect("pattern"),
    )
    .generate_keypair()
    .expect("keypair");
    let canned = devboule_protocol::DaemonMessage::PeerAgents {
        id: 0,
        scope: devboule_protocol::PeerRosterScope::PairingUser,
        agents: vec![devboule_protocol::PeerAgent {
            session_id: "s.far.1".to_string(),
            name: "Builder".to_string(),
            provider: Some("claude".to_string()),
            model: None,
            state: devboule_protocol::AgentTaskState::Working,
            depth: 1,
        }],
    };
    let private: [u8; 32] = keypair.private.clone().try_into().expect("32 bytes");
    let address = crate::test_support::spawn_canned_noise_responder(
        private,
        vec![devboule_protocol::Capability::new(
            devboule_protocol::caps::PEER_AGENTS,
        )],
        canned,
    );

    let state = ServerState::new("mcp-peer-agents-audit".to_string());
    let owner = owner("S-1-5-21-peeragents-audit", "mcp-peer-agents-client");
    crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
    state
        .peer_upsert(crate::journal::PeerRecord {
            device_id: "dev-audit".to_string(),
            display_name: "Far daemon".to_string(),
            role: "daemon".to_string(),
            public_key: keypair.public.clone(),
            paired_by_user: Some(owner.user.clone()),
            binding_kind: "tailnet".to_string(),
            binding_stable_id: None,
            binding_node_name: None,
            binding_login_name: None,
            address: address.to_string(),
            paired_at: 1,
            revoked_at: None,
            caps: vec![crate::peer_policy::CAP_ROSTER.to_string()],
        })
        .expect("peer row");
    let guard = state
        .mcp
        .register("session", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("session").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let call = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_peer_agents","arguments":{"deviceId":"dev-audit"}}}"#,
    );
    let body = response_json(&call);
    assert_eq!(body["result"]["isError"], false, "{body}");

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    let connection =
        rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
    let mut statement = connection
        .prepare("SELECT action, session_id, outcome FROM audit ORDER BY id")
        .expect("prepare");
    let rows: Vec<(String, Option<String>, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query")
        .map(Result::unwrap)
        .collect();
    assert!(
        rows.contains(&(
            crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL.to_string(),
            Some("session".to_string()),
            "ok".to_string()
        )),
        "the dialled roster call is audited as ok with its actor: {rows:?}"
    );

    drop(guard);
    drop(server);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// The far side's scope refusal keeps its word in this machine's trail
/// too: a canned `unscoped` answer is audited `unscoped`, never
/// `denied` and never `failed` — "the device declined to scope its
/// roster to me" reads differently from a dead dial.
#[test]
fn a_far_side_scope_refusal_is_audited_as_unscoped() {
    let keypair = snow::Builder::new(
        crate::peer_transport::PEER_NOISE_PATTERN
            .parse()
            .expect("pattern"),
    )
    .generate_keypair()
    .expect("keypair");
    let canned = devboule_protocol::DaemonMessage::PeerAgents {
        id: 0,
        scope: devboule_protocol::PeerRosterScope::Unscoped,
        agents: Vec::new(),
    };
    let private: [u8; 32] = keypair.private.clone().try_into().expect("32 bytes");
    let address = crate::test_support::spawn_canned_noise_responder(
        private,
        vec![devboule_protocol::Capability::new(
            devboule_protocol::caps::PEER_AGENTS,
        )],
        canned,
    );

    let state = ServerState::new("mcp-peer-agents-far-unscoped".to_string());
    let owner = owner("S-1-5-21-peeragents-far-unscoped", "mcp-peer-agents-client");
    crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
    state
        .peer_upsert(crate::journal::PeerRecord {
            device_id: "dev-far".to_string(),
            display_name: "Far daemon".to_string(),
            role: "daemon".to_string(),
            public_key: keypair.public.clone(),
            paired_by_user: Some(owner.user.clone()),
            binding_kind: "tailnet".to_string(),
            binding_stable_id: None,
            binding_node_name: None,
            binding_login_name: None,
            address: address.to_string(),
            paired_at: 1,
            revoked_at: None,
            caps: vec![crate::peer_policy::CAP_ROSTER.to_string()],
        })
        .expect("peer row");
    let guard = state
        .mcp
        .register("session", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("session").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let call = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_peer_agents","arguments":{"deviceId":"dev-far"}}}"#,
    );
    let body = response_json(&call);
    assert_eq!(body.pointer("/error/code"), Some(&json!(-32602)), "{body}");
    assert!(
        body.pointer("/error/message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("cannot scope its roster")),
        "{body}"
    );

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    let connection =
        rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
    let mut statement = connection
        .prepare("SELECT action, session_id, outcome FROM audit ORDER BY id")
        .expect("prepare");
    let rows: Vec<(String, Option<String>, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query")
        .map(Result::unwrap)
        .collect();
    assert!(
        rows.contains(&(
            crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL.to_string(),
            Some("session".to_string()),
            "unscoped".to_string()
        )),
        "the scope refusal is audited as unscoped: {rows:?}"
    );

    drop(guard);
    drop(server);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// A dial that goes nowhere is a failure, not a refusal: the closed
/// loopback port refuses immediately, so the test pays no connect
/// timeout, and the trail says `failed`.
#[test]
fn a_dial_that_goes_nowhere_is_audited_as_failed() {
    let state = ServerState::new("mcp-peer-agents-dial-failed".to_string());
    let owner = owner("S-1-5-21-peeragents-dial-failed", "mcp-peer-agents-client");
    crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
    state
        .peer_upsert(crate::journal::PeerRecord {
            device_id: "dev-asleep".to_string(),
            display_name: "Sleeping daemon".to_string(),
            role: "daemon".to_string(),
            public_key: vec![7u8; 32],
            paired_by_user: Some(owner.user.clone()),
            binding_kind: "tailnet".to_string(),
            binding_stable_id: None,
            binding_node_name: None,
            binding_login_name: None,
            address: "127.0.0.1:1".to_string(),
            paired_at: 1,
            revoked_at: None,
            caps: vec![crate::peer_policy::CAP_ROSTER.to_string()],
        })
        .expect("peer row");
    let guard = state
        .mcp
        .register("session", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("session").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let call = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_peer_agents","arguments":{"deviceId":"dev-asleep"}}}"#,
    );
    let body = response_json(&call);
    assert_eq!(body.pointer("/error/code"), Some(&json!(-32602)), "{body}");

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    let connection =
        rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
    let mut statement = connection
        .prepare("SELECT action, session_id, outcome FROM audit ORDER BY id")
        .expect("prepare");
    let rows: Vec<(String, Option<String>, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query")
        .map(Result::unwrap)
        .collect();
    assert!(
        rows.contains(&(
            crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL.to_string(),
            Some("session".to_string()),
            "failed".to_string()
        )),
        "the dead dial is audited as failed, never denied: {rows:?}"
    );

    drop(guard);
    drop(server);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// A bearer with no readable row is refused, but retryably: an agent's first
/// call can land before its own commit, and a reaped session's in-flight
/// calls outlive its row, and the ecosystem retries exactly this sentence.
/// Still a refusal — never the local person's answer. (This is why the older
/// tests above register their callers as live local sessions first.)
#[test]
fn a_caller_without_a_row_is_refused_as_absent() {
    let state = ServerState::new("mcp-p0-norow".to_string());
    let owner = owner("mcp-p0-norow-user", "mcp-p0-norow-client");
    let guard = state
        .mcp
        .register("ghost", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("ghost").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_agents"}}"#,
    );
    let body = response_json(&response);
    assert_eq!(body.pointer("/error/code"), Some(&json!(-32601)));
    assert_eq!(
        body.pointer("/error/message"),
        Some(&json!("No session with that id.")),
        "absent is a refusal, and a retryable one: {body}"
    );
    drop(guard);
    drop(server);
}

/// A stored `Unknown` origin is not an absence: it never resolves, so the
/// refusal is hard rather than retryable.
#[test]
fn a_caller_with_an_unknown_origin_is_refused_hard() {
    let state = ServerState::new("mcp-p0-unknown".to_string());
    let owner = owner("mcp-p0-unknown-user", "mcp-p0-unknown-client");
    let creator = "s.unknown.1".to_string();
    crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
    state
        .sessions
        .set_test_origin(&creator, SessionOrigin::unknown());
    let guard = state
        .mcp
        .register(&creator, &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token(&creator).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_agents"}}"#,
    );
    let body = response_json(&response);
    assert_eq!(body.pointer("/error/code"), Some(&json!(-32601)));
    assert!(
        body.pointer("/error/message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("origin is unknown")),
        "a stored unknown never renders as the benign one: {body}"
    );
    drop(guard);
    drop(server);
}

/// A peer-origin caller is refused the profile move with the policy's own
/// model sentence — it holds every act-named capability and no `admin`, which
/// is the capability that sentence names — and the child is untouched: the door
/// returns before the move's checks run, so no mode ask lands, no model ask
/// lands, and no profile change is recorded.
#[test]
fn a_peer_caller_is_refused_the_model_half_with_the_policy_sentence() {
    let state = ServerState::new("mcp-p0-peer-model".to_string());
    let owner = owner("mcp-p0-model-user", "mcp-p0-model-client");
    state
        .agent_profiles
        .set(
            serde_json::from_value(document(
                vec![profile(
                    "Solo",
                    "profile-solo",
                    "claude",
                    "bypassPermissions",
                    serde_json::json!({}),
                    &[],
                    true,
                )],
                "",
            ))
            .expect("the document"),
        )
        .expect("the store admits this document");
    let creator = "s.peer.1".to_string();
    let child = "s.peer.1.child".to_string();
    crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
    state.sessions.insert_test_move_child(
        &child,
        owner.clone(),
        &creator,
        "Worker",
        &["bypassPermissions"],
        Some("model-a"),
        false,
    );
    state.sessions.set_test_origin(
        &creator,
        SessionOrigin::peer("device-p0", crate::peer_policy::PeerRole::Client),
    );
    state
        .peer_upsert(peer_row(
            "device-p0",
            &["view", "send", "answer_permissions", "create_sessions"],
        ))
        .expect("store a peer");
    let guard = state
        .mcp
        .register(&creator, &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token(&creator).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_set_agent_profile","arguments":{"session":"Worker","profile":"Solo"}}}"#,
    );
    let body = response_json(&response);
    assert_eq!(body.pointer("/error/code"), Some(&json!(-32601)));
    assert_eq!(
        body.pointer("/error/message"),
        Some(&json!("capability 'admin' was not negotiated")),
        "the policy's own sentence: {body}"
    );
    drop(guard);
    drop(server);
}

/// Without `send` the same call is refused one half earlier, with that
/// half's own sentence: the two refusals must not read the same.
#[test]
fn a_peer_without_send_is_refused_the_mode_half_first() {
    let state = ServerState::new("mcp-p0-peer-mode".to_string());
    let owner = owner("mcp-p0-mode-user", "mcp-p0-mode-client");
    let creator = "s.peer.2".to_string();
    crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
    state.sessions.set_test_origin(
        &creator,
        SessionOrigin::peer("device-p0-mode", crate::peer_policy::PeerRole::Client),
    );
    state
        .peer_upsert(peer_row("device-p0-mode", &["view"]))
        .expect("store a peer");
    let guard = state
        .mcp
        .register(&creator, &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token(&creator).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_set_agent_profile","arguments":{"session":"Worker","profile":"Solo"}}}"#,
    );
    let body = response_json(&response);
    assert_eq!(
        body.pointer("/error/message"),
        Some(&json!("capability 'send' was not negotiated")),
        "the mode half fires first: {body}"
    );
    drop(guard);
    drop(server);
}

/// The audit row for a peer-origin caller names the device, not `"local"`.
#[test]
fn a_peer_denial_is_audited_under_the_device_not_local() {
    let state = ServerState::new("mcp-p0-audit".to_string());
    let owner = owner("mcp-p0-audit-user", "mcp-p0-audit-client");
    let store = Arc::clone(&state.delegation);
    store.set(true).expect("set on");
    let creator = "s.peer.3".to_string();
    let child = "s.peer.3.child".to_string();
    crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
    state
        .sessions
        .insert_test_child(&child, owner.clone(), &creator);
    state.sessions.set_test_origin(
        &creator,
        SessionOrigin::peer("device-p0-audit", crate::peer_policy::PeerRole::Client),
    );
    // No `answer_permissions` on the row: the door refuses.
    state
        .peer_upsert(peer_row("device-p0-audit", &["view"]))
        .expect("store a peer");
    let guard = state
        .mcp
        .register(&creator, &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token(&creator).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let runtime = Arc::new(crate::session::SessionRuntime::new());
    runtime.require_mcp();
    state.mcp.bind_runtime(&creator, &runtime);
    state.sessions.test_park_card(&child, "card-p0");
    let response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_answer_permission","arguments":{"cardId":"card-p0","outcome":"deny"}}}"#,
    );
    let body = response_json(&response);
    assert_eq!(body.pointer("/error/code"), Some(&json!(-32601)));
    assert!(
        body.pointer("/error/message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("answer_permissions")),
        "{body}"
    );
    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    let connection =
        rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
    let mut statement = connection
        .prepare("SELECT device_id, role, action, session_id, outcome FROM audit ORDER BY id")
        .expect("prepare");
    let rows: Vec<(String, String, String, Option<String>, String)> = statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .expect("query")
        .map(Result::unwrap)
        .collect();
    assert!(
        rows.contains(&(
            "device-p0-audit".to_string(),
            "client".to_string(),
            crate::provider_catalog::MCP_ANSWER_PERMISSION_TOOL.to_string(),
            Some(creator.clone()),
            "denied".to_string()
        )),
        "the log names the device, never local: {rows:?}"
    );
    assert!(
        !rows.iter().any(|(_, role, action, _, _)| role == "local"
            && action == crate::provider_catalog::MCP_ANSWER_PERMISSION_TOOL),
        "no peer denial is ever recorded as local: {rows:?}"
    );
    drop(guard);
    drop(server);
}

/// The roster is the `view` act: a peer without it is refused, a peer with
/// it is answered — and past the door the behaviour is the local behaviour.
#[test]
fn a_peer_roster_is_the_view_act() {
    let state = ServerState::new("mcp-p0-roster".to_string());
    let owner = owner("mcp-p0-roster-user", "mcp-p0-roster-client");
    let creator = "s.peer.4".to_string();
    crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
    state.sessions.set_test_origin(
        &creator,
        SessionOrigin::peer("device-p0-roster", crate::peer_policy::PeerRole::Client),
    );
    state
        .peer_upsert(peer_row("device-p0-roster", &[]))
        .expect("store a peer");
    let guard = state
        .mcp
        .register(&creator, &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token(&creator).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let refused = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_agents"}}"#,
    );
    assert_eq!(
        response_json(&refused).pointer("/error/message"),
        Some(&json!("capability 'view' was not negotiated"))
    );
    state
        .peer_upsert(peer_row("device-p0-roster", &["view"]))
        .expect("grant view");
    let allowed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_list_agents"}}"#,
    );
    assert_eq!(response_json(&allowed)["result"]["isError"], false);
    drop(guard);
    drop(server);
}

/// The sender is the `send` act: past a refused door the next check reads
/// exactly what a local caller reads.
#[test]
fn a_peer_send_is_the_send_act() {
    let state = ServerState::new("mcp-p0-send".to_string());
    let owner = owner("mcp-p0-send-user", "mcp-p0-send-client");
    let creator = "s.peer.5".to_string();
    crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
    state.sessions.set_test_origin(
        &creator,
        SessionOrigin::peer("device-p0-send", crate::peer_policy::PeerRole::Client),
    );
    state
        .peer_upsert(peer_row("device-p0-send", &["view"]))
        .expect("store a peer");
    let guard = state
        .mcp
        .register(&creator, &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token(&creator).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let refused = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_send_message","arguments":{"to_agent":"nobody","text":"hi"}}}"#,
    );
    assert_eq!(
        response_json(&refused).pointer("/error/message"),
        Some(&json!("capability 'send' was not negotiated"))
    );
    state
        .peer_upsert(peer_row("device-p0-send", &["view", "send"]))
        .expect("grant send");
    // Past the door, the missing target reads as it does for a local caller.
    let missing = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_send_message","arguments":{"to_agent":"nobody","text":"hi"}}}"#,
    );
    assert_eq!(
        response_json(&missing).pointer("/error/message"),
        Some(&json!("target agent not found"))
    );
    drop(guard);
    drop(server);
}

/// F4 (MAX RECALL, authority): the tool's act is performed AS the caller.
/// The send used to act through an unmarked connection, so a peer's
/// delivery read `local` to the receiving agent (S4-05) and carried the
/// steer-refusal interrupt authority the wire denies every peer (S4-01).
/// The connection now carries the caller's resolved identity: the envelope
/// arrives naming the device, exactly as the wire's own S4-05 test pins.
#[test]
fn a_peer_tool_send_is_attributed_to_the_peer() {
    let state = ServerState::new("mcp-f4-send".to_string());
    let owner = owner("S-1-5-21-f4-user", "mcp-f4-client");
    let creator = "s.peer.7".to_string();
    crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
    state.sessions.set_test_origin(
        &creator,
        SessionOrigin::peer("device-f4-send", crate::peer_policy::PeerRole::Client),
    );
    let received = crate::session::insert_test_live_agent_with_recording_writer(
        &state.sessions,
        "s.f4.target",
        owner.clone(),
        SessionKind::Pi,
    );
    let mut row = peer_row("device-f4-send", &["view", "send"]);
    row.paired_by_user = Some("S-1-5-21-f4-user".to_string());
    state.peer_upsert(row).expect("store a peer");
    let guard = state
        .mcp
        .register(&creator, &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token(&creator).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let reply = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_send_message","arguments":{"to_agent":"s.f4.target","text":"hello from the peer"}}}"#,
    );
    let body = response_json(&reply);
    assert_eq!(
        body.pointer("/result/isError"),
        Some(&json!(false)),
        "a paired client-role device may message its own user's agents: {body}"
    );
    let envelope = String::from_utf8(received.lock().expect("received").clone())
        .expect("the envelope is utf8");
    assert!(
        envelope.contains("origin: peer:device-f4-send"),
        "the delivery must name the device, not this machine: {envelope}"
    );
    assert!(
        envelope.contains("from_agent: s.peer.7"),
        "MCP supplies a local source row, so its sender stays in the local namespace: {envelope}"
    );
    drop(guard);
    drop(server);
}

/// The live MCP door does not reach the new daemon-peer allowance. Its
/// target lookup is scoped by the registration owner; a real daemon peer
/// registers as `peer_<device_id>`, while this daemon's local target has
/// the pairing user's owner. The lookup stops the call before
/// `agent_message_send`, so this is not a test of the target ownership
/// rule. Peer-to-peer sending remains an inbound wire-frame path until a
/// producer is added.
#[test]
fn a_daemon_role_tool_send_stops_at_owner_scoped_target_lookup() {
    let state = ServerState::new("mcp-f4-scope".to_string());
    let peer_owner = owner("peer_device-f4-scope", "daemon");
    let local_owner = owner("S-1-5-21-f4d-user", "mcp-f4d-client");
    let creator = "s.peer.8".to_string();
    crate::session::insert_test_live_agent(&state.sessions, &creator, peer_owner.clone());
    state.sessions.set_test_origin(
        &creator,
        SessionOrigin::peer("device-f4-scope", crate::peer_policy::PeerRole::Daemon),
    );
    let received = crate::session::insert_test_live_agent_with_recording_writer(
        &state.sessions,
        "s.f4d.local",
        local_owner,
        SessionKind::Pi,
    );
    let mut row = peer_row("device-f4-scope", &["view", "send"]);
    row.paired_by_user = Some("S-1-5-21-f4d-user".to_string());
    state.peer_upsert(row).expect("store a peer");
    let guard = state
        .mcp
        .register(&creator, &peer_owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token(&creator).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let reply = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_send_message","arguments":{"to_agent":"s.f4d.local","text":"hello from outside my scope"}}}"#,
    );
    let body = response_json(&reply);
    assert_eq!(
        body.pointer("/error/message"),
        Some(&json!("target agent not found")),
        "the MCP roster is scoped by the peer registration owner: {body}"
    );
    assert!(
        received.lock().expect("received").is_empty(),
        "the target-not-found MCP call writes no local transcript"
    );
    drop(guard);
    drop(server);
}

/// Creation is the `create_sessions` act **plus** the `send` the mandatory
/// initial prompt performs: past the door the profile check reads exactly
/// what a local caller reads, and a device that may create but may not talk
/// is refused with the policy's own sentence before anything is spawned —
/// the live row count proves no child was created-then-refused.
#[test]
fn a_peer_create_is_the_create_act() {
    let state = ServerState::new("mcp-p0-create".to_string());
    let owner = owner("mcp-p0-create-user", "mcp-p0-create-client");
    let creator = "s.peer.6".to_string();
    crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
    state.sessions.set_test_origin(
        &creator,
        SessionOrigin::peer("device-p0-create", crate::peer_policy::PeerRole::Client),
    );
    state
        .peer_upsert(peer_row("device-p0-create", &["view"]))
        .expect("store a peer");
    let guard = state
        .mcp
        .register(&creator, &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token(&creator).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let refused = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_create_agent","arguments":{"profile":"Solo","title":"Kid","initialPrompt":"hi"}}}"#,
    );
    assert_eq!(
        response_json(&refused).pointer("/error/message"),
        Some(&json!("capability 'create_sessions' was not negotiated"))
    );
    state
        .peer_upsert(peer_row("device-p0-create", &["view", "create_sessions"]))
        .expect("grant create");
    // The re-audit's row: `create_sessions` without `send` is refused the
    // whole tool — the initial prompt always sends — with the policy's own
    // sentence, and the refusal lands at the door, before anything is
    // spawned: the roster still holds only the creator.
    let sendless = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_create_agent","arguments":{"profile":"Solo","title":"Kid","initialPrompt":"hi"}}}"#,
    );
    assert_eq!(
        response_json(&sendless).pointer("/error/message"),
        Some(&json!("capability 'send' was not negotiated"))
    );
    assert_eq!(
        state
            .sessions
            .live_agent_entries(&owner)
            .expect("roster")
            .len(),
        1,
        "the refused create spawned nothing"
    );
    state
        .peer_upsert(peer_row(
            "device-p0-create",
            &["view", "create_sessions", "send"],
        ))
        .expect("grant send");
    // Past the door, the profile check reads as it does for a local caller.
    let missing = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"devboule_create_agent","arguments":{"profile":"Solo","title":"Kid","initialPrompt":"hi"}}}"#,
    );
    let body = response_json(&missing);
    assert_eq!(body["result"]["isError"], true);
    assert!(
        body["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("profile")),
        "past the door, the empty store reads as it does for a local caller: {body}"
    );
    drop(guard);
    drop(server);
}

/// F1 (MAX RECALL, authority), the decided composition stated on the
/// consent surface: a peer holding `answer_permissions` may answer its own
/// creation card, so the card names that device. Without the cap — and for
/// every local caller — the card says nothing about answering.
#[test]
fn a_creation_card_names_a_device_that_may_answer_it() {
    let state = ServerState::new("mcp-f1-note".to_string());
    let mut row = peer_row("device-f1-note", &["view", "answer_permissions"]);
    row.display_name = "Phone".to_string();
    row.paired_by_user = Some("S-1-5-21-f1".to_string());
    state.peer_upsert(row).expect("store a peer");
    let holder = McpCaller::Peer {
        device_id: "device-f1-note".to_string(),
        role: crate::peer_policy::PeerRole::Client,
        caps: vec!["view".to_string(), "answer_permissions".to_string()],
    };
    let note = self_answer_note(&state, &holder)
        .expect("a device holding answer_permissions gets the note");
    assert!(note.contains("Phone"), "the note names the device: {note}");
    assert!(
        note.contains("answer_permissions"),
        "the note names the grant: {note}"
    );
    let plain = McpCaller::Peer {
        device_id: "device-f1-note".to_string(),
        role: crate::peer_policy::PeerRole::Client,
        caps: vec!["view".to_string()],
    };
    assert!(
        self_answer_note(&state, &plain).is_none(),
        "no note without the cap"
    );
    assert!(self_answer_note(&state, &McpCaller::Local).is_none());
}

/// The door reads origin, never kind (S9): a pi-kind caller meets exactly the
/// judgment an ACP-kind caller meets. A peer's pi session without `send` is
/// refused the create tool with the policy's sentence and spawns nothing;
/// with `send` it passes the door to the same profile check. A local pi
/// session passes untouched.
#[test]
fn a_peer_pi_caller_meets_the_same_door_as_acp() {
    let state = ServerState::new("mcp-door-pi".to_string());
    let owner = owner("mcp-door-pi-user", "mcp-door-pi-client");
    let creator = "s.peer.8".to_string();
    crate::session::insert_test_live_agent_with_kind(
        &state.sessions,
        &creator,
        owner.clone(),
        SessionKind::Pi,
    );
    state.sessions.set_test_origin(
        &creator,
        SessionOrigin::peer("device-pi-door", crate::peer_policy::PeerRole::Client),
    );
    state
        .peer_upsert(peer_row("device-pi-door", &["view", "create_sessions"]))
        .expect("store a peer");
    let guard = state
        .mcp
        .register_with_provider(
            &creator,
            &owner,
            &SessionKind::Pi,
            Some("pi"),
            AgentLineage::root(),
        )
        .expect("S9 registers pi")
        .expect("a bearer is minted");
    let token = state.mcp.test_token(&creator).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let refused = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_create_agent","arguments":{"profile":"Solo","title":"Kid","initialPrompt":"hi"}}}"#,
    );
    assert_eq!(
        response_json(&refused).pointer("/error/message"),
        Some(&json!("capability 'send' was not negotiated")),
        "a peer's pi session without send is refused like any other kind"
    );
    assert_eq!(
        state
            .sessions
            .live_agent_entries(&owner)
            .expect("roster")
            .len(),
        1,
        "the refused create spawned nothing"
    );
    state
        .peer_upsert(peer_row(
            "device-pi-door",
            &["view", "create_sessions", "send"],
        ))
        .expect("grant send");
    let missing = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_create_agent","arguments":{"profile":"Solo","title":"Kid","initialPrompt":"hi"}}}"#,
    );
    let body = response_json(&missing);
    assert_eq!(body["result"]["isError"], true);
    assert!(
        body["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("profile")),
        "past the door, the profile check reads as for a local caller: {body}"
    );
    drop(guard);
    drop(server);
}

/// The ticked list performs nothing judged: a peer holding nothing reads it.
#[test]
fn a_peer_reads_the_ticked_list_holding_nothing() {
    let state = ServerState::new("mcp-p0-list".to_string());
    let owner = owner("mcp-p0-list-user", "mcp-p0-list-client");
    let creator = "s.peer.7".to_string();
    crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
    state.sessions.set_test_origin(
        &creator,
        SessionOrigin::peer("device-p0-list", crate::peer_policy::PeerRole::Client),
    );
    state
        .peer_upsert(peer_row("device-p0-list", &[]))
        .expect("store a peer");
    let guard = state
        .mcp
        .register(&creator, &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token(&creator).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_profiles"}}"#,
    );
    let body = response_json(&response);
    assert_eq!(body["result"]["isError"], false, "{body}");
    drop(guard);
    drop(server);
}

#[test]
fn broker_tools_list_is_the_readiness_authority() {
    let state = ServerState::new("mcp-readiness".to_string());
    let owner = owner("mcp-readiness-user", "mcp-readiness-client");
    let guard = state
        .mcp
        .register("session", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let runtime = Arc::new(crate::session::SessionRuntime::new());
    runtime.require_mcp();
    state.mcp.bind_runtime("session", &runtime);
    let token = state.mcp.test_token("session").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let waiting = Arc::clone(&runtime);
    let result = std::thread::spawn(move || waiting.wait_for_mcp_ready(Duration::from_secs(1)));
    let response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(result.join().expect("readiness waiter").is_ok());
    drop(server);
    let error = runtime
        .wait_for_mcp_ready(Duration::from_millis(1))
        .expect_err("a stopped broker must revoke readiness");
    assert!(error.message.contains("MCP broker stopped"));
    drop(guard);
}

#[test]
fn the_tool_list_filter_drops_a_named_disabled_tool() {
    let catalog: &[(&str, &str)] = &[
        (crate::provider_catalog::MCP_ROSTER_TOOL, "the roster"),
        ("some_future_tool", "a tool a policy can turn off"),
    ];
    assert_eq!(enabled_tool_list(catalog, None, ToolOverlay::NONE).len(), 2);

    let selective = ToolPolicyEntry {
        provider_id: "claude".to_string(),
        enabled: Some(true),
        disabled_tools: vec!["some_future_tool".to_string()],
    };
    let filtered = enabled_tool_list(catalog, Some(&selective), ToolOverlay::NONE);
    assert_eq!(filtered.len(), 1);
    assert_eq!(
        filtered[0]["name"],
        crate::provider_catalog::MCP_ROSTER_TOOL
    );

    let globally_off = ToolPolicyEntry {
        provider_id: "claude".to_string(),
        enabled: Some(false),
        disabled_tools: Vec::new(),
    };
    let always_on = enabled_tool_list(catalog, Some(&globally_off), ToolOverlay::NONE);
    assert_eq!(
        always_on.len(),
        1,
        "a disabled policy still lists the always-on roster tool"
    );

    let broker_tools = enabled_tool_list(
        crate::provider_catalog::MCP_BROKER_TOOLS,
        None,
        ToolOverlay::NONE,
    );
    let send_tool = broker_tools
        .iter()
        .find(|tool| tool["name"] == crate::provider_catalog::MCP_SEND_MESSAGE_TOOL)
        .expect("message tool");
    assert_eq!(
        send_tool["inputSchema"]["required"],
        serde_json::json!(["to_agent", "text"])
    );
}

#[test]
fn a_disabled_tool_is_refused_at_call_time_and_the_roster_still_answers() {
    let state = ServerState::new("mcp-tool-policy-call".to_string());
    let owner = owner("mcp-policy-user", "mcp-policy-client");
    // The caller is a live local session: the tool door resolves every
    // bearer to its registry row, and a rowless registration is refused.
    crate::session::insert_test_live_agent(&state.sessions, "policy-session", owner.clone());
    let guard = state
        .mcp
        .register_with_provider(
            "policy-session",
            &owner,
            &SessionKind::Acp,
            Some("claude"),
            AgentLineage::root(),
        )
        .expect("registration")
        .expect("MCP guard");
    state
        .tool_policy
        .set("claude", Some(true), vec!["some_future_tool".to_string()])
        .expect("policy");
    let token = state.mcp.test_token("policy-session").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let refused = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"some_future_tool"}}"#,
    );
    let refused_body = response_json(&refused);
    assert_eq!(refused_body.pointer("/error/code"), Some(&json!(-32601)));
    assert_eq!(
        refused_body.pointer("/error/message"),
        Some(&json!("Tool disabled by policy"))
    );

    // The same policy leaves the always-on roster tool working: a toggle
    // that locked the agent out of its own roster would be a footgun.
    let allowed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_list_agents"}}"#,
    );
    assert!(allowed.starts_with("HTTP/1.1 200"));
    assert_eq!(response_json(&allowed)["result"]["isError"], false);

    // A name no policy mentions is still `Unknown tool`: the two refusals
    // mean different things to the agent.
    let unknown = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"no_such_tool"}}"#,
    );
    assert_eq!(
        response_json(&unknown).pointer("/error/message"),
        Some(&json!("Unknown tool"))
    );

    // And `tools/list` for the same session still reports every tool the
    // session is served: the roster, the device list, the peer-agents
    // read, the profile list, the sender, the creation tool, the
    // delegated permission answer, the profile move, the activity read,
    // the stop/close pair, and the project-graph trio. Disabling one does
    // not shrink the other rows, which is the point of this test.
    let listed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#,
    );
    assert_eq!(
        response_json(&listed)
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .map(|tools| tools.len()),
        Some(14)
    );
    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(server);
    drop(guard);
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

#[test]
fn a_globally_disabled_policy_still_lists_the_always_on_tool() {
    let state = ServerState::new("mcp-tool-policy-list".to_string());
    let owner = owner("mcp-policy-list-user", "mcp-policy-list-client");
    let guard = state
        .mcp
        .register_with_provider(
            "silent-session",
            &owner,
            &SessionKind::Acp,
            Some("grok"),
            AgentLineage::root(),
        )
        .expect("registration")
        .expect("MCP guard");
    state
        .tool_policy
        .set(
            "grok",
            Some(false),
            vec![crate::provider_catalog::MCP_ROSTER_TOOL.to_string()],
        )
        .expect("policy");
    let token = state.mcp.test_token("silent-session").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    let response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    assert!(response.starts_with("HTTP/1.1 200"));
    let body = response_json(&response);
    let tools = body
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .expect("tool list");
    assert_eq!(
        tools.len(),
        2,
        "the always-on pair: the roster and the profile list"
    );
    assert_eq!(tools[0]["name"], crate::provider_catalog::MCP_ROSTER_TOOL);
    assert_eq!(
        tools[1]["name"],
        crate::provider_catalog::MCP_LIST_PROFILES_TOOL
    );
    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(server);
    drop(guard);
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// What a `None` provider id means at HEAD, and nothing more.
///
/// [`McpBroker::register`] is a `#[cfg(test)]` seam: both production call
/// sites in `session.rs` — spawn and ACP resume — register through
/// [`McpBroker::register_with_provider`], so this test does not exercise a
/// live production path. It pins one behaviour: a session registered
/// without a provider id has `provider_id: None`, `is_tool_enabled(None,
/// _)` is therefore true for every tool, and the whole catalog is served
/// whatever the store holds — no name is ever refused as `Tool disabled by
/// policy`. The same request against the same stored policy is refused in
/// `the_provider_registered_path_applies_that_policy`; the two differ only
/// in the provider id the session registered with, so together they pin
/// the gate to that id rather than to the policy file alone.
///
/// The `None` id is reachable in production only through the same-user
/// `DEVBOULE_ACP_COMMAND` development override, described on
/// [`McpBroker::register_with_provider`].
#[test]
fn a_registration_without_a_provider_id_consults_no_policy() {
    let state = ServerState::new("mcp-tool-policy-gap".to_string());
    let owner = owner("mcp-policy-gap-user", "mcp-policy-gap-client");
    let guard = state
        .mcp
        .register("unnamed-session", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    // The caller is a live local session: the tool door resolves every
    // bearer to its registry row, and a rowless registration is refused.
    crate::session::insert_test_live_agent(&state.sessions, "unnamed-session", owner.clone());
    state
        .tool_policy
        .set("claude", Some(true), vec!["some_future_tool".to_string()])
        .expect("policy");
    let token = state.mcp.test_token("unnamed-session").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    // A policy that disables `some_future_tool` for claude is on disk and
    // this session has no provider id, so the lookup yields nothing and
    // the name falls through to the unknown-tool answer. "Tool disabled
    // by policy" here would mean the gate had been reached.
    let response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"some_future_tool"}}"#,
    );
    assert_eq!(
        response_json(&response).pointer("/error/message"),
        Some(&json!("Unknown tool")),
        "a registration without a provider id consults no policy"
    );

    // And the broker still lists its whole catalog — measured against
    // the catalog rather than against a literal, so the claim stays
    // "every tool" as the catalog grows.
    let listed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    );
    let listed_body = response_json(&listed);
    assert_eq!(
        listed_body
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .map(|tools| tools.len()),
        Some(crate::provider_catalog::MCP_BROKER_TOOLS.len())
    );

    // The one tool the catalog does serve still answers, so the session
    // is fully served: nothing on this path consults a policy.
    let roster = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"devboule_list_agents"}}"#,
    );
    assert!(roster.starts_with("HTTP/1.1 200"));
    assert_eq!(response_json(&roster)["result"]["isError"], false);
    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(server);
    drop(guard);
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// The inverse of `a_registration_without_a_provider_id_consults_no_policy`:
/// the same request and the same stored policy, refused by the policy
/// because this session did name its provider. Both production call sites
/// in `session.rs` register through [`McpBroker::register_with_provider`],
/// so this is the production path.
#[test]
fn the_provider_registered_path_applies_that_policy() {
    let state = ServerState::new("mcp-tool-policy-gated".to_string());
    let owner = owner("mcp-policy-gated-user", "mcp-policy-gated-client");
    // The caller is a live local session: the tool door resolves every
    // bearer to its registry row, and a rowless registration is refused.
    crate::session::insert_test_live_agent(&state.sessions, "gated-session", owner.clone());
    let guard = state
        .mcp
        .register_with_provider(
            "gated-session",
            &owner,
            &SessionKind::Acp,
            Some("claude"),
            AgentLineage::root(),
        )
        .expect("registration")
        .expect("MCP guard");
    state
        .tool_policy
        .set("claude", Some(true), vec!["some_future_tool".to_string()])
        .expect("policy");
    let token = state.mcp.test_token("gated-session").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"some_future_tool"}}"#,
    );
    assert_eq!(
        response_json(&response).pointer("/error/message"),
        Some(&json!("Tool disabled by policy"))
    );
    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(server);
    drop(guard);
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

#[test]
fn broker_readiness_is_isolated_by_the_authenticated_session() {
    let state = ServerState::new("mcp-readiness-isolation".to_string());
    let first_owner = owner("mcp-readiness-first", "mcp-readiness-first-client");
    let second_owner = owner("mcp-readiness-second", "mcp-readiness-second-client");
    let first_guard = state
        .mcp
        .register("first", &first_owner, &SessionKind::Acp)
        .expect("first registration")
        .expect("first MCP guard");
    let second_guard = state
        .mcp
        .register("second", &second_owner, &SessionKind::Acp)
        .expect("second registration")
        .expect("second MCP guard");
    let first_runtime = Arc::new(crate::session::SessionRuntime::new());
    first_runtime.require_mcp();
    state.mcp.bind_runtime("first", &first_runtime);
    let second_runtime = Arc::new(crate::session::SessionRuntime::new());
    second_runtime.require_mcp();
    state.mcp.bind_runtime("second", &second_runtime);
    let first_token = state.mcp.test_token("first").expect("first token");
    let server = state.mcp.start(&state).expect("MCP server");
    let response = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {first_token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(first_runtime
        .wait_for_mcp_ready(Duration::from_millis(1))
        .is_ok());
    let second_error = second_runtime
        .wait_for_mcp_ready(Duration::from_millis(1))
        .expect_err("the other session must remain unready");
    assert!(second_error.message.contains("first prompt was not sent"));
    drop(server);
    drop(first_guard);
    drop(second_guard);
}

#[test]
fn provider_failure_after_broker_proof_does_not_revoke_readiness() {
    let runtime = crate::session::SessionRuntime::new();
    runtime.require_mcp();
    runtime.mark_mcp_ready();
    runtime.fail_mcp("provider reported a transient MCP failure");
    assert!(runtime.wait_for_mcp_ready(Duration::from_millis(1)).is_ok());
}

#[test]
fn claude_bearer_file_is_removed_with_the_session_guard() {
    let state = ServerState::new("mcp-config".to_string());
    let session_id = "claude-session";
    let agent = owner("mcp-config-user", "mcp-config-client");
    let guard = state
        .mcp
        .register(session_id, &agent, &SessionKind::Claude)
        .expect("registration")
        .expect("MCP guard");
    let config = state.mcp.launch_config(session_id).expect("launch config");
    let path = config.claude_config_path.clone().expect("Claude path");
    assert!(path.exists());
    let contents = fs::read_to_string(&path).expect("config contents");
    assert!(contents.contains("Authorization"));
    drop(guard);
    assert!(!path.exists());
    assert!(state.mcp.test_token(session_id).is_none());
}

#[test]
fn first_prompt_waits_for_ready_and_provider_silence_times_out() {
    let runtime = Arc::new(crate::session::SessionRuntime::new());
    runtime.require_mcp();
    let waiting = Arc::clone(&runtime);
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        tx.send(waiting.wait_for_mcp_ready(Duration::from_secs(1)))
            .expect("wait result");
    });
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
    runtime.mark_mcp_ready();
    assert!(rx
        .recv_timeout(Duration::from_secs(1))
        .expect("ready result")
        .is_ok());

    let silent = crate::session::SessionRuntime::new();
    silent.require_mcp();
    let error = silent
        .wait_for_mcp_ready(Duration::from_millis(1))
        .expect_err("silent provider must not wait forever");
    assert!(error.message.contains("first prompt was not sent"));
}

#[test]
fn session_exit_wakes_an_mcp_readiness_waiter() {
    let runtime = Arc::new(crate::session::SessionRuntime::new());
    runtime.require_mcp();
    let waiting = Arc::clone(&runtime);
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        tx.send(waiting.wait_for_mcp_ready(Duration::from_secs(1)))
            .expect("wait result");
    });
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
    runtime.mark_exited(None);
    let error = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("exit should wake the waiter")
        .expect_err("an exited session cannot become MCP ready");
    assert!(error.message.contains("process exited"));
}

/// The closed schema (`S5` §2) is the first bound: nothing beyond the seven
/// parameters the tool publishes, no way to name a mode, and no way to name a
/// provider or a preset (`create-from-profile`).
#[test]
fn the_creation_schema_refuses_unknown_parameters_and_has_no_mode() {
    let full = json!({
        "title": "  builder  ",
        "profile": "worker",
        "labels": {"ticket": "S5-42"},
        "workspaceId": "workspace-1",
        "cwd": "crates",
        "initialPrompt": "count the tests",
        "notifyOnFinish": false,
    });
    let request = AgentCreateRequest::parse(&full).expect("the schema's own parameters");
    assert_eq!(request.title, "builder", "the name is trimmed");
    assert!(!request.notify);
    assert_eq!(request.cwd.as_deref(), Some("crates"));
    assert_eq!(
        request.labels.get("ticket").map(String::as_str),
        Some("S5-42"),
        "the caller's own labels come through as written"
    );
    let bare = json!({
        "title": "builder",
        "profile": "worker",
        "initialPrompt": "count the tests",
    });
    assert!(
        AgentCreateRequest::parse(&bare)
            .expect("notify defaults")
            .notify
    );
    for (arguments, sentence) in [
        (
            json!({"title": "b", "profile": "worker", "initialPrompt": "x", "mode": "bypass"}),
            "unknown parameter 'mode'",
        ),
        (
            json!({"title": "b", "profile": "worker", "initialPrompt": "x", "depth": 1}),
            "unknown parameter 'depth'",
        ),
        (
            json!({"title": "b", "profile": "worker", "initialPrompt": "x", "bypassMode": true}),
            "unknown parameter 'bypassMode'",
        ),
        (
            json!({"title": "b", "profile": "worker"}),
            "initialPrompt is required",
        ),
        (
            json!({"title": "b", "profile": "worker", "initialPrompt": "   "}),
            "initialPrompt is required",
        ),
        (
            json!({"title": "b".repeat(61).as_str(), "profile": "worker", "initialPrompt": "x"}),
            "display name is 61 characters",
        ),
        (
            json!({"title": "b", "profile": "worker", "initialPrompt": "x", "notifyOnFinish": "yes"}),
            "notifyOnFinish must be a boolean",
        ),
        // Audit S5-09: a wrong type is an invalid-params error, never a
        // silently ignored parameter. A caller that asked for a workspace
        // and got its word ignored would create a child somewhere it did
        // not ask for.
        (
            json!({"title": "b", "profile": "worker", "initialPrompt": "x", "workspaceId": 5}),
            "workspaceId must be a string",
        ),
        (
            json!({"title": "b", "profile": "worker", "initialPrompt": "x", "workspaceId": ["w"]}),
            "workspaceId must be a string",
        ),
        (
            json!({"title": "b", "profile": "worker", "initialPrompt": "x", "cwd": {"path": "crates"}}),
            "cwd must be a string",
        ),
        (
            json!({"title": "b", "profile": "worker", "initialPrompt": "x", "cwd": true}),
            "cwd must be a string",
        ),
        // The two parameters this slice removed are refused like any other
        // name the schema does not publish (`S5` §2, rev 9).
        (
            json!({"title": "b", "profile": "worker", "initialPrompt": "x", "provider": "claude"}),
            "unknown parameter 'provider'",
        ),
        (
            json!({"title": "b", "profile": "worker", "initialPrompt": "x", "preset": "worker"}),
            "unknown parameter 'preset'",
        ),
        (
            json!(["not", "an", "object"]),
            "arguments must be an object",
        ),
    ] {
        let refused = AgentCreateRequest::parse(&arguments).expect_err("refused");
        assert!(
            refused.contains(sentence),
            "{refused:?} should contain {sentence:?}"
        );
    }
    // `null` is absent, not an error: a client that serializes an optional
    // field as null asked for nothing, and gets the creator's own values.
    let explicit_null = json!({
        "title": "b",
        "profile": "worker",
        "initialPrompt": "x",
        "workspaceId": null,
        "cwd": null,
    });
    let parsed = AgentCreateRequest::parse(&explicit_null).expect("null is absent");
    assert!(parsed.workspace_id.is_none());
    assert!(parsed.cwd.is_none());
}

/// Audit S5-08: the fingerprint is what a retry must match, so it covers
/// every field the answer depends on — including the ones that say *where*
/// the child runs and whether the creator wants to hear about it.
///
/// Two payloads that differ in any of them are different creations, and a
/// fingerprint that ignored them would answer the second call with the
/// first child: a session in somebody else's workspace, or a quiet child
/// for a creator that asked to be told.
#[test]
fn a_creation_fingerprint_covers_workspace_cwd_and_notify() {
    let base = [
        "session-alex",
        "builder",
        "builder",
        "count the tests",
        "workspace-1",
        "crates",
        "notify",
        "{}",
    ];
    let request = AgentCreateRequest {
        profile: "builder".to_string(),
        title: "builder".to_string(),
        labels: std::collections::BTreeMap::new(),
        workspace_id: Some("workspace-1".to_string()),
        cwd: Some("crates".to_string()),
        initial_prompt: "count the tests".to_string(),
        notify: true,
    };
    let labels = request.labels_fingerprint();
    let fingerprint = creation_fingerprint(&AgentCreateRequest::creation_fingerprint_fields(
        "session-alex",
        &request,
        "notify",
        &labels,
    ));
    let labels = request.labels_fingerprint();
    let elsewhere = creation_fingerprint(&AgentCreateRequest::creation_fingerprint_fields(
        "session-alex",
        &AgentCreateRequest {
            cwd: Some("elsewhere".to_string()),
            ..request
        },
        "notify",
        &labels,
    ));
    assert_ne!(
        fingerprint, elsewhere,
        "a retry that changed cwd is not the same creation"
    );
    for (fields, what) in [
        (
            [
                "session-alex",
                "builder",
                "builder",
                "count the tests",
                "workspace-1",
                "crates",
                "quiet",
                "{}",
            ],
            "notifyOnFinish",
        ),
        (
            [
                "session-alex",
                "builder",
                "builder",
                "count the tests",
                "workspace-2",
                "crates",
                "notify",
                "{}",
            ],
            "workspaceId",
        ),
        (
            [
                "session-alex",
                "builder",
                "builder",
                "count the tests",
                "workspace-1",
                "src",
                "notify",
                "{}",
            ],
            "cwd",
        ),
    ] {
        let other = creation_fingerprint(&fields);
        assert_ne!(
            fingerprint, other,
            "a retry that changed {what} is not the same creation"
        );
    }
    // The length prefix is what keeps two different payloads from spelling
    // the same string by moving a delimiter.
    assert_ne!(
        creation_fingerprint(&["ab", "c"]),
        creation_fingerprint(&["a", "bc"]),
        "fields must not run into each other"
    );
    assert_eq!(
        creation_fingerprint(&base),
        creation_fingerprint(&base),
        "the same payload always spells the same fingerprint"
    );
}

/// The overlay is the same rule in both places (`S5` §2 and its checklist):
/// hidden at `tools/list`, refused at `tools/call` — and the roster survives
/// both, because naming a session is how an agent reports to a human.
#[test]
fn the_design_overlay_hides_both_tools_from_list_and_call() {
    let listed = enabled_tool_list(
        crate::provider_catalog::MCP_BROKER_TOOLS,
        None,
        ToolOverlay::DESIGN,
    )
    .into_iter()
    .filter_map(|tool| tool["name"].as_str().map(str::to_string))
    .collect::<Vec<_>>();
    assert!(!listed.iter().any(|name| name == MCP_CREATE_AGENT_TOOL));
    assert!(!listed.iter().any(|name| name == MCP_SEND_MESSAGE_TOOL));
    assert!(listed.iter().any(|name| name == MCP_ROSTER_TOOL));
    assert_eq!(
        tool_call_refusal(None, &ToolOverlay::DESIGN, MCP_CREATE_AGENT_TOOL),
        Some("Tool disabled by policy")
    );
    assert_eq!(
        tool_call_refusal(None, &ToolOverlay::DESIGN, MCP_SEND_MESSAGE_TOOL),
        Some("Tool disabled by policy")
    );
    assert_eq!(
        tool_call_refusal(None, &ToolOverlay::DESIGN, MCP_ROSTER_TOOL),
        None
    );
    assert_eq!(
        tool_call_refusal(None, &ToolOverlay::NONE, MCP_CREATE_AGENT_TOOL),
        None
    );
    // A worker has all three: the overlay is what removes them, nothing else.
    let listed = enabled_tool_list(
        crate::provider_catalog::MCP_BROKER_TOOLS,
        None,
        ToolOverlay::NONE,
    );
    assert_eq!(
        listed.len(),
        crate::provider_catalog::MCP_BROKER_TOOLS.len()
    );
}
/// Both enforcement points consult a profile-sourced overlay: the two
/// peer tools hidden at `tools/list` and refused at `tools/call`, the
/// roster kept — while a profile without the tick serves everything. The
/// store is real, but this stops at the resolved overlay: it does not
/// walk the creation road that stamps it onto the child
/// (`overlay: profile.overlay.clone()` at the spawn assembly).
#[test]
fn a_profile_overlay_hides_peer_tools_from_the_child_it_creates() {
    let store = profile_store(document(
        vec![
            profile(
                "hermit",
                "profile-hermit",
                "claude",
                "default",
                serde_json::json!({}),
                &[MCP_SEND_MESSAGE_TOOL, MCP_CREATE_AGENT_TOOL],
                true,
            ),
            profile(
                "social",
                "profile-social",
                "claude",
                "default",
                serde_json::json!({}),
                &[],
                true,
            ),
        ],
        "",
    ));
    let names = |overlay: &ToolOverlay| {
        enabled_tool_list(
            crate::provider_catalog::MCP_BROKER_TOOLS,
            None,
            overlay.clone(),
        )
        .into_iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_string))
        .collect::<Vec<_>>()
    };
    let hermit = resolve_profile(&store, "hermit").expect("ticked");
    let listed = names(&hermit.overlay);
    assert!(!listed.iter().any(|name| name == MCP_CREATE_AGENT_TOOL));
    assert!(!listed.iter().any(|name| name == MCP_SEND_MESSAGE_TOOL));
    assert!(listed.iter().any(|name| name == MCP_ROSTER_TOOL));
    assert_eq!(
        tool_call_refusal(None, &hermit.overlay, MCP_CREATE_AGENT_TOOL),
        Some("Tool disabled by policy")
    );
    assert_eq!(
        tool_call_refusal(None, &hermit.overlay, MCP_SEND_MESSAGE_TOOL),
        Some("Tool disabled by policy")
    );
    assert_eq!(
        tool_call_refusal(None, &hermit.overlay, MCP_ROSTER_TOOL),
        None
    );
    let social = resolve_profile(&store, "social").expect("ticked");
    let listed = names(&social.overlay);
    assert!(listed.iter().any(|name| name == MCP_CREATE_AGENT_TOOL));
    assert!(listed.iter().any(|name| name == MCP_SEND_MESSAGE_TOOL));
    assert_eq!(
        tool_call_refusal(None, &social.overlay, MCP_CREATE_AGENT_TOOL),
        None
    );
}
// -----------------------------------------------------------------------
// `create-from-profile`: resolving a profile, and the sentences a refusal
// uses (`BRIEF-slice-5.md` §2, rev 9).
// -----------------------------------------------------------------------

/// A profile store holding `document`, in a directory of its own.
///
/// The real store, not a fake: the resolution rules are about what the human
/// has saved *at the moment of the call*, and a fake that answered from a map
/// would be a second implementation of the thing under test.
fn profile_store(document: serde_json::Value) -> crate::agent_profiles::AgentProfilesStore {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "devboule broker profiles {}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let store = crate::agent_profiles::AgentProfilesStore::load(&dir);
    store
        .set(serde_json::from_value(document).expect("a profile document"))
        .expect("the store admits this document");
    store
}

/// One profile, with every field a test wants to choose.
fn profile(
    name: &str,
    id: &str,
    provider: &str,
    mode: &str,
    features: serde_json::Value,
    overlay: &[&str],
    enabled: bool,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "note": "when to use this one",
        "provider": provider,
        "model": "the model the human saved",
        "modeId": mode,
        "thinkingOptionId": "high",
        "features": features,
        "toolOverlay": overlay,
        "enabledForAgents": enabled,
    })
}

fn document(profiles: Vec<serde_json::Value>, standing: &str) -> serde_json::Value {
    serde_json::json!({ "profiles": profiles, "standingInstructions": standing })
}

/// The card's three auto-accept wordings, one per arm of the tri-state:
/// the affirmative names the mode that answers, the negative names the
/// mode that asks, and the unknown asserts neither direction.
#[test]
fn the_card_auto_accept_line_speaks_all_three_answers() {
    use devboule_protocol::UnattendedState;
    assert_eq!(
        auto_accept_line(UnattendedState::Yes, "auto_accept"),
        "Yes (mode auto_accept)"
    );
    assert_eq!(
        auto_accept_line(UnattendedState::No, "default"),
        "No — mode default asks the human"
    );
    assert_eq!(
            auto_accept_line(UnattendedState::Unknown, "default"),
            "Cannot establish — mode default belongs to the agent's own vocabulary, so whether it asks is not something Devboule can check",
        );
}

/// No profile enabled at all: every creation is refused, and the refusal
/// names **no** profile — an agent must not learn what exists but is
/// forbidden.
#[test]
fn no_enabled_profile_refuses_and_names_none() {
    let store = profile_store(document(
        vec![profile(
            "design",
            "profile-design",
            "claude",
            "default",
            serde_json::json!({}),
            &[],
            false,
        )],
        "",
    ));
    let refusal = resolve_profile(&store, "design").expect_err("nothing is enabled");
    assert_eq!(refusal, "no profile is enabled for agents");
    assert!(
        !refusal.contains("design") && !refusal.contains("profile-design"),
        "the refusal names no profile: {refusal}"
    );
}

/// An unknown name and an un-ticked one are refused the same way, and neither
/// refusal tells the caller what it is not allowed to name.
#[test]
fn an_unknown_or_unticked_profile_is_refused_with_the_list_sentence() {
    let store = profile_store(document(
        vec![
            profile(
                "worker",
                "profile-worker",
                "claude",
                "default",
                serde_json::json!({}),
                &[],
                true,
            ),
            profile(
                "design",
                "profile-design",
                "claude",
                "default",
                serde_json::json!({}),
                &[],
                false,
            ),
        ],
        "",
    ));
    assert!(resolve_profile(&store, "worker").is_ok());
    for name in ["nobody", "design"] {
        let refusal = resolve_profile(&store, name).expect_err("refused");
        assert_eq!(refusal, "unknown profile; call devboule_list_profiles");
        assert!(
            !refusal.contains(name),
            "the sentence does not echo what was asked for: {refusal}"
        );
    }
}

/// Two ticked profiles may share a name (the id is the identity), so a name
/// that resolves to both is refused rather than answered with the first: the
/// first would be a provider the human did not name.
#[test]
fn one_name_on_two_enabled_profiles_is_refused() {
    let twin = |id: &str, provider: &str, enabled: bool| {
        profile(
            "worker",
            id,
            provider,
            "default",
            serde_json::json!({}),
            &[],
            enabled,
        )
    };
    let two = profile_store(document(
        vec![
            twin("profile-a", "claude", true),
            twin("profile-b", "grok", true),
        ],
        "",
    ));
    assert_eq!(
        resolve_profile(&two, "worker").expect_err("two of them"),
        "more than one profile is called worker"
    );

    // One ticked and one not is one profile: the un-ticked twin is not a
    // candidate at all, so the name resolves to the ticked one.
    let one = profile_store(document(
        vec![
            twin("profile-a", "claude", true),
            twin("profile-b", "grok", false),
        ],
        "",
    ));
    let resolved = resolve_profile(&one, "worker").expect("one ticked twin");
    assert_eq!(resolved.id, "profile-a");
}

/// The ticked list is read **at the moment of the call**, never cached: a
/// profile un-ticked between a `devboule_list_profiles` and the creation is
/// not enabled when the creation happens, and one ticked in between is.
#[test]
fn the_ticked_list_is_read_at_the_moment_of_the_call() {
    // Two profiles, so that un-ticking the one a creation names is answered
    // by the *name* rule and not by the empty-list rule below.
    let naming = |worker: bool, design: bool| {
        document(
            vec![
                profile(
                    "worker",
                    "profile-worker",
                    "claude",
                    "default",
                    serde_json::json!({}),
                    &[],
                    worker,
                ),
                profile(
                    "design",
                    "profile-design",
                    "claude",
                    "default",
                    serde_json::json!({}),
                    &[],
                    design,
                ),
            ],
            "",
        )
    };
    let store = profile_store(naming(true, true));
    // What an agent would have read a moment ago.
    let listed = list_profiles(&store, &json!(1));
    let listed = listed["result"]["structuredContent"]["profiles"].clone();
    assert_eq!(listed[0]["name"], "worker");

    // The human un-ticks the one this creation names.
    store
        .set(serde_json::from_value(naming(false, true)).expect("document"))
        .expect("store");
    assert_eq!(
        resolve_profile(&store, "worker").expect_err("un-ticked"),
        "unknown profile; call devboule_list_profiles"
    );
    assert!(
        resolve_profile(&store, "design").is_ok(),
        "the profile it did not name is still creatable"
    );

    // Un-ticking everything instead refuses every creation, and the refusal
    // names no profile at all.
    store
        .set(serde_json::from_value(naming(false, false)).expect("document"))
        .expect("store");
    assert_eq!(
        resolve_profile(&store, "worker").expect_err("nothing ticked"),
        "no profile is enabled for agents"
    );

    // And ticking it again is enough for the next call: nothing was cached
    // from the list above, in either direction.
    store
        .set(serde_json::from_value(naming(true, true)).expect("document"))
        .expect("store");
    assert!(resolve_profile(&store, "worker").is_ok());
}

/// What the creation runs is exactly what the human saved, with no
/// substitution in either direction — including a mode no preset table would
/// ever have named.
#[test]
fn a_profile_resolves_to_exactly_what_was_saved() {
    let store = profile_store(document(
        vec![profile(
            "runner",
            "profile-runner",
            "grok",
            "bypass",
            serde_json::json!({"autoAccept": true, "sandbox": "none"}),
            &["devboule_send_message"],
            true,
        )],
        "",
    ));
    let resolved = resolve_profile(&store, "runner").expect("ticked");
    assert_eq!(resolved.id, "profile-runner");
    assert_eq!(resolved.name, "runner");
    assert_eq!(resolved.provider, "grok");
    assert_eq!(resolved.model, "the model the human saved");
    assert_eq!(resolved.mode, "bypass");
    assert_eq!(resolved.thinking_option_id.as_deref(), Some("high"));
    assert_eq!(
        resolved.features.get("sandbox"),
        Some(&serde_json::json!("none"))
    );
    assert!(!resolved.overlay.allows("devboule_send_message"));
    assert!(resolved.overlay.allows("devboule_list_agents"));
    // The marker is no longer a field on the resolution: the birth derives
    // it from the delivery. The same prediction the list serves says what
    // this profile's mode would earn.
    assert_eq!(
        predicted_unattended(&resolved.provider, &resolved.mode),
        devboule_protocol::UnattendedState::Yes,
        "the mode auto-answers permission prompts"
    );
}

/// A retry of a creation that **committed** is answered by the idempotency
/// store even when the profile it named is gone: the human renamed or
/// un-ticked it inside the retry window, and the child from the first
/// attempt is already alive. Refusing the retry would tell the creator its
/// creation failed, and the re-issue it would then spend a second live slot
/// on a child it already has — which is why the store is consulted before
/// the profile is resolved.
///
/// The other half, unchanged by that ordering: a **new** call — a different
/// frame id, no remembered answer — naming the same stale name is still
/// refused, with the same sentence the store's state earns.
#[test]
fn a_retry_is_answered_even_when_its_profile_is_gone() {
    let state = ServerState::new("mcp-retry-stale-profile".to_string());
    let owner = owner("mcp-retry-user", "mcp-retry-client");
    // The profile was ticked when the first attempt ran; it has since been
    // un-ticked. (A rename reads the same way here: the name no longer
    // resolves, and the refusal sentence is resolve_profile's to choose.)
    state
        .agent_profiles
        .set(
            serde_json::from_value(document(
                vec![profile(
                    "worker",
                    "profile-worker",
                    "claude",
                    "default",
                    serde_json::json!({}),
                    &[],
                    false,
                )],
                "",
            ))
            .expect("the document"),
        )
        .expect("the store admits this document");

    let registration = RegisteredSession {
        session_id: "s.creator".to_string(),
        owner: owner.clone(),
        provider_id: None,
        depth: 0,
        overlay: crate::provider_catalog::ToolOverlay::NONE,
        bearer: "the bearer".to_string(),
        claude_config_path: None,
        runtime: None,
        broker_ready: Arc::new(AtomicBool::new(false)),
    };
    let arguments = json!({
        "profile": "worker",
        "title": "child",
        "initialPrompt": "report your result",
    });
    let id = serde_json::json!(7);

    // The first attempt's answer, remembered under this frame's key while
    // the profile was still ticked: the same key and the same fingerprint
    // the handler itself computes for the re-sent frame.
    let creator_id = registration.session_id.clone();
    let request = AgentCreateRequest::parse(&arguments).expect("the request parses");
    let retry_key = crate::server::creation_retry_key(&creator_id, &id).expect("retry key");
    let notify_field = if request.notify { "notify" } else { "quiet" };
    let labels_field = request.labels_fingerprint();
    let fingerprint = creation_fingerprint(&AgentCreateRequest::creation_fingerprint_fields(
        &creator_id,
        &request,
        notify_field,
        &labels_field,
    ));
    let first_child = crate::journal::new_session_record(
        "s.child.1",
        "mcp-retry-user",
        None,
        SessionKind::Acp,
        "child",
    )
    .to_session();
    crate::server::remember_creation_session(
        &state,
        &owner,
        &retry_key,
        &fingerprint,
        &first_child,
    );

    // The re-sent frame: same id, same payload, stale profile. It is
    // answered with the first child, and creates nothing.
    let answer = create_agent(
        &state,
        &state.mcp,
        &McpCaller::Local,
        &registration,
        &id,
        request,
    );
    assert_eq!(
        answer["result"]["structuredContent"]["sessionId"], "s.child.1",
        "the retry answers the first call's session: {answer}"
    );

    // A genuinely new call naming the stale profile is still refused, with
    // the sentence the empty ticked list earns.
    let new_id = serde_json::json!(8);
    let new_request = AgentCreateRequest::parse(&arguments).expect("the request parses");
    let refusal = create_agent(
        &state,
        &state.mcp,
        &McpCaller::Local,
        &registration,
        &new_id,
        new_request,
    );
    assert_eq!(
        refusal["result"]["content"][0]["text"], "no profile is enabled for agents",
        "a new call still meets the profile check: {refusal}"
    );
    assert_eq!(
        refusal["result"]["isError"], true,
        "the refusal is an error result, not a session: {refusal}"
    );
}

/// The labels a caller may write, and the ones it may not.
#[test]
fn a_caller_cannot_write_a_reserved_label_and_may_write_its_own() {
    let parsed = parse_labels(
        &json!({"labels": {"ticket": "S5", "note": "a sentence"}})
            .as_object()
            .expect("object")
            .clone(),
    )
    .expect("free labels");
    assert_eq!(parsed.get("ticket").map(String::as_str), Some("S5"));
    assert_eq!(parsed.get("note").map(String::as_str), Some("a sentence"));

    for key in [
        "devboule.created-by",
        "devboule.depth",
        "devboule.origin",
        "devboule.profile",
        "devboule.",
    ] {
        let refusal = parse_labels(
            &json!({"labels": {key: "mine"}})
                .as_object()
                .expect("object")
                .clone(),
        )
        .expect_err("reserved");
        assert_eq!(refusal, "reserved label prefix", "{key}");
    }

    // Absent is empty, and a value that is not a string is refused by name.
    assert!(
        parse_labels(&json!({}).as_object().expect("object").clone())
            .expect("no labels")
            .is_empty()
    );
    assert_eq!(
        parse_labels(
            &json!({"labels": {"ticket": 5}})
                .as_object()
                .expect("object")
                .clone()
        )
        .expect_err("not a string"),
        "the label 'ticket' must be a string"
    );
}

/// The four facts the daemon stamps into every child, from its own bookkeeping
/// and never from the request.
#[test]
fn the_daemon_stamps_its_four_labels_into_the_callers_map() {
    let store = profile_store(document(
        vec![profile(
            "runner",
            "profile-runner",
            "grok",
            "default",
            serde_json::json!({}),
            &[],
            true,
        )],
        "",
    ));
    let profile = resolve_profile(&store, "runner").expect("ticked");
    let mut caller = std::collections::BTreeMap::new();
    caller.insert("ticket".to_string(), "S5".to_string());
    let labels = stamped_labels(
        &caller,
        "s.parent.1",
        &profile,
        2,
        &devboule_protocol::SessionOrigin::peer(
            "device-phone",
            devboule_protocol::PeerRole::Client,
        ),
    );
    assert_eq!(labels.get("ticket").map(String::as_str), Some("S5"));
    assert_eq!(
        labels.get("devboule.created-by").map(String::as_str),
        Some("s.parent.1")
    );
    assert_eq!(labels.get("devboule.depth").map(String::as_str), Some("2"));
    assert_eq!(
        labels.get("devboule.origin").map(String::as_str),
        Some("peer:device-phone")
    );
    assert_eq!(
        labels.get("devboule.profile").map(String::as_str),
        Some("profile-runner"),
        "the stamp is the profile's stable id, like the session's own field"
    );
}

/// The A2A result names the task and the context (`S5` §2, decision 8b), with
/// no bookkeeping of the caller's own.
#[test]
fn a_creation_result_carries_the_task_id_and_the_context() {
    let session = devboule_protocol::Session {
        id: "s.parent.2".to_string(),
        workspace_id: None,
        cwd: None,
        kind: devboule_protocol::SessionKind::Acp,
        title: "Agent".to_string(),
        provider: Some("grok".to_string()),
        peer_session_id: None,
        state: devboule_protocol::SessionState::Live { generation: 1 },
        elapsed_ms: Some(0),
        created_at_ms: 1,
        origin: devboule_protocol::SessionOrigin::local(),
        display_name: Some("builder".to_string()),
        created_by: Some("s.parent.1".to_string()),
        profile_id: Some("profile-worker".to_string()),
        context_id: Some("s.parent.1".to_string()),
        unattended: devboule_protocol::UnattendedState::No,
        labels: Default::default(),
        resumable: false,
    };
    let result = created_result(&json!(7), &session, true);
    let content = &result["result"]["structuredContent"];
    assert_eq!(content["sessionId"], "s.parent.2");
    assert_eq!(
        content["taskId"], content["sessionId"],
        "a child is the task; there is no second id to keep"
    );
    assert_eq!(
        content["contextId"], "s.parent.1",
        "the context is the creator's, not a fresh one"
    );
    assert_eq!(content["displayName"], "builder");
    assert_eq!(content["state"], "submitted");
}

/// `devboule_list_profiles` serves the ticked profiles, in the human's order,
/// with the note verbatim — and nothing else.
#[test]
fn the_profile_list_is_the_humans_order_with_verbatim_notes() {
    let long_note = "a".repeat(2000);
    let store = profile_store(document(
        vec![
            profile(
                "second",
                "profile-2",
                "claude",
                "default",
                serde_json::json!({}),
                &[],
                true,
            ),
            profile(
                "first",
                "profile-1",
                "grok",
                "bypass",
                serde_json::json!({"autoAccept": true}),
                &[],
                true,
            ),
            profile(
                "hidden",
                "profile-3",
                "codex",
                "default",
                serde_json::json!({}),
                &[],
                false,
            ),
        ],
        "the standing instructions are not a profile's business to read",
    ));
    let mut listed = list_profiles(&store, &json!(1));
    let profiles = listed["result"]["structuredContent"]["profiles"]
        .as_array_mut()
        .expect("an array of profiles")
        .clone();
    assert_eq!(profiles.len(), 2, "only the ticked ones");
    assert_eq!(profiles[0]["name"], "second");
    assert_eq!(
        profiles[1]["name"], "first",
        "the human's order, never sorted"
    );
    assert_eq!(profiles[0]["note"], "when to use this one");
    assert_eq!(profiles[0]["provider"], "claude");
    assert_eq!(profiles[0]["model"], "the model the human saved");
    assert_eq!(profiles[0]["mode"], "default");
    assert_eq!(profiles[0]["unattended"], "no");
    assert_eq!(
        profiles[1]["unattended"], "yes",
        "the mode auto-answers, so creating from it yields a session that will not ask"
    );
    assert_eq!(
        profiles[0]["provider"], "claude",
        "fixture sanity: the first profile is the daemon-authored asking mode"
    );
    assert!(
        !listed["result"]["structuredContent"]
            .to_string()
            .contains("standing"),
        "the standing instructions are not served to a caller"
    );
    assert!(
        !listed["result"]["structuredContent"]
            .to_string()
            .contains("profile-1"),
        "the id is the daemon's key for the session it records, not the caller's to name"
    );

    // A note is never truncated: the only thing a model routes work with.
    let store = profile_store(document(
        vec![profile(
            "long",
            "profile-long",
            "claude",
            "default",
            serde_json::json!({}),
            &[],
            true,
        )],
        "",
    ));
    let mut document = store.document();
    document.profiles[0].note = long_note.clone();
    store.set(document).expect("store");
    let listed = list_profiles(&store, &json!(1));
    assert_eq!(
        listed["result"]["structuredContent"]["profiles"][0]["note"],
        serde_json::json!(long_note),
        "the note arrives as the human wrote it, whole"
    );
}
