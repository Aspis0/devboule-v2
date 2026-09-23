//! Tests for the daemon client: request framing, timeouts and error mapping.

use super::{
    fail_connection, ClientInner, PROVIDER_UPDATE_RPC_TIMEOUT, RPC_TIMEOUT,
    SESSION_CREATE_RPC_TIMEOUT, SESSION_RESUME_RPC_TIMEOUT,
};
use crate::error::DaemonError;
use crate::framing::Framed;
use crate::provider_update::UPDATE_TIMEOUT;
use crate::session::{ACP_FIRST_RESPONSE_TIMEOUT, ACP_RESPONSE_TIMEOUT};
#[cfg(windows)]
use crate::transport::{Listener, NamedPipeListener};
use devboule_protocol::{
    ClientMessage, DaemonHello, DaemonMessage, ErrorCode, Persistence, PersistenceKind,
    ResumeResult, SessionEvent, SessionKind,
};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

#[test]
fn provider_update_deadline_has_install_headroom() {
    // Keep the RPC deadline above the runner timeout plus 30 seconds: reverting
    // provider_update to the normal 30-second RPC default would silently cut
    // off long installs. The complete wiring needs a fake pipe to test; these
    // constants protect the deadline relationship directly.
    assert!(PROVIDER_UPDATE_RPC_TIMEOUT > UPDATE_TIMEOUT + Duration::from_secs(30));
    assert_eq!(RPC_TIMEOUT, Duration::from_secs(30));
}

/// The resume road's budget, checked against the daemon's own.
///
/// The daemon cannot answer `session_resume` before the provider startup it
/// runs inline has finished, and that startup carries two bounds: `initialize`
/// (the first-answer window) and the `session/load` or `session/new` behind
/// it. A client on the control-plane default gives up 30 seconds in — the
/// measured defect of 2026-09-21, where the daemon answered at 36 s.
///
/// Mutant: the road back on `RPC_TIMEOUT` (or the constant lowered under the
/// daemon's own bounds) — this assertion fails.
#[test]
fn session_resume_deadline_covers_the_provider_startups_it_waits_for() {
    // Twice the pair of bounds, because one call can carry two startups: the
    // resume the provider refuses, and the replacement session the daemon then
    // builds to keep the conversation.
    assert!(SESSION_RESUME_RPC_TIMEOUT > 2 * (ACP_FIRST_RESPONSE_TIMEOUT + ACP_RESPONSE_TIMEOUT));
}

/// The create road's budget, checked against the daemon's own reads.
///
/// The daemon cannot answer `session_create` before the same inline provider
/// startup `session_resume` waits for — `acp_client::spawn_process` runs the
/// handshake — and a creation can cross **five** awaited replies: `initialize`
/// on the first-answer bound, then `session/new`, the `session/set_mode` a
/// creation with a mode owes when the agent declares standard modes, and the
/// delivery's confirmation, which reads the primary reply and, when the
/// switch needs a follow-up, a second one. The four behind `initialize` each
/// wait the response bound, so the ceiling is 180 s — and the budget must
/// keep the journal and queue margin **above** it, or the client surrenders in
/// the instant the daemon's worst case ends, which is the defect this budget
/// exists to remove.
///
/// The pair of assertions lives here rather than in
/// [`only_the_named_roads_leave_the_thirty_second_default`] because that pin
/// is the resume road's proof and stays as it was; the default is checked
/// here too, so this road's own size cannot quietly become the default.
///
/// Mutant: the road back on `RPC_TIMEOUT`, or the budget lowered onto the
/// ceiling (180 s) with the margin gone — this assertion fails.
#[test]
fn session_create_deadline_covers_the_provider_startup_it_waits_for() {
    const MARGIN: Duration = Duration::from_secs(30);
    // Four response-bound reads behind `initialize`: `session/new`, the mode
    // switch, and the delivery's primary and follow-up confirmations.
    let ceiling = ACP_FIRST_RESPONSE_TIMEOUT + 4 * ACP_RESPONSE_TIMEOUT;
    assert!(
        SESSION_CREATE_RPC_TIMEOUT >= ceiling + MARGIN,
        "a create budget of {SESSION_CREATE_RPC_TIMEOUT:?} leaves no margin over the {ceiling:?} ceiling the daemon's own reads declare"
    );
    assert_eq!(RPC_TIMEOUT, Duration::from_secs(30));
    assert!(SESSION_CREATE_RPC_TIMEOUT > RPC_TIMEOUT);
}

