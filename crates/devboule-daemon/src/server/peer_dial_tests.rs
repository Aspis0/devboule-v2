//! Tests for the outbound dial (`peer_dial.rs`), kept out of the production
//! file: a fake Noise responder and the fixture rows are test-only weight.

use super::*;
use crate::journal::PeerRecord;
use crate::peer_transport::responder_handshake;
use devboule_protocol::{DaemonHello, PROTOCOL_MIN_VERSION, PROTOCOL_VERSION};
use std::io;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// One X25519 keypair for the fixtures: the fake responder holds the
/// private half, the stored row pins the public half — the same shape a
/// real pairing writes into `peers`.
fn pinned_keypair() -> snow::Keypair {
    snow::Builder::new(PEER_NOISE_PATTERN.parse().expect("pattern"))
        .generate_keypair()
        .expect("keypair")
}

fn dial_row(address: String, pinned_public: &[u8]) -> PeerRecord {
    PeerRecord {
        device_id: "b".to_string(),
        display_name: "peer b".to_string(),
        role: "daemon".to_string(),
        public_key: pinned_public.to_vec(),
        paired_by_user: None,
        binding_kind: "tailscale".to_string(),
        binding_stable_id: None,
        binding_node_name: None,
        binding_login_name: None,
        address,
        paired_at: 0,
        revoked_at: None,
        caps: vec!["view".to_string()],
    }
}

/// A fake responder for the loopback dials the tests make — loopback is a
/// test-only dial target (`is_tailnet_or_test_loopback`). Completes the
/// responder side of the steady-state handshake, answers the hello, then
/// answers the next request with `reply`.
fn spawn_talking_responder(static_private: [u8; 32], reply: DaemonMessage) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fake responder");
    let address = listener.local_addr().expect("fake responder address");
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept one dial");
        let deadline = Instant::now() + Duration::from_secs(10);
        let session = responder_handshake(
            &stream,
            deadline,
            &static_private,
            PEER_PROLOGUE,
            None,
            PEER_NOISE_PATTERN,
        )
        .expect("fake responder handshake");
        let (reader, writer, closer) = split_session(&stream, session).expect("split");
        let framed = Framed::from_stream(reader, writer, closer);
        let hello: ClientMessage = framed
            .recv_timeout(Duration::from_secs(10))
            .expect("the dial's hello");
        assert!(matches!(hello, ClientMessage::Hello(_)));
        framed
            .send(&DaemonMessage::Hello(DaemonHello {
                protocol_version: PROTOCOL_VERSION,
                min_protocol_version: PROTOCOL_MIN_VERSION,
                daemon_version: "test".to_string(),
                instance_id: "fake-responder".to_string(),
                pid: std::process::id(),
                capabilities: Vec::new(),
            }))
            .expect("hello reply");
        let _request: ClientMessage = framed
            .recv_timeout(Duration::from_secs(10))
            .expect("the dial's request");
        framed.send(&reply).expect("send the canned reply");
    });
    address
}

