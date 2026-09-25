use super::*;
use devboule_protocol::{ClientMessage, OwnerId, PermissionOutcome, RetentionPatch};

use crate::journal::new_session_record;
use crate::peer_policy::{TransportBinding, CAP_ADMIN};

fn state() -> Arc<ServerState> {
    ServerState::new("test-instance".to_string())
}

/// A peer's creation is a session on that peer's device, so the device's own
/// `create_sessions` grant has to still hold (`S5` §3 and the §5 checklist).
/// The fail-closed half is what this pins: no row, no grant.
#[test]
fn a_peer_creator_without_create_sessions_is_refused_before_anything_is_created() {
    let (path, state) = temp_state("agent-create-peer-gate");
    let creator = |origin: devboule_protocol::SessionOrigin| crate::session::AgentCreator {
        owner: OwnerId::new("alex", "app").expect("owner"),
        origin,
        workspace_id: None,
        display_name: None,
        title: "Agent".to_string(),
        // The context a child of this creator would inherit (its own id,
        // since this fixture is a root session).
        context_id: "s.parent".to_string(),
    };
    // This machine's own person: the daemon is their daemon.
    assert!(creator(devboule_protocol::SessionOrigin::local()).may_create_sessions(&state));
    // A device with no row — unknown, unpaired or revoked — holds nothing.
    assert!(!creator(devboule_protocol::SessionOrigin::peer(
        "device-phone",
        devboule_protocol::PeerRole::Client
    ))
    .may_create_sessions(&state));
    // An origin the daemon cannot read is not a licence either.
    assert!(!creator(devboule_protocol::SessionOrigin::unknown()).may_create_sessions(&state));
    let _ = std::fs::remove_dir_all(&path);
}

#[test]
fn a_creation_retry_answers_the_first_session_and_a_changed_payload_is_a_conflict() {
    let (path, state) = temp_state("agent-create-idempotency");
    let owner = OwnerId::new("alex", "app").expect("owner");
    let session = crate::journal::new_session_record(
        "child-1",
        "alex",
        None,
        SessionKind::Terminal,
        "worker",
    )
    .to_session();
    let key = creation_retry_key("session-parent", &serde_json::json!(7)).expect("key");
    assert!(
        idempotent_creation_session(&state, &owner, &key, "fp").is_none(),
        "the first call is a miss"
    );
    remember_creation_session(&state, &owner, &key, "fp", &session);
    assert_eq!(
        idempotent_creation_session(&state, &owner, &key, "fp").map(|session| session.id),
        Some(session.id.clone()),
        "a retry gets the first session back"
    );
    // Same key, different payload: a conflict, not a retry. Answering it
    // with the first session would be a lie about what was created.
    assert!(idempotent_creation_session(&state, &owner, &key, "other").is_none());
    // A different frame id is a different call.
    let other = creation_retry_key("session-parent", &serde_json::json!(8)).expect("key");
    assert!(idempotent_creation_session(&state, &owner, &other, "fp").is_none());
    // An id that cannot be spelled as an idempotency key has no retry
    // identity at all, and one notification (no id) has none either.
    assert!(creation_retry_key("session-parent", &serde_json::json!("has space")).is_none());
    assert!(creation_retry_key("session-parent", &serde_json::json!({"nested": true})).is_none());
    let _ = std::fs::remove_dir_all(&path);
}

/// A state with a runtime dir whose `journal.db` the test can also open
/// directly, for asserting what the daemon wrote to `audit`/`peers`.
fn temp_state(tag: &str) -> (std::path::PathBuf, Arc<ServerState>) {
    let path = crate::test_dirs::test_temp_dir(&format!("devboule-{tag}"));
    let state = ServerState::with_paths(
        "test-instance".to_string(),
        RuntimePaths::from_dir(path.clone()),
    )
    .expect("state");
    (path, state)
}

/// A user-declared row is a provider the daemon can spawn, so the catalogue
/// list must answer it. Seeds `providers.json`, holds the rows lock across
/// the whole read (the global-registry discipline), and asks both surfaces:
/// the vocabulary already answers the row `absent` (free-text — correct for
/// an ACP row), while `ProvidersList` omits it. That disagreement is the gap.
#[test]
fn providers_list_answers_a_live_user_row() {
    let (path, state) = temp_state("user-rows-visible");
    std::fs::create_dir_all(&path).expect("runtime dir");
    std::fs::write(
        path.join("providers.json"),
        r#"{"fieldtest-grok": {"extends": "acp", "command": ["/usr/local/bin/fieldtest-grok", "--chat"]}}"#,
    )
    .expect("seed user row");
    let mut gate = crate::user_providers::lock_rows_state();
    crate::user_providers::refresh_user_rows_with(&mut gate, &path);
    assert!(
        crate::session::catalog_registry()
            .user_row_for("fieldtest-grok")
            .is_some(),
        "setup: the row is live"
    );
    // Second gate first: the vocabulary already sees the row, as `absent`.
    let vocab = crate::provider_vocabulary::provider_vocabulary_reply(
        &state,
        2,
        "fieldtest-grok",
        None,
        false,
    );
    assert!(
        matches!(
            &vocab,
            devboule_protocol::DaemonMessage::ProviderVocabulary { provider, models, .. }
            if provider == "fieldtest-grok" && models.state == devboule_protocol::VocabularyState::Absent
        ),
        "vocabulary answers the live row absent: {vocab:?}"
    );
    // The list does not — red.
    let reply = super::providers::providers_reply(&state, 1, false);
    let providers = match reply {
        DaemonMessage::Providers { providers, .. } => providers,
        other => panic!("ProvidersList must answer Providers, got {other:?}"),
    };
    let row = providers
        .iter()
        .find(|provider| provider.id == "fieldtest-grok")
        .unwrap_or_else(|| {
            let ids: Vec<&str> = providers
                .iter()
                .map(|provider| provider.id.as_str())
                .collect();
            panic!(
                "the catalogue list must answer the live user row \"fieldtest-grok\", got {ids:?}"
            )
        });
    assert_eq!(
        row.protocol.as_deref(),
        Some("acp"),
        "a user row is an ACP row"
    );
    assert!(row.installed, "a live declaration reads as installed");
    assert!(
        row.acp_available,
        "a live declaration reads as ACP-available"
    );
    assert_ne!(
        row.origin.as_deref(),
        Some("npx-wrapper"),
        "a local declaration never asks for npx consent"
    );
    crate::session::apply_user_rows(std::collections::BTreeMap::new());
    drop(gate);
    drop(state);
    let _ = std::fs::remove_dir_all(&path);
}

fn remote_conn(role: PeerRole, paired_by_user: Option<&str>) -> Arc<ConnHandle> {
    remote_conn_with_caps(role, paired_by_user, &[])
}

/// The same connection, holding the capability set `caps` names: what the
/// gate actually reads (`DESIGN-remote-agents.md` §8b A9/A11).
fn remote_conn_with_caps(
    role: PeerRole,
    paired_by_user: Option<&str>,
    caps: &[&str],
) -> Arc<ConnHandle> {
    ConnHandle::with_peer_caps(
        7,
        None,
        Some(ConnPeer::Remote {
            device_id: "dev-peer-1".to_string(),
            role,
            paired_by_user: paired_by_user.map(str::to_string),
            binding: TransportBinding::tailnet(
                "nstable",
                "host.tailnet.ts.net.",
                "user@example.com",
            ),
        }),
        caps.iter().map(|cap| cap.to_string()).collect(),
        QuitIntent::default(),
    )
}

/// Every file under `dir`, recursively. A missing `dir` is zero files.
fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

/// Audit rows as `action:outcome`, in insertion order.
fn audit_rows(path: &std::path::Path) -> Vec<String> {
    let connection = rusqlite::Connection::open(path.join("journal.db")).expect("journal");
    let mut statement = connection
        .prepare("SELECT action, outcome FROM audit ORDER BY id")
        .expect("prepare");
    statement
        .query_map([], |row| {
            Ok(format!(
                "{}:{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?
            ))
        })
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows")
}

/// The session each audit row names, in insertion order. `None` is a row
/// whose frame names no session, or one the daemon could not read a session
/// out of (`request_session_id`).
fn audit_sessions(path: &std::path::Path) -> Vec<Option<String>> {
    let connection = rusqlite::Connection::open(path.join("journal.db")).expect("journal");
    let mut statement = connection
        .prepare("SELECT session_id FROM audit ORDER BY id")
        .expect("prepare");
    statement
        .query_map([], |row| row.get::<_, Option<String>>(0))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows")
}

/// The actor each audit row names, in insertion order: `(device_id, role)`.
fn audit_actors(path: &std::path::Path) -> Vec<(String, String)> {
    let connection = rusqlite::Connection::open(path.join("journal.db")).expect("journal");
    let mut statement = connection
        .prepare("SELECT device_id, role FROM audit ORDER BY id")
        .expect("prepare");
    statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows")
}

fn wire_attachment(name: &str, bytes: &[u8]) -> PromptAttachment {
    use base64::Engine;
    PromptAttachment {
        name: name.to_string(),
        mime_type: "image/png".to_string(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

/// One stored-attachment reference, shaped like the deposit reply's: a
/// session, a 64-character lowercase-hex digest and a size. The digest is
/// built from a seed rather than hashed, because these tests are about what
/// the fingerprint does with the string and not about where it came from.
fn stored_reference(session_id: &str, seed: char) -> AttachmentReference {
    AttachmentReference {
        session_id: session_id.to_string(),
        digest: seed.to_string().repeat(64),
        stored_bytes: 1024,
    }
}

#[test]
fn one_text_with_two_images_is_two_fingerprints() {
    // The defect this closes: with the text alone in the fingerprint, the
    // second send comes back as an idempotent replay of the first, so the
    // user swaps the picture, presses Generate, and gets the old answer.
    let first = send_fingerprint(
        "s.a.1",
        "draw this",
        &[wire_attachment("a.png", b"one")],
        &[],
        None,
    );
    let second = send_fingerprint(
        "s.a.1",
        "draw this",
        &[wire_attachment("a.png", b"two")],
        &[],
        None,
    );
    assert_ne!(first, second);
}

#[test]
fn the_same_text_and_images_keep_one_fingerprint() {
    let attachments = [
        wire_attachment("a.png", b"one"),
        wire_attachment("b.png", b"two"),
    ];
    assert_eq!(
        send_fingerprint("s.a.1", "draw this", &attachments, &[], None),
        send_fingerprint("s.a.1", "draw this", &attachments, &[], None),
    );
}

#[test]
fn attachment_order_is_part_of_the_fingerprint() {
    let first = wire_attachment("a.png", b"one");
    let second = wire_attachment("b.png", b"two");
    assert_ne!(
        send_fingerprint(
            "s.a.1",
            "draw this",
            &[first.clone(), second.clone()],
            &[],
            None
        ),
        send_fingerprint("s.a.1", "draw this", &[second, first], &[], None),
    );
}

/// The other half of the same defect (see
/// `one_text_with_two_images_is_two_fingerprints`): a client that keeps one
/// idempotency key while pointing at a different stored deck must not be
/// answered from the first send's receipt, because the second deck would
/// then never reach the agent. Two sends that differ only in which stored
/// file they name are two fingerprints, in both directions of the argument
/// (the digest list and its order).
#[test]
fn a_different_stored_reference_is_a_different_fingerprint() {
    let first = stored_reference("s.a.1", 'a');
    let second = stored_reference("s.a.1", 'b');
    assert_ne!(
        send_fingerprint(
            "s.a.1",
            "draw this",
            &[],
            std::slice::from_ref(&first),
            None
        ),
        send_fingerprint(
            "s.a.1",
            "draw this",
            &[],
            std::slice::from_ref(&second),
            None
        ),
    );
    assert_ne!(
        send_fingerprint(
            "s.a.1",
            "draw this",
            &[],
            &[first.clone(), second.clone()],
            None
        ),
        send_fingerprint(
            "s.a.1",
            "draw this",
            &[],
            &[second.clone(), first.clone()],
            None
        ),
        "the order the client listed the references in is part of the payload"
    );
    // And a reference is not a substitute spelling of an inline attachment:
    // the two halves carry their own counts for exactly this reason, so the
    // inline list and the reference list can never be read as one another.
    assert_eq!(
        send_fingerprint(
            "s.a.1",
            "draw this",
            &[],
            std::slice::from_ref(&first),
            None
        ),
        send_fingerprint("s.a.1", "draw this", &[], &[first], None),
    );
}

#[test]
fn a_text_that_looks_like_a_digest_does_not_collide_with_one() {
    // The count field is what makes the digest region unambiguous, so a text
    // ending in hex cannot be read as an attachment's digest.
    let attachment = wire_attachment("a.png", b"one");
    let digest = crate::attachment_store::attachment_digest(&attachment);
    assert_ne!(
        send_fingerprint("s.a.1", &format!(":{digest}"), &[], &[], None),
        send_fingerprint("s.a.1", "", &[attachment], &[], None),
    );
}

/// The two halves of an attachment payload are counted separately, so a
/// reference's digest cannot be read as an inline attachment's and the
/// text cannot be read as either. Without the second count, a send naming
/// one reference and no inline file would share a fingerprint with a send
/// naming one inline file and no reference whenever the two digest strings
/// matched the same text — the collision the counts exist to prevent.
#[test]
fn the_two_attachment_halves_are_counted_separately() {
    let reference = stored_reference("s.a.1", 'a');
    let fingerprint = send_fingerprint("s.a.1", "", &[], std::slice::from_ref(&reference), None);
    assert_ne!(
        fingerprint,
        send_fingerprint("s.a.1", "", &[], &[], None),
        "naming a reference is not the same request as naming none"
    );
    assert!(
        fingerprint.contains(&reference.digest),
        "the reference's own digest travels in the fingerprint: {fingerprint}"
    );
}

#[test]
fn the_fingerprint_does_not_carry_the_encoded_bytes() {
    let attachment = wire_attachment("a.png", b"the png bytes");
    let fingerprint = send_fingerprint(
        "s.a.1",
        "draw this",
        std::slice::from_ref(&attachment),
        &[],
        None,
    );
    assert!(!fingerprint.contains(&attachment.data));
    assert!(fingerprint.contains(&crate::attachment_store::attachment_digest(&attachment)));
}

#[test]
fn record_provider_health_strips_agent_stderr_from_the_reason() {
    let state = state();
    let error = WireError::new(
        ErrorCode::Io,
        "ACP request failed: {\"code\":-32000} Agent stderr: SECRET-TOKEN leaked | C:\\Users",
    );
    state.record_provider_health("stub", Err(&error));
    let value = state.provider_health("stub");
    assert!(
        value.starts_with("failed: ") && value.contains("ACP request failed"),
        "the pre-stderr part of the message must survive: {value:?}"
    );
    assert!(
        !value.contains("SECRET-TOKEN"),
        "health must not carry agent stderr: {value:?}"
    );
}

#[test]
fn collapse_health_reason_cases() {
    assert_eq!(collapse_health_reason(""), "");
    assert_eq!(collapse_health_reason("  \n\t "), "");
    let exactly_200 = "x".repeat(200);
    assert_eq!(collapse_health_reason(&exactly_200), exactly_200);
    assert_eq!(
        collapse_health_reason(&"y".repeat(201)).chars().count(),
        200
    );
    // Char-boundary-safe truncation: 300 two-byte characters must yield
    // exactly 200 valid characters, not a byte slice mid-character.
    let collapsed = collapse_health_reason(&"\u{e8}".repeat(300));
    assert_eq!(collapsed, "\u{e8}".repeat(200));
    assert_eq!(collapse_health_reason("a\n\tb   c"), "a b c");
}

#[test]
fn unchanged_roster_transition_is_not_resent() {
    let state = state();
    let owner = OwnerId::new("roster-user", "roster-client").expect("owner");
    let conn = ConnHandle::new(1);

    state.watch_sessions(&owner, &conn);
    assert_eq!(conn.pull_state_events().len(), 1, "initial snapshot");

    state.broadcast_session_state(&owner);

    assert!(
        conn.pull_state_events().is_empty(),
        "an unchanged full roster must not be resent"
    );
}

fn wait_for_shutdown(state: &ServerState) {
    let deadline = Instant::now() + IDLE_SHUTDOWN_GRACE + Duration::from_millis(500);
    while !state.is_shutting_down() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(state.is_shutting_down(), "idle daemon did not shut down");
}

#[test]
fn idle_daemon_exits_after_grace_period() {
    let state = state();
    assert!(state.client_connected(ClientKind::LocalApp));
    state.client_disconnected(ClientKind::LocalApp, false);
    wait_for_shutdown(&state);
}

#[test]
fn connected_client_prevents_idle_shutdown() {
    let state = state();
    assert!(state.client_connected(ClientKind::LocalApp));
    std::thread::sleep(IDLE_SHUTDOWN_GRACE + Duration::from_millis(100));
    assert!(!state.is_shutting_down());
    state.client_disconnected(ClientKind::LocalApp, false);
    wait_for_shutdown(&state);
}

/// The client slot comes back when the connection's own thread panics: the
/// guard releases on unwind, so `clients` cannot stay above zero forever,
/// where it would keep the idle exit from arming again for the rest of the
/// daemon's life.
#[test]
fn a_panicking_connection_releases_its_client_slot() {
    let state = state();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe({
        let state = Arc::clone(&state);
        move || {
            let _slot = state
                .admit_client(ClientKind::LocalApp)
                .expect("the connection is admitted");
            panic!("the connection thread panicked");
        }
    }));
    assert!(outcome.is_err(), "the fixture must have panicked");
    assert_eq!(
        state.live_client_count(),
        0,
        "the slot must come back on the way out, a panic included"
    );
}

#[test]
fn live_session_prevents_shutdown_even_without_a_client() {
    let state = state();
    assert!(state.session_started());
    std::thread::sleep(IDLE_SHUTDOWN_GRACE + Duration::from_millis(100));
    assert!(!state.is_shutting_down());
    state.session_finished();
    wait_for_shutdown(&state);
}

#[test]
fn reconnect_inside_grace_invalidates_idle_shutdown() {
    let state = state();
    assert!(state.client_connected(ClientKind::LocalApp));
    state.client_disconnected(ClientKind::LocalApp, false);
    std::thread::sleep(IDLE_SHUTDOWN_GRACE / 2);
    assert!(state.client_connected(ClientKind::LocalApp));
    std::thread::sleep(IDLE_SHUTDOWN_GRACE + Duration::from_millis(100));
    assert!(!state.is_shutting_down());
    state.client_disconnected(ClientKind::LocalApp, false);
    wait_for_shutdown(&state);
}

#[test]
fn shutting_down_rejects_new_client_with_stable_error() {
    let state = state();
    state.request_shutdown();
    assert!(!state.client_connected(ClientKind::LocalApp));
    let conn = ConnHandle::new(1);
    let reply = dispatch(
        &state,
        &OwnerId::new("test-user", "test-client").expect("owner"),
        ClientMessage::Ping { id: 7 },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("immediate dispatch reply");
    assert!(matches!(
        reply,
        DaemonMessage::Error(WireError {
            code: ErrorCode::ShuttingDown,
            id: Some(7),
            ..
        })
    ));
}

/// The counting rule the `Shutdown` arm consults, as pure logic: the daemon
/// survives a quit while another local app client is connected, and a peer
/// never tips the count either way.
#[test]
fn shutdown_is_refused_exactly_while_another_local_app_client_is_connected() {
    // Zero local clients — a peer caller's view when no app is connected:
    // accepted.
    assert!(shutdown_accepted(0));
    // One local client, the caller itself: the last app out may stop the
    // daemon, and a connected phone does not change that.
    assert!(shutdown_accepted(1));
    // Two local apps: this quit must not kill the other window's daemon.
    assert!(!shutdown_accepted(2));
    assert!(!shutdown_accepted(5));
}

/// Two app windows share one daemon; the first to quit must be refused,
/// never silently stop the daemon under the second window.
#[test]
fn shutdown_with_two_local_clients_is_refused_with_a_reason() {
    let state = state();
    let _first = state
        .admit_client(ClientKind::LocalApp)
        .expect("first local client is admitted");
    let _second = state
        .admit_client(ClientKind::LocalApp)
        .expect("second local client is admitted");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(21);
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::Shutdown { id: 21 },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("shutdown always answers");
    let DaemonMessage::Shutdown {
        accepted: false,
        reason: Some(reason),
        id: 21,
    } = reply
    else {
        panic!("expected a refused shutdown with a reason, got {reply:?}");
    };
    assert!(
        reason.contains("local app"),
        "the reason must name the local app it protects: {reason}"
    );
    assert!(
        !state.is_shutting_down(),
        "a refused shutdown must not stop the daemon"
    );
}

/// One local app and one paired phone: the phone is not a local app client,
/// so it neither keeps the app's quit from stopping the daemon nor gets the
/// refusal a second window would get.
#[test]
fn shutdown_with_one_local_client_and_a_peer_connected_is_accepted() {
    let state = state();
    let _app = state
        .admit_client(ClientKind::LocalApp)
        .expect("the app is admitted");
    let _phone = state
        .admit_client(ClientKind::Peer)
        .expect("the peer is admitted");
    assert_eq!(
        state.local_client_count(),
        1,
        "a peer must not count as a local app client"
    );
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(22);
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::Shutdown { id: 22 },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("shutdown always answers");
    let DaemonMessage::Shutdown {
        accepted: true,
        reason: None,
        ..
    } = reply
    else {
        panic!("expected an accepted shutdown, got {reply:?}");
    };
}

#[test]
fn permission_response_requires_the_negotiated_typed_capability() {
    let state = state();
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(8);
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::SessionPermissionRespond {
            id: 9,
            session_id: "s.test-client.missing".to_string(),
            subscription_id: 1,
            request_id: "tool-1".to_string(),
            outcome: PermissionOutcome::AllowOnce,
            option_id: None,
            idempotency_key: None,
        },
        &conn,
        false,
        true,
        true,
        false,
    )
    .expect("immediate dispatch reply");
    assert!(matches!(
        reply,
        DaemonMessage::Error(WireError {
            code: ErrorCode::CapabilityNotSupported,
            id: Some(9),
            ..
        })
    ));
}

#[test]
fn providers_list_returns_catalog_entries_with_unknown_authentication() {
    let path = crate::test_dirs::test_temp_dir("devboule-providers-test");
    let state = ServerState::with_paths(
        "test-instance".to_string(),
        RuntimePaths::from_dir(path.clone()),
    )
    .expect("state");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(2);
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::ProvidersList { id: 11 },
        &conn,
        false,
        false,
        false,
        false,
    )
    .expect("immediate dispatch reply");
    let _ = std::fs::remove_dir_all(&path);
    let DaemonMessage::Providers {
        id,
        providers,
        unreadable_dirs: _,
    } = reply
    else {
        panic!("providers_list must reply with Providers, got {reply:?}");
    };
    assert_eq!(id, 11);
    for provider in &providers {
        assert!(!provider.id.is_empty());
        assert_eq!(provider.authentication, "unknown");
        if provider.installed {
            assert!(!provider.executable.is_empty());
        } else {
            assert!(provider.executable.is_empty());
            assert!(!provider.acp_available);
            assert_eq!(provider.protocol, None);
            assert_eq!(
                provider.pickable,
                Some(false),
                "synthetic not-installed providers have no launchable chat protocol"
            );
            assert!(provider.npm_package.is_some());
        }
    }
}

#[test]
fn tool_policy_set_then_get_round_trips_through_dispatch() {
    let path = crate::test_dirs::test_temp_dir("devboule-tool-policy-dispatch");
    let state = ServerState::with_paths(
        "test-instance".to_string(),
        RuntimePaths::from_dir(path.clone()),
    )
    .expect("state");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(4);

    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::ToolPolicySet {
            id: 21,
            provider_id: "claude".to_string(),
            enabled: Some(false),
            disabled_tools: vec!["devboule_list_agents".to_string()],
        },
        &conn,
        false,
        false,
        false,
        false,
    )
    .expect("set reply");
    assert!(
        matches!(reply, DaemonMessage::ToolPolicySetOk { id: 21 }),
        "got {reply:?}"
    );

    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::ToolPolicyGet { id: 22 },
        &conn,
        false,
        false,
        false,
        false,
    )
    .expect("get reply");
    let DaemonMessage::ToolPolicy { id, policies } = reply else {
        panic!("tool_policy_get must reply with ToolPolicy, got {reply:?}");
    };
    assert_eq!(id, 22);
    assert_eq!(policies.len(), 1);
    assert_eq!(policies[0].provider_id, "claude");
    assert_eq!(policies[0].enabled, Some(false));
    assert_eq!(policies[0].disabled_tools, ["devboule_list_agents"]);

    // The file lives beside the journal, and a store loaded fresh from
    // the same directory sees the write — which is what a restart does.
    assert!(path.join("tool-policies.json").is_file());
    let reopened = crate::tool_policy::ToolPolicyStore::load(&path);
    assert_eq!(
        reopened.get(Some("claude")).map(|policy| policy.enabled),
        Some(Some(false))
    );
    let _ = std::fs::remove_dir_all(&path);
}