/// The default stays the default: everything that is not a provider startup
/// still surrenders at 30 seconds, because a client that waits silently is
/// how a dead daemon turns into a frozen window.
#[test]
fn only_the_named_roads_leave_the_thirty_second_default() {
    assert_eq!(RPC_TIMEOUT, Duration::from_secs(30));
    assert!(SESSION_RESUME_RPC_TIMEOUT > RPC_TIMEOUT);
    assert!(PROVIDER_UPDATE_RPC_TIMEOUT > RPC_TIMEOUT);
}

#[test]
fn connection_failure_answers_pending_requests_with_connection_lost_code() {
    let (reply_tx, reply_rx) = mpsc::channel();
    let mut pending = HashMap::new();
    pending.insert(41, reply_tx);
    let inner = ClientInner {
        framed: Framed::new(
            std::fs::File::open(std::env::current_exe().expect("exe")).expect("open exe"),
        ),
        next_id: std::sync::atomic::AtomicU64::new(1),
        next_subscription_id: std::sync::atomic::AtomicU64::new(1),
        pending: Mutex::new(pending),
        pending_subscriptions: Mutex::new(HashMap::new()),
        subscriptions: Mutex::new(HashMap::new()),
        default_subscriptions: Mutex::new(HashMap::new()),
        session_state_subscription: Mutex::new(None),
        delegation_subscription: Mutex::new(None),
        stop: AtomicBool::new(false),
        hello: DaemonHello::plugin_backend("connection-loss-test", std::process::id()),
        server_pid: None,
    };

    fail_connection(&inner, DaemonError::ConnectionLost);

    let DaemonMessage::Error(error) = reply_rx.recv().expect("pending reply") else {
        panic!("connection failure must answer the pending request with an error");
    };
    assert_eq!(error.code, ErrorCode::ConnectionLost);
    assert_eq!(
        serde_json::to_value(error.code).expect("code json"),
        "connection_lost"
    );
    assert_eq!(error.message, "daemon connection was lost");
}

#[test]
fn dead_connection_recovery_still_spawns_then_retries() {
    let paths = crate::paths::RuntimePaths::from_dir("fake-dead-daemon");
    let hello = devboule_protocol::ClientHello::m3a(
        super::test_owner("dead-recovery-test").expect("owner"),
        "dead-recovery-test",
    );
    let mut connects = 0;
    let mut spawns = 0;
    let result = super::connect_or_spawn_with(
        &paths,
        hello,
        std::path::Path::new("fake-daemon.exe"),
        |_, _| {
            connects += 1;
            if connects == 1 {
                Err(crate::DaemonError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "dead daemon",
                )))
            } else {
                Ok(42u32)
            }
        },
        |_, _| {
            spawns += 1;
            Ok(())
        },
    )
    .expect("the next connection recovers");
    assert_eq!(result, 42);
    assert_eq!(connects, 2);
    assert_eq!(spawns, 1);
}