/// A responder that accepts and holds every dial without speaking, so a
/// dial against it burns its handshake budget while holding whatever the
/// dial path reserves. Counts accepts, so a test can see how many dials
/// really connected.
fn spawn_stall_responder() -> (SocketAddr, Arc<AtomicBool>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stall responder");
    let address = listener.local_addr().expect("stall responder address");
    let stop = Arc::new(AtomicBool::new(false));
    let accepts = Arc::new(AtomicUsize::new(0));
    let (stop_for_thread, accepts_for_thread) = (Arc::clone(&stop), Arc::clone(&accepts));
    std::thread::spawn(move || {
        listener
            .set_nonblocking(true)
            .expect("stall listener goes non-blocking");
        let mut held: Vec<TcpStream> = Vec::new();
        loop {
            if stop_for_thread.load(Ordering::SeqCst) {
                for stream in held.drain(..) {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
                return;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    accepts_for_thread.fetch_add(1, Ordering::SeqCst);
                    held.push(stream);
                }
                Err(ref error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => return,
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    (address, stop, accepts)
}

#[test]
fn call_peer_answers_one_request_through_the_paired_row() {
    let state = ServerState::new("peer-dial-happy".into());
    let keypair = pinned_keypair();
    let address = spawn_talking_responder(
        keypair.private.clone().try_into().expect("32 bytes"),
        DaemonMessage::Sessions {
            id: 5,
            sessions: Vec::new(),
        },
    );
    state
        .peer_upsert(dial_row(address.to_string(), &keypair.public))
        .expect("upsert the row");
    let reply = call_peer(&state, "b", ClientMessage::SessionsList { id: 5 })
        .expect("the dialled request is answered");
    match reply {
        DaemonMessage::Sessions { id, sessions } => {
            assert_eq!(id, 5, "the reply carries the request id");
            assert!(
                sessions.is_empty(),
                "the canned reply comes back: {sessions:?}"
            );
        }
        other => panic!("expected Sessions, got {other:?}"),
    }
}

#[test]
fn a_revoked_row_is_not_dialable() {
    let state = ServerState::new("peer-dial-revoked".into());
    let keypair = pinned_keypair();
    let address = spawn_talking_responder(
        keypair.private.clone().try_into().expect("32 bytes"),
        DaemonMessage::Sessions {
            id: 6,
            sessions: Vec::new(),
        },
    );
    state
        .peer_upsert(dial_row(address.to_string(), &keypair.public))
        .expect("upsert the row");
    state.peer_revoke("b", 1).expect("revoke the row");
    let error = call_peer(&state, "b", ClientMessage::SessionsList { id: 6 })
        .expect_err("a revoked peer is not dialable");
    assert_eq!(error.step(), "revoked", "{error}");
}

/// A row whose port is `0` is the record of a peer that never advertised a
/// listener port. The dial refuses with its own named step before any socket
/// is opened: connecting anyway would mean dialling whatever now owns an
/// ephemeral port, and the remedy is to re-pair, not to retry.
#[test]
fn a_dial_to_port_zero_refuses_before_it_connects() {
    let keypair = pinned_keypair();
    let private: [u8; 32] = keypair.private.clone().try_into().expect("32 bytes");
    let hello = ClientHello::m3a(
        OwnerId::new("peer_fixture", "devboule-daemon").expect("owner"),
        "devboule-daemon",
    );
    let error = dial_peer(
        &private,
        &keypair.public,
        "100.64.0.10:0",
        &hello,
        &ClientMessage::SessionsList { id: 1 },
    )
    .expect_err("a peer that never advertised a port is not dialable");
    assert_eq!(error.step(), "no_listen_port", "{error}");
}

/// A revoke that lands while the dial is connecting is heard before any
/// application byte leaves. The ordering is a barrier, not a sleep: the
/// accept proves the dial already passed its first revocation check, the
/// revoke is written, and only then is the handshake allowed to finish.
#[test]
fn a_revoke_during_the_handshake_stops_the_request() {
    let state = ServerState::new("peer-dial-revoke-race".into());
    let keypair = pinned_keypair();
    let private: [u8; 32] = keypair.private.clone().try_into().expect("32 bytes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fake responder");
    let address = listener.local_addr().expect("fake responder address");
    let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel();
    let (saw_tx, saw_rx) = std::sync::mpsc::channel();

    let responder = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept one dial");
        accepted_tx.send(()).expect("report the accept");
        go_rx.recv().expect("wait until the revoke has landed");
        let session = responder_handshake(
            &stream,
            Instant::now() + Duration::from_secs(10),
            &private,
            PEER_PROLOGUE,
            None,
            PEER_NOISE_PATTERN,
        )
        .expect("fake responder handshake");
        let (reader, writer, closer) = split_session(&stream, session).expect("split");
        let framed = Framed::from_stream(reader, writer, closer);
        // The dialer must refuse before its hello, so nothing may arrive.
        let got = framed
            .recv_timeout::<ClientMessage>(Duration::from_secs(2))
            .is_ok();
        saw_tx.send(got).expect("report what arrived");
    });

    state
        .peer_upsert(dial_row(address.to_string(), &keypair.public))
        .expect("upsert the row");
    let dialer = {
        let state = Arc::clone(&state);
        std::thread::spawn(move || call_peer(&state, "b", ClientMessage::SessionsList { id: 7 }))
    };
    accepted_rx.recv().expect("the dial connected");
    state
        .peer_revoke("b", 1)
        .expect("revoke while the dial is in flight");
    go_tx.send(()).expect("let the handshake finish");

    let error = dialer
        .join()
        .expect("the dialer thread")
        .expect_err("a peer revoked mid-dial must not be talked to");
    assert_eq!(error.step(), "revoked", "{error}");
    assert!(
        !saw_rx.recv().expect("the responder reports"),
        "no application frame may reach a peer revoked before the handshake finished"
    );
    responder.join().expect("responder thread");
}

#[test]
fn the_outbound_cap_caps_concurrent_dials() {
    let state = ServerState::new("peer-dial-cap".into());
    let keypair = pinned_keypair();
    let (address, stop, accepts) = spawn_stall_responder();
    state
        .peer_upsert(dial_row(address.to_string(), &keypair.public))
        .expect("upsert the row");

    std::thread::scope(|scope| {
        let mut dials = Vec::new();
        for _ in 0..MAX_OUTBOUND_CALLS {
            let state = &state;
            dials.push(
                scope.spawn(move || call_peer(state, "b", ClientMessage::SessionsList { id: 1 })),
            );
        }
        // The cap is genuinely full: every dial is parked in its handshake
        // against the silent responder, on the far side of connect.
        let deadline = Instant::now() + Duration::from_secs(5);
        while accepts.load(Ordering::SeqCst) < MAX_OUTBOUND_CALLS {
            assert!(
                Instant::now() < deadline,
                "the capped dials never all connected"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let busy = call_peer(&state, "b", ClientMessage::SessionsList { id: 2 })
            .expect_err("the dial past the cap is refused");
        assert_eq!(busy.step(), "busy", "{busy}");
        // Nothing past the cap ever reached the wire.
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            accepts.load(Ordering::SeqCst),
            MAX_OUTBOUND_CALLS,
            "a dial refused by the cap must not connect"
        );
        // Release the held dials so the scope joins promptly: each fails
        // at its handshake, it never hangs.
        stop.store(true, Ordering::SeqCst);
        for dial in dials {
            assert!(
                dial.join().expect("dial thread").is_err(),
                "a stalled dial fails, it never succeeds silently"
            );
        }
    });
}