#[test]
fn a_refused_tool_policy_set_is_an_invalid_request_not_an_io_failure() {
    let state = ServerState::new("tool-policy-refused".to_string());
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(5);

    // A provider id the catalog publishes no tools for: the gate is keyed
    // by that id, so the daemon refuses the row instead of storing one it
    // could never consult. The code is what tells the app to fix the
    // request rather than to retry a write that failed.
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::ToolPolicySet {
            id: 31,
            provider_id: "does-not-exist".to_string(),
            enabled: Some(false),
            disabled_tools: Vec::new(),
        },
        &conn,
        false,
        false,
        false,
        false,
    )
    .expect("dispatch reply");
    let DaemonMessage::Error(error) = reply else {
        panic!("a refused policy must be an error, got {reply:?}");
    };
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(error.id, Some(31));
    assert!(
        error.message.contains("does-not-exist"),
        "the sentence names what was refused: {}",
        error.message
    );

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

#[test]
fn agent_profiles_set_then_get_round_trips_through_dispatch() {
    let path = crate::test_dirs::test_temp_dir("devboule-agent-profiles-dispatch");
    let state = ServerState::with_paths(
        "test-instance".to_string(),
        RuntimePaths::from_dir(path.clone()),
    )
    .expect("state");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(24);

    let mut reviewer = devboule_protocol::AgentProfile {
        id: "p-1".to_string(),
        name: "Reviewer".to_string(),
        icon: None,
        note: "Use when the first answer has to be checked.".to_string(),
        spawn_prompt: String::new(),
        provider: "claude".to_string(),
        model: "claude-opus-4-6".to_string(),
        mode_id: "default".to_string(),
        thinking_option_id: Some("high".to_string()),
        features: serde_json::Map::new(),
        tool_overlay: vec!["devboule_create_agent".to_string()],
        enabled_for_agents: true,
    };
    reviewer
        .features
        .insert("autoAccept".to_string(), serde_json::json!(false));
    let mut document = devboule_protocol::AgentProfilesDocument {
        profiles: vec![reviewer],
        standing_instructions: "Report your result in your final message.".to_string(),
    };
    // A second profile, so the ordered list is a list and not one row.
    let mut second = document.profiles[0].clone();
    second.id = "p-2".to_string();
    second.name = "Second opinion".to_string();
    second.tool_overlay.clear();
    second.enabled_for_agents = false;
    document.profiles.push(second);

    let sent = document.clone();
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::AgentProfilesSet { id: 21, document },
        &conn,
        false,
        false,
        false,
        false,
    )
    .expect("set reply");
    assert!(
        matches!(reply, DaemonMessage::AgentProfilesSetOk { id: 21 }),
        "got {reply:?}"
    );

    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::AgentProfilesGet { id: 22 },
        &conn,
        false,
        false,
        false,
        false,
    )
    .expect("get reply");
    let DaemonMessage::AgentProfiles { id, document } = reply else {
        panic!("AgentProfilesGet must reply with AgentProfiles, got {reply:?}");
    };
    assert_eq!(id, 22);
    assert_eq!(
        document, sent,
        "the whole document comes back, in the order it was sent"
    );
    assert_eq!(document.profiles[0].name, "Reviewer");
    assert_eq!(document.profiles[1].name, "Second opinion");

    // The file lives beside the journal, and a store loaded fresh from the
    // same directory sees the write — which is what a restart does.
    assert!(path.join("agent-profiles.json").is_file());
    let reopened = crate::agent_profiles::AgentProfilesStore::load(&path);
    assert_eq!(reopened.document(), sent);
    let _ = std::fs::remove_dir_all(&path);
}