#[cfg(windows)]
#[test]
fn subscription_events_route_by_their_subscription_id() {
    let dir = crate::test_dirs::test_temp_dir("devboule-client-routing");
    let paths = crate::paths::RuntimePaths::from_dir(&dir);
    let stop = Arc::new(AtomicBool::new(false));
    let mut listener = NamedPipeListener::bind(&paths, Arc::clone(&stop)).expect("bind");
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let file = listener.accept().expect("accept");
        let framed = Framed::new(file);
        let hello = framed.recv::<ClientMessage>().expect("client hello");
        assert!(matches!(hello, ClientMessage::Hello(_)));
        framed
            .send(&DaemonMessage::Hello(DaemonHello::plugin_backend(
                "routing-test",
                std::process::id(),
            )))
            .expect("hello reply");

        let first = framed
            .recv::<ClientMessage>()
            .expect("first attach request");
        let ClientMessage::SessionAttach {
            id: first_id,
            subscription_id: first_subscription,
            ..
        } = first
        else {
            panic!("expected first attach request");
        };
        framed
            .send(&DaemonMessage::SessionAttached {
                id: first_id,
                subscription_id: first_subscription,
            })
            .expect("first attach reply");
        framed
            .send(&DaemonMessage::SubscriptionEvent {
                subscription_id: first_subscription,
                envelope: devboule_protocol::SessionEventEnvelope {
                    session_id: "s.routing".to_string(),
                    generation: 1,
                    transcript_seq: None,
                    event: SessionEvent::AgentMessage {
                        message_id: None,
                        text: "a-1".to_string(),
                        parent_tool_use_id: None,
                        spawn_depth: None,
                    },
                },
            })
            .expect("first A event");

        let second = framed
            .recv::<ClientMessage>()
            .expect("second attach request");
        let ClientMessage::SessionAttach {
            id: second_id,
            subscription_id: second_subscription,
            ..
        } = second
        else {
            panic!("expected second attach request");
        };
        framed
            .send(&DaemonMessage::SubscriptionEvent {
                subscription_id: first_subscription,
                envelope: devboule_protocol::SessionEventEnvelope {
                    session_id: "s.routing".to_string(),
                    generation: 1,
                    transcript_seq: None,
                    event: SessionEvent::AgentMessage {
                        message_id: None,
                        text: "a-2".to_string(),
                        parent_tool_use_id: None,
                        spawn_depth: None,
                    },
                },
            })
            .expect("remaining A event");
        framed
            .send(&DaemonMessage::SessionAttached {
                id: second_id,
                subscription_id: second_subscription,
            })
            .expect("second attach reply");
        framed
            .send(&DaemonMessage::SubscriptionEvent {
                subscription_id: second_subscription,
                envelope: devboule_protocol::SessionEventEnvelope {
                    session_id: "s.routing".to_string(),
                    generation: 1,
                    transcript_seq: None,
                    event: SessionEvent::AgentMessage {
                        message_id: None,
                        text: "b-1".to_string(),
                        parent_tool_use_id: None,
                        spawn_depth: None,
                    },
                },
            })
            .expect("B event");
        let _ = release_rx.recv_timeout(Duration::from_secs(10));
    });

    let connection_deadline = std::time::Instant::now() + Duration::from_secs(10);
    let connection = loop {
        match crate::transport::connect(&paths) {
            Ok(connection) => break connection,
            Err(_) if std::time::Instant::now() < connection_deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("connect: {error}"),
        }
    };
    let client = super::handshake(
        connection,
        devboule_protocol::ClientHello::m3a(
            super::test_owner("client-routing-test").expect("owner"),
            "client-routing-test",
        ),
    )
    .expect("handshake");
    let (a_tx, a_rx) = mpsc::channel();
    client
        .session_attach(
            "s.routing",
            None,
            Arc::new(move |envelope| {
                let _ = a_tx.send(envelope);
            }),
        )
        .expect("attach A");
    let (b_tx, b_rx) = mpsc::channel();
    client
        .session_attach(
            "s.routing",
            None,
            Arc::new(move |envelope| {
                let _ = b_tx.send(envelope);
            }),
        )
        .expect("attach B");

    let a_events = [
        a_rx.recv_timeout(Duration::from_secs(10))
            .expect("first subscription event")
            .event,
        a_rx.recv_timeout(Duration::from_secs(10))
            .expect("second subscription event")
            .event,
    ];
    let b_events = [b_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("other subscription event")
        .event];
    let text = |event: &SessionEvent| match event {
        SessionEvent::AgentMessage { text, .. } => text.clone(),
        other => format!("{other:?}"),
    };
    assert_eq!(
        a_events.iter().map(text).collect::<Vec<_>>(),
        vec!["a-1", "a-2"]
    );
    assert_eq!(
        b_events.iter().map(text).collect::<Vec<_>>(),
        vec!["b-1"],
        "the second subscription must not receive the first subscription's events"
    );

    let _ = release_tx.send(());
    drop(client);
    server.join().expect("server joins");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(windows)]
#[test]
fn session_detach_removes_only_its_subscription() {
    let dir = crate::test_dirs::test_temp_dir("devboule-client-detach-pending");
    let paths = crate::paths::RuntimePaths::from_dir(&dir);
    let stop = Arc::new(AtomicBool::new(false));
    let mut listener = NamedPipeListener::bind(&paths, Arc::clone(&stop)).expect("bind");
    let (attach_seen_tx, attach_seen_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let file = listener.accept().expect("accept");
        let framed = Framed::new(file);
        let hello = framed.recv::<ClientMessage>().expect("client hello");
        assert!(matches!(hello, ClientMessage::Hello(_)));
        framed
            .send(&DaemonMessage::Hello(DaemonHello::plugin_backend(
                "detach-pending-test",
                std::process::id(),
            )))
            .expect("hello reply");

        let attach = framed.recv::<ClientMessage>().expect("attach request");
        let ClientMessage::SessionAttach {
            id: attach_id,
            subscription_id,
            ..
        } = attach
        else {
            panic!("expected attach request");
        };
        attach_seen_tx.send(()).expect("attach seen");
        framed
            .send(&DaemonMessage::SessionAttached {
                id: attach_id,
                subscription_id,
            })
            .expect("attach reply");
        let detach = framed.recv::<ClientMessage>().expect("detach request");
        let detach_id = detach.request_id().expect("detach id");
        framed
            .send(&DaemonMessage::Ok { id: detach_id })
            .expect("detach reply");
        release_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("detach completed");
        framed
            .send(&DaemonMessage::SubscriptionEvent {
                subscription_id,
                envelope: devboule_protocol::SessionEventEnvelope {
                    session_id: "s.detach.pending".to_string(),
                    generation: 1,
                    transcript_seq: None,
                    event: SessionEvent::AgentMessage {
                        message_id: None,
                        text: "resurrected".to_string(),
                        parent_tool_use_id: None,
                        spawn_depth: None,
                    },
                },
            })
            .expect("late event");
    });

    let connection_deadline = std::time::Instant::now() + Duration::from_secs(10);
    let connection = loop {
        match crate::transport::connect(&paths) {
            Ok(connection) => break connection,
            Err(_) if std::time::Instant::now() < connection_deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("connect: {error}"),
        }
    };
    let client = Arc::new(
        super::handshake(
            connection,
            devboule_protocol::ClientHello::m3a(
                super::test_owner("client-detach-pending-test").expect("owner"),
                "client-detach-pending-test",
            ),
        )
        .expect("handshake"),
    );
    let (event_tx, event_rx) = mpsc::channel();
    let attach_client = Arc::clone(&client);
    let attach_thread = thread::spawn(move || {
        attach_client.session_attach(
            "s.detach.pending",
            None,
            Arc::new(move |envelope| {
                let _ = event_tx.send(envelope);
            }),
        )
    });
    attach_seen_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("attach reached server");
    let subscription_id = attach_thread
        .join()
        .expect("attach joins")
        .expect("attach succeeds");
    client
        .session_detach_with_subscription("s.detach.pending", subscription_id)
        .expect("detach roundtrip");
    release_tx.send(()).expect("release server");
    assert!(event_rx.recv_timeout(Duration::from_millis(100)).is_err());

    drop(client);
    server.join().expect("server joins");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A daemon that predates `ToolPolicyGet`/`ToolPolicySet` answers nothing
/// to them: its reader cannot deserialize the variants. The client helper
/// therefore has to refuse on the negotiated capability instead of sending
/// a frame that would kill the connection — this fake daemon replies to no
/// request at all, so a helper that did send one would sit out the 30
/// second RPC deadline instead of returning here.
#[cfg(windows)]
#[test]
fn a_daemon_that_did_not_negotiate_tool_policy_is_never_sent_a_policy_rpc() {
    let dir = crate::test_dirs::test_temp_dir("devboule-client-tool-policy-cap");
    let paths = crate::paths::RuntimePaths::from_dir(&dir);
    let stop = Arc::new(AtomicBool::new(false));
    let mut listener = NamedPipeListener::bind(&paths, Arc::clone(&stop)).expect("bind");
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let file = listener.accept().expect("accept");
        let framed = Framed::new(file);
        let hello = framed.recv::<ClientMessage>().expect("client hello");
        assert!(matches!(hello, ClientMessage::Hello(_)));
        // The plugin-backend set: the capabilities of a daemon from before
        // the tool policy existed.
        framed
            .send(&DaemonMessage::Hello(DaemonHello::plugin_backend(
                "tool-policy-cap-test",
                std::process::id(),
            )))
            .expect("hello reply");

        let _ = release_rx.recv_timeout(Duration::from_secs(10));
        // Bounded, so a pipe left open by a bug cannot hang the suite: an
        // `Ok` here is a policy RPC that should never have been sent, an
        // `Err` is the closed pipe.
        let next = framed.recv_timeout::<ClientMessage>(Duration::from_millis(500));
        assert!(
            next.is_err(),
            "a client must not send a policy RPC to a daemon that did not \
                 advertise the capability, got {next:?}"
        );
    });

    let connection_deadline = std::time::Instant::now() + Duration::from_secs(10);
    let connection = loop {
        match crate::transport::connect(&paths) {
            Ok(connection) => break connection,
            Err(_) if std::time::Instant::now() < connection_deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("connect: {error}"),
        }
    };
    let client = super::handshake(
        connection,
        devboule_protocol::ClientHello::m3a(
            super::test_owner("tool-policy-cap-test").expect("owner"),
            "tool-policy-cap-test",
        ),
    )
    .expect("handshake");
    assert!(
        !client
            .hello()
            .capabilities
            .iter()
            .any(|capability| capability.as_str() == devboule_protocol::caps::TOOL_POLICY),
        "the fake daemon must not have offered the capability"
    );

    for error in [
        client.tool_policy_get().expect_err("get must be refused"),
        client
            .tool_policy_set("claude", Some(false), Vec::new())
            .expect_err("set must be refused"),
    ] {
        let crate::DaemonError::Handshake(wire) = error else {
            panic!("a capability refusal is a wire error, got {error:?}");
        };
        assert_eq!(
            wire.code,
            devboule_protocol::ErrorCode::CapabilityNotSupported
        );
        assert_eq!(wire.message, "capability 'tool_policy' was not negotiated");
    }

    let _ = release_tx.send(());
    drop(client);
    server.join().expect("server joins");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same door as the tool policy's, for the profile store: a client