#[test]
fn a_refused_agent_profiles_set_is_an_invalid_request_not_an_io_failure() {
    let state = ServerState::new("agent-profiles-refused".to_string());
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(25);

    // A provider the catalog does not publish: the profile could never be
    // created from, so the daemon refuses the document instead of storing
    // one it could not honour. The code is what tells the app to fix the
    // request rather than to retry a write that failed.
    let document = devboule_protocol::AgentProfilesDocument {
        profiles: vec![devboule_protocol::AgentProfile {
            id: "p-1".to_string(),
            name: "Ghost".to_string(),
            icon: None,
            note: String::new(),
            spawn_prompt: String::new(),
            provider: "does-not-exist".to_string(),
            model: "m".to_string(),
            mode_id: "default".to_string(),
            thinking_option_id: None,
            features: serde_json::Map::new(),
            tool_overlay: Vec::new(),
            enabled_for_agents: true,
        }],
        standing_instructions: String::new(),
    };
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::AgentProfilesSet { id: 31, document },
        &conn,
        false,
        false,
        false,
        false,
    )
    .expect("dispatch reply");
    let DaemonMessage::Error(error) = reply else {
        panic!("a refused document must be an error, got {reply:?}");
    };
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(error.id, Some(31));
    assert!(
        error.message.contains("does-not-exist"),
        "the sentence names what was refused: {}",
        error.message
    );

    // Nothing was stored, and the empty document is what a get now answers.
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::AgentProfilesGet { id: 32 },
        &conn,
        false,
        false,
        false,
        false,
    )
    .expect("get reply");
    let DaemonMessage::AgentProfiles { document, .. } = reply else {
        panic!("got {reply:?}");
    };
    assert!(document.profiles.is_empty());
    assert!(document.standing_instructions.is_empty());

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// The agent-profile store at the dispatch layer, in the two halves the parity
/// decision is made of. The negative control first: a paired device holding
/// every act-named capability and no `admin` is refused both frames, with the
/// capability error that now names `admin` — the sentence changed with the
/// rule, and pinning it is what keeps a peer's refusal actionable. Then the
/// parity: the same frames from a device that holds `admin` are served.
#[test]
fn both_agent_profile_frames_ride_the_administrative_capability() {
    let state = ServerState::new("agent-profiles-peer".to_string());
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let operational = ["view", "send", "answer_permissions", "create_sessions"];
    let conn = remote_conn_with_caps(PeerRole::Client, None, &operational);

    for request in [
        ClientMessage::AgentProfilesGet { id: 41 },
        ClientMessage::AgentProfilesSet {
            id: 42,
            document: devboule_protocol::AgentProfilesDocument::default(),
        },
    ] {
        let reply = dispatch(&state, &owner, request, &conn, true, true, true, true)
            .expect("dispatch reply");
        let DaemonMessage::Error(error) = reply else {
            panic!("a peer without `admin` must be refused, got {reply:?}");
        };
        assert_eq!(error.code, ErrorCode::CapabilityNotSupported);
        assert_eq!(
            error.message, "capability 'admin' was not negotiated",
            "the refusal names the capability that would open the arm"
        );
    }

    // The parity half, through the same gate: the administrative capability is
    // what the arm needs, and the wire answer is the handler's own.
    let mut admin = operational.to_vec();
    admin.push(crate::peer_policy::CAP_ADMIN);
    let conn = remote_conn_with_caps(PeerRole::Client, None, &admin);
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::AgentProfilesGet { id: 43 },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("dispatch reply");
    assert!(
        matches!(reply, DaemonMessage::AgentProfiles { id: 43, .. }),
        "got {reply:?}"
    );
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::AgentProfilesSet {
            id: 44,
            document: devboule_protocol::AgentProfilesDocument::default(),
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("dispatch reply");
    assert!(
        matches!(reply, DaemonMessage::AgentProfilesSetOk { id: 44 }),
        "got {reply:?}"
    );

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// The delegation pair, end to end at the dispatch layer: a fresh daemon
/// answers `default`, a set stores the file beside the journal and replies
/// with what was **stored**, the next get reads it back, and every
/// watching connection is pushed the same pair the store returned.
#[test]
fn delegation_get_and_set_round_trip_and_push_the_stored_value() {
    let state = ServerState::new("delegation-dispatch".to_string());
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(27);
    let watcher = ConnHandle::new(28);
    state.watch_sessions(&owner, &watcher);

    // A fresh daemon has no file: off, and `default` — the answer that
    // says "never configured", not "the human turned it off".
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::DelegationGet { id: 31 },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("get reply");
    let DaemonMessage::DelegationState {
        id,
        enabled,
        source,
    } = reply
    else {
        panic!("DelegationGet must reply with DelegationState, got {reply:?}");
    };
    assert_eq!(id, 31);
    assert!(!enabled);
    assert_eq!(source, devboule_protocol::DelegationSource::Default);

    // The set: the reply is what the daemon stored, the durable copy
    // lands beside the journal, and the watcher is pushed the same pair.
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::DelegationSet {
            id: 32,
            enabled: true,
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("set reply");
    let DaemonMessage::DelegationSetOk {
        id,
        enabled,
        source,
    } = reply
    else {
        panic!("DelegationSet must reply with DelegationSetOk, got {reply:?}");
    };
    assert_eq!(id, 32);
    assert!(enabled);
    assert_eq!(source, devboule_protocol::DelegationSource::File);

    let pushed = watcher.outbound.pull_replies();
    assert!(
        pushed.iter().any(|message| matches!(
            message,
            DaemonMessage::DelegationChanged {
                enabled: true,
                source: devboule_protocol::DelegationSource::File
            }
        )),
        "the watcher must be pushed the stored pair, got {pushed:?}"
    );

    // The durable copy lands beside the journal and holds the stored
    // value — the thing a restart reads back (the store's own round-trip
    // test covers the fresh-load half). This test reads the file, rather
    // than constructing a second store, so the nothing-reads-it test can
    // hold `server.rs` to exactly one construction of the store.
    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    let durable = std::fs::read(runtime_dir.join("delegation.json")).expect("switch file");
    let durable: serde_json::Value = serde_json::from_slice(&durable).expect("switch json");
    assert_eq!(durable["enabled"], true, "the stored value is on the file");

    // The setting change is an audited act, and the actor is always the
    // person at this machine (the peer gate refuses the pair upstream).
    let connection =
        rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
    let mut statement = connection
        .prepare("SELECT action, outcome FROM audit ORDER BY id")
        .expect("prepare");
    let rows: Vec<String> = statement
        .query_map([], |row| {
            Ok(format!(
                "{}:{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?
            ))
        })
        .expect("query")
        .map(Result::unwrap)
        .collect();
    assert!(
        rows.iter().any(|row| row == "DelegationSet:ok"),
        "the flip is audited: {rows:?}"
    );
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::DelegationGet { id: 33 },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("get reply");
    assert!(
        matches!(
            reply,
            DaemonMessage::DelegationState {
                id: 33,
                enabled: true,
                source: devboule_protocol::DelegationSource::File
            }
        ),
        "got {reply:?}"
    );

    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// The delegation pair is refused to a paired device holding **every**
/// act-named capability — the negative control — and is served to one holding
/// the administrative capability, through the same gate. The refusal sentence
/// is the capability error the app already renders, naming `admin`.
#[test]
fn both_delegation_frames_ride_the_administrative_capability() {
    let state = ServerState::new("delegation-peer".to_string());
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let operational = ["view", "send", "answer_permissions", "create_sessions"];
    let conn = remote_conn_with_caps(PeerRole::Client, None, &operational);

    for request in [
        ClientMessage::DelegationGet { id: 41 },
        ClientMessage::DelegationSet {
            id: 42,
            enabled: true,
        },
    ] {
        let reply = dispatch(&state, &owner, request, &conn, true, true, true, true)
            .expect("dispatch reply");
        let DaemonMessage::Error(error) = reply else {
            panic!("a peer without `admin` must be refused, got {reply:?}");
        };
        assert_eq!(error.code, ErrorCode::CapabilityNotSupported);
        assert_eq!(
            error.message, "capability 'admin' was not negotiated",
            "the refusal names the capability that would open the arm"
        );
    }

    // The parity half: with `admin`, the switch is read and written like any
    // other setting — and the write is audited under the peer's own identity by
    // the gate, never as a local act (`stores.rs::delegation_set`).
    let mut admin = operational.to_vec();
    admin.push(crate::peer_policy::CAP_ADMIN);
    let conn = remote_conn_with_caps(PeerRole::Client, None, &admin);
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::DelegationGet { id: 43 },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("dispatch reply");
    assert!(
        matches!(reply, DaemonMessage::DelegationState { id: 43, .. }),
        "got {reply:?}"
    );
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::DelegationSet {
            id: 44,
            enabled: true,
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("dispatch reply");
    assert!(
        matches!(reply, DaemonMessage::DelegationSetOk { id: 44, .. }),
        "got {reply:?}"
    );

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// The parity decision put `DelegationSet` inside a paired device's reach, and
/// that made the row `stores.rs` wrote false: it claimed the actor was "the
/// person at this machine" because the gate used to refuse the arm before the
/// handler ran. The trail now says what the connection proves — a local write
/// writes the local row, a peer's write carries the peer's device id and role,
/// and the peer's write is **not** double-written as a local act.
#[test]
fn a_peers_delegation_write_is_audited_under_the_peers_identity() {
    let (path, state) = temp_state("delegation-audit");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");

    let local = ConnHandle::new(31);
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::DelegationSet {
            id: 1,
            enabled: false,
        },
        &local,
        true,
        true,
        true,
        true,
    )
    .expect("dispatch reply");
    assert!(
        matches!(reply, DaemonMessage::DelegationSetOk { id: 1, .. }),
        "got {reply:?}"
    );

    let peer = remote_conn_with_caps(
        PeerRole::Client,
        Some("S-user-a"),
        &[crate::peer_policy::CAP_VIEW, crate::peer_policy::CAP_ADMIN],
    );
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::DelegationSet {
            id: 2,
            enabled: true,
        },
        &peer,
        true,
        true,
        true,
        true,
    )
    .expect("dispatch reply");
    assert!(
        matches!(reply, DaemonMessage::DelegationSetOk { id: 2, .. }),
        "got {reply:?}"
    );

    let actors = audit_actors(&path);
    assert_eq!(
        actors.len(),
        2,
        "one row per write: the peer's is the gate's, not a second local one: {actors:?}"
    );
    assert_eq!(actors[0].1, "local", "the local write is the local row");
    assert_eq!(
        actors[1],
        ("dev-peer-1".to_string(), "client".to_string()),
        "the peer's write carries the peer's own device id and role"
    );

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// The vocabulary query is the profile store's companion read: refused to a
/// paired device holding **every** act-named capability — the negative control
/// — and served to one holding the administrative capability, through the same
/// gate. The handshake capability that advertises the query to the app is a
/// frame-compatibility name, a different mechanism from the peer capability set.
#[test]
fn a_vocabulary_request_rides_the_administrative_capability() {
    let state = ServerState::new("vocabulary-peer".to_string());
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let operational = ["view", "send", "answer_permissions", "create_sessions"];
    let conn = remote_conn_with_caps(PeerRole::Client, None, &operational);

    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::ProviderVocabularyGet {
            id: 51,
            provider: "claude".to_string(),
            model: None,
            refresh: false,
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("dispatch reply");
    let DaemonMessage::Error(error) = reply else {
        panic!("a peer without `admin` must be refused, got {reply:?}");
    };
    assert_eq!(error.code, ErrorCode::CapabilityNotSupported);
    assert_eq!(
        error.message, "capability 'admin' was not negotiated",
        "the refusal names the capability that would open the arm"
    );

    // The parity half: with `admin`, the same frame reaches the same handler the
    // local pipe reaches.
    let mut admin = operational.to_vec();
    admin.push(crate::peer_policy::CAP_ADMIN);
    let conn = remote_conn_with_caps(PeerRole::Client, None, &admin);
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::ProviderVocabularyGet {
            id: 52,
            provider: "claude".to_string(),
            model: None,
            refresh: false,
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("dispatch reply");
    assert!(
        matches!(reply, DaemonMessage::ProviderVocabulary { id: 52, .. }),
        "got {reply:?}"
    );

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// An unknown provider id is the caller's mistake and is refused with
/// the catalog's own sentence — the same walk, and the same sentence,
/// the profile store refuses a document with.
#[test]
fn an_unknown_provider_vocabulary_request_is_an_invalid_request() {
    let state = ServerState::new("vocabulary-bogus".to_string());
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(28);

    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::ProviderVocabularyGet {
            id: 53,
            provider: "bogus".to_string(),
            model: None,
            refresh: false,
        },
        &conn,
        false,
        false,
        false,
        false,
    )
    .expect("dispatch reply");
    let DaemonMessage::Error(error) = reply else {
        panic!("an unknown provider must be refused, got {reply:?}");
    };
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(error.id, Some(53));
    assert!(
        error.message.contains("bogus"),
        "the sentence names what was refused: {}",
        error.message
    );

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// What pass 1 answers, end to end through dispatch, asserted on the
/// serialised wire:
///
/// - Claude answers `present` on both axes, its models origin following
///   the catalog state and its modes `daemon`-origin (the launcher's
///   `--permission-mode` values — the provider cannot report modes).
/// - Every other provider answers `absent` — a different wire word from
///   `none`, never an empty `present`.
/// - `origin` is present exactly when the state is `present`, and a
///   probe reply omits `probedAtMs` rather than sending `null`.
/// - No spawn outcome was recorded: Claude's read costs no process.
#[test]
fn a_vocabulary_read_answers_on_the_wire_as_the_spec_spells_it() {
    let state = ServerState::new("vocabulary-wire".to_string());
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(29);

    let claude_reply = dispatch(
        &state,
        &owner,
        ClientMessage::ProviderVocabularyGet {
            id: 54,
            provider: "claude".to_string(),
            model: None,
            refresh: true,
        },
        &conn,
        false,
        false,
        false,
        false,
    )
    .expect("claude reply");
    let DaemonMessage::ProviderVocabulary { provider, .. } = &claude_reply else {
        panic!("got {claude_reply:?}");
    };
    assert_eq!(provider, "claude", "the reply carries the canonical id");
    assert_vocabulary_wire(&claude_reply);

    let json = serde_json::to_value(&claude_reply).expect("json");
    for axis in ["models", "modes"] {
        let axis_json = &json[axis];
        assert_eq!(
            axis_json["state"],
            serde_json::json!("present"),
            "Claude's {axis} axis answers present"
        );
        assert!(
            !axis_json["items"]
                .as_array()
                .expect("present items")
                .is_empty(),
            "a present axis never ships empty items"
        );
    }
    // Modes are ours, not Claude's: the launcher's vocabulary, honestly
    // labelled.
    assert_eq!(
        json["modes"]["origin"],
        serde_json::json!("daemon"),
        "Claude cannot report modes; the daemon must say the list is its own"
    );
    // Models origin follows the catalog: extraction worked (`provider`)
    // or the fallback table answered (`daemon`). Either is honest; both
    // carry a non-empty list.
    assert!(
        json["models"]["origin"] == serde_json::json!("provider")
            || json["models"]["origin"] == serde_json::json!("daemon"),
        "got {}",
        json["models"]["origin"]
    );
    // A probe reply omits probedAtMs entirely — it is fresh by
    // definition.
    assert!(json.get("probedAtMs").is_none(), "got {json}");
    // The read itself recorded no health outcome. `provider_health` is
    // written by session-start handshakes, and a real version probe
    // records none either, so this pins only that THIS read wrote
    // nothing — the read's process cost is pinned where a process
    // actually begins, by the version-probe seam test below.
    assert_eq!(
        state.provider_health("claude"),
        "unknown",
        "a vocabulary read must not record a health outcome"
    );

    // The debug-only provider whose binary cannot exist anywhere makes
    // `not installed` reachable without depending on this machine.
    let absent_reply = dispatch(
        &state,
        &owner,
        ClientMessage::ProviderVocabularyGet {
            id: 55,
            provider: "devboule-absent-probe".to_string(),
            model: None,
            refresh: true,
        },
        &conn,
        false,
        false,
        false,
        false,
    )
    .expect("absent reply");
    let DaemonMessage::ProviderVocabulary { provider, .. } = &absent_reply else {
        panic!("got {absent_reply:?}");
    };
    assert_eq!(provider, "devboule-absent-probe");
    let json = serde_json::to_value(&absent_reply).expect("json");
    for axis in ["models", "modes"] {
        assert_eq!(
            json[axis]["state"],
            serde_json::json!("absent"),
            "a provider that cannot answer is `absent`, a distinct wire value"
        );
        assert!(
            json[axis].get("origin").is_none(),
            "an absent axis carries no origin, got {}",
            json[axis]
        );
        assert_eq!(
            json[axis]["items"].as_array().expect("items").len(),
            0,
            "an absent axis carries no items"
        );
    }
    assert_vocabulary_wire(&absent_reply);

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// The read's process cost, observed where a process actually begins.
/// `probe_native_version` is the only function on the Claude vocabulary
/// path that can start a process — a release build turns each entry into
/// one `claude --version` — so the test counts entries at that seam on a
/// synthetic native install (a `claude.exe` in a directory only this
/// test scans), where the machine's own installs cannot reach the
/// assertion. A cold read — version unknown — enters the seam exactly
/// once, which is the honest cost the module doc states; a read whose
/// version is already settled, the state `providers_list` leaves the
/// daemon in, enters it zero times.
#[test]
fn a_claude_read_enters_the_version_probe_seam_only_while_the_version_is_unknown() {
    let temp = crate::test_dirs::test_temp_dir("devboule-vocabulary-seam");
    let fake = temp.join("claude.exe");
    std::fs::write(&fake, b"not really claude").expect("fake binary");

    let state = ServerState::new("vocabulary-spawn-seam".to_string());
    let directories = vec![temp.clone()];
    let agent = crate::provider_catalog::find_available_in_paths("claude", &directories)
        .expect("the synthetic native claude must resolve");
    assert_eq!(
        agent.install_channel,
        crate::provider_catalog::InstallChannel::Native,
        "a bare executable is a native install, no npm prefix"
    );

    // Cold: the version is unknown, and the read starts the version
    // probe. In a test build the probe body is stubbed — no process is
    // ever launched — but the entry, the seam itself, is still counted.
    let before = state.version_probe_entry_count();
    let snapshot = state.claude_models_in_paths(&directories);
    assert_eq!(
        snapshot.state,
        crate::claude_catalog::ClaudeCatalogState::Provisional
    );
    // The probe runs on its own thread and leaves the in-flight guard
    // when it is done; the entry count is final once the guard clears.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state
        .claude_version_probes
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains(&agent.executable)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the version probe never finished"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(
        state.version_probe_entry_count(),
        before + 1,
        "an unknown version must start exactly one version probe"
    );

    // Warm: the version is settled — what a real probe or
    // `providers_list` records — and the same read costs no process.
    let fingerprint = executable_fingerprint(&agent.executable).expect("fingerprint");
    state.record_provider_cli_version("claude", "9.9.9-test", fingerprint);
    let before = state.version_probe_entry_count();
    let _ = state.claude_models_in_paths(&directories);
    assert_eq!(
        state.version_probe_entry_count(),
        before,
        "a read whose version fact is settled must enter no spawn seam"
    );

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
    let _ = std::fs::remove_dir_all(temp);
}

/// The biconditional, on the daemon's actual replies and in both
/// directions: an axis carries `origin` if and only if its state is
/// `present`. Together with the protocol-crate wire test this is what
/// keeps the two fields from becoming a second, unstated source of truth
/// about the state.
#[test]
fn origin_travels_exactly_with_a_present_state_on_every_reply() {
    let state = ServerState::new("vocabulary-biconditional".to_string());
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(30);

    for (provider, refresh) in [
        ("claude", true),
        ("devboule-absent-probe", true),
        ("claude", false),
        ("devboule-absent-probe", false),
    ] {
        let reply = dispatch(
            &state,
            &owner,
            ClientMessage::ProviderVocabularyGet {
                id: 56,
                provider: provider.to_string(),
                model: None,
                refresh,
            },
            &conn,
            false,
            false,
            false,
            false,
        )
        .expect("reply");
        let json = serde_json::to_value(&reply).expect("json");
        for axis in ["models", "modes"] {
            let state_word = json[axis]["state"].as_str().expect("state word");
            let has_origin = json[axis].get("origin").is_some();
            assert_eq!(
                has_origin,
                state_word == "present",
                "{provider} (refresh: {refresh}) {axis}: origin presence must equal present, got {json}"
            );
            if state_word == "present" {
                assert!(
                    !json[axis]["items"].as_array().expect("items").is_empty(),
                    "a present axis never ships empty items: {json}"
                );
            } else {
                assert_eq!(
                    state_word, "absent",
                    "pass 1 answers only present or absent: {json}"
                );
            }
        }
    }

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// Assert the reply's optional fields keep the one encoding the protocol
/// went with: absent from the wire, never an explicit `null`. Read
/// through `.get()`, which distinguishes a missing key from a null one.
fn assert_vocabulary_wire(reply: &DaemonMessage) {
    let json = serde_json::to_value(reply).expect("json");
    match json.get("probedAtMs") {
        None => {}
        Some(serde_json::Value::Number(_)) => {}
        Some(other) => panic!("probedAtMs must be a number or absent, got {other}"),
    }
    for axis in ["models", "modes"] {
        match json[axis].get("origin") {
            None => {}
            Some(serde_json::Value::String(_)) => {}
            Some(other) => panic!("{axis}.origin must be a string or absent, got {other}"),
        }
    }
}

/// The cache: two reads within the TTL probe once, the second answer
/// says `cache` and reports when its entry was filled; `refresh: true`
/// re-probes despite the warm entry and answers as a probe; an entry
/// older than the TTL is probed again. Counted on the probe counter, not
/// inferred from log lines.
#[test]
fn the_vocabulary_cache_serves_second_reads_and_refresh_re_probes() {
    let state = ServerState::new("vocabulary-cache".to_string());
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(31);
    let request = |id: u64, refresh: bool| ClientMessage::ProviderVocabularyGet {
        id,
        provider: "devboule-absent-probe".to_string(),
        model: None,
        refresh,
    };

    let first = dispatch(
        &state,
        &owner,
        request(61, false),
        &conn,
        false,
        false,
        false,
        false,
    )
    .expect("first reply");
    assert_eq!(state.provider_vocabulary.probe_count(), 1);
    let DaemonMessage::ProviderVocabulary { source, .. } = &first else {
        panic!("got {first:?}");
    };
    assert!(matches!(source, devboule_protocol::VocabularySource::Probe));

    let second = dispatch(
        &state,
        &owner,
        request(62, false),
        &conn,
        false,
        false,
        false,
        false,
    )
    .expect("second reply");
    assert_eq!(
        state.provider_vocabulary.probe_count(),
        1,
        "the second read within the TTL must be served from the cache"
    );
    let DaemonMessage::ProviderVocabulary {
        source,
        probed_at_ms,
        ..
    } = &second
    else {
        panic!("got {second:?}");
    };
    assert!(matches!(source, devboule_protocol::VocabularySource::Cache));
    assert!(
        probed_at_ms.is_some(),
        "a cache reply names when the entry was filled"
    );

    let refreshed = dispatch(
        &state,
        &owner,
        request(63, true),
        &conn,
        false,
        false,
        false,
        false,
    )
    .expect("refresh reply");
    assert_eq!(
        state.provider_vocabulary.probe_count(),
        2,
        "refresh: true must re-probe despite a warm entry"
    );
    let DaemonMessage::ProviderVocabulary {
        source,
        probed_at_ms,
        ..
    } = &refreshed
    else {
        panic!("got {refreshed:?}");
    };
    assert!(matches!(source, devboule_protocol::VocabularySource::Probe));
    assert!(probed_at_ms.is_none());

    // Past the TTL the warm entry is stale and the read probes again.
    state
        .provider_vocabulary
        .backdate("devboule-absent-probe", 30 * 60 * 1000);
    let aged = dispatch(
        &state,
        &owner,
        request(64, false),
        &conn,
        false,
        false,
        false,
        false,
    )
    .expect("aged reply");
    assert_eq!(
        state.provider_vocabulary.probe_count(),
        3,
        "an entry past the TTL must be probed again"
    );
    let DaemonMessage::ProviderVocabulary { source, .. } = &aged else {
        panic!("got {aged:?}");
    };
    assert!(matches!(source, devboule_protocol::VocabularySource::Probe));

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// The vocabulary cache is form-only: the live catalog path — the one
/// that feeds the session manifest's model chips — never reads it, so a
/// poisoned cache entry cannot reach a session's manifest.
#[test]
fn the_vocabulary_cache_never_feeds_the_live_catalog_path() {
    let state = ServerState::new("vocabulary-manifest".to_string());
    let marker = "bogus-from-vocabulary-cache".to_string();
    state.provider_vocabulary.inject(
        "claude",
        devboule_protocol::VocabularyModels {
            state: devboule_protocol::VocabularyState::Present,
            origin: Some(devboule_protocol::VocabularyOrigin::Daemon),
            items: vec![devboule_protocol::SessionModel {
                model_id: marker.clone(),
                name: marker.clone(),
                description: None,
                context_tokens: None,
                current_effort: None,
                efforts: None,
            }],
        },
        devboule_protocol::VocabularyModes {
            state: devboule_protocol::VocabularyState::Absent,
            origin: None,
            items: Vec::new(),
        },
    );
    // The catalog path still answers from the derivation/cache chain,
    // not from the vocabulary cache.
    let snapshot = state.claude_models();
    assert!(
        snapshot.models.iter().all(|model| model.model_id != marker),
        "the live catalog path must not read the vocabulary cache"
    );

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

#[test]
fn diagnostics_rpc_reports_the_open_journal_without_user_content() {
    let state = state();
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(3);
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::DaemonDiagnostics { id: 12 },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("immediate diagnostics reply");
    let DaemonMessage::Diagnostics { id, report } = reply else {
        panic!("diagnostics must reply with Diagnostics");
    };
    assert_eq!(id, 12);
    assert_eq!(report["daemon"]["instanceId"], "test-instance");
    assert_eq!(
        report["health"]["journalSchemaVersion"],
        JOURNAL_SCHEMA_VERSION
    );
    assert!(
        report["health"]["journalFileBytes"]
            .as_u64()
            .is_some_and(|bytes| bytes > 0),
        "journal file bytes were not positive: report={report}, journal_error={:?}",
        state
            .journal_error
            .lock()
            .ok()
            .and_then(|error| error.clone())
    );
    let encoded = report.to_string();
    assert!(!encoded.contains("title"));
    assert!(!encoded.contains("transcript"));
    assert!(!encoded.contains("AgentStderr"));
    assert!(!encoded.contains("permission env"));
}

#[cfg(windows)]
#[test]
fn host_os_version_matches_an_independent_windows_version_report() {
    // Resolve PowerShell by absolute path. A child spawned from a POSIX shell can
    // inherit a PATH without System32, and `program not found` would then read exactly
    // like a version mismatch — the environment failing, disguised as the assertion failing.
    let system_root = std::env::var("SystemRoot").expect("SystemRoot must be set on Windows");
    let powershell = std::path::Path::new(&system_root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    assert!(
        powershell.is_file(),
        "PowerShell must exist at {}",
        powershell.display()
    );
    let output = std::process::Command::new(&powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "(Get-CimInstance Win32_OperatingSystem).Version",
        ])
        .output()
        .expect("PowerShell must report the Windows version");
    assert!(
        output.status.success(),
        "PowerShell version query failed: {:?}",
        output.status
    );
    let independent = String::from_utf8(output.stdout)
        .expect("PowerShell version must be UTF-8")
        .trim()
        .to_string();
    let reported = host_os_version();
    let reported_version = reported
        .strip_prefix("Windows ")
        .and_then(|value| value.split_whitespace().next())
        .expect("host OS report must contain a Windows version");
    assert_eq!(reported_version, independent);
}

struct RecordingNpmRunner {
    calls: Arc<Mutex<Vec<Vec<String>>>>,
    result: crate::provider_update::NpmInstallResult,
}

impl NpmInstallRunner for RecordingNpmRunner {
    fn run(
        &self,
        _program: &std::path::Path,
        _prefix_args: &[String],
        args: &[String],
        _job: &crate::process_tree::JobObject,
    ) -> crate::provider_update::NpmInstallResult {
        self.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(args.to_vec());
        self.result.clone()
    }
}

fn update_test_agent(
    id: &str,
    package: Option<&'static str>,
    installed: bool,
    install_channel: crate::provider_catalog::InstallChannel,
    executable: std::path::PathBuf,
) -> crate::provider_catalog::InstalledAgent {
    crate::provider_catalog::InstalledAgent {
        id: id.to_string(),
        aliases: &[],
        installed,
        executable,
        prefix_args: Vec::new(),
        acp_command: None,
        stream_json_command: None,
        rpc_command: None,
        app_server_command: None,
        authentication: crate::provider_catalog::AuthenticationStatus::Unknown,
        origin: crate::provider_catalog::ProviderOrigin::UserBinary,
        launch_args: None,
        pickable: None,
        installed_version: None,
        latest_version: None,
        install_channel,
        npm_package: package,
        tools: crate::provider_catalog::mcp_tools_for(id),
        spawn_path_env: None,
        launch_directory: None,
    }
}

fn wait_for_worker_reply(conn: &ConnHandle) -> DaemonMessage {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(reply) = conn.outbound.pull_replies().pop_front() {
            return reply;
        }
        assert!(
            Instant::now() < deadline,
            "background request did not reply"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn queued_session_frames_flow_while_creation_waits_off_dispatch() {
    let (path, state) = temp_state("create-worker-dispatch");
    let owner = OwnerId::new("alex", "app").expect("owner");
    let conn = ConnHandle::new(91);
    let other = crate::session::insert_test_live_agent(&state.sessions, "s.other", owner.clone());
    state
        .sessions
        .attach_with_subscription("s.other", 1, None, &conn, &owner, false)
        .expect("attach the other session");
    let create_guard = state
        .session_create_lock
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::SessionCreate {
            id: 41,
            workspace_id: None,
            kind: SessionKind::Claude,
            provider: Some("unknown-test-provider".to_string()),
            mode: None,
            display_name: Some("   ".to_string()),
            idempotency_key: None,
        },
        &conn,
        true,
        true,
        true,
        true,
    );
    assert!(
        reply.is_none(),
        "the dispatch loop hands creation to a worker"
    );

    assert!(other.publish_agent_error("transcript frame".to_string()));
    let frames = conn.pull_events();
    assert!(
        !frames.is_empty(),
        "the other session's transcript is available"
    );
    for frame in &frames {
        conn.event_sent(frame);
    }

    drop(create_guard);
    let created = wait_for_worker_reply(&conn);
    assert!(matches!(created, DaemonMessage::Error(error) if error.id == Some(41)));
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn provider_update_invalidates_the_acp_feature_answer() {
    let state = state();
    let key = crate::provider_feature_probe::ProbeKey::new("updated-acp-provider");
    assert!(state.acp_features.claim(&key));
    state
        .acp_features
        .finish(&key, crate::provider_feature_probe::Probe::Answered(vec![]));
    crate::provider_feature_probe::record_answer_for_test(&key, vec![]);

    state.invalidate_provider_update_caches("updated-acp-provider");

    assert_eq!(state.acp_features.peek(&key), None);
    assert_eq!(
        crate::provider_feature_probe::cached_declarations(&key),
        None
    );
}

#[test]
fn provider_update_drops_fingerprint_but_preserves_latest_version_cache() {
    let path = crate::test_dirs::test_temp_dir("devboule-provider-update-test");
    let executable = path.join("codex.cmd");
    std::fs::write(&executable, b"shim").expect("fake executable");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let runner = Arc::new(RecordingNpmRunner {
        calls: Arc::clone(&calls),
        result: crate::provider_update::NpmInstallResult {
            exit_code: Some(0),
            log: "npm stdout\nnpm stderr".to_string(),
        },
    });
    let state = ServerState::with_paths_and_npm_install_runner(
        "test-instance".to_string(),
        RuntimePaths::from_dir(path.clone()),
        runner,
    )
    .expect("state");
    state.set_provider_update_catalog(crate::provider_catalog::ProviderDiscovery {
        agents: vec![update_test_agent(
            "codex",
            Some("@openai/codex"),
            true,
            crate::provider_catalog::InstallChannel::Npm,
            executable.clone(),
        )],
        unreadable_dirs: 0,
    });
    state.set_provider_update_npm_command(std::path::PathBuf::from(r"C:\fake\npm.cmd"), vec![]);
    let fingerprint = executable_fingerprint(&executable).expect("fingerprint");
    state.record_provider_cli_version("codex", "1.0.0", fingerprint);
    crate::registry::reset_npm_version_cache("@openai/codex");
    struct FakeNpmVersion;
    impl crate::registry::NpmVersionFetch for FakeNpmVersion {
        fn latest(&self, _package: &str) -> Result<String, String> {
            Ok("9.9.9".to_string())
        }
    }
    assert_eq!(
        crate::registry::load_latest_npm_version(&FakeNpmVersion, "@openai/codex", true),
        Some("9.9.9".to_string())
    );
    assert_eq!(
        state.provider_cli_version("codex", &executable),
        Some("1.0.0".to_string())
    );

    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(42);
    assert!(dispatch(
        &state,
        &owner,
        ClientMessage::ProviderUpdate {
            id: 43,
            provider_id: "codex".to_string(),
        },
        &conn,
        false,
        false,
        false,
        false,
    )
    .is_none());
    assert_eq!(
        wait_for_worker_reply(&conn),
        DaemonMessage::ProviderUpdated {
            id: 43,
            ok: true,
            exit_code: Some(0),
            log: "npm stdout\nnpm stderr".to_string(),
        }
    );
    assert_eq!(
        calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
        &[vec![
            "install".to_string(),
            "-g".to_string(),
            "@openai/codex@latest".to_string()
        ]]
    );
    assert_eq!(
        state.provider_cli_version("codex", &executable),
        None,
        "successful update must drop the native --version cache entry"
    );
    assert_eq!(
        crate::registry::cached_latest_npm_version("@openai/codex"),
        Some("9.9.9".to_string()),
        "successful update must preserve the npm latest cache entry"
    );
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn provider_update_failure_preserves_both_version_caches() {
    let path = crate::test_dirs::test_temp_dir("devboule-provider-update-failure-test");
    let package = "@qwen-code/qwen-code";
    crate::registry::reset_npm_version_cache(package);
    let executable = path.join("qwen.cmd");
    std::fs::write(&executable, b"shim").expect("fake executable");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let runner = Arc::new(RecordingNpmRunner {
        calls: Arc::clone(&calls),
        result: crate::provider_update::NpmInstallResult {
            exit_code: Some(1),
            log: "npm failed".to_string(),
        },
    });
    let state = ServerState::with_paths_and_npm_install_runner(
        "test-instance".to_string(),
        RuntimePaths::from_dir(path.clone()),
        runner,
    )
    .expect("state");
    state.set_provider_update_catalog(crate::provider_catalog::ProviderDiscovery {
        agents: vec![update_test_agent(
            "qwen",
            Some(package),
            true,
            crate::provider_catalog::InstallChannel::Npm,
            executable.clone(),
        )],
        unreadable_dirs: 0,
    });
    state.set_provider_update_npm_command(std::path::PathBuf::from(r"C:\fake\npm.cmd"), vec![]);
    let fingerprint = executable_fingerprint(&executable).expect("fingerprint");
    state.record_provider_cli_version("qwen", "2.0.0", fingerprint);
    struct FakeNpmVersion;
    impl crate::registry::NpmVersionFetch for FakeNpmVersion {
        fn latest(&self, _package: &str) -> Result<String, String> {
            Ok("8.8.8".to_string())
        }
    }
    assert_eq!(
        crate::registry::load_latest_npm_version(&FakeNpmVersion, package, true),
        Some("8.8.8".to_string())
    );

    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(50);
    assert!(dispatch(
        &state,
        &owner,
        ClientMessage::ProviderUpdate {
            id: 51,
            provider_id: "qwen".to_string(),
        },
        &conn,
        false,
        false,
        false,
        false,
    )
    .is_none());
    assert_eq!(
        wait_for_worker_reply(&conn),
        DaemonMessage::ProviderUpdated {
            id: 51,
            ok: false,
            exit_code: Some(1),
            log: "npm failed".to_string(),
        }
    );
    assert_eq!(
        state.provider_cli_version("qwen", &executable),
        Some("2.0.0".to_string()),
        "failed update must preserve the --version cache entry"
    );
    assert_eq!(
        crate::registry::cached_latest_npm_version(package),
        Some("8.8.8".to_string()),
        "failed update must preserve the npm latest cache entry"
    );
    crate::registry::reset_npm_version_cache(package);
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn provider_update_refuses_native_even_when_a_package_is_known() {
    let state = state();
    state.set_provider_update_catalog(crate::provider_catalog::ProviderDiscovery {
        agents: vec![update_test_agent(
            "claude",
            Some("@anthropic-ai/claude-code"),
            true,
            crate::provider_catalog::InstallChannel::Native,
            std::path::PathBuf::from(r"C:\Program Files\claude.exe"),
        )],
        unreadable_dirs: 0,
    });
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(44);
    assert!(dispatch(
        &state,
        &owner,
        ClientMessage::ProviderUpdate {
            id: 45,
            provider_id: "claude".to_string(),
        },
        &conn,
        false,
        false,
        false,
        false,
    )
    .is_none());
    let DaemonMessage::Error(error) = wait_for_worker_reply(&conn) else {
        panic!("native provider update must return an InvalidRequest");
    };
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("native installation"));
}

#[test]
fn provider_update_refuses_the_native_debug_stub_before_package_lookup() {
    let state = state();
    state.set_provider_update_catalog(crate::provider_catalog::ProviderDiscovery {
        agents: vec![update_test_agent(
            "devboule-acp-stub",
            None,
            true,
            crate::provider_catalog::InstallChannel::Native,
            std::path::PathBuf::from(r"C:\devboule-acp-stub.exe"),
        )],
        unreadable_dirs: 0,
    });
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(46);
    assert!(dispatch(
        &state,
        &owner,
        ClientMessage::ProviderUpdate {
            id: 47,
            provider_id: "devboule-acp-stub".to_string(),
        },
        &conn,
        false,
        false,
        false,
        false,
    )
    .is_none());
    let DaemonMessage::Error(error) = wait_for_worker_reply(&conn) else {
        panic!("native debug stub update must return an InvalidRequest");
    };
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("native installation"));
}

#[test]
fn provider_update_reports_missing_npm_without_invoking_the_runner() {
    let path = crate::test_dirs::test_temp_dir("devboule-provider-update-missing-npm");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let runner = Arc::new(RecordingNpmRunner {
        calls: Arc::clone(&calls),
        result: crate::provider_update::NpmInstallResult {
            exit_code: Some(0),
            log: "must not run".to_string(),
        },
    });
    let state = ServerState::with_paths_and_npm_install_runner(
        "test-instance".to_string(),
        RuntimePaths::from_dir(path.clone()),
        runner,
    )
    .expect("state");
    state.set_provider_update_catalog(crate::provider_catalog::ProviderDiscovery {
        agents: vec![update_test_agent(
            "codex",
            Some("@openai/codex"),
            false,
            crate::provider_catalog::InstallChannel::Npm,
            std::path::PathBuf::new(),
        )],
        unreadable_dirs: 0,
    });
    state.set_provider_update_npm_missing();
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(48);
    assert!(dispatch(
        &state,
        &owner,
        ClientMessage::ProviderUpdate {
            id: 49,
            provider_id: "codex".to_string(),
        },
        &conn,
        false,
        false,
        false,
        false,
    )
    .is_none());
    assert_eq!(
        wait_for_worker_reply(&conn),
        DaemonMessage::ProviderUpdated {
            id: 49,
            ok: false,
            exit_code: None,
            log: "npm was not found on PATH; install Node.js/npm and try again.".to_string(),
        }
    );
    assert!(calls
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_empty());
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn version_token_parser_finds_first_semver_like_token() {
    assert_eq!(
        parse_version_token(b"grok version 1.2.3\n"),
        Some("1.2.3".to_string())
    );
    assert_eq!(
        parse_version_token(b"v2.10.0-beta.1"),
        Some("2.10.0".to_string())
    );
    assert_eq!(parse_version_token(b"build 2026-09-06"), None);
    assert_eq!(parse_version_token(b"no version here"), None);
}

#[test]
fn cli_version_cache_is_invalidated_when_executable_metadata_changes() {
    let cached = CliVersionFingerprint {
        modified: UNIX_EPOCH + Duration::from_secs(10),
        len: 100,
    };
    assert!(cli_version_cache_is_current(&cached, Some(&cached)));
    assert!(!cli_version_cache_is_current(
        &cached,
        Some(&CliVersionFingerprint {
            modified: UNIX_EPOCH + Duration::from_secs(11),
            len: 100,
        })
    ));
    assert!(!cli_version_cache_is_current(
        &cached,
        Some(&CliVersionFingerprint {
            modified: cached.modified,
            len: 101,
        })
    ));
    assert!(!cli_version_cache_is_current(&cached, None));
}

#[test]
fn journal_commands_dispatch_and_reject_invalid_retention_patches() {
    let path = crate::test_dirs::test_temp_dir("devboule-command-test");
    let state = ServerState::with_paths(
        "test-instance".to_string(),
        RuntimePaths::from_dir(path.clone()),
    )
    .expect("state");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(2);

    let usage = dispatch(
        &state,
        &owner,
        ClientMessage::JournalUsage { id: 1 },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("immediate dispatch reply");
    assert!(matches!(usage, DaemonMessage::JournalUsage { id: 1, .. }));
    let retention = dispatch(
        &state,
        &owner,
        ClientMessage::JournalRetentionGet { id: 2 },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("immediate dispatch reply");
    assert!(matches!(
        retention,
        DaemonMessage::JournalRetention { id: 2, .. }
    ));

    for patch in [
        RetentionPatch {
            max_age_ms: Some(-1),
            max_bytes: None,
            max_sessions: None,
            session_max_bytes: None,
        },
        RetentionPatch {
            max_age_ms: None,
            max_bytes: Some(-1),
            max_sessions: None,
            session_max_bytes: None,
        },
        RetentionPatch {
            max_age_ms: None,
            max_bytes: None,
            max_sessions: Some(-1),
            session_max_bytes: None,
        },
        RetentionPatch {
            max_age_ms: None,
            max_bytes: None,
            max_sessions: None,
            session_max_bytes: Some(-1),
        },
        RetentionPatch {
            max_age_ms: None,
            max_bytes: Some(10),
            max_sessions: None,
            session_max_bytes: Some(11),
        },
    ] {
        let RetentionPatch {
            max_age_ms,
            max_bytes,
            max_sessions,
            session_max_bytes,
        } = patch;
        let reply = dispatch(
            &state,
            &owner,
            ClientMessage::JournalRetentionSet {
                id: 3,
                max_age_ms,
                max_bytes,
                max_sessions,
                session_max_bytes,
                idempotency_key: None,
            },
            &conn,
            true,
            true,
            true,
            true,
        )
        .expect("immediate dispatch reply");
        assert!(matches!(
            reply,
            DaemonMessage::Error(WireError {
                code: ErrorCode::InvalidRequest,
                id: Some(3),
                ..
            })
        ));
    }
    let db = rusqlite::Connection::open(RuntimePaths::from_dir(path.clone()).journal_file())
        .expect("open journal for live row");
    db.execute(
        "INSERT INTO sessions (
            id, owner, kind, title, created_at_ms, updated_at_ms, generation,
            status, closed, last_seq, degraded, payload_bytes, unsnapshotted_bytes, reaped
         ) VALUES (?1, ?2, 'terminal', 'Live', 1, 1, 1, 'live', 0, 0, 0, 0, 0, 0)",
        ["s.test-client.live", "test-user"],
    )
    .expect("insert live row");
    drop(db);
    let delete = dispatch(
        &state,
        &owner,
        ClientMessage::SessionDelete {
            id: 4,
            session_id: "s.test-client.live".to_string(),
            idempotency_key: None,
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("immediate dispatch reply");
    assert!(matches!(
        delete,
        DaemonMessage::Error(WireError {
            code: ErrorCode::InvalidRequest,
            id: Some(4),
            message,
            ..
        }) if message == "Close the session before deleting it."
    ));
    assert!(state
        .sessions
        .list(&owner)
        .expect("list after refused delete")
        .iter()
        .any(|session| session.id == "s.test-client.live"));
    state.sessions.flush_journal();
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn journal_mutations_replay_idempotently() {
    let path = crate::test_dirs::test_temp_dir("devboule-idempotency-test");
    let state = ServerState::with_paths(
        "test-instance".to_string(),
        RuntimePaths::from_dir(path.clone()),
    )
    .expect("state");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(3);

    let first_retention = dispatch(
        &state,
        &owner,
        ClientMessage::JournalRetentionSet {
            id: 1,
            max_age_ms: None,
            max_bytes: Some(20_000),
            max_sessions: None,
            session_max_bytes: Some(10_000),
            idempotency_key: Some("retention-once".to_string()),
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("immediate dispatch reply");
    assert!(matches!(
        first_retention,
        DaemonMessage::JournalRetention { id: 1, .. }
    ));
    let replayed_retention = dispatch(
        &state,
        &owner,
        ClientMessage::JournalRetentionSet {
            id: 2,
            max_age_ms: None,
            max_bytes: Some(20_000),
            max_sessions: None,
            session_max_bytes: Some(10_000),
            idempotency_key: Some("retention-once".to_string()),
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("immediate dispatch reply");
    assert!(matches!(
        replayed_retention,
        DaemonMessage::JournalRetention { id: 2, .. }
    ));
    let conflict = dispatch(
        &state,
        &owner,
        ClientMessage::JournalRetentionSet {
            id: 3,
            max_age_ms: None,
            max_bytes: Some(20_001),
            max_sessions: None,
            session_max_bytes: Some(10_000),
            idempotency_key: Some("retention-once".to_string()),
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("immediate dispatch reply");
    assert!(matches!(
        conflict,
        DaemonMessage::Error(WireError {
            code: ErrorCode::IdempotencyConflict,
            id: Some(3),
            ..
        })
    ));

    let db = rusqlite::Connection::open(RuntimePaths::from_dir(path.clone()).journal_file())
        .expect("open journal for deleted row");
    db.execute(
        "INSERT INTO sessions (
            id, owner, kind, title, created_at_ms, updated_at_ms, generation,
            status, closed, last_seq, degraded, payload_bytes, unsnapshotted_bytes, reaped
         ) VALUES (?1, ?2, 'terminal', 'Deleted', 1, 1, 1, 'ended', 0, 0, 0, 0, 0, 0)",
        ["s.test-client.idempotent-delete", "test-user"],
    )
    .expect("insert deleted row");
    drop(db);
    let first_delete = dispatch(
        &state,
        &owner,
        ClientMessage::SessionDelete {
            id: 4,
            session_id: "s.test-client.idempotent-delete".to_string(),
            idempotency_key: Some("delete-once".to_string()),
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("immediate dispatch reply");
    assert!(matches!(first_delete, DaemonMessage::Ok { id: 4 }));
    let replayed_delete = dispatch(
        &state,
        &owner,
        ClientMessage::SessionDelete {
            id: 5,
            session_id: "s.test-client.idempotent-delete".to_string(),
            idempotency_key: Some("delete-once".to_string()),
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("immediate dispatch reply");
    assert!(matches!(replayed_delete, DaemonMessage::Ok { id: 5 }));
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// The peer gate must return **before** the `ProvidersRefresh` and
/// `ProviderUpdate` spawns, so a remote peer can never trigger an npm
/// install or a catalog refresh on this device (design §8b A1).
///
/// Two assertions make this real rather than a claim about a reply's shape:
///
/// 1. the same `dispatch` call from a **local** connection returns `None`
///    for these variants, which is the async wrapper's own signature for
///    "I spawned a worker" — so `Some(Error)` from a peer is the gate
///    returning early, not the variant being synchronous by nature; and
/// 2. the update path's process-launch seam is a recording runner, and a
///    remote `ProviderUpdate` leaves it with no new invocation while a
///    local one adds one. That is the side effect the gate exists to
///    prevent, observed directly.
#[test]
fn the_peer_gate_denies_the_destructive_set_before_any_spawn() {
    let path = crate::test_dirs::test_temp_dir("devboule-peer-gate");
    let executable = path.join("codex.cmd");
    std::fs::write(&executable, b"shim").expect("fake executable");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let runner = Arc::new(RecordingNpmRunner {
        calls: Arc::clone(&calls),
        result: crate::provider_update::NpmInstallResult {
            exit_code: Some(0),
            log: String::new(),
        },
    });
    let state = ServerState::with_paths_and_npm_install_runner(
        "test-instance".to_string(),
        RuntimePaths::from_dir(path.clone()),
        runner,
    )
    .expect("state");
    state.set_provider_update_catalog(crate::provider_catalog::ProviderDiscovery {
        agents: vec![update_test_agent(
            "codex",
            Some("@openai/codex"),
            true,
            crate::provider_catalog::InstallChannel::Npm,
            executable.clone(),
        )],
        unreadable_dirs: 0,
    });
    state.set_provider_update_npm_command(std::path::PathBuf::from(r"C:\fake\npm.cmd"), vec![]);

    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let local = ConnHandle::new(1);
    let remote = remote_conn(PeerRole::Daemon, None);

    // 1. The local path really does spawn, for both async variants. This is
    //    the control that makes the peer assertion below meaningful.
    for request in [
        ClientMessage::ProvidersRefresh { id: 2 },
        ClientMessage::ProviderUpdate {
            id: 3,
            provider_id: "codex".to_string(),
        },
    ] {
        let name = request.name();
        assert!(
            dispatch(&state, &owner, request, &local, true, true, true, true).is_none(),
            "the local path spawns a worker for {name}: `None` is how the async wrapper says so"
        );
    }
    // The local update reaches the runner, which proves the seam is armed.
    let deadline = Instant::now() + Duration::from_secs(5);
    while calls.lock().unwrap_or_else(|e| e.into_inner()).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let local_calls = calls.lock().unwrap_or_else(|e| e.into_inner()).len();
    assert_eq!(
        local_calls, 1,
        "a local ProviderUpdate must reach the npm runner"
    );

    // 2. Every one of these is refused to a peer, synchronously.
    let requests = [
        ClientMessage::Shutdown { id: 1 },
        ClientMessage::ProvidersRefresh { id: 2 },
        ClientMessage::ProviderUpdate {
            id: 3,
            provider_id: "codex".to_string(),
        },
        ClientMessage::Status { id: 4 },
    ];
    for request in requests {
        let name = request.name();
        let reply = dispatch(&state, &owner, request, &remote, true, true, true, true)
            .unwrap_or_else(|| panic!("the gate must answer {name} instead of spawning a worker"));
        match reply {
            DaemonMessage::Error(error) => assert_eq!(
                error.code,
                ErrorCode::CapabilityNotSupported,
                "unexpected refusal for {name}: {error:?}"
            ),
            other => panic!("expected CapabilityNotSupported for {name}, got {other:?}"),
        }
    }

    // Give a thread that should not exist a moment to prove otherwise: if
    // the gate had let the update through, this is where its npm call would
    // land, and the count would move past the local one.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        calls.lock().unwrap_or_else(|e| e.into_inner()).len(),
        local_calls,
        "a remote ProviderUpdate must not start an npm install"
    );

    drop(state);
    assert_eq!(
        audit_rows(&path),
        vec![
            "Shutdown:denied",
            "ProvidersRefresh:denied",
            "ProviderUpdate:denied",
            "Status:denied",
        ]
    );
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn an_allowed_read_writes_no_audit_row() {
    let (path, state) = temp_state("peer-ping");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = remote_conn(PeerRole::Client, Some("S-1-5-21-1"));
    for id in 0..20 {
        let reply = dispatch(
            &state,
            &owner,
            ClientMessage::Ping { id },
            &conn,
            true,
            true,
            true,
            true,
        )
        .expect("ping replies");
        assert!(matches!(reply, DaemonMessage::Pong { .. }));
    }
    drop(state);
    assert!(
        audit_rows(&path).is_empty(),
        "20 allowed pings must not write an audit row"
    );
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn a_remote_sessions_list_is_projected_by_role_and_paired_user() {
    let (path, state) = temp_state("peer-sessions");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    {
        let journal = state.journal.as_ref().expect("journal");
        journal
            .upsert_blocking(new_session_record(
                "s.user-a.1",
                "S-user-a",
                None,
                SessionKind::Terminal,
                "A",
            ))
            .expect("session a");
        journal
            .upsert_blocking(new_session_record(
                "s.user-b.1",
                "S-user-b",
                None,
                SessionKind::Terminal,
                "B",
            ))
            .expect("session b");
    }

    let ids = |conn: &Arc<ConnHandle>| match dispatch(
        &state,
        &owner,
        ClientMessage::SessionsList { id: 1 },
        conn,
        true,
        true,
        true,
        true,
    )
    .expect("sessions reply")
    {
        DaemonMessage::Sessions { sessions, .. } => sessions
            .into_iter()
            .map(|session| session.id)
            .collect::<Vec<_>>(),
        other => panic!("expected Sessions, got {other:?}"),
    };

    let ids_error = |conn: &Arc<ConnHandle>| match dispatch(
        &state,
        &owner,
        ClientMessage::SessionsList { id: 1 },
        conn,
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::Error(error) => (error.code, error.message),
        other => panic!("expected an error, got {other:?}"),
    };

    assert_eq!(
        ids(&remote_conn_with_caps(
            PeerRole::Client,
            Some("S-user-a"),
            &[crate::peer_policy::CAP_VIEW]
        )),
        vec!["s.user-a.1".to_string()]
    );
    assert_eq!(
        ids(&remote_conn_with_caps(
            PeerRole::Client,
            Some("S-user-b"),
            &[crate::peer_policy::CAP_VIEW]
        )),
        vec!["s.user-b.1".to_string()]
    );
    assert!(ids(&remote_conn_with_caps(
        PeerRole::Client,
        None,
        &[crate::peer_policy::CAP_VIEW]
    ))
    .is_empty());
    assert!(ids(&remote_conn_with_caps(
        PeerRole::Daemon,
        Some("S-user-a"),
        &[crate::peer_policy::CAP_VIEW]
    ))
    .is_empty());
    // H10: without `view` the list is not an unconditional read any more.
    // The refusal is the capability gate's, and it names `view`.
    assert_eq!(
        ids_error(&remote_conn(PeerRole::Client, Some("S-user-a"))),
        (
            ErrorCode::CapabilityNotSupported,
            format!(
                "capability '{}' was not negotiated",
                crate::peer_policy::CAP_VIEW
            )
        )
    );

    // A read is a read: the four allowed lists above wrote no audit row.
    // The one denial did — the capability gate's trail, and the only row
    // this test produces.
    drop(state);
    assert_eq!(audit_rows(&path), vec!["SessionsList:denied"]);
    let _ = std::fs::remove_dir_all(path);
}

/// The initiator path must be reachable from the dispatch gate, not just
/// compiled. The address is a closed port, so the pairing fails at connect:
/// what this asserts is that the request reaches `PairingService::complete`
/// (an error, not a `CapabilityNotSupported` refusal and not a panic) and
/// that the failure text never echoes the code the caller typed.
#[test]
fn pairing_complete_reaches_the_initiator_and_never_echoes_the_code() {
    use std::net::TcpListener;

    let (path, state) = temp_state("pairing-complete");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn = ConnHandle::new(3);
    assert!(
        state
            .set_peer_transport(Arc::new(crate::peer_transport::TestTransport::default()))
            .is_ok(),
        "the stub transport is installed before anything can choose the real one"
    );

    // A port that was bound and then released: connecting is refused rather
    // than left hanging.
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr").port()
    };
    let address = std::net::SocketAddr::new("127.0.0.1".parse().expect("ip"), port).to_string();

    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::PairingComplete {
            id: 5,
            address,
            code: devboule_protocol::PairingSecret::new("ABCD2345"),
            role: PeerRole::Client,
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("immediate dispatch reply");
    match reply {
        DaemonMessage::Error(error) => {
            assert_eq!(error.id, Some(5));
            assert_ne!(
                error.code,
                ErrorCode::CapabilityNotSupported,
                "the devices capability is negotiated here, so this must not be a policy refusal"
            );
            assert!(
                !error.message.contains("ABCD2345"),
                "a pairing failure must never carry the code: {}",
                error.message
            );
        }
        other => panic!("expected a pairing failure, got {other:?}"),
    }

    // A code the alphabet cannot express is refused before any socket work.
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::PairingComplete {
            id: 6,
            address: "127.0.0.1:1".to_string(),
            code: devboule_protocol::PairingSecret::new("aaaa0000"),
            role: PeerRole::Client,
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .expect("immediate dispatch reply");
    match reply {
        DaemonMessage::Error(error) => {
            assert!(!error.message.contains("aaaa0000"), "{}", error.message)
        }
        other => panic!("expected a malformed-code refusal, got {other:?}"),
    }

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn status_carries_the_secret_store_selector_and_a_remote_state() {
    let (path, state) = temp_state("status");
    let _ = state.secret_store();

    let remote_of = |state: &Arc<ServerState>| match state.status_body(9) {
        DaemonMessage::Status { body, .. } => body.remote.expect("remote is always reported"),
        other => panic!("expected Status, got {other:?}"),
    };

    // The selector is one of exactly two values, and it is reported.
    match state.status_body(9) {
        DaemonMessage::Status { body, .. } => assert!(
            matches!(body.secret_store.as_deref(), Some("keyring" | "file")),
            "unexpected selector: {:?}",
            body.secret_store
        ),
        other => panic!("expected Status, got {other:?}"),
    }

    // `disabled` is the start-up state, with a reason.
    let disabled = remote_of(&state);
    assert_eq!(disabled.state, devboule_protocol::RemoteStateKind::Disabled);
    assert!(
        disabled.reason.is_some(),
        "a disabled state explains itself"
    );

    // `enabled`: the listener is up. Status carries the state and reason
    // only — the addresses and port live in `SelfInfo`.
    state.set_remote_state(RemoteState::Enabled {
        addresses: vec!["100.102.128.70".parse().expect("ip")],
        port: 47831,
    });
    let enabled = remote_of(&state);
    assert_eq!(enabled.state, devboule_protocol::RemoteStateKind::Enabled);
    assert!(
        enabled.reason.is_none(),
        "an enabled listener has nothing to explain: {enabled:?}"
    );
    let json = serde_json::to_value(&enabled).expect("json");
    assert_eq!(json["state"], "enabled");
    assert!(json["reason"].is_null(), "the key is present as null");

    // `key_missing`: device.json exists but the private key does not. This
    // is a refusal to guess, and it must be distinguishable from
    // `disabled` because the remedy differs (re-pair vs start Tailscale).
    state.set_remote_state(RemoteState::KeyMissing);
    let missing = remote_of(&state);
    assert_eq!(
        missing.state,
        devboule_protocol::RemoteStateKind::KeyMissing
    );
    assert!(missing.reason.is_some());
    assert_ne!(
        serde_json::to_value(&missing).expect("json")["state"],
        "disabled",
        "a missing key is not the same state as a disabled listener"
    );

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// The leak that this guards: a state built by a test must not reach the OS
/// credential store.
///
/// `ServerState::initial_secret_store` pins a test build to the file store,
/// so the identity lands in `<runtime dir>/secrets/noise-static.bin` and
/// dies with the temp dir. The credential store is deliberately not read
/// here — reading it is still touching it, and keeping this suite out of it
/// is the point; `cmdkey /list | Select-String noise-static-` from outside
/// the process is the gate that observes that, and the store selector plus
/// the file path below are what is observable from in here.
///
/// See `reports/remote-agents/keyring-test-leak-fix-report.md`.
#[test]
fn a_test_built_state_uses_the_file_store_and_never_the_credential_store() {
    let (path, state) = temp_state("secret-store-pin");
    assert_eq!(
        state.secret_store().1,
        "file",
        "a test build must select the file store, never the credential store"
    );

    let identity = state.device_identity().as_ref().expect("identity");
    let stored = path.join("secrets").join("noise-static.bin");
    let bytes = std::fs::read(&stored).expect("the static key under the runtime dir");
    // The bytes under the temp dir are this identity's own envelope: the
    // store is rooted in this state's runtime directory, not somewhere else.
    let expected = crate::device_identity::encode_envelope(identity.private_key());
    assert_eq!(bytes.as_slice(), &expected[..]);
    // ...and the path is the one the file store derives for that name, so a
    // later test cannot satisfy this through some other mechanism.
    assert_eq!(
        crate::secret_store::FileStore::new(&path)
            .path_for(crate::device_identity::NOISE_STATIC_SECRET_NAME)
            .expect("plain secret name"),
        stored
    );

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}
/// M1: the accept path must not hit the journal per accepted socket. The
/// table is loaded once and reused, and every peer mutation drops it so the
/// next connection sees the change.
#[test]
fn the_peer_table_is_loaded_once_and_refreshed_on_change() {
    let (path, state) = temp_state("peer-table-cache");

    // First read loads; the next N reads within the TTL do not.
    let first = state.peer_table().expect("first load");
    assert!(first.rows().is_empty(), "no peers yet");
    let loads_after_first = state.peer_table_loads();
    for _ in 0..16 {
        state.peer_table().expect("cached read");
    }
    assert_eq!(
        state.peer_table_loads(),
        loads_after_first,
        "16 reads within the TTL must not reload the table"
    );

    // A pairing invalidates it: the next read sees the new row.
    state
        .peer_upsert(PeerRecord {
            device_id: "6f1e5b7a-0000-4000-8000-00000000c0db".to_string(),
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
            caps: vec!["view".to_string()],
        })
        .expect("store a peer");
    let after_pairing = state.peer_table().expect("reload after pairing");
    assert_eq!(after_pairing.rows().len(), 1, "the new peer is visible");
    assert_eq!(
        state.peer_table_loads(),
        loads_after_first + 1,
        "a pairing loads exactly once more"
    );

    // A revoke invalidates it too: the row stops owning its address.
    state
        .peer_revoke("6f1e5b7a-0000-4000-8000-00000000c0db", 2)
        .expect("revoke");
    let after_revoke = state.peer_table().expect("reload after revoke");
    assert_eq!(
        after_revoke.rows().len(),
        1,
        "the revoked row is still listed"
    );
    assert!(
        after_revoke
            .by_address(&"100.64.0.2".parse().expect("ip"))
            .is_none(),
        "a revoked peer's address must no longer pass the filter"
    );
    assert_eq!(state.peer_table_loads(), loads_after_first + 2);

    // And the TTL is a backstop for a mutation path that forgets to
    // invalidate: a zero TTL forces the next read to reload.
    state.set_peer_table_ttl(Duration::ZERO);
    state.peer_table().expect("reload after ttl");
    assert_eq!(state.peer_table_loads(), loads_after_first + 3);

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// C5: the listener is a resource the state owns, started idempotently and
/// stoppable. This is the machinery `PairingStart` relies on when it retries
/// after the user starts Tailscale, exercised here with the stub transport
/// (which binds loopback) so it needs no Tailscale.
#[test]
fn the_remote_listener_starts_once_and_stops() {
    let (path, state) = temp_state("listener-lifecycle");
    // `is_ok()`, not `expect`: the error is an `Arc<dyn PeerTransport>`,
    // which is not `Debug` and so cannot be printed by `expect`.
    assert!(
        state
            .set_peer_transport(Arc::new(crate::peer_transport::TestTransport::default()))
            .is_ok(),
        "the stub transport is installed before anything picks the real one"
    );

    // Starts, and reports `Enabled` with the bound port.
    assert!(state.ensure_remote_listener(), "the first call starts it");
    assert_eq!(state.listener_starts(), 1);
    assert!(state.has_remote_listener());
    let enabled = state.remote_state();
    assert_eq!(
        enabled.state,
        devboule_protocol::RemoteStateKind::Enabled,
        "a started listener is reported as enabled"
    );
    assert!(enabled.reason.is_none(), "nothing to explain when it is up");

    // Idempotent: further calls do not start a second listener.
    for _ in 0..3 {
        assert!(state.ensure_remote_listener());
    }
    assert_eq!(
        state.listener_starts(),
        1,
        "ensure_remote_listener must not start a second listener"
    );

    // Stopping takes it down, and a start afterwards is refused because the
    // daemon is shutting down (the stop flag would make a new loop exit at
    // once, so `listening` would be a lie).
    state.stop_remote_listener();
    assert!(!state.has_remote_listener());
    assert!(
        !state.ensure_remote_listener(),
        "a listener must not be started after it has been stopped"
    );

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// A capability a peer does not hold is the whole answer: the request is
/// refused before anything looks at what it would do (§8b A9/A11). The
/// same request with the capability is answered by the session layer —
/// which is what proves the gate, and not the missing session, stopped it.
#[test]
fn a_peer_request_the_caps_do_not_open_never_reaches_the_session_layer() {
    let (path, state) = temp_state("peer-caps-first");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let send = || ClientMessage::SessionSend {
        id: 1,
        session_id: "s.none.1".to_string(),
        subscription_id: 1,
        text: "hello".to_string(),
        attachments: Vec::new(),
        active_turn_behavior: None,
        attachment_references: Vec::new(),
        idempotency_key: None,
    };

    let viewer = remote_conn_with_caps(
        PeerRole::Client,
        Some("S-user-a"),
        &[crate::peer_policy::CAP_VIEW],
    );
    match dispatch(&state, &owner, send(), &viewer, true, true, true, true)
        .expect("the gate answers")
    {
        DaemonMessage::Error(error) => {
            assert_eq!(error.code, ErrorCode::CapabilityNotSupported, "{error:?}")
        }
        other => panic!("a view-only peer must not reach a session: {other:?}"),
    }

    let sender = remote_conn_with_caps(
        PeerRole::Client,
        Some("S-user-a"),
        &[crate::peer_policy::CAP_VIEW, crate::peer_policy::CAP_SEND],
    );
    match dispatch(&state, &owner, send(), &sender, true, true, true, true)
        .expect("the gate answers")
    {
        DaemonMessage::Error(error) => assert_ne!(
            error.code,
            ErrorCode::CapabilityNotSupported,
            "with the capability the answer comes from the session layer: {error:?}"
        ),
        other => panic!("expected a session-layer error, got {other:?}"),
    }

    drop(state);
    // The refusal is recorded, and the allowed send that followed is
    // recorded as the decision it was: the trail distinguishes the two.
    let rows = audit_rows(&path);
    assert_eq!(
        rows.first().map(String::as_str),
        Some("SessionSend:denied"),
        "the capability refusal comes first: {rows:?}"
    );
    assert_eq!(
        rows.len(),
        2,
        "the refusal, then the allowed send: {rows:?}"
    );
    let _ = std::fs::remove_dir_all(path);
}

/// §8b A9/A11 for the agent-message frame: the capability that names the act
/// is the one that opens it, and `view` is not it.
#[test]
fn an_agent_message_from_a_view_only_peer_is_refused() {
    let (path, state) = temp_state("peer-agent-message-caps");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let viewer = remote_conn_with_caps(
        PeerRole::Client,
        Some("S-user-a"),
        &[crate::peer_policy::CAP_VIEW],
    );
    match dispatch(
        &state,
        &owner,
        ClientMessage::AgentMessageSend {
            id: 1,
            from_session: "s.msg.source".to_string(),
            to_session: "s.msg.target".to_string(),
            text: "hello".to_string(),
            idempotency_key: None,
        },
        &viewer,
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::Error(error) => {
            assert_eq!(error.code, ErrorCode::CapabilityNotSupported, "{error:?}");
            assert!(error.message.contains("send"), "{error:?}");
        }
        other => panic!("a view-only peer must not send an agent message: {other:?}"),
    }
    drop(state);
    assert_eq!(audit_rows(&path), vec!["AgentMessageSend:denied"]);
    let _ = std::fs::remove_dir_all(path);
}

/// Every receipt state the frame can answer with, each produced by the path
/// that produces it: a delivered message, an unknown target, and a paired
/// device refused a message it may not send (A2-07).
///
/// `RejectedUnpaired` stays in the enum but is no longer produced here: an
/// unpaired connection never reaches this dispatch — the peer gate refuses a
/// request the device's capability set does not open, before anything looks
/// at what the request would do — so a refusal that arrives as
/// `ErrorCode::Unauthorized` is a *denied* caller, not an unpaired one, and
/// the receipt says what happened rather than blaming the pairing.
#[test]
fn every_agent_message_receipt_state_is_produced() {
    let (path, state) = temp_state("agent-message-receipts");
    let owner = OwnerId::new("S-user-a", "test-client").expect("owner");
    let peer = remote_conn_with_caps(
        PeerRole::Client,
        Some("S-user-a"),
        &[crate::peer_policy::CAP_VIEW, crate::peer_policy::CAP_SEND],
    );
    let local = ConnHandle::new(11);
    crate::session::insert_test_live_agent_with_writer(
        &state.sessions,
        "s.msg.source",
        owner.clone(),
        SessionKind::Pi,
        Box::new(std::io::sink()),
    );
    crate::session::insert_test_live_agent_with_writer(
        &state.sessions,
        "s.msg.target",
        owner.clone(),
        SessionKind::Pi,
        Box::new(std::io::sink()),
    );
    // A session of the other account: a remote frame's source is far and is
    // not resolved here. The target below is already in the turn started by
    // the local send, so this receipt exercises the paired device's refusal
    // to interrupt that turn, not source ownership.
    let other = OwnerId::new("S-user-b", "test-client").expect("owner");
    crate::session::insert_test_live_agent(&state.sessions, "s.msg.foreign", other);

    let receipt = |from: &str, to: &str, conn: &Arc<ConnHandle>| match dispatch(
        &state,
        &owner,
        ClientMessage::AgentMessageSend {
            id: 1,
            from_session: from.to_string(),
            to_session: to.to_string(),
            text: "hello".to_string(),
            idempotency_key: None,
        },
        conn,
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::AgentMessageReceipt {
            state: receipt_state,
            ..
        } => Some(receipt_state),
        other => panic!("expected a receipt, got {other:?}"),
    };

    assert_eq!(
        receipt("s.msg.source", "s.msg.target", &local),
        Some(AgentMessageState::Accepted),
        "a live target takes the envelope"
    );
    assert_eq!(
        receipt("s.msg.source", "s.none.1", &local),
        Some(AgentMessageState::RejectedAbsent),
        "an unknown target is the absence the receipt names"
    );
    assert_eq!(
        receipt("s.msg.foreign", "s.msg.target", &peer),
        Some(AgentMessageState::RejectedDenied),
        "a paired device cannot interrupt a target turn when steer is refused"
    );

    drop(state);
    let rows = audit_rows(&path);
    assert!(
        rows.contains(&"AgentMessageSend:denied".to_string()),
        "the paired device's refusal is in the trail: {rows:?}"
    );
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn a_daemon_peer_uses_far_sender_ids_and_refuses_relays_at_the_gate() {
    let (path, state) = temp_state("agent-message-remote-sender");
    let owner = OwnerId::new("peer_dev-peer-1", "daemon").expect("owner");
    let peer = remote_conn_with_caps(
        PeerRole::Daemon,
        Some("local-user"),
        &[crate::peer_policy::CAP_VIEW, crate::peer_policy::CAP_SEND],
    );
    let received = crate::session::insert_test_live_agent_with_recording_writer(
        &state.sessions,
        "s.remote.local",
        OwnerId::new("local-user", "local-process").expect("local owner"),
        SessionKind::Pi,
    );
    let third = crate::session::insert_test_live_agent_with_recording_writer(
        &state.sessions,
        "s.remote.third",
        owner.clone(),
        SessionKind::Pi,
    );
    state.sessions.set_test_origin(
        "s.remote.third",
        devboule_protocol::SessionOrigin::peer("dev-tablet", PeerRole::Daemon),
    );

    let accepted = dispatch(
        &state,
        &owner,
        ClientMessage::AgentMessageSend {
            id: 10,
            from_session: "s.far.source".to_string(),
            to_session: "s.remote.local".to_string(),
            text: "hello from the far daemon".to_string(),
            idempotency_key: None,
        },
        &peer,
        true,
        true,
        true,
        true,
    )
    .expect("the dispatch returns a receipt");
    assert!(matches!(
        accepted,
        DaemonMessage::AgentMessageReceipt {
            state: AgentMessageState::Accepted,
            ..
        }
    ));
    let envelope = String::from_utf8(received.lock().expect("received").clone())
        .expect("the envelope is utf8");
    assert!(envelope.contains("origin: peer:dev-peer-1"), "{envelope}");
    assert!(
        envelope.contains("from_agent: peer:dev-peer-1/s.far.source"),
        "{envelope}"
    );

    let denied = dispatch(
        &state,
        &owner,
        ClientMessage::AgentMessageSend {
            id: 11,
            from_session: "s.far.source".to_string(),
            to_session: "s.remote.third".to_string(),
            text: "must not relay".to_string(),
            idempotency_key: None,
        },
        &peer,
        true,
        true,
        true,
        true,
    )
    .expect("the dispatch returns the gate's denial");
    assert!(matches!(
        denied,
        DaemonMessage::Error(WireError {
            code: ErrorCode::Unauthorized,
            ..
        })
    ));
    assert!(third.lock().expect("third target").is_empty());
    assert_eq!(
        audit_rows(&path),
        vec![
            "AgentMessageSend:ok".to_string(),
            "AgentMessageSend:denied".to_string(),
        ],
        "the relay is denied at the gate without an earlier ok row"
    );

    let malformed = dispatch(
        &state,
        &owner,
        ClientMessage::AgentMessageSend {
            id: 12,
            from_session: "s.bad id".to_string(),
            to_session: "s.remote.local".to_string(),
            text: "must not write".to_string(),
            idempotency_key: None,
        },
        &peer,
        true,
        true,
        true,
        true,
    )
    .expect("the dispatch returns an invalid request");
    assert!(matches!(
        malformed,
        DaemonMessage::Error(WireError {
            code: ErrorCode::InvalidRequest,
            ..
        })
    ));
    let envelope_after_malformed = String::from_utf8(received.lock().expect("received").clone())
        .expect("the envelope is utf8");
    assert_eq!(envelope_after_malformed, envelope);

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// §8b A5 in the trail: a paired device asking for a mode that runs without
/// the prompt is refused, and the row says *that*, not a plain denial.
#[test]
fn a_peer_create_in_a_prompt_skipping_mode_is_refused_and_labelled() {
    let (path, state) = temp_state("peer-prompt-skipping");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let creator = remote_conn_with_caps(
        PeerRole::Client,
        Some("S-user-a"),
        &[
            crate::peer_policy::CAP_VIEW,
            crate::peer_policy::CAP_CREATE_SESSIONS,
        ],
    );

    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::SessionCreate {
            id: 1,
            workspace_id: None,
            kind: SessionKind::Claude,
            provider: Some("claude".to_string()),
            mode: Some("bypassPermissions".to_string()),
            display_name: None,
            idempotency_key: None,
        },
        &creator,
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers");
    match reply {
        DaemonMessage::Error(error) => {
            assert_eq!(error.code, ErrorCode::CapabilityNotSupported, "{error:?}");
            assert!(
                error.message.contains("permission prompt"),
                "the refusal must say why: {}",
                error.message
            );
        }
        other => panic!("a prompt-skipping create must be refused: {other:?}"),
    }

    drop(state);
    assert_eq!(
        audit_rows(&path),
        vec!["SessionCreate:prompt_skipping_refused"]
    );
    let _ = std::fs::remove_dir_all(path);
}

/// A4/A5/R3, the frame side of the same rule: which frames have a mode
/// question *at all*.
///
/// The match in `peer_mode_refusal_for_conn` is closed over `ClientMessage` with no
/// `_` arm, so the compiler is what proves every variant has an answer. What
/// this test adds is the frame list: `peer_policy`'s matrix carries one
/// sample per variant, pinned by `VARIANT_COUNT`, so walking it here asks
/// every frame the gate will ever see. A frame whose sample names no mode
/// must answer `None` — the gate has no verdict for a mode it does not name
/// The two closed matches over `ClientMessage` (`S5`, block 6), walked over
/// the peer matrix samples: every sample either names a session (and the id
/// is the frame's own) or is refused with one constant sentence, the request
/// id back, and nothing of the frame echoed.
#[test]
fn the_closed_session_matches_classify_every_matrix_sample() {
    let mut named: Vec<&'static str> = Vec::new();
    let mut refused: Vec<&'static str> = Vec::new();
    for frame in crate::peer_policy::tests::matrix_samples() {
        let name = frame.name();
        match request_session_id(&frame) {
            Some(session_id) => {
                assert!(!session_id.is_empty(), "{name} named an empty session id");
                named.push(name);
            }
            None => {
                let reply = unexpected_session_frame(&frame);
                let DaemonMessage::Error(error) = reply else {
                    panic!("{name} must be refused, got {reply:?}");
                };
                assert_eq!(error.code, ErrorCode::InvalidRequest, "{name}");
                assert_eq!(error.message, UNEXPECTED_SESSION_FRAME, "{name}");
                assert_eq!(error.id, frame.request_id(), "{name}");
                refused.push(name);
            }
        }
    }
    assert!(
        !named.is_empty() && !refused.is_empty(),
        "the samples must cover both halves of the classification"
    );
}

/// — and the samples that do name one are pinned as a list, so the two sides
/// cannot swap silently.
///
/// `SessionCreate`'s pinned sample carries `mode: None`, so it lands on the
/// mode-free side here; its other flavour is the existing test's subject,
/// which asserts the refusal for a named prompt-skipping mode.
#[test]
fn only_a_frame_that_names_a_mode_is_vetted_for_one() {
    let (path, state) = temp_state("peer-mode-sides");
    let mut vetted: Vec<&'static str> = Vec::new();
    for frame in crate::peer_policy::tests::matrix_samples() {
        let names_a_mode = matches!(
            &frame,
            ClientMessage::SessionCreate { mode: Some(_), .. }
                | ClientMessage::SessionSetMode { .. }
                | ClientMessage::SessionSend { .. }
                | ClientMessage::SessionAttach { .. }
                | ClientMessage::AgentMessageSend { .. }
        );
        if names_a_mode {
            vetted.push(frame.name());
            continue;
        }
        assert_eq!(
            peer_mode_refusal_for_conn(&state, &frame, &None),
            None,
            "{} carries no mode, so this gate has no verdict for it",
            frame.name()
        );
    }
    assert_eq!(
        vetted,
        vec![
            "SessionAttach",
            "SessionSend",
            "AgentMessageSend",
            "SessionSetMode"
        ],
        "the frames whose pinned sample names a mode, in `name()` order"
    );
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// The A4/A5/R3 decision itself, keyed on the frame's own facts. The
/// session-backed half (send, set-mode) is the registry guard — tested in
/// `session.rs` — composed with this same list. The answer is the audit
/// label, so the trail names the rule that fired.
#[test]
fn the_prompt_skipping_decision_reads_the_frame_and_refuses_unknown_sessions() {
    let (path, state) = temp_state("peer-prompt-skipping-table");
    let create = |kind: SessionKind, mode: &str| ClientMessage::SessionCreate {
        id: 1,
        workspace_id: None,
        kind,
        provider: None,
        // Built by the closure so one frame builder serves every case.
        display_name: None,
        mode: Some(mode.to_string()),
        idempotency_key: None,
    };

    // The label is part of the answer: the trail and the peer's error name
    // the same rule.
    assert_eq!(
        peer_mode_refusal_for_conn(
            &state,
            &create(SessionKind::Claude, "bypassPermissions"),
            &None
        ),
        Some(crate::peer_policy::PROMPT_SKIPPING_REFUSED)
    );
    assert_eq!(
        peer_mode_refusal_for_conn(&state, &create(SessionKind::Claude, "auto"), &None),
        Some(crate::peer_policy::PROMPT_SKIPPING_REFUSED)
    );
    assert_eq!(
        peer_mode_refusal_for_conn(&state, &create(SessionKind::Codex, "full-access"), &None),
        Some(crate::peer_policy::PROMPT_SKIPPING_REFUSED)
    );
    // Codex `auto` and Claude's `default` only prompt, so they are not
    // refusals — and a Terminal has no prompt to skip at all.
    assert_eq!(
        peer_mode_refusal_for_conn(&state, &create(SessionKind::Codex, "auto"), &None),
        None
    );
    assert_eq!(
        peer_mode_refusal_for_conn(&state, &create(SessionKind::Claude, "default"), &None),
        None
    );
    assert_eq!(
        peer_mode_refusal_for_conn(
            &state,
            &create(SessionKind::Terminal, "bypassPermissions"),
            &None,
        ),
        None
    );
    // §8b A5/R3, H1: an ACP create may not name *any* mode, because the
    // agent owns the ids and this daemon has no list to vet them against.
    for mode in ["auto_accept", "ask", "default"] {
        assert_eq!(
            peer_mode_refusal_for_conn(&state, &create(SessionKind::Acp, mode), &None),
            Some(crate::peer_policy::ACP_MODES_UNVETTED_REFUSED),
            "ACP mode {mode:?}"
        );
    }
    // ...and neither may an ACP *session* be switched into one.
    assert_eq!(
        peer_mode_refusal_for_conn(
            &state,
            &ClientMessage::SessionSetMode {
                id: 4,
                session_id: "s.nobody.1".to_string(),
                mode_id: "ask".to_string(),
            },
            &None,
        ),
        None,
        "unknown sessions are not a policy verdict here; `dispatch` refuses them first"
    );

    // A session this daemon does not know is not a policy verdict: the
    // answer for it is the ownership denial, which `dispatch` produces
    // before this function is consulted (H6).
    assert_eq!(
        peer_mode_refusal_for_conn(
            &state,
            &ClientMessage::SessionSend {
                id: 2,
                session_id: "s.nobody.1".to_string(),
                subscription_id: 1,
                text: "hello".to_string(),
                attachments: Vec::new(),
                active_turn_behavior: None,
                attachment_references: Vec::new(),
                idempotency_key: None,
            },
            &None,
        ),
        None
    );
    // The same holds for a mode: an unknown session is still not this
    // function's verdict, even when the mode named is one §8b A5 refuses
    // for a session the daemon does know.
    assert_eq!(
        peer_mode_refusal_for_conn(
            &state,
            &ClientMessage::SessionSetMode {
                id: 3,
                session_id: "s.nobody.1".to_string(),
                mode_id: "bypassPermissions".to_string(),
            },
            &None,
        ),
        None
    );

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// §8 R7 at the boundary: one gate, and every local fact in an error is
/// replaced for a peer while the person's own pipe sees it unchanged.
#[test]
fn a_remote_reply_is_redacted_at_the_boundary_and_a_local_one_is_not() {
    let path = "C:\\Users\\me\\AppData\\Local\\devboule\\journal.db";
    let digest = "a".repeat(64);
    // Quoted, because a path run deliberately swallows an unquoted tail
    // (`path_run_len` stops at punctuation, not at a colon or a space): the
    // digest has to be its own token for this test to be about the digest.
    let message =
        format!("could not write \"{path}\"; key {digest} was rejected by device dev-phone");
    let error = || WireError::new(ErrorCode::Io, message.clone());

    let local = redact_for_conn(&ConnHandle::new(1), DaemonMessage::Error(error()));
    match local {
        DaemonMessage::Error(error) => {
            assert_eq!(error.message, message, "the pipe is unchanged")
        }
        other => panic!("expected an error, got {other:?}"),
    }

    let remote = redact_for_conn(
        &remote_conn_with_caps(PeerRole::Client, Some("S-user-a"), &[]),
        DaemonMessage::Error(error()),
    );
    match remote {
        DaemonMessage::Error(error) => {
            assert!(
                !error.message.contains("C:\\Users"),
                "a peer must not learn a path: {}",
                error.message
            );
            assert!(error.message.contains("<path>"), "{}", error.message);
            assert!(
                !error.message.contains(&digest),
                "a peer must not learn a digest: {}",
                error.message
            );
            assert!(error.message.contains("<digest>"), "{}", error.message);
            assert_eq!(error.code, ErrorCode::Io, "the code stays: what failed");
        }
        other => panic!("expected an error, got {other:?}"),
    }

    // Everything that is not an error is not a place to rewrite: the event
    // stream carries the owner's own screen (§8b A14).
    let event = DaemonMessage::Ok { id: 4 };
    assert!(matches!(
        redact_for_conn(&remote_conn_with_caps(PeerRole::Daemon, None, &[]), event),
        DaemonMessage::Ok { id: 4 }
    ));
}

/// The capability set is read from the device's own row, and every failure
/// — unknown device, revoked row — yields the empty set, which the gate
/// reads as "no capability".
#[test]
fn the_capability_set_of_a_device_comes_from_its_row_and_fails_closed() {
    let (path, state) = temp_state("peer-caps-lookup");
    assert!(state.peer_caps("dev-unknown").is_empty());

    let mut record = PeerRecord {
        device_id: "dev-phone".to_string(),
        display_name: "Phone".to_string(),
        role: "client".to_string(),
        // The store refuses a peer key that is not a 32-byte X25519 public key,
        // and that refusal is the point: a fixture cannot skip the shape.
        public_key: vec![7u8; 32],
        paired_by_user: Some("S-user-a".to_string()),
        binding_kind: "tailnet".to_string(),
        binding_stable_id: Some("nstable".to_string()),
        binding_node_name: Some("node".to_string()),
        binding_login_name: Some("user@example.com".to_string()),
        address: "100.64.0.2:47831".to_string(),
        paired_at: 1,
        revoked_at: None,
        caps: vec![
            crate::peer_policy::CAP_VIEW.to_string(),
            crate::peer_policy::CAP_SEND.to_string(),
        ],
    };
    state.peer_upsert(record.clone()).expect("store");

    let mut caps = state.peer_caps("dev-phone");
    caps.sort();
    assert_eq!(
        caps,
        vec![
            crate::peer_policy::CAP_SEND.to_string(),
            crate::peer_policy::CAP_VIEW.to_string()
        ]
    );

    // A revoked row grants nothing, whatever it still carries.
    record.revoked_at = Some(2);
    state.peer_upsert(record).expect("re-store");
    assert!(state.peer_caps("dev-phone").is_empty());

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// The panel narrows a device by sending `PeerSetCaps` over the local pipe,
/// and the role a device was paired as must not stand in the way: `validate_caps`
/// refuses to strip `view` from a `Client` and says nothing about a `Daemon`, so
/// a daemon peer can be left holding one act. What is asserted is the set the
/// next connection will read (`peer_caps`), not the sentence in the reply.
#[test]
fn a_daemon_peer_narrowed_from_the_panel_is_stored_and_read_back() {
    let (path, state) = temp_state("daemon-peer-narrowed");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    state
        .peer_upsert(PeerRecord {
            device_id: "dev-daemon".to_string(),
            display_name: "Other devboule".to_string(),
            role: "daemon".to_string(),
            public_key: vec![9u8; 32],
            paired_by_user: Some("S-user-a".to_string()),
            binding_kind: "tailnet".to_string(),
            binding_stable_id: Some("nstable".to_string()),
            binding_node_name: Some("node".to_string()),
            binding_login_name: Some("user@example.com".to_string()),
            address: "100.64.0.3:47831".to_string(),
            paired_at: 1,
            revoked_at: None,
            caps: devboule_protocol::PEER_DEFAULT_CAPS
                .iter()
                .map(|cap| cap.to_string())
                .collect(),
        })
        .expect("store a daemon peer");

    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::PeerSetCaps {
            id: 4,
            device_id: "dev-daemon".to_string(),
            caps: vec![crate::peer_policy::CAP_VIEW.to_string()],
        },
        &ConnHandle::new(9),
        true,
        true,
        true,
        true,
    )
    .expect("immediate dispatch reply");

    match reply {
        DaemonMessage::PeerUpdated { id, peer } => {
            assert_eq!(id, 4);
            assert_eq!(peer.role, PeerRole::Daemon);
            assert_eq!(
                peer.caps,
                vec![crate::peer_policy::CAP_VIEW.to_string()],
                "the panel keeps the set the daemon answered with"
            );
        }
        other => panic!("PeerSetCaps must answer PeerUpdated, got {other:?}"),
    }
    assert_eq!(
        state.peer_caps("dev-daemon"),
        vec![crate::peer_policy::CAP_VIEW.to_string()],
        "the next connection reads the narrowed set, not the one it was paired with"
    );
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// S7 after the scope correction: the attachment counter is the peer's
/// deposit branch, so until it lands a send from a paired device that
/// carries an attachment is refused — before any decode, and the store
/// stays empty. A peer's text-only send and a local send are unchanged.
#[test]
fn a_peer_send_with_attachments_is_refused_until_the_deposit_counter_lands() {
    let (path, state) = temp_state("peer-attachments");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let sender = remote_conn_with_caps(
        PeerRole::Client,
        Some("S-user-a"),
        &[crate::peer_policy::CAP_VIEW, crate::peer_policy::CAP_SEND],
    );
    let send = |attachments: Vec<PromptAttachment>| ClientMessage::SessionSend {
        id: 1,
        session_id: "s.none.1".to_string(),
        subscription_id: 1,
        text: "hello".to_string(),
        attachments,
        active_turn_behavior: None,
        attachment_references: Vec::new(),
        idempotency_key: None,
    };
    let dispatch_send = |attachments: Vec<PromptAttachment>, conn: &Arc<ConnHandle>| {
        dispatch(
            &state,
            &owner,
            send(attachments),
            conn,
            true,
            true,
            true,
            true,
        )
        .expect("the gate answers")
    };

    match dispatch_send(vec![wire_attachment("a.png", b"one")], &sender) {
        DaemonMessage::Error(error) => {
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
            assert_eq!(
                error.message,
                crate::peer_policy::PEER_ATTACHMENTS_UNSUPPORTED
            );
        }
        other => panic!("a peer's attachment send must be refused: {other:?}"),
    }
    assert_eq!(
        files_under(&path.join("attachments")).len(),
        0,
        "a refused send must not write an attachment file"
    );

    match dispatch_send(Vec::new(), &sender) {
        DaemonMessage::Error(error) => assert_ne!(
            error.message,
            crate::peer_policy::PEER_ATTACHMENTS_UNSUPPORTED,
            "the refusal is about the attachments, not the device"
        ),
        other => panic!("a peer's text-only send reaches the session layer: {other:?}"),
    }

    match dispatch_send(vec![wire_attachment("a.png", b"one")], &ConnHandle::new(3)) {
        DaemonMessage::Error(error) => assert_ne!(
            error.message,
            crate::peer_policy::PEER_ATTACHMENTS_UNSUPPORTED,
            "the local pipe keeps its attachments"
        ),
        other => panic!("a local send reaches the session layer: {other:?}"),
    }

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// §8b A5/R3, H1: a paired device may not name a mode for an ACP session —
/// at the create, or by switching a live one — while the person at this
/// machine keeps their own path.
#[test]
fn a_peer_may_not_name_an_acp_mode_while_the_local_pipe_may() {
    let (path, state) = temp_state("peer-acp-modes");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let creator = remote_conn_with_caps(
        PeerRole::Client,
        Some("S-user-a"),
        &[
            crate::peer_policy::CAP_VIEW,
            crate::peer_policy::CAP_CREATE_SESSIONS,
        ],
    );
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::SessionCreate {
            id: 1,
            workspace_id: None,
            kind: SessionKind::Acp,
            provider: Some("claude-acp".to_string()),
            mode: Some("auto_accept".to_string()),
            display_name: None,
            idempotency_key: None,
        },
        &creator,
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers");
    match reply {
        DaemonMessage::Error(error) => {
            assert_eq!(error.code, ErrorCode::CapabilityNotSupported, "{error:?}");
            assert_eq!(
                error.message,
                crate::peer_policy::ACP_MODES_UNVETTED_MESSAGE
            );
            assert_eq!(error.id, Some(1));
        }
        other => panic!("an ACP create naming a mode must be refused: {other:?}"),
    }

    // A live ACP session, and the same device asking to switch it: refused
    // outright, whatever the id looks like — `ask` and `default` included,
    // because the agent defines what they mean.
    let member = OwnerId::new("S-user-a", "client").expect("owner");
    crate::session::insert_test_live_agent(&state.sessions, "s.acp.1", member);
    let switcher = remote_conn_with_caps(
        PeerRole::Client,
        Some("S-user-a"),
        &[crate::peer_policy::CAP_VIEW, crate::peer_policy::CAP_SEND],
    );
    for mode in ["ask", "auto_accept", "default"] {
        let reply = dispatch(
            &state,
            &owner,
            ClientMessage::SessionSetMode {
                id: 2,
                session_id: "s.acp.1".to_string(),
                mode_id: mode.to_string(),
            },
            &switcher,
            true,
            true,
            true,
            true,
        )
        .expect("the gate answers");
        match reply {
            DaemonMessage::Error(error) => {
                assert_eq!(error.code, ErrorCode::CapabilityNotSupported, "{error:?}");
                assert_eq!(
                    error.message,
                    crate::peer_policy::ACP_MODES_UNVETTED_MESSAGE,
                    "ACP mode {mode}"
                );
            }
            other => panic!("a peer may not switch an ACP session: {other:?}"),
        }
    }

    // The person at this machine is not under this rule: the same frame on
    // the local pipe reaches the sessions layer, which answers about the
    // session (this one has no mode manifest, so it says that) and never
    // with the ACP sentence.
    let local = ConnHandle::new(4);
    let local_reply = dispatch(
        &state,
        &owner,
        ClientMessage::SessionSetMode {
            id: 3,
            session_id: "s.acp.1".to_string(),
            mode_id: "auto_accept".to_string(),
        },
        &local,
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers");
    match local_reply {
        DaemonMessage::Error(error) => assert_ne!(
            error.message,
            crate::peer_policy::ACP_MODES_UNVETTED_MESSAGE,
            "the local path keeps its mode changes"
        ),
        other => panic!("a local set-mode on a manifest-less session is an error: {other:?}"),
    }

    drop(state);
    assert_eq!(
        audit_rows(&path),
        vec![
            "SessionCreate:acp_modes_unvetted_refused",
            "SessionSetMode:acp_modes_unvetted_refused",
            "SessionSetMode:acp_modes_unvetted_refused",
            "SessionSetMode:acp_modes_unvetted_refused",
        ],
        "one row per refusal, each naming the ACP rule"
    );
    let _ = std::fs::remove_dir_all(path);
}

/// §8b A11, H10: `view` is what makes a paired device a reader, and the two
/// list acts are reads. A peer holding nothing reaches neither, and the
/// same two requests are served once it holds `view`.
#[test]
fn a_peer_with_no_capability_cannot_read_the_lists() {
    let (path, state) = temp_state("peer-zero-caps");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let unpaired = remote_conn_with_caps(PeerRole::Daemon, None, &[]);
    for request in [
        ClientMessage::SessionsList { id: 1 },
        ClientMessage::DevicesList { id: 2 },
    ] {
        let name = request.name();
        match dispatch(&state, &owner, request, &unpaired, true, true, true, true)
            .expect("the gate answers")
        {
            DaemonMessage::Error(error) => {
                assert_eq!(
                    error.code,
                    ErrorCode::CapabilityNotSupported,
                    "{name}: {error:?}"
                );
                assert!(
                    error.message.contains(crate::peer_policy::CAP_VIEW),
                    "{name}: the refusal must name the capability: {}",
                    error.message
                );
            }
            other => panic!("a peer with no capability may not read {name}: {other:?}"),
        }
    }

    let reader = remote_conn_with_caps(PeerRole::Daemon, None, &[crate::peer_policy::CAP_VIEW]);
    for request in [
        ClientMessage::SessionsList { id: 3 },
        ClientMessage::DevicesList { id: 4 },
    ] {
        let name = request.name();
        if let DaemonMessage::Error(error) =
            dispatch(&state, &owner, request, &reader, true, true, true, true)
                .expect("the gate answers")
        {
            panic!("a viewer must be served {name}: {error:?}");
        }
    }

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// H6: a peer probing a session it may not reach gets exactly what a
/// nonexistent one gets. The mode policy is consulted only after the
/// ownership question, so its answer cannot say "that session exists, and
/// it runs this provider" to a peer that may not reach it.
#[test]
fn a_peer_probing_a_foreign_session_gets_the_same_answer_as_a_nonexistent_one() {
    let (path, state) = temp_state("peer-session-oracle");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let someone_else = OwnerId::new("S-user-b", "client").expect("owner");
    crate::session::insert_test_live_agent(&state.sessions, "s.acp.foreign", someone_else);
    let prober = remote_conn_with_caps(
        PeerRole::Client,
        Some("S-user-a"),
        &[crate::peer_policy::CAP_VIEW, crate::peer_policy::CAP_SEND],
    );
    let probes = |session_id: &str, id: u64| {
        [
            ClientMessage::SessionAttach {
                id,
                session_id: session_id.to_string(),
                subscription_id: 1,
                from_cursor: None,
            },
            ClientMessage::SessionSend {
                id,
                session_id: session_id.to_string(),
                subscription_id: 1,
                text: "hi".to_string(),
                attachments: Vec::new(),
                active_turn_behavior: None,
                attachment_references: Vec::new(),
                idempotency_key: None,
            },
            ClientMessage::SessionSetMode {
                id,
                session_id: session_id.to_string(),
                mode_id: "acceptEdits".to_string(),
            },
        ]
    };
    let answer = |request: ClientMessage| match dispatch(
        &state, &owner, request, &prober, true, true, true, true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::Error(error) => (error.code, error.message),
        other => panic!("a probe must be answered with an error: {other:?}"),
    };
    for (foreign, missing) in probes("s.acp.foreign", 1)
        .into_iter()
        .zip(probes("s.acp.missing", 2))
    {
        let name = foreign.name();
        let foreign_answer = answer(foreign);
        let missing_answer = answer(missing);
        assert_eq!(foreign_answer.0, ErrorCode::Unauthorized, "{name}");
        assert_eq!(
            foreign_answer, missing_answer,
            "{name}: a foreign session and a nonexistent one must be one answer"
        );
        assert_ne!(
            foreign_answer.1,
            crate::peer_policy::ACP_MODES_UNVETTED_MESSAGE,
            "{name}: no mode or kind may leak through the denial"
        );
    }

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// H4: the refusal is before the decode, not after it. An attachment whose
/// base64 is invalid *would* fail validation in the sessions layer; the
/// peer is refused with the attachment sentence instead, and nothing is
/// remembered for it, so the same idempotency key is a fresh request.
#[test]
fn a_peers_attachment_refusal_comes_before_any_decode() {
    let (path, state) = temp_state("peer-attachment-decode");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let sender = remote_conn_with_caps(
        PeerRole::Client,
        Some("S-user-a"),
        &[crate::peer_policy::CAP_VIEW, crate::peer_policy::CAP_SEND],
    );
    let mut broken = wire_attachment("a.png", b"one");
    broken.data = "!!! not base64 !!!".to_string();
    let send =
        |id: u64, attachments: Vec<PromptAttachment>, text: &str| ClientMessage::SessionSend {
            id,
            session_id: "s.test-client.decode1".to_string(),
            subscription_id: 1,
            text: text.to_string(),
            attachments,
            active_turn_behavior: None,
            attachment_references: Vec::new(),
            idempotency_key: Some("retry-me".to_string()),
        };
    let message = |request: ClientMessage, conn: &Arc<ConnHandle>| match dispatch(
        &state, &owner, request, conn, true, true, true, true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::Error(error) => error.message,
        other => panic!("a send with no live session is an error: {other:?}"),
    };

    let peer = message(send(1, vec![broken.clone()], "hello"), &sender);
    assert_eq!(
        peer,
        crate::peer_policy::PEER_ATTACHMENTS_UNSUPPORTED,
        "the peer's refusal is about attachments, never about the payload"
    );
    // The local pipe does decode, and says so: that is what makes the
    // assertion above evidence that the peer's path never reached the
    // decoder. (Not `!=` but the decoder's own sentence.)
    let local = message(send(2, vec![broken], "hello"), &ConnHandle::new(4));
    assert!(
        local.contains(&devboule_protocol::invalid_base64_message()),
        "the local answer must be the decoder's: {local}"
    );
    // A refusal is not idempotent-cached: the same key with a text-only
    // send reaches the sessions layer instead of replaying the refusal.
    let text_only = message(send(3, Vec::new(), "hello"), &sender);
    assert_ne!(text_only, crate::peer_policy::PEER_ATTACHMENTS_UNSUPPORTED);
    assert_eq!(
        files_under(&path.join("attachments")).len(),
        0,
        "no decode, no store file"
    );

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// DEP-02: the peer attachment refusal covers the deposit form, because it
/// is the frame that carries an attachment that is refused and a deposit
/// carries exactly one by construction. The discriminator is the sentence:
/// drop the `SessionDeposit` arm from `peer_refusal_before_mode` and the
/// deposit is answered by the deposit handler in `dispatch_session`
/// instead, whose answer about an absent session is a different sentence.
#[test]
fn a_peers_deposit_is_refused_with_the_attachment_sentence() {
    let (path, state) = temp_state("peer-deposit-refused");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let peer = remote_conn_with_caps(
        PeerRole::Client,
        Some("S-user-a"),
        &[crate::peer_policy::CAP_VIEW, crate::peer_policy::CAP_SEND],
    );
    // `id` is the only thing this frame can vary: the refusal cannot depend
    // on a session that does not exist, or it would be a later layer's.
    let deposit = |id: u64| ClientMessage::SessionDeposit {
        id,
        session_id: "s.none.1".to_string(),
        attachment: wire_attachment("deck.pdf", b"one"),
    };

    let refusal = match dispatch(&state, &owner, deposit(1), &peer, true, true, true, true)
        .expect("the gate answers")
    {
        DaemonMessage::Error(error) => error,
        other => panic!("a peer's deposit must be refused: {other:?}"),
    };
    assert_eq!(refusal.code, ErrorCode::InvalidRequest, "{refusal:?}");
    assert_eq!(
        refusal.message,
        crate::peer_policy::PEER_ATTACHMENTS_UNSUPPORTED,
        "the same sentence a peer's attachment send gets"
    );
    assert_eq!(
        refusal.id,
        Some(1),
        "the refusal still names the frame it refuses"
    );
    assert_eq!(
        files_under(&path.join("attachments")).len(),
        0,
        "a refused deposit must not write an attachment file"
    );

    // The control: the rule is about the device, so the local pipe keeps
    // its deposit — it reaches the layer that answers today.
    let local = match dispatch(
        &state,
        &owner,
        deposit(2),
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::Error(error) => error,
        other => panic!("a local deposit is answered, not dropped: {other:?}"),
    };
    assert_ne!(
        local.message,
        crate::peer_policy::PEER_ATTACHMENTS_UNSUPPORTED,
        "a local deposit is not an attachment refusal"
    );

    drop(state);
    assert_eq!(
        audit_sessions(&path),
        vec![Some("s.none.1".to_string())],
        "the refused deposit's audit row must name the session the frame named: \
         a row with no session cannot say which session a device asked about"
    );
    let _ = std::fs::remove_dir_all(path);
}

/// DEP-09's property, on the handler that now answers: a deposit replies
/// with the store's reference, and a frame the wire refuses is still
/// answered with a sentence rather than with its own contents — `name` and
/// `mime_type` are unbounded strings inside a frame that may be close to
/// `MAX_FRAME_BYTES`, and the fallback arm this used to reach formatted the
/// whole frame — `PromptAttachment`'s own `Debug` prints both — into the
/// error text. The one field that *must* survive a refusal is the request
/// `id`: it is the caller's correlation token, and a refusal without it is
/// a silence.
#[test]
fn a_local_deposit_answers_a_reference_and_a_refusal_echoes_nothing() {
    let (path, state) = temp_state("deposit-handler");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let session_id =
        devboule_protocol::compose_session_id(&owner.session_token(), "depo01").expect("id");
    crate::session::insert_test_live_agent(&state.sessions, &session_id, owner.clone());
    // The accepted half: the arm answers with the store's reference, under
    // the id the caller chose, and the file it names is on the disk.
    let stored = match dispatch(
        &state,
        &owner,
        ClientMessage::SessionDeposit {
            id: 8,
            session_id: session_id.clone(),
            attachment: wire_attachment("a.png", &crate::raster_metadata::clean_png(0x0b)),
        },
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::SessionDeposited { id, reference } => {
            assert_eq!(id, 8, "the reply names the call it answers");
            reference
        }
        other => panic!("a local deposit is answered, not refused: {other:?}"),
    };
    assert_eq!(stored.session_id, session_id);
    let files = files_under(&path.join("attachments"));
    assert_eq!(
        files[0].file_stem().and_then(|value| value.to_str()),
        Some(stored.digest.as_str()),
        "the reply's digest names the file the store wrote"
    );
    assert_eq!(
        stored.stored_bytes,
        std::fs::metadata(&files[0]).expect("stat").len()
    );

    // Long enough that echoing them would dominate the reply: a frame is
    // capped near 1 MiB, so this is what a real one can carry.
    let name = "n".repeat(300_000);
    let mime_type = "m".repeat(300_000);
    let mut attachment = wire_attachment("a.png", b"one");
    attachment.name = name.clone();
    attachment.mime_type = mime_type.clone();

    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::SessionDeposit {
            id: 9,
            session_id: session_id.clone(),
            attachment,
        },
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers");
    let error = match reply {
        DaemonMessage::Error(error) => error,
        other => panic!("a refused deposit is an error: {other:?}"),
    };

    assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    assert_eq!(
        error.id,
        Some(9),
        "the refusal must name the call it refuses, or the caller cannot match it"
    );
    assert!(
        !error.message.contains(&name),
        "the answer must not mirror the frame's name"
    );
    assert!(
        !error.message.contains(&mime_type),
        "the answer must not mirror the frame's mime type"
    );
    assert!(
        !error.message.contains(&session_id),
        "the answer must not mirror the frame's session"
    );
    assert!(
        error.message.len() < 512,
        "the sentence is a sentence, not a frame: {} bytes",
        error.message.len()
    );
    assert_eq!(
        files_under(&path.join("attachments")).len(),
        1,
        "the refused frame wrote nothing: only the accepted deposit's file is there"
    );

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// The read half of the deposit at the gate, both directions, walked over both
/// roles. A paired device holding every act-named capability and no `admin` is
/// refused: the gate refuses first, so the handler — and its ownership check —
/// never runs for it. The same device holding `admin` passes the gate and meets
/// the handler's own answer, which for an absent session is `SessionNotFound` —
/// the same answer the local pipe gets, and the last block proves that.
#[test]
fn a_peers_attachment_read_rides_the_administrative_capability() {
    let (path, state) = temp_state("peer-read-refused");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let read = |id: u64| ClientMessage::SessionAttachmentRead {
        id,
        reference: stored_reference("s.none.1", 'b'),
    };
    let operational = [
        crate::peer_policy::CAP_VIEW,
        crate::peer_policy::CAP_SEND,
        crate::peer_policy::CAP_ROSTER,
        crate::peer_policy::CAP_ANSWER_PERMISSIONS,
        crate::peer_policy::CAP_CREATE_SESSIONS,
    ];
    let mut admin = operational.to_vec();
    admin.push(crate::peer_policy::CAP_ADMIN);
    for role in [PeerRole::Client, PeerRole::Daemon] {
        let peer = remote_conn_with_caps(role, Some("S-user-a"), &operational);
        let refusal = match dispatch(&state, &owner, read(1), &peer, true, true, true, true)
            .expect("the gate answers")
        {
            DaemonMessage::Error(error) => error,
            other => panic!("{role:?} peer's read must be refused: {other:?}"),
        };
        assert_eq!(
            refusal.code,
            ErrorCode::CapabilityNotSupported,
            "{refusal:?}"
        );
        assert_eq!(
            refusal.message, "capability 'admin' was not negotiated",
            "{refusal:?}"
        );
        assert_eq!(refusal.id, Some(1));

        // The parity half: with the administrative capability the same frame
        // reaches the store, and the store's answer for an unknown session is
        // the handler's, not the gate's.
        let peer = remote_conn_with_caps(role, Some("S-user-a"), &admin);
        let answered = match dispatch(&state, &owner, read(2), &peer, true, true, true, true)
            .expect("the gate answers")
        {
            DaemonMessage::Error(error) => error,
            other => panic!("{role:?} peer's read reached the store, not the gate: {other:?}"),
        };
        assert_eq!(answered.code, ErrorCode::SessionNotFound, "{answered:?}");
        assert_eq!(answered.id, Some(2));
    }

    // The control: the local pipe reaches the same handler with the same
    // answer, so the peer's parity is not a different code path.
    let local = match dispatch(
        &state,
        &owner,
        read(3),
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::Error(error) => error,
        other => panic!("a local read is answered, not dropped: {other:?}"),
    };
    assert_eq!(local.code, ErrorCode::SessionNotFound, "{local:?}");

    drop(state);
    assert_eq!(
        audit_sessions(&path),
        vec![Some("s.none.1".to_string()), Some("s.none.1".to_string())],
        "one audit row per refused read, each naming the session it named; the two allowed reads write none"
    );
    let _ = std::fs::remove_dir_all(path);
}

/// The accepted half: a local read of a deposited file answers the store's
/// own bytes and MIME type under the caller's id.
#[test]
fn a_local_attachment_read_answers_the_store_s_bytes() {
    let (path, state) = temp_state("read-handler");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let session_id =
        devboule_protocol::compose_session_id(&owner.session_token(), "read01").expect("id");
    crate::session::insert_test_live_agent(&state.sessions, &session_id, owner.clone());
    let stored = match dispatch(
        &state,
        &owner,
        ClientMessage::SessionDeposit {
            id: 8,
            session_id: session_id.clone(),
            attachment: wire_attachment("a.png", &crate::raster_metadata::clean_png(0x0b)),
        },
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::SessionDeposited { reference, .. } => reference,
        other => panic!("setup deposit is answered: {other:?}"),
    };
    let attachment = match dispatch(
        &state,
        &owner,
        ClientMessage::SessionAttachmentRead {
            id: 9,
            reference: stored.clone(),
        },
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::SessionAttachment { id, attachment } => {
            assert_eq!(id, 9, "the reply names the call it answers");
            attachment
        }
        other => panic!("a local read is answered, not refused: {other:?}"),
    };
    assert_eq!(attachment.mime_type, "image/png");
    {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&attachment.data)
            .expect("the reply carries base64");
        let file = files_under(&path.join("attachments"));
        assert_eq!(file.len(), 1);
        assert_eq!(bytes, std::fs::read(&file[0]).expect("the stored file"));
    }

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// A journaled reference outlives the folder it names, so the app will ask
/// for references that no longer resolve. Three disk states, two answers:
/// no session behind the id (the registry answers), a folder holding a
/// different file than the digest names, and no folder at all — the last
/// two are the store's one sentence, byte for byte, although the disk
/// differs. The store cannot tell a swept folder from a digest deposited
/// elsewhere, and this test pins that it does not try.
#[test]
fn a_read_of_a_dead_reference_names_what_is_missing() {
    let (path, state) = temp_state("read-dead");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let session_id =
        devboule_protocol::compose_session_id(&owner.session_token(), "dead01").expect("id");
    crate::session::insert_test_live_agent(&state.sessions, &session_id, owner.clone());
    // One real deposit first, so the folder and a file exist: the cases
    // below differ in disk state, not just in digest spelling.
    let stored = match dispatch(
        &state,
        &owner,
        ClientMessage::SessionDeposit {
            id: 7,
            session_id: session_id.clone(),
            attachment: wire_attachment("a.png", &crate::raster_metadata::clean_png(0x0b)),
        },
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::SessionDeposited { reference, .. } => reference,
        other => panic!("setup deposit is answered: {other:?}"),
    };
    let read = |id: u64, reference: AttachmentReference| match dispatch(
        &state,
        &owner,
        ClientMessage::SessionAttachmentRead { id, reference },
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::Error(error) => error,
        other => panic!("a dead reference is an error: {other:?}"),
    };
    const NO_STORED_FILE: &str = "The store holds no attachment with that digest in this session.";

    // No session behind the id: the registry answers, before the store.
    let error = read(1, stored_reference("s.none.9", 'b'));
    assert_eq!(error.code, ErrorCode::SessionNotFound, "{error:?}");
    assert_eq!(error.message, "No session with that id.");

    // The folder exists and holds the deposit, but the digest names a
    // different file: the store answers its one sentence. The digest is
    // well formed and the session is the request's own, so neither the
    // wire rules nor the ownership check can be the refusal.
    let error = read(
        2,
        AttachmentReference {
            session_id: session_id.clone(),
            digest: "c".repeat(64),
            stored_bytes: 7,
        },
    );
    assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    assert_eq!(error.message, NO_STORED_FILE);

    // The folder is gone — the deposit above gave the path, so this
    // removes the folder it wrote. The request is the deposit's own
    // reference, and it reads the same sentence, byte for byte.
    let file = files_under(&path.join("attachments"));
    assert_eq!(file.len(), 1);
    std::fs::remove_dir_all(file[0].parent().expect("session folder")).expect("sweep the folder");
    let error = read(3, stored.clone());
    assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    assert_eq!(error.message, NO_STORED_FILE);

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// The claimed size is compared against the store's, and a disagreement is
/// a refusal: a request naming a size the file does not have is naming
/// something it did not deposit.
#[test]
fn a_read_naming_the_wrong_size_is_refused() {
    let (path, state) = temp_state("read-size");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let session_id =
        devboule_protocol::compose_session_id(&owner.session_token(), "size01").expect("id");
    crate::session::insert_test_live_agent(&state.sessions, &session_id, owner.clone());
    let stored = match dispatch(
        &state,
        &owner,
        ClientMessage::SessionDeposit {
            id: 8,
            session_id: session_id.clone(),
            attachment: wire_attachment("a.png", &crate::raster_metadata::clean_png(0x0b)),
        },
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::SessionDeposited { reference, .. } => reference,
        other => panic!("setup deposit is answered: {other:?}"),
    };
    let wrong = AttachmentReference {
        stored_bytes: stored.stored_bytes + 1,
        ..stored.clone()
    };
    let error = match dispatch(
        &state,
        &owner,
        ClientMessage::SessionAttachmentRead {
            id: 9,
            reference: wrong.clone(),
        },
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::Error(error) => error,
        other => panic!("a mis-sized read is an error: {other:?}"),
    };
    assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    assert_eq!(
        error.message,
        format!(
            "The stored attachment '{}' is not {} bytes as the reference states.",
            stored.digest, wrong.stored_bytes
        ),
        "the refusal names the claim it would not serve, not the file's size"
    );

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// The read cap is the artifact cap: one constant, the number the deposit
/// already enforces. A file over it is refused with the cap named.
#[test]
fn a_read_over_the_artifact_cap_names_the_cap() {
    let (path, state) = temp_state("read-cap");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let session_id =
        devboule_protocol::compose_session_id(&owner.session_token(), "capp01").expect("id");
    crate::session::insert_test_live_agent(&state.sessions, &session_id, owner.clone());
    let stored = match dispatch(
        &state,
        &owner,
        ClientMessage::SessionDeposit {
            id: 8,
            session_id: session_id.clone(),
            attachment: wire_attachment("a.png", &crate::raster_metadata::clean_png(0x0b)),
        },
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::SessionDeposited { reference, .. } => reference,
        other => panic!("setup deposit is answered: {other:?}"),
    };
    // Grow the stored file past the cap without renaming it: the digest
    // still names the file, so the read reaches the cap refusal rather
    // than the identity one.
    let file = files_under(&path.join("attachments"));
    assert_eq!(file.len(), 1);
    let grown = crate::session::MAX_AGENT_ARTIFACT_BYTES + 1024;
    std::fs::write(&file[0], vec![0u8; grown]).expect("grow the stored file");
    let big = AttachmentReference {
        stored_bytes: grown as u64,
        ..stored.clone()
    };
    let error = match dispatch(
        &state,
        &owner,
        ClientMessage::SessionAttachmentRead {
            id: 9,
            reference: big,
        },
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::Error(error) => error,
        other => panic!("an over-cap read is an error: {other:?}"),
    };
    assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    assert_eq!(
        error.message,
        format!(
            "The reference states {} bytes for '{}', over the {}-byte read cap.",
            grown as u64,
            stored.digest,
            crate::session::MAX_AGENT_ARTIFACT_BYTES
        ),
        "the refusal names the cap, and only what the reference states"
    );

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// Bytes rewritten under the same name at the same length are still
/// refused: the size check alone cannot catch a substitution that keeps
/// the length, which is the reason the read hashes what it returns.
#[test]
fn a_read_of_rewritten_bytes_is_refused() {
    let (path, state) = temp_state("read-rewritten");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let session_id =
        devboule_protocol::compose_session_id(&owner.session_token(), "rwrt01").expect("id");
    crate::session::insert_test_live_agent(&state.sessions, &session_id, owner.clone());
    let stored = match dispatch(
        &state,
        &owner,
        ClientMessage::SessionDeposit {
            id: 8,
            session_id: session_id.clone(),
            attachment: wire_attachment("a.png", &crate::raster_metadata::clean_png(0x0b)),
        },
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::SessionDeposited { reference, .. } => reference,
        other => panic!("setup deposit is answered: {other:?}"),
    };
    // Rewrite the file in place with different bytes of the same length:
    // the digest still names the file and the size still matches, so only
    // the hash can catch this.
    let file = files_under(&path.join("attachments"));
    assert_eq!(file.len(), 1);
    let bytes = std::fs::read(&file[0]).expect("the stored file");
    let rewritten: Vec<u8> = bytes.iter().map(|byte| !byte).collect();
    assert_ne!(rewritten, bytes);
    std::fs::write(&file[0], &rewritten).expect("rewrite the stored file");
    let error = match dispatch(
        &state,
        &owner,
        ClientMessage::SessionAttachmentRead {
            id: 9,
            reference: stored.clone(),
        },
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::Error(error) => error,
        other => panic!("rewritten bytes must be refused, not served: {other:?}"),
    };
    assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    assert_eq!(
        error.message,
        format!(
            "The stored attachment '{}' does not match its digest.",
            stored.digest
        ),
        "rewritten bytes fail verification with their own sentence"
    );

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// A file grown huge is refused from a bounded read: the door opens the
/// file and takes at most the cap plus one byte, so a monster is never
/// fully allocated and never reaches the frame. The request below still
/// names the original small size, so the length check — not the cap — is
/// what refuses it.
#[test]
fn a_read_of_a_huge_file_is_refused_without_reading_it_all() {
    let (path, state) = temp_state("read-huge");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let session_id =
        devboule_protocol::compose_session_id(&owner.session_token(), "huge01").expect("id");
    crate::session::insert_test_live_agent(&state.sessions, &session_id, owner.clone());
    let stored = match dispatch(
        &state,
        &owner,
        ClientMessage::SessionDeposit {
            id: 8,
            session_id: session_id.clone(),
            attachment: wire_attachment("a.png", &crate::raster_metadata::clean_png(0x0b)),
        },
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::SessionDeposited { reference, .. } => reference,
        other => panic!("setup deposit is answered: {other:?}"),
    };
    let file = files_under(&path.join("attachments"));
    assert_eq!(file.len(), 1);
    std::fs::write(&file[0], vec![0u8; 256 * 1024]).expect("grow the stored file");
    let error = match dispatch(
        &state,
        &owner,
        ClientMessage::SessionAttachmentRead {
            id: 9,
            reference: stored.clone(),
        },
        &ConnHandle::new(4),
        true,
        true,
        true,
        true,
    )
    .expect("the gate answers")
    {
        DaemonMessage::Error(error) => error,
        other => panic!("a huge file must be refused, not served: {other:?}"),
    };
    assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    assert_eq!(
        error.message,
        format!(
            "The stored attachment '{}' is not {} bytes as the reference states.",
            stored.digest, stored.stored_bytes
        ),
        "a huge file fails the length check with its own sentence"
    );

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// A stand-in for the pipe a connection writes to, so a test can read back
/// exactly the frames a teardown wrote. On Windows `Framed` writes with
/// `WriteFile` and an `OVERLAPPED`, which a disk handle must be opened for.
fn pipe_stand_in(path: &std::path::Path) -> std::fs::File {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
        options.custom_flags(FILE_FLAG_OVERLAPPED);
    }
    options.open(path).expect("pipe stand-in")
}

/// H3: a revoked connection sends nothing more — not even an event that was
/// already queued when the close flag went up. The control run proves the
/// queue *would* have been written.
#[test]
fn a_revoked_connection_writes_no_queued_event() {
    let (path, state) = temp_state("revoked-teardown");
    let queued = || {
        let mut queue: VecDeque<SessionEventEnvelope> = VecDeque::new();
        queue.push_back(session_state_event(Vec::new()));
        queue
    };

    let sent_path = path.join("sent.frames");
    {
        let framed = Framed::new(pipe_stand_in(&sent_path));
        let conn = ConnHandle::new(1);
        let (mut events, mut state_events) = (VecDeque::new(), queued());
        flush_final_events(
            &framed,
            &conn,
            &state.sessions,
            &mut events,
            &mut state_events,
            false,
        );
    }
    assert!(
        !std::fs::read(&sent_path)
            .expect("read control frames")
            .is_empty(),
        "an unrevoked teardown writes what was queued"
    );

    let revoked_path = path.join("revoked.frames");
    {
        let framed = Framed::new(pipe_stand_in(&revoked_path));
        let conn = ConnHandle::new(2);
        let (mut events, mut state_events) = (VecDeque::new(), queued());
        flush_final_events(
            &framed,
            &conn,
            &state.sessions,
            &mut events,
            &mut state_events,
            true,
        );
    }
    let written = std::fs::read(&revoked_path).expect("read revoked frames");
    assert!(
        written.is_empty(),
        "a revoked connection must write no frame; it wrote {} bytes",
        written.len()
    );

    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// A refused resume after the slot increment must give the slot back: every
/// other exit past the gate balances with `session_finished`, and a leaked
/// slot keeps `sessions != 0` forever, so the idle shutdown never re-arms
/// for the life of the process. The row here is recovered with an
/// unreadable overlay cell — the exact state that refuses at the lineage
/// line — and the command resolves from the environment so the test never
/// spawns anything: the failure lands before any provider starts.
#[test]
fn a_refused_resume_releases_its_lifecycle_slot() {
    let (path, state) = temp_state("resume-slot-balance");
    let owner = OwnerId::new("slot-user", "slot-client").expect("owner");
    let session_id =
        devboule_protocol::compose_session_id(&owner.session_token(), "slot01").expect("id");
    let _acp_env = crate::session::lock_acp_env();
    std::env::set_var(
        "DEVBOULE_ACP_COMMAND",
        r#"["definitely-not-a-real-program-xyz"]"#,
    );
    // The row's provider must resolve through the paired override (parse
    // only — the lineage failure lands before any spawn), never through
    // PATH or the registry.
    std::env::set_var("DEVBOULE_ACP_PROVIDER_ID", "devboule-acp-stub");
    let mut record = crate::journal::new_session_record(
        session_id.clone(),
        owner.user.clone(),
        None,
        devboule_protocol::SessionKind::Acp,
        "Slot",
    );
    record.provider = Some("devboule-acp-stub".to_string());
    record.peer_session_id = Some("peer-slot".to_string());
    record.created_by = Some("slot-creator".to_string());
    // Unreadable on purpose: not a deny list, so the lineage line refuses.
    // Written around the typed API, which cannot produce these bytes.
    let journal = state.journal.clone().expect("state journal");
    journal.create_session(record).expect("birth row");
    rusqlite::Connection::open(path.join("journal.db"))
        .expect("open journal file")
        .execute(
            "UPDATE sessions SET overlay = '{\"broken\":' WHERE id = ?1",
            [&session_id],
        )
        .expect("rot the cell");
    let conn = crate::session::ConnHandle::new(9);
    let before = state.live_session_count();
    let result = state.sessions.resume(&state, &session_id, &owner, &conn);
    std::env::remove_var("DEVBOULE_ACP_COMMAND");
    std::env::remove_var("DEVBOULE_ACP_PROVIDER_ID");
    let error = result.expect_err("the unreadable cell refuses the resume");
    assert!(
        error.message.contains("overlay"),
        "the refusal names the column: {}",
        error.message
    );
    assert_eq!(
        state.live_session_count(),
        before,
        "the refused resume gave its slot back"
    );
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// Two simultaneous quits are both refused while both windows are counted —
/// and both windows then leave. The daemon must not outlive its UI: each
/// leaver carries its own refused intent, and the LAST one out is a window
/// that asked, so the daemon stops with it (live session included).
#[test]
fn two_refused_quits_stop_the_daemon_when_both_windows_leave() {
    let state = state();
    let (guard_a, intent_a) = state
        .admit_client(ClientKind::LocalApp)
        .expect("window A is admitted");
    let conn_a = ConnHandle::with_peer_caps(31, None, None, Vec::new(), intent_a);
    let (guard_b, intent_b) = state
        .admit_client(ClientKind::LocalApp)
        .expect("window B is admitted");
    let conn_b = ConnHandle::with_peer_caps(32, None, None, Vec::new(), intent_b);
    assert!(state.session_started(), "a live agent pins the idle exit");
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    for conn in [&conn_a, &conn_b] {
        let reply = dispatch(
            &state,
            &owner,
            ClientMessage::Shutdown { id: 41 },
            conn,
            true,
            true,
            true,
            true,
        )
        .expect("shutdown always answers");
        let DaemonMessage::Shutdown {
            accepted: false,
            reason: Some(reason),
            ..
        } = reply
        else {
            panic!("both quits are refused while both windows count, got {reply:?}");
        };
        assert!(
            !reason.contains("  "),
            "the refusal is one sentence, without the old gap: {reason:?}"
        );
    }
    // Window A leaves: its own quit was refused, but B still holds the
    // daemon. The guard's release IS the disconnect, its own intent read
    // on the way out.
    drop(guard_a);
    assert!(
        !state.is_shutting_down(),
        "one window left; the other still holds the daemon"
    );
    drop(guard_b);
    assert!(
        state.is_shutting_down(),
        "the last local app out asked to quit: the daemon stops, live session included"
    );
}

/// A refused quit never keeps the daemon from serving the window that
/// stays: the leaver is gone, the remaining window quits and is accepted.
#[test]
fn a_refused_quit_leaves_the_daemon_running_for_the_window_that_stays() {
    let state = state();
    let (guard_a, intent_a) = state
        .admit_client(ClientKind::LocalApp)
        .expect("window A is admitted");
    let conn_a = ConnHandle::with_peer_caps(33, None, None, Vec::new(), intent_a);
    let (guard_b, intent_b) = state
        .admit_client(ClientKind::LocalApp)
        .expect("window B is admitted");
    let conn_b = ConnHandle::with_peer_caps(34, None, None, Vec::new(), intent_b);
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::Shutdown { id: 43 },
        &conn_a,
        true,
        true,
        true,
        true,
    )
    .expect("shutdown always answers");
    let DaemonMessage::Shutdown {
        accepted: false, ..
    } = reply
    else {
        panic!("the quit is refused while both windows count, got {reply:?}");
    };
    // The refused window leaves anyway; the window that stayed keeps the
    // daemon, and its own later quit is the one that is accepted.
    drop(guard_a);
    assert!(
        !state.is_shutting_down(),
        "the window that stayed keeps the daemon running"
    );
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::Shutdown { id: 44 },
        &conn_b,
        true,
        true,
        true,
        true,
    )
    .expect("shutdown always answers");
    let DaemonMessage::Shutdown { accepted: true, .. } = reply else {
        panic!("the last window out is accepted, got {reply:?}");
    };
    drop(guard_b);
}

/// The count check and entering shutdown are one atomic step: once the
/// handshake accepts, a new local client is refused at the door — there is
/// no window between the decision and the shutdown state for an admission
/// to slip through.
#[test]
fn an_accepted_quit_refuses_late_admission() {
    let state = state();
    let _app = state
        .admit_client(ClientKind::LocalApp)
        .expect("the app is admitted");
    state
        .request_local_shutdown()
        .expect("the only local app out may stop the daemon");
    assert!(
        state.is_shutting_down(),
        "accepting the quit IS entering shutdown, in the same step"
    );
    assert!(
        state.admit_client(ClientKind::LocalApp).is_none(),
        "a client arriving after the decision finds shutdown already begun"
    );
}

/// A refused quit belongs to the connection that asked. Window A's quit was
/// refused and A left; window B never asked and crashes later. The daemon
/// must still be there: B's crash is not A's quit.
#[test]
fn a_crash_of_the_last_window_after_a_refused_quit_never_stops_the_daemon() {
    let state = state();
    let (guard_a, intent_a) = state
        .admit_client(ClientKind::LocalApp)
        .expect("A is admitted");
    let conn_a = ConnHandle::with_peer_caps(51, None, None, Vec::new(), intent_a);
    let (guard_b, intent_b) = state
        .admit_client(ClientKind::LocalApp)
        .expect("B is admitted");
    // B is connected but never speaks: its handle holds nothing the test
    // reads — the guard's own copy of the intent is what a crash releases.
    let _conn_b = ConnHandle::with_peer_caps(52, None, None, Vec::new(), intent_b);
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::Shutdown { id: 71 },
        &conn_a,
        true,
        true,
        true,
        true,
    )
    .expect("shutdown always answers");
    let DaemonMessage::Shutdown {
        accepted: false, ..
    } = reply
    else {
        panic!("A's quit is refused while B counts, got {reply:?}");
    };
    // A leaves, its own quit refused; B still holds the daemon.
    drop(guard_a);
    assert!(!state.is_shutting_down(), "B still holds the daemon");
    // B crashes later, never having asked to quit: its release carries no
    // intent, so the daemon stays for the work on it.
    drop(guard_b);
    assert!(
        !state.is_shutting_down(),
        "a crash of a window that never asked must not stop the daemon"
    );
}

/// A peer's refused `Shutdown` memorizes nothing: dispatch reads the caller
/// kind, the peer has no local slot, and when the last local window leaves
/// without asking, nothing stops.
#[test]
fn a_peers_refused_shutdown_is_never_memorized() {
    let state = state();
    let (guard_a, intent_a) = state
        .admit_client(ClientKind::LocalApp)
        .expect("A is admitted");
    let _conn_a = ConnHandle::with_peer_caps(53, None, None, Vec::new(), intent_a);
    let (guard_b, intent_b) = state
        .admit_client(ClientKind::LocalApp)
        .expect("B is admitted");
    let _conn_b = ConnHandle::with_peer_caps(54, None, None, Vec::new(), intent_b);
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let conn_peer = remote_conn_with_caps(PeerRole::Daemon, None, &[CAP_ADMIN]);
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::Shutdown { id: 72 },
        &conn_peer,
        true,
        true,
        true,
        true,
    )
    .expect("shutdown always answers");
    let DaemonMessage::Shutdown {
        accepted: false, ..
    } = reply
    else {
        panic!("the peer's shutdown is refused while two local apps count, got {reply:?}");
    };
    drop(conn_peer);
    drop(guard_a);
    drop(guard_b);
    assert!(
        !state.is_shutting_down(),
        "no local window ever asked to quit: nothing stops"
    );
}

/// A relaunch severs the old connection and its refusal: the new window's
/// quit stands on its own, and the daemon outlives every window that did
/// not ask.
#[test]
fn a_relaunched_windows_old_refusal_does_not_decide_for_others() {
    let state = state();
    let (guard_a, intent_a) = state
        .admit_client(ClientKind::LocalApp)
        .expect("A is admitted");
    let conn_a = ConnHandle::with_peer_caps(55, None, None, Vec::new(), intent_a);
    let (guard_b, intent_b) = state
        .admit_client(ClientKind::LocalApp)
        .expect("B is admitted");
    let conn_b = ConnHandle::with_peer_caps(56, None, None, Vec::new(), intent_b);
    let owner = OwnerId::new("test-user", "test-client").expect("owner");
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::Shutdown { id: 73 },
        &conn_a,
        true,
        true,
        true,
        true,
    )
    .expect("shutdown always answers");
    let DaemonMessage::Shutdown {
        accepted: false, ..
    } = reply
    else {
        panic!("A's quit is refused while B counts, got {reply:?}");
    };
    drop(guard_a);
    // A relaunches: a fresh connection, a fresh intent.
    let (guard_a2, intent_a2) = state
        .admit_client(ClientKind::LocalApp)
        .expect("A is relaunched");
    let conn_a2 = ConnHandle::with_peer_caps(57, None, None, Vec::new(), intent_a2);
    // B quits: refused while two windows count; B leaves, its own quit
    // refused — but A-relaunched never asked, so the daemon stays.
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::Shutdown { id: 74 },
        &conn_b,
        true,
        true,
        true,
        true,
    )
    .expect("shutdown always answers");
    let DaemonMessage::Shutdown {
        accepted: false, ..
    } = reply
    else {
        panic!("B's quit is refused while two windows count, got {reply:?}");
    };
    drop(guard_b);
    assert!(!state.is_shutting_down(), "A-relaunched never asked");
    // A-relaunched quits as the only window out: accepted.
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::Shutdown { id: 75 },
        &conn_a2,
        true,
        true,
        true,
        true,
    )
    .expect("shutdown always answers");
    let DaemonMessage::Shutdown { accepted: true, .. } = reply else {
        panic!("the last window out is accepted, got {reply:?}");
    };
    drop(guard_a2);
}