/// refuses both RPCs when the handshake did not negotiate
/// `agent_profiles`, so a daemon that predates them is never sent a frame
/// its reader cannot deserialize.
#[cfg(windows)]
#[test]
fn a_daemon_that_did_not_negotiate_agent_profiles_is_never_sent_a_profile_rpc() {
    let dir = crate::test_dirs::test_temp_dir("devboule-client-agent-profiles-cap");
    let paths = crate::paths::RuntimePaths::from_dir(&dir);
    let stop = Arc::new(AtomicBool::new(false));
    let mut listener = NamedPipeListener::bind(&paths, Arc::clone(&stop)).expect("bind");
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let file = listener.accept().expect("accept");
        let framed = Framed::new(file);
        let hello = framed.recv::<ClientMessage>().expect("client hello");
        assert!(matches!(hello, ClientMessage::Hello(_)));
        // The plugin-backend set: the capabilities of a daemon from before
        // the profile store existed.
        framed
            .send(&DaemonMessage::Hello(DaemonHello::plugin_backend(
                "agent-profiles-cap-test",
                std::process::id(),
            )))
            .expect("hello reply");

        let _ = release_rx.recv_timeout(Duration::from_secs(10));
        // Bounded, so a pipe left open by a bug cannot hang the suite: an
        // `Ok` here is a profile RPC that should never have been sent, an
        // `Err` is the closed pipe.
        let next = framed.recv_timeout::<ClientMessage>(Duration::from_millis(500));
        assert!(
            next.is_err(),
            "a client must not send a profile RPC to a daemon that did not \
                 advertise the capability, got {next:?}"
        );
    });

    let connection_deadline = std::time::Instant::now() + Duration::from_secs(10);
    let connection = loop {
        match crate::transport::connect(&paths) {
            Ok(connection) => break connection,
            Err(_) if std::time::Instant::now() < connection_deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("connect: {error}"),
        }
    };
    let client = super::handshake(
        connection,
        devboule_protocol::ClientHello::m3a(
            super::test_owner("agent-profiles-cap-test").expect("owner"),
            "agent-profiles-cap-test",
        ),
    )
    .expect("handshake");
    assert!(
        !client
            .hello()
            .capabilities
            .iter()
            .any(|capability| capability.as_str() == devboule_protocol::caps::AGENT_PROFILES),
        "the fake daemon must not have offered the capability"
    );

    for error in [
        client
            .agent_profiles_get()
            .expect_err("get must be refused"),
        client
            .agent_profiles_set(devboule_protocol::AgentProfilesDocument::default())
            .expect_err("set must be refused"),
    ] {
        let crate::DaemonError::Handshake(wire) = error else {
            panic!("a capability refusal is a wire error, got {error:?}");
        };
        assert_eq!(
            wire.code,
            devboule_protocol::ErrorCode::CapabilityNotSupported
        );
        assert_eq!(
            wire.message,
            "capability 'agent_profiles' was not negotiated"
        );
    }

    let _ = release_tx.send(());
    drop(client);
    server.join().expect("server joins");
    let _ = std::fs::remove_dir_all(&dir);
}

/// One client on a fake daemon that runs `serve` against the connection, plus
/// the teardown both deadline tests need: they differ only in what the far
/// side does with the frames and when.
#[cfg(windows)]
fn with_a_fake_daemon(
    label: &str,
    serve: impl FnOnce(Framed) + Send + 'static,
    body: impl FnOnce(&super::DaemonClient),
) {
    let dir = crate::test_dirs::test_temp_dir(&format!("devboule-client-{label}"));
    let paths = crate::paths::RuntimePaths::from_dir(&dir);
    let stop = Arc::new(AtomicBool::new(false));
    let mut listener = NamedPipeListener::bind(&paths, Arc::clone(&stop)).expect("bind");
    let server_label = label.to_string();
    let server = thread::spawn(move || {
        let file = listener.accept().expect("accept");
        let framed = Framed::new(file);
        let hello = framed.recv::<ClientMessage>().expect("client hello");
        assert!(matches!(hello, ClientMessage::Hello(_)));
        framed
            .send(&DaemonMessage::Hello(DaemonHello::plugin_backend(
                &server_label,
                std::process::id(),
            )))
            .expect("hello reply");
        serve(framed);
    });
    let connection_deadline = std::time::Instant::now() + Duration::from_secs(10);
    let connection = loop {
        match crate::transport::connect(&paths) {
            Ok(connection) => break connection,
            Err(_) if std::time::Instant::now() < connection_deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("connect: {error}"),
        }
    };
    let client = super::handshake(
        connection,
        devboule_protocol::ClientHello::m3a(super::test_owner(label).expect("owner"), label),
    )
    .expect("handshake");
    body(&client);
    drop(client);
    server.join().expect("server joins");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The resume road's deadline is its own and the control plane keeps the
/// short one, proved on one connection under one silence.
///
/// The fake daemon sits on both replies for 400 ms. The test pulls the resume
/// road's deadline down to 120 ms through the seam: the resume must time out,
/// while the `ping` behind it — still on [`RPC_TIMEOUT`] — rides the same
/// silence out and answers.
///
/// Mutants: the road back on `roundtrip` (the resume no longer times out, so
/// `expect_err` fails), and the short deadline wired into the generic path
/// instead of the resume road (the ping times out and its own assert fails).
#[cfg(windows)]
#[test]
fn a_resume_wait_is_the_resume_roads_and_the_ping_keeps_the_default() {
    let slow = Duration::from_millis(400);
    let short = Duration::from_millis(120);
    with_a_fake_daemon(
        "resume-deadline",
        move |framed| {
            while let Ok(request) = framed.recv_timeout::<ClientMessage>(Duration::from_secs(10)) {
                let id = request.request_id().expect("request id");
                let reply = match request {
                    ClientMessage::SessionResume { .. } => DaemonMessage::Resume {
                        id,
                        result: ResumeResult::NotSupported,
                    },
                    ClientMessage::Ping { .. } => DaemonMessage::Pong { id, ts_ms: 7 },
                    other => panic!("unexpected request on this connection: {other:?}"),
                };
                thread::sleep(slow);
                framed.send(&reply).expect("reply");
            }
        },
        move |client| {
            super::SESSION_RESUME_DEADLINE.with(|slot| slot.set(Some(short)));
            let resume = client.session_resume(
                Persistence {
                    kind: PersistenceKind::Acp {
                        handle: "handle-1".to_string(),
                    },
                },
                None,
            );
            let ping = client.ping();
            super::SESSION_RESUME_DEADLINE.with(|slot| slot.set(None));
            let error =
                resume.expect_err("a resume the daemon answers late must wait its own budget");
            assert!(matches!(&error, DaemonError::TimedOut(_)), "got {error:?}");
            assert_eq!(ping.expect("the control plane keeps the default"), 7);
        },
    );
}

/// The create road's deadline is wired to its own budget, and the control
/// plane keeps the default — the sibling of the resume road's proof, on a
/// create the fake daemon never answers.
///
/// The seam pulls the create road's window down to 120 ms: the create must
/// time out there, while the `ping` behind it — still on [`RPC_TIMEOUT`] —
/// rides the same silence out and answers.
///
/// Mutants: the road back on `roundtrip` (the create waits out the default,
/// so the elapsed-time assertion fails), and the short window wired into the
/// generic path instead of the create road (the ping times out).
#[cfg(windows)]
#[test]
fn a_create_wait_is_the_create_roads_and_the_ping_keeps_the_default() {
    let short = Duration::from_millis(120);
    with_a_fake_daemon(
        "create-deadline",
        move |framed| {
            let first = framed
                .recv_timeout::<ClientMessage>(Duration::from_secs(10))
                .expect("the create request");
            assert!(
                matches!(first, ClientMessage::SessionCreate { .. }),
                "expected the create, got {first:?}"
            );
            // The create itself is never answered. The ping that follows the
            // abandon is the only reply on this connection, and its arrival
            // is what proves the short window stayed on the create road.
            let ClientMessage::Ping { id } = framed
                .recv_timeout::<ClientMessage>(Duration::from_secs(10))
                .expect("the frame after the abandoned create")
            else {
                panic!("an abandoned create must be followed by the caller's next request");
            };
            framed
                .send(&DaemonMessage::Pong { id, ts_ms: 12 })
                .expect("pong");
            let trailing = framed.recv_timeout::<ClientMessage>(Duration::from_millis(300));
            assert!(
                trailing.is_err(),
                "nothing else may travel on the wire: {trailing:?}"
            );
        },
        move |client| {
            super::SESSION_CREATE_DEADLINE.with(|slot| slot.set(Some(short)));
            let started = std::time::Instant::now();
            let create = client.session_create_with(None, SessionKind::Acp, None, None, None);
            let elapsed = started.elapsed();
            super::SESSION_CREATE_DEADLINE.with(|slot| slot.set(None));
            let error = create.expect_err("a create the daemon never answers must time out");
            let DaemonError::TimedOut(what) = &error else {
                panic!("expected the timeout the window renders, got {error:?}");
            };
            assert_eq!(what, "waiting for a daemon reply");
            assert!(
                elapsed < Duration::from_secs(5),
                "the create waited on the default, not its own budget: {elapsed:?}"
            );
            assert_eq!(
                client.ping().expect("the control plane keeps the default"),
                12
            );
        },
    );
}

/// A client that gives up on a resume writes nothing else: no cancel request,
/// no close, and the next request travels on the same connection.
///
/// The fake daemon never answers the `SessionResume` frame and reads what
/// follows it. An abandon frame of any kind would arrive between the abandoned
/// request and the caller's next one, and the ping's own reply is what proves
/// the connection survived the abandon.
///
/// Mutant: a frame added to the timeout arm of `roundtrip_with_deadline` (or a
/// detach/close sent by an abandoning client).
#[cfg(windows)]
#[test]
fn giving_up_on_a_resume_writes_nothing_and_leaves_the_connection_usable() {
    let slow = Duration::from_millis(400);
    let short = Duration::from_millis(120);
    with_a_fake_daemon(
        "resume-abandon",
        move |framed| {
            let first = framed
                .recv_timeout::<ClientMessage>(Duration::from_secs(10))
                .expect("the resume request");
            assert!(
                matches!(first, ClientMessage::SessionResume { .. }),
                "expected the resume, got {first:?}"
            );
            let next = framed
                .recv_timeout::<ClientMessage>(Duration::from_secs(10))
                .expect("the frame after the abandoned request");
            let ClientMessage::Ping { id } = next else {
                panic!("an abandoned resume must be followed by the caller's next request, got {next:?}");
            };
            framed
                .send(&DaemonMessage::Pong { id, ts_ms: 3 })
                .expect("pong");
            let trailing = framed.recv_timeout::<ClientMessage>(slow);
            assert!(
                trailing.is_err(),
                "nothing else may travel on the wire: {trailing:?}"
            );
        },
        move |client| {
            super::SESSION_RESUME_DEADLINE.with(|slot| slot.set(Some(short)));
            let resume = client.session_resume(
                Persistence {
                    kind: PersistenceKind::Acp {
                        handle: "handle-1".to_string(),
                    },
                },
                None,
            );
            let ping = client.ping();
            super::SESSION_RESUME_DEADLINE.with(|slot| slot.set(None));
            let error = resume.expect_err("an unanswered resume must time out");
            let DaemonError::TimedOut(what) = &error else {
                panic!("expected the timeout the window renders, got {error:?}");
            };
            assert_eq!(what, "waiting for a daemon reply");
            assert_eq!(
                ping.expect("the connection survives the abandoned request"),
                3
            );
        },
    );
}

/// Parse one trace line into its `key=value` fields, core order included.
fn trace_fields(line: &str) -> Vec<(&str, &str)> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(tokens.first().copied(), Some("rpc"), "{line}");
    tokens[1..]
        .iter()
        .map(|token| token.split_once('=').expect("key=value token"))
        .collect()
}

/// A client whose every frame lands in a file: enough wire for the trace to
/// see a request leave, with no daemon behind it.
fn trace_stub_inner(dir: &std::path::Path) -> Arc<ClientInner> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
        options.custom_flags(FILE_FLAG_OVERLAPPED);
    }
    let framed = Framed::new(options.open(dir.join("wire.out")).expect("wire file"));
    Arc::new(ClientInner {
        framed,
        next_id: std::sync::atomic::AtomicU64::new(1),
        next_subscription_id: std::sync::atomic::AtomicU64::new(1),
        pending: Mutex::new(HashMap::new()),
        pending_subscriptions: Mutex::new(HashMap::new()),
        subscriptions: Mutex::new(HashMap::new()),
        default_subscriptions: Mutex::new(HashMap::new()),
        session_state_subscription: Mutex::new(None),
        delegation_subscription: Mutex::new(None),
        stop: AtomicBool::new(false),
        hello: DaemonHello::plugin_backend("trace-test", std::process::id()),
        server_pid: None,
    })
}

/// The trace names the command the caller waited on: a `start` line at
/// departure carrying the calling thread, a `done` line at arrival carrying
/// the waited time — and never the payload the request rode in on.
#[test]
fn the_roundtrip_trace_names_the_wait_the_thread_and_no_payload() {
    let dir = crate::rpc_trace::tests::scratch("roundtrip-ok");
    let _env = crate::rpc_trace::tests::trace_on(&dir);
    let inner = trace_stub_inner(&dir);
    let responder = {
        let inner = Arc::clone(&inner);
        thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                let reply = {
                    let pending = inner.pending.lock().unwrap_or_else(|err| err.into_inner());
                    pending.get(&987_654_321).cloned()
                };
                if let Some(tx) = reply {
                    tx.send(DaemonMessage::Ok { id: 987_654_321 })
                        .expect("reply channel");
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "the request never reached the pending map"
                );
                thread::sleep(Duration::from_millis(5));
            }
        })
    };
    let client = super::DaemonClient {
        inner,
        reader: Mutex::new(None),
    };

    let result = client.roundtrip_with_deadline(
        ClientMessage::AgentMessageSend {
            id: 987_654_321,
            from_session: "session-a".to_string(),
            to_session: "session-b".to_string(),
            text: "TRACE-SENTINEL-4d07 the prompt body must not be logged".to_string(),
            idempotency_key: None,
        },
        Duration::from_secs(5),
    );
    assert!(result.is_ok(), "{result:?}");
    responder.join().expect("responder thread");

    let log = crate::rpc_trace::tests::read_app_log(&dir);
    // The env sink is process-wide: roundtrips of tests running in parallel
    // land in this file too. Both of this test's lines carry its distinctive
    // name+id pair (adjacent core fields), and no other test uses that id.
    let mine: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("name=AgentMessageSend id=987654321"))
        .collect();
    assert_eq!(mine.len(), 2, "one start, one done: {log}");

    let start = trace_fields(mine[0]);
    let start_keys: Vec<&str> = start.iter().map(|(key, _)| *key).collect();
    assert_eq!(
        start_keys,
        [
            "side",
            "event",
            "t",
            "name",
            "id",
            "thread",
            "tid",
            "budget_ms"
        ],
        "{}",
        mine[0]
    );
    assert_eq!(start[0].1, "app");
    assert_eq!(start[1].1, "start");
    start[2].1.parse::<u64>().expect("t is epoch ms");
    assert_eq!(start[3].1, "AgentMessageSend");
    assert_eq!(start[4].1, "987654321");
    assert!(!start[5].1.is_empty(), "the calling thread is named");
    start[6].1.parse::<u64>().expect("tid is numeric");
    assert_eq!(start[7].1, "5000");

    let done = trace_fields(mine[1]);
    let done_keys: Vec<&str> = done.iter().map(|(key, _)| *key).collect();
    assert_eq!(
        done_keys,
        [
            "side",
            "event",
            "t",
            "name",
            "id",
            "waited_ms",
            "budget_ms",
            "status"
        ],
        "{}",
        mine[1]
    );
    assert_eq!(done[1].1, "done");
    assert_eq!(done[3].1, "AgentMessageSend");
    done[5].1.parse::<u64>().expect("waited_ms is a duration");
    assert_eq!(done[6].1, "5000");
    assert_eq!(done[7].1, "ok");

    assert!(!log.contains("TRACE-SENTINEL"), "{log}");
    assert!(!log.contains("session-a"), "{log}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The deadline the request carried shows up in the log: a wait that ends at
/// the budget is `status=timeout`, and `waited_ms` proves how long the
/// calling thread stood still.
#[test]
fn the_roundtrip_trace_reports_the_deadline_it_expired_on() {
    let dir = crate::rpc_trace::tests::scratch("roundtrip-timeout");
    let _env = crate::rpc_trace::tests::trace_on(&dir);
    let inner = trace_stub_inner(&dir);
    let client = super::DaemonClient {
        inner,
        reader: Mutex::new(None),
    };

    let result = client.roundtrip_with_deadline(
        ClientMessage::Ping { id: 987_654_322 },
        Duration::from_millis(100),
    );
    assert!(
        matches!(result, Err(DaemonError::TimedOut(_))),
        "{result:?}"
    );

    let log = crate::rpc_trace::tests::read_app_log(&dir);
    // Parallel tests share this file; both of this test's lines carry its
    // distinctive name+id pair, and no other test uses that id.
    let mine: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("name=Ping id=987654322"))
        .collect();
    assert_eq!(mine.len(), 2, "one start, one done: {log}");
    let start = trace_fields(mine[0]);
    assert_eq!(start[3].1, "Ping");
    assert_eq!(start[4].1, "987654322");
    assert_eq!(start[7].1, "100");
    let done = trace_fields(mine[1]);
    assert_eq!(done[1].1, "done");
    assert_eq!(done[3].1, "Ping");
    assert_eq!(done[4].1, "987654322");
    let waited: u64 = done[5].1.parse().expect("waited_ms is a duration");
    assert!(waited >= 100, "waited {waited} ms: {log}");
    assert_eq!(done[6].1, "100");
    assert_eq!(done[7].1, "timeout");
    let _ = std::fs::remove_dir_all(&dir);
}
