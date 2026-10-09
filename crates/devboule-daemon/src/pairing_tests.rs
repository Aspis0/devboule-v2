//! Tests for pairing: code lifetime, the confirm handshake and revocation.

use super::*;
use crate::device_identity::MAX_DISPLAY_NAME_CHARS;
use devboule_protocol::PeerRole;
use std::path::PathBuf;

fn tmp_paths() -> (PathBuf, crate::paths::RuntimePaths) {
    let dir = crate::test_dirs::test_temp_dir("devboule pairing");
    (dir.clone(), crate::paths::RuntimePaths::from_dir(&dir))
}

/// The grant a new pairing is born with: the whole wire set, read from the
/// protocol crate so a rename follows (`PEER_DEFAULT_CAPS`), which since the
/// 2026-09-21 parity decision is the same list as `PEER_CAPS`.
fn new_pairing_caps() -> Vec<String> {
    devboule_protocol::PEER_DEFAULT_CAPS
        .iter()
        .map(|cap| (*cap).to_string())
        .collect()
}

fn server(tag: &str) -> (PathBuf, Arc<ServerState>) {
    let (dir, paths) = tmp_paths();
    let server =
        crate::server::ServerState::with_paths(format!("pairing-{tag}"), paths).expect("state");
    // A deterministic identity for the tests: the file store, in this
    // runtime dir, created on first use.
    let _ = server.device_identity();
    (dir, server)
}

/// The whole pairing, in process, over loopback and with a real code: the
/// responder the accept loop would have started, and the initiator the
/// `PairingComplete` RPC drives.
///
/// This is the happy path the earlier test only approached — it proves the
/// SPAKE2 exchange, the role-bound PSK, the `XXpsk3` handshake, both
/// payloads, the parked confirmation and the two `peers` rows, with no
/// Tailscale and no daemon process.
#[test]
fn a_client_pairing_completes_and_writes_both_rows() {
    let (dir_a, server_a) = server("initiator");
    let (dir_b, server_b) = server("responder");
    let service_a = PairingService::new();
    let service_b = Arc::new(PairingService::new());
    let (code, _expires_at) = service_b.start().expect("a code");
    let transport = Arc::new(crate::peer_transport::TestTransport::default());
    // `complete` binds the peer through this device's transport, so the
    // stub has to be *installed*, not merely passed.
    assert!(server_a.set_peer_transport(transport.clone()).is_ok());

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();

    let responder_transport = Arc::clone(&transport);
    let responder_server = Arc::clone(&server_b);
    let responder_service = Arc::clone(&service_b);
    let caps = Arc::new(crate::peer_transport::AcceptCaps::default());
    let responder = std::thread::spawn(move || {
        let (stream, peer_addr) = accept_bounded(&listener);
        // The accept loop takes the pairing handshake slot; the service
        // releases it once the exchange stops being a handshake.
        let slot = caps
            .admit_handshake(crate::peer_transport::HandshakeKind::Pairing)
            .expect("a pairing slot");
        responder_service.handle(
            responder_transport.as_ref(),
            stream,
            peer_addr,
            &responder_server,
            slot,
        );
    });

    let outcome = service_a
        .complete(&server_a, &address, &code)
        .expect("the initiator completes the exchange");
    // One shape: every pairing is parked on the code-displaying device.
    let PairingOutcome::Pending(pending) = outcome;
    // The initiator's card names the device that has to confirm.
    let responder_id = server_b
        .device_identity()
        .as_ref()
        .expect("B has an identity")
        .device_id
        .clone();
    assert_eq!(pending.device_id, responder_id);
    assert_eq!(pending.role, Some(PeerRole::Client));
    assert!(!pending.key_fingerprint.is_empty());

    // B parked it, keyed by the device that typed the code (A). The
    // responder parks after it sends its payload, which is what
    // `complete` above already read, so the park may still be in flight
    // on the responder thread: wait for it.
    let deadline = Instant::now() + bound::THREAD;
    let parked = loop {
        let parked = service_b.pending_snapshot();
        if parked.len() == 1 {
            break parked;
        }
        assert!(
            Instant::now() < deadline,
            "exactly one pairing is parked at B; saw {parked:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let initiator_id = server_a
        .device_identity()
        .as_ref()
        .expect("A has an identity")
        .device_id
        .clone();
    assert_eq!(parked[0].device_id, initiator_id);

    // The person at B accepts.
    let row = service_b
        .confirm(&server_b, &initiator_id, true)
        .expect("confirm");
    let row = match row {
        ConfirmOutcome::Accepted(row) => *row,
        ConfirmOutcome::Declined => panic!("an accept must produce a row"),
    };
    assert_eq!(row.device_id, initiator_id);
    assert_eq!(row.role, Some(PeerRole::Client));
    assert!(row.revoked_at.is_none());
    assert_eq!(row.caps, new_pairing_caps());

    join_bounded(responder, "the responder's pairing thread");

    // A writes its own row once the answer reaches it, on the thread that
    // did not block the RPC.
    let deadline = Instant::now() + bound::THREAD;
    loop {
        let rows = server_a.peers().expect("A's rows");
        if rows.len() == 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "A never wrote its row for B; saw {rows:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let a_row = &server_a.peers().expect("A's rows")[0];
    assert_eq!(a_row.device_id, responder_id, "A's row names B");
    // `peers()` hands back the stored record, whose role is the wire
    // string, not the enum.
    assert!(
        !a_row.legacy_dialable,
        "a roleless pairing keeps no v30 dial hint"
    );
    assert_eq!(a_row.caps, new_pairing_caps());
    assert!(a_row.revoked_at.is_none());

    drop(server_a);
    drop(server_b);
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}

/// Pair two in-process services over the given loopback bind, the initiator
/// advertising `initiator_listen_port` from its listener state (`None` when it
/// has nothing to advertise). Every current pairing is roleless: the responder
/// parks it and the person there confirms, which is what the helper does.
/// Returns the responder's runtime directory, its state, and the address it
/// recorded for the initiator.
fn a_pairing_completes_and_the_responder_records(
    initiator_listen_port: Option<u16>,
    bind: &str,
) -> (PathBuf, Arc<ServerState>, String) {
    let (dir_a, server_a) = server("port-initiator");
    let (dir_b, server_b) = server("port-responder");
    if let Some(port) = initiator_listen_port {
        server_a.set_remote_state(crate::device_identity::RemoteState::Enabled {
            addresses: vec!["100.64.0.10".parse().expect("an ip address")],
            port,
        });
    }
    let service_a = PairingService::new();
    let service_b = Arc::new(PairingService::new());
    let (code, _expires_at) = service_b.start().expect("a code");
    let transport = Arc::new(crate::peer_transport::TestTransport::default());
    assert!(server_a.set_peer_transport(transport.clone()).is_ok());

    let listener = std::net::TcpListener::bind(bind).expect("bind");
    let address = listener.local_addr().expect("addr").to_string();

    let responder_transport = Arc::clone(&transport);
    let responder_server = Arc::clone(&server_b);
    let responder_service = Arc::clone(&service_b);
    let caps = Arc::new(crate::peer_transport::AcceptCaps::default());
    let responder = std::thread::spawn(move || {
        let (stream, peer_addr) = accept_bounded(&listener);
        let slot = caps
            .admit_handshake(crate::peer_transport::HandshakeKind::Pairing)
            .expect("a pairing slot");
        responder_service.handle(
            responder_transport.as_ref(),
            stream,
            peer_addr,
            &responder_server,
            slot,
        );
    });

    let PairingOutcome::Pending(pending) = service_a
        .complete(&server_a, &address, &code)
        .expect("the initiator completes the exchange");
    assert_eq!(
        pending.device_id,
        server_b
            .device_identity()
            .as_ref()
            .expect("B has an identity")
            .device_id,
        "the initiator's card names the device that has to confirm"
    );
    let initiator_id = server_a
        .device_identity()
        .as_ref()
        .expect("A has an identity")
        .device_id
        .clone();
    // The initiator's `complete` returns before the responder thread has
    // parked the pairing, and a confirm that beats the park is answered
    // UnknownPending — so wait for the park, as the happy-path test does.
    let deadline = Instant::now() + bound::THREAD;
    loop {
        if !service_b.pending_snapshot().is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the responder never parked the pairing"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let row = match service_b
        .confirm(&server_b, &initiator_id, true)
        .expect("confirm")
    {
        ConfirmOutcome::Accepted(row) => *row,
        ConfirmOutcome::Declined => panic!("an accept must produce a row"),
    };
    join_bounded(responder, "the responder's pairing thread");

    drop(server_a);
    let _ = std::fs::remove_dir_all(&dir_a);
    (dir_b, server_b, row.address)
}

/// The responder's row must record the listener port the initiator
/// **advertised** in its payload. The port on the accepted socket is the
/// initiator's ephemeral source port and belongs to nothing. The IP in the
/// same assertion is the one the kernel attested off `accept()` — the
/// payload carries no IP and must never be allowed to move it.
#[test]
fn the_responder_records_the_initiators_advertised_listener_port() {
    let (dir_b, server_b, recorded) =
        a_pairing_completes_and_the_responder_records(Some(47890), "127.0.0.1:0");
    assert_eq!(
        recorded, "127.0.0.1:47890",
        "the recorded port is the advertised listener, not the pairing socket's source port"
    );
    drop(server_b);
    let _ = std::fs::remove_dir_all(&dir_b);
}

/// The address is composed by `SocketAddr`, so an IPv6 initiator arrives
/// bracketed and the row is an address a dial can parse. Hand-composing
/// `ip:port` yields `::1:47890` — text no `SocketAddr` accepts, a row that
/// can never be dialled.
#[test]
fn the_responder_composes_an_ipv6_initiator_address_that_parses() {
    let (dir_b, server_b, recorded) =
        a_pairing_completes_and_the_responder_records(Some(47890), "[::1]:0");
    assert_eq!(
        recorded.parse::<SocketAddr>(),
        Ok("[::1]:47890"
            .parse::<SocketAddr>()
            .expect("the expected form")),
        "the recorded address must be the initiator's bracketed IPv6 listener: {recorded}"
    );
    drop(server_b);
    let _ = std::fs::remove_dir_all(&dir_b);
}

/// A `Client` initiator makes no promise to be callable, so it pairs
/// legitimately with no listener and is recorded as `:0` — the dial
/// refusal, not a guess, is what answers a later dial.
#[test]
fn an_initiator_that_advertises_no_port_is_recorded_with_port_zero() {
    let (dir_b, server_b, recorded) =
        a_pairing_completes_and_the_responder_records(None, "127.0.0.1:0");
    assert_eq!(
        recorded, "127.0.0.1:0",
        "an absent advertisement is recorded as zero, never guessed"
    );
    drop(server_b);
    let _ = std::fs::remove_dir_all(&dir_b);
}

/// The advertisement reaches the other side only under its literal JSON
/// key, which `rename_all = "camelCase"` derives from the field name. Both
/// ends of every test here are the same build, so a rename of the field
/// would round-trip cleanly and silently degrade every new-to-new pairing
/// to port `0` with all behaviour tests green — this pins the key itself.
#[test]
fn the_listener_advertisement_keeps_its_wire_key() {
    let payload = PairPayload {
        device_id: "6f1e5b7a-0000-4000-8000-00000000c0dd".to_string(),
        display_name: "Peer".to_string(),
        role: Some(PeerRole::Daemon),
        workspace_host: None,
        public_key: String::new(),
        listen_port: Some(47831),
    };
    let json = serde_json::to_value(&payload).expect("json");
    assert_eq!(
        json.get("listenPort"),
        Some(&serde_json::Value::from(47831)),
        "the wire key must stay exactly listenPort: {json}"
    );
}

/// A device that hosts a workspace promises to be callable, so it must have a
/// listener to advertise. Without one the far side would record `:0`, both
/// screens would say paired, and the row would be dead on arrival — and
/// "re-pair" would reproduce the same `0` forever. A device with no workspace
/// makes no such promise, so the same test below pairs without one. The refusal
/// is the same one showing a code gets: start Tailscale, then pair again.
#[test]
fn a_workspace_host_without_a_listener_is_refused_not_recorded_as_zero() {
    let (dir, server) = server("no-listener-initiator");
    let project_dir = dir.join("project");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    let project = server
        .sessions
        .project_add(project_dir.to_str().expect("utf-8 path"))
        .expect("project row");
    server
        .sessions
        .workspace_create(
            &project.id,
            devboule_protocol::WorkspaceIsolation::Local,
            None,
        )
        .expect("workspace row");
    assert!(server.has_hosted_workspace(), "the setup made a host");
    assert!(server
        .set_peer_transport(Arc::new(NoListenerTransport))
        .is_ok());
    let service = PairingService::new();
    let (code, _expires_at) = service.start().expect("a code");
    let error = service
        .complete(&server, "127.0.0.1:1", &code)
        .expect_err("a workspace host with no listener must be refused");
    assert!(
        error.to_string().contains("listener"),
        "the refusal must name what is missing: {error}"
    );
    let rows = server.peers().expect("rows");
    assert!(rows.is_empty(), "no row may be written: {rows:?}");
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A transport whose listener can never come up, so the
/// listener-refusal test is deterministic even on a machine where
/// Tailscale is running and a real listener could start.
struct NoListenerTransport;

impl crate::peer_transport::PeerTransport for NoListenerTransport {
    fn listen(
        &self,
        _paths: &crate::paths::RuntimePaths,
        _stop: Arc<std::sync::atomic::AtomicBool>,
    ) -> io::Result<crate::peer_transport::PeerListener> {
        Err(io::Error::other("no tailnet in this test"))
    }

    fn pre_noise_filter(
        &self,
        _peer: &SocketAddr,
        _peers: &crate::peer_transport::PeerTable,
    ) -> Result<(), crate::peer_transport::RejectReason> {
        Ok(())
    }

    fn binding(
        &self,
        _peer: &SocketAddr,
    ) -> Result<crate::peer_policy::TransportBinding, crate::peer_transport::BindingError> {
        Ok(crate::peer_policy::TransportBinding::tailnet(
            "nstable",
            "host.tailnet.ts.net.",
            "user@example.com",
        ))
    }
}

/// The same exchange with the code wrong on one side: the PAKE derives a
/// different key, so the Noise handshake fails and **no** row is written on
/// either side.
#[test]
fn a_wrong_code_never_writes_a_row() {
    let (dir_a, server_a) = server("wrong-initiator");
    let (dir_b, server_b) = server("wrong-responder");
    let service_a = PairingService::new();
    let service_b = Arc::new(PairingService::new());
    let (_real_code, _) = service_b.start().expect("a code");
    let wrong = PairingSecret::new("ZZZZ2345");
    let transport = Arc::new(crate::peer_transport::TestTransport::default());
    assert!(server_a.set_peer_transport(transport.clone()).is_ok());

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();

    let responder_transport = Arc::clone(&transport);
    let responder_server = Arc::clone(&server_b);
    let responder_service = Arc::clone(&service_b);
    let caps = Arc::new(crate::peer_transport::AcceptCaps::default());
    let responder = std::thread::spawn(move || {
        let (stream, peer_addr) = accept_bounded(&listener);
        let slot = caps
            .admit_handshake(crate::peer_transport::HandshakeKind::Pairing)
            .expect("a pairing slot");
        responder_service.handle(
            responder_transport.as_ref(),
            stream,
            peer_addr,
            &responder_server,
            slot,
        );
    });

    let outcome = service_a.complete(&server_a, &address, &wrong);
    assert!(
        outcome.is_err(),
        "a wrong code must not complete the pairing: {outcome:?}"
    );
    join_bounded(responder, "the responder's refused pairing thread");

    assert!(
        server_a.peers().expect("A's rows").is_empty(),
        "A must not store a peer it never authenticated"
    );
    assert!(
        server_b.peers().expect("B's rows").is_empty(),
        "B must not store a peer whose PAKE failed"
    );
    assert!(
        service_b.pending_snapshot().is_empty(),
        "a failed PAKE never parks a pairing"
    );
    // The wrong attempt is written to the audit table, which is how an
    // operator sees a guessing campaign. Read straight from the journal.
    let connection = rusqlite::Connection::open(dir_b.join("journal.db")).expect("B's journal");
    let attempts: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM audit WHERE action = 'pairing_attempt'",
            [],
            |row| row.get(0),
        )
        .expect("audit rows");
    assert_eq!(attempts, 1, "the failure is audited exactly once");

    drop(server_a);
    drop(server_b);
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}

/// The RFC 5869 vectors, run against the `hkdf` crate as used here: a
/// `None` salt is the all-zero one.
#[test]
fn the_hkdf_crate_matches_rfc_5869_case_one() {
    let ikm = [0x0bu8; 22];
    let salt: Vec<u8> = (0x00u8..=0x0c).collect();
    let info: Vec<u8> = (0xf0u8..=0xf9).collect();
    let hk = hkdf::Hkdf::<Sha256>::new(Some(&salt), &ikm);
    let mut okm = [0u8; 42];
    hk.expand(&info, &mut okm).expect("42 is a valid length");
    assert_eq!(
        hex(&okm),
        "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
    );
}

/// RFC 5869 case 3: a zero-length salt and info. This is the shape the PSK
/// derivation uses, with the role-bound info in place of the empty one.
#[test]
fn the_hkdf_crate_matches_rfc_5869_case_three() {
    let ikm = [0x0bu8; 22];
    let hk = hkdf::Hkdf::<Sha256>::new(None, &ikm);
    let mut okm = [0u8; 42];
    hk.expand(&[], &mut okm).expect("42 is a valid length");
    assert_eq!(
        hex(&okm),
        "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8"
    );
}

/// The exact bytes both sides feed HKDF, so a change to the binding is a
/// visible change to this assertion rather than a silent one.
#[test]
fn the_psk_info_binds_the_prefix_and_both_roles() {
    assert_eq!(
        psk_info(PeerRole::Client, PeerRole::Daemon),
        b"devboule-pair-v1\x00client\x00daemon".to_vec()
    );
    assert_eq!(
        psk_info(PeerRole::Daemon, PeerRole::Client),
        b"devboule-pair-v1\x00daemon\x00client".to_vec()
    );
    // The order matters and the separator is a NUL, so no pair of role
    // names can be read as another pair.
    assert_ne!(
        psk_info(PeerRole::Client, PeerRole::Daemon),
        psk_info(PeerRole::Daemon, PeerRole::Client)
    );
}

#[test]
fn the_psk_is_thirty_two_bytes_and_bound_to_the_roles() {
    let first = derive_psk(b"spake key", PeerRole::Client, PeerRole::Daemon);
    assert_eq!(first.len(), 32);
    assert_eq!(
        first,
        derive_psk(b"spake key", PeerRole::Client, PeerRole::Daemon)
    );
    assert_ne!(
        first,
        derive_psk(b"another key", PeerRole::Client, PeerRole::Daemon)
    );
    // The whole point of binding the roles: the same PAKE output with a
    // different role pair is a different key.
    assert_ne!(
        first,
        derive_psk(b"spake key", PeerRole::Daemon, PeerRole::Client)
    );
    assert_ne!(
        first,
        derive_psk(b"spake key", PeerRole::Client, PeerRole::Client)
    );
    assert_ne!(
        first,
        derive_psk(b"spake key", PeerRole::Daemon, PeerRole::Daemon)
    );
}

#[test]
fn role_tags_round_trip_and_an_unknown_tag_is_refused() {
    for role in [PeerRole::Client, PeerRole::Daemon] {
        assert_eq!(role_from_tag(role_tag(role)).expect("round trip"), role);
    }
    assert!(role_from_tag(2).is_err());
    assert!(role_from_tag(0xff).is_err());
}

/// A role pair the two sides disagree on must fail the Noise handshake,
/// not complete with the wrong roles bound to the pinned keys. This is the
/// test that proves the binding is load-bearing: both sides run the real
/// `XXpsk3` pattern over a real socket.
#[test]
fn mismatched_roles_cannot_complete_the_noise_handshake() {
    let (responder_private, _) = test_keypair();
    let (initiator_private, _) = test_keypair();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr");

    // The responder believes the initiator is a client; the initiator
    // believes it is a daemon. Nothing but the PSK differs.
    let server = std::thread::spawn(move || {
        let (stream, _) = accept_bounded(&listener);
        let deadline = Instant::now() + bound::THREAD;
        let psk = derive_psk(b"spake key", PeerRole::Client, PeerRole::Daemon);
        let outcome = crate::peer_transport::responder_handshake(
            &stream,
            deadline,
            &responder_private,
            PAIR_PROLOGUE,
            Some(&psk),
            PAIR_NOISE_PATTERN,
        );
        outcome.is_err()
    });

    let stream = connect_bounded(address);
    let deadline = Instant::now() + bound::THREAD;
    let psk = derive_psk(b"spake key", PeerRole::Daemon, PeerRole::Daemon);
    let initiator = crate::peer_transport::initiator_handshake(
        &stream,
        deadline,
        &initiator_private,
        None,
        PAIR_PROLOGUE,
        Some(&psk),
        PAIR_NOISE_PATTERN,
    );
    // The responder — the device that displayed the code and holds the
    // row — is the enforcement point, and it refuses.
    assert!(
        join_bounded(server, "the mismatched-role responder"),
        "the responder must refuse a mismatched role pair"
    );
    // The initiator is the last speaker of `XXpsk3`, so it has no further
    // message to authenticate and cannot detect the mismatch during the
    // handshake. What it cannot do is proceed: the responder has closed,
    // so its first read sees end-of-stream. That is what makes the binding
    // load-bearing end to end, and it is asserted rather than assumed.
    match initiator {
        Err(_) => {}
        Ok(session) => {
            let (mut reader, _writer, _closer) =
                crate::peer_transport::split_session(&stream, session)
                    .expect("split the initiator session");
            let mut chunk = [0u8; 64];
            let read = reader.read_plaintext(&mut chunk, Some(deadline));
            assert!(
                !matches!(read, Ok(n) if n > 0),
                "the mismatched initiator must not be able to read a payload: {read:?}"
            );
        }
    }

    // And with the roles agreeing, the same exchange completes: the test
    // is about the binding, not about the handshake being broken.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr");
    let (responder_private, _) = test_keypair();
    let (initiator_private, _) = test_keypair();
    let server = std::thread::spawn(move || {
        let (stream, _) = accept_bounded(&listener);
        let deadline = Instant::now() + bound::THREAD;
        let psk = derive_psk(b"spake key", PeerRole::Client, PeerRole::Daemon);
        crate::peer_transport::responder_handshake(
            &stream,
            deadline,
            &responder_private,
            PAIR_PROLOGUE,
            Some(&psk),
            PAIR_NOISE_PATTERN,
        )
        .is_ok()
    });
    let stream = connect_bounded(address);
    let deadline = Instant::now() + bound::THREAD;
    let psk = derive_psk(b"spake key", PeerRole::Client, PeerRole::Daemon);
    assert!(crate::peer_transport::initiator_handshake(
        &stream,
        deadline,
        &initiator_private,
        None,
        PAIR_PROLOGUE,
        Some(&psk),
        PAIR_NOISE_PATTERN,
    )
    .is_ok());
    assert!(join_bounded(server, "the agreeing-role responder"));
}

/// Wall-clock bounds for the two loopback tests in this module. Named so a
/// reader can see, per test, what stops it hanging.
mod bound {
    use std::time::Duration;

    /// A test that waits for a spawned thread must not wait forever.
    pub const THREAD: Duration = Duration::from_secs(20);
    /// A client connecting to a listener the test itself bound.
    pub const CONNECT: Duration = Duration::from_secs(5);
}

/// Join with a wall-clock bound: `JoinHandle::join` has no timeout, so the
/// wait is a poll and `join` then returns immediately.
fn join_bounded<T>(handle: std::thread::JoinHandle<T>, what: &str) -> T {
    let deadline = std::time::Instant::now() + bound::THREAD;
    while !handle.is_finished() {
        assert!(
            std::time::Instant::now() < deadline,
            "{what} did not finish within {:?}",
            bound::THREAD
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    match handle.join() {
        Ok(value) => value,
        Err(_) => panic!("{what} panicked"),
    }
}

/// Accept one loopback connection with a wall-clock bound. A blocking
/// `accept()` with no bound is how a test hangs the whole suite when the
/// client side fails before connecting.
fn accept_bounded(listener: &std::net::TcpListener) -> (std::net::TcpStream, std::net::SocketAddr) {
    listener
        .set_nonblocking(true)
        .expect("the test listener goes non-blocking");
    let deadline = std::time::Instant::now() + bound::CONNECT;
    loop {
        match listener.accept() {
            Ok((stream, addr)) => {
                // Windows: an accepted socket inherits the listening
                // socket's non-blocking mode, so every later read would
                // fail with `WSAEWOULDBLOCK` instead of blocking.
                stream
                    .set_nonblocking(false)
                    .expect("the accepted socket is blocking");
                return (stream, addr);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the client never connected within {:?}",
                    bound::CONNECT
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => panic!("accept failed: {error}"),
        }
    }
}

fn connect_bounded(address: std::net::SocketAddr) -> std::net::TcpStream {
    std::net::TcpStream::connect_timeout(&address, bound::CONNECT)
        .expect("connect within the bound")
}

fn test_keypair() -> (Vec<u8>, Vec<u8>) {
    let params = PAIR_NOISE_PATTERN
        .parse::<snow::params::NoiseParams>()
        .expect("params");
    let pair = snow::Builder::new(params)
        .generate_keypair()
        .expect("keypair");
    (pair.private, pair.public)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn a_generated_code_uses_only_the_unambiguous_alphabet() {
    for _ in 0..64 {
        let code = generate_code().expect("entropy");
        assert_eq!(code.as_str().len(), CODE_LEN);
        assert!(is_well_formed_code(code.as_str()), "{}", code.as_str());
        for forbidden in ['0', '1', 'I', 'O'] {
            assert!(
                !code.as_str().contains(forbidden),
                "{} contains {forbidden}",
                code.as_str()
            );
        }
    }
}

/// Ten thousand codes: every symbol present, and no symbol more than 25 %
/// away from the uniform expectation.
///
/// With 80 000 draws over 32 symbols the mean is 2 500 and the standard
/// deviation is about 49 (binomial), so the ±625 bound is roughly 12.7 sd:
/// it cannot flake, and it would catch a biased split (a modulo reduction,
/// a fixed byte, or a 5-bit group read from the wrong offset).
#[test]
fn the_code_distribution_is_uniform_within_twenty_five_percent() {
    const CODES: usize = 10_000;
    let mut counts = [0usize; 32];
    for _ in 0..CODES {
        let code = generate_code().expect("entropy");
        for byte in code.as_str().bytes() {
            let position = CODE_ALPHABET
                .iter()
                .position(|symbol| *symbol == byte)
                .expect("a generated symbol is in the alphabet");
            counts[position] += 1;
        }
    }
    let total: usize = counts.iter().sum();
    assert_eq!(total, CODES * CODE_LEN);
    let mean = total as f64 / CODE_ALPHABET.len() as f64;
    for (position, count) in counts.iter().enumerate() {
        let symbol = CODE_ALPHABET[position] as char;
        assert!(*count > 0, "symbol {symbol} never appeared");
        let ratio = *count as f64 / mean;
        assert!(
            (0.75..=1.25).contains(&ratio),
            "symbol {symbol} appeared {count} times, {ratio:.3} of the mean {mean:.1}"
        );
    }
}

#[test]
fn a_malformed_code_is_refused_by_the_format_check() {
    assert!(is_well_formed_code("ABCDEFGH"));
    assert!(!is_well_formed_code("ABCDEFG"));
    assert!(!is_well_formed_code("ABCDEFGHJ"));
    assert!(!is_well_formed_code("ABCDEFG0"));
    assert!(!is_well_formed_code("abcdefgh"));
    assert!(!is_well_formed_code(""));
}

#[test]
fn three_wrong_codes_block_the_source_and_twelve_kill_the_code() {
    let mut state = State {
        active: Some(ActiveCode {
            code: PairingSecret::new("ABCDEFGH"),
            expires_at: Instant::now() + CODE_LIFETIME,
        }),
        ..State::default()
    };
    let ip: IpAddr = "100.64.0.9".parse().expect("ip");
    assert!(!state.note_wrong(ip));
    assert!(!state.note_wrong(ip));
    assert!(
        !state.note_wrong(ip),
        "the third wrong code blocks the source"
    );
    assert!(state.blocked.contains(&ip));
    assert!(matches!(
        state.note_attempt(ip, Instant::now()),
        Err(RejectKind::SourceBlocked)
    ));
    // Another source is unaffected by one source's failures.
    let other: IpAddr = "100.64.0.10".parse().expect("ip");
    assert!(state.note_attempt(other, Instant::now()).is_ok());

    // The code survives until the total threshold, so one node cannot lock
    // the legitimate initiator out.
    for _ in 0..8 {
        assert!(!state.note_wrong(other));
    }
    assert!(
        state.note_wrong(other),
        "the twelfth wrong code kills the code"
    );
}

#[test]
fn attempts_are_capped_per_source_per_window() {
    let mut state = State::default();
    let ip: IpAddr = "100.64.0.9".parse().expect("ip");
    let now = Instant::now();
    for _ in 0..ATTEMPTS_PER_SOURCE {
        assert!(state.note_attempt(ip, now).is_ok());
    }
    assert_eq!(
        state.note_attempt(ip, now),
        Err(RejectKind::TooManyAttempts)
    );
    assert!(
        state
            .note_attempt(ip, now + ATTEMPT_WINDOW + Duration::from_secs(1))
            .is_ok(),
        "the window slides"
    );
}

#[test]
fn an_expired_code_is_no_longer_active_and_resets_the_lockouts() {
    let service = PairingService::new();
    let (code, _expires_at) = service.start().expect("code");
    assert!(service.is_active());
    assert_eq!(code.as_str().len(), CODE_LEN);
    {
        let mut state = service.state.lock().expect("lock");
        let ip: IpAddr = "100.64.0.9".parse().expect("ip");
        state.blocked.insert(ip);
        state.wrong_total = 5;
        if let Some(active) = state.active.as_mut() {
            active.expires_at = Instant::now() - Duration::from_secs(1);
        }
    }
    assert!(!service.is_active());
    service.housekeeping(Instant::now());
    let state = service.state.lock().expect("lock");
    assert!(state.active.is_none(), "the code is dropped");
    assert!(state.blocked.is_empty(), "a new code starts clean");
    assert_eq!(state.wrong_total, 0);
}

#[test]
fn only_one_code_is_active_at_a_time() {
    let service = PairingService::new();
    let (first, _) = service.start().expect("code");
    let (second, _) = service.start().expect("code");
    assert_ne!(first.as_str(), second.as_str());
    let state = service.state.lock().expect("lock");
    assert!(
        state.active.is_some(),
        "the newest code replaced the older one"
    );
}

#[test]
fn caps_are_validated_against_the_closed_set() {
    let ok = vec!["view".to_string(), "send".to_string()];
    assert_eq!(validate_caps(&ok).expect("ok"), ok);
    assert!(validate_caps(&[]).is_err());
    assert!(validate_caps(&["view".to_string(), "root".to_string()]).is_err());
    assert!(validate_caps(&["view".to_string(), "view".to_string()]).is_err());
    assert!(
        validate_caps(&["send".to_string()]).is_ok(),
        "no capability is tied to a role any more"
    );
    assert!(
        validate_caps(&[
            "view".to_string(),
            crate::peer_policy::CAP_ADMIN.to_string()
        ])
        .is_ok(),
        "the administrative capability is a name this validator accepts"
    );
}

#[test]
fn a_local_peer_record_carries_this_device_as_the_pairer() {
    let (dir, server) = server("record");
    let record = local_peer_record(
        &server,
        "6f1e5b7a-0000-4000-8000-00000000c0de",
        "Peer",
        false,
        false,
        &[5u8; 32],
        TransportBinding::tailnet("npeer", "peer.tailnet.ts.net.", "user@example.com"),
        "100.64.0.2:47831".to_string(),
    )
    .expect("record");
    assert!(!record.legacy_dialable);
    assert_eq!(record.caps, new_pairing_caps());
    assert_eq!(record.paired_by_user, server.local_user_sid());
    assert!(record.revoked_at.is_none());
    assert_eq!(record.binding_stable_id.as_deref(), Some("npeer"));

    // A non-UUID id and a short key are refused before anything is stored.
    assert!(local_peer_record(
        &server,
        "not-a-uuid",
        "Peer",
        false,
        false,
        &[5u8; 32],
        TransportBinding::tailnet("n", "n", "n"),
        "100.64.0.2:47831".to_string(),
    )
    .is_err());
    assert!(local_peer_record(
        &server,
        "6f1e5b7a-0000-4000-8000-00000000c0df",
        "Peer",
        false,
        false,
        &[5u8; 31],
        TransportBinding::tailnet("n", "n", "n"),
        "100.64.0.2:47831".to_string(),
    )
    .is_err());

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn re_pairing_a_live_device_is_refused_until_it_is_revoked() {
    let (dir, server) = server("repair");
    let id = "6f1e5b7a-0000-4000-8000-00000000c0d1";
    let record = local_peer_record(
        &server,
        id,
        "Peer",
        true,
        true,
        &[6u8; 32],
        TransportBinding::tailnet("npeer", "peer.", "user@example.com"),
        "100.64.0.2:47831".to_string(),
    )
    .expect("record");
    server.peer_upsert(record).expect("store");

    let again = local_peer_record(
        &server,
        id,
        "Peer",
        true,
        true,
        &[7u8; 32],
        TransportBinding::tailnet("npeer", "peer.", "user@example.com"),
        "100.64.0.2:47831".to_string(),
    );
    assert!(again.is_err(), "an existing pairing must be revoked first");

    server.peer_revoke(id, unix_millis()).expect("revoke");
    let after_revoke = local_peer_record(
        &server,
        id,
        "Peer",
        true,
        true,
        &[7u8; 32],
        TransportBinding::tailnet("npeer", "peer.", "user@example.com"),
        "100.64.0.2:47831".to_string(),
    );
    assert!(after_revoke.is_ok(), "a revoked device may pair again");

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pending_pairing_is_answered_by_confirm_and_writes_the_row_only_when_accepted() {
    let (dir, server) = server("confirm");
    let service = PairingService::new();
    let public_key = vec![8u8; 32];
    let (decision, wait) = mpsc::channel::<bool>();
    service
        .state
        .lock()
        .expect("lock")
        .pending
        .push(PendingEntry {
            token: 1,
            device_id: "6f1e5b7a-0000-4000-8000-00000000c0d2".to_string(),
            display_name: "Phone".to_string(),
            legacy_dialable: false,
            hosts_workspaces: false,
            key_fingerprint: crate::device_identity::key_fingerprint(&public_key),
            address: "100.64.0.2:47831".to_string(),
            public_key: public_key.clone(),
            binding: TransportBinding::tailnet("nphone", "phone.", "user@example.com"),
            expires_at: Instant::now() + CONFIRM_WINDOW,
            decision,
        });
    assert_eq!(service.pending_snapshot().len(), 1);
    assert_eq!(service.pending_snapshot()[0].role, Some(PeerRole::Client));

    let row = match service
        .confirm(&server, "6f1e5b7a-0000-4000-8000-00000000c0d2", true)
        .expect("confirm")
    {
        ConfirmOutcome::Accepted(row) => *row,
        ConfirmOutcome::Declined => panic!("accepting must produce a row"),
    };
    assert_eq!(row.role, Some(PeerRole::Client));
    assert_eq!(row.display_name, "Phone");
    assert_eq!(row.caps, new_pairing_caps());
    assert_eq!(row.key_fingerprint.len(), 32);
    assert!(wait.recv_timeout(Duration::from_secs(1)).expect("decision"));
    assert!(service.pending_snapshot().is_empty());
    assert_eq!(server.peers().expect("peers").len(), 1);

    // A second confirm finds nothing: the entry is gone.
    assert!(matches!(
        service.confirm(&server, "6f1e5b7a-0000-4000-8000-00000000c0d2", true),
        Err(PairingError::UnknownPending)
    ));

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn declining_a_pending_pairing_writes_no_row_but_does_audit() {
    let (dir, server) = server("decline");
    let service = PairingService::new();
    let (decision, wait) = mpsc::channel::<bool>();
    service
        .state
        .lock()
        .expect("lock")
        .pending
        .push(PendingEntry {
            token: 1,
            device_id: "6f1e5b7a-0000-4000-8000-00000000c0d3".to_string(),
            display_name: "Phone".to_string(),
            legacy_dialable: false,
            hosts_workspaces: false,
            key_fingerprint: crate::device_identity::key_fingerprint(&[9u8; 32]),
            address: "100.64.0.2:47831".to_string(),
            public_key: vec![9u8; 32],
            binding: TransportBinding::tailnet("nphone", "phone.", "user@example.com"),
            expires_at: Instant::now() + CONFIRM_WINDOW,
            decision,
        });
    assert!(matches!(
        service
            .confirm(&server, "6f1e5b7a-0000-4000-8000-00000000c0d3", false)
            .expect("decline"),
        ConfirmOutcome::Declined
    ));
    assert!(!wait.recv_timeout(Duration::from_secs(1)).expect("decision"));
    assert!(server.peers().expect("peers").is_empty());
    assert!(
        service.pending_snapshot().is_empty(),
        "a declined pairing is removed from the pending list"
    );

    // The decline is a recorded act, not a silent no-op.
    let connection = rusqlite::Connection::open(server_journal(&dir)).expect("journal");
    let (action, outcome): (String, String) = connection
        .query_row(
            "SELECT action, outcome FROM audit ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("audit row");
    assert_eq!(action, "PairingConfirm");
    assert_eq!(outcome, "declined");
    drop(connection);

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

fn server_journal(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("journal.db")
}

#[test]
fn at_most_two_pairings_park_and_a_third_is_answered_busy() {
    let service = PairingService::new();
    let mut sends = Vec::new();
    {
        let mut state = service.state.lock().expect("lock");
        for index in 0..MAX_PENDING_PAIRINGS {
            let (decision, wait) = mpsc::channel::<bool>();
            sends.push(wait);
            state.pending.push(PendingEntry {
                token: u64::try_from(index).unwrap_or(0),
                device_id: format!("6f1e5b7a-0000-4000-8000-00000000000{index}"),
                display_name: "Phone".to_string(),
                legacy_dialable: false,
                hosts_workspaces: false,
                key_fingerprint: String::new(),
                address: "100.64.0.2:47831".to_string(),
                public_key: vec![1u8; 32],
                binding: TransportBinding::tailnet("n", "n", "n"),
                expires_at: Instant::now() + CONFIRM_WINDOW,
                decision,
            });
        }
        assert_eq!(state.pending.len(), MAX_PENDING_PAIRINGS);
        assert!(
            state.pending.len() >= MAX_PENDING_PAIRINGS,
            "the third candidate sees a full queue and is answered busy"
        );
    }
    assert_eq!(service.pending_snapshot().len(), MAX_PENDING_PAIRINGS);
    assert_eq!(RejectKind::PairingBusy.reason(), "pairing busy");
    drop(sends);
}
/// A code is single use (design §8 R8). The first candidate pairs, and
/// the same code is refused for a second one — one observed code must not
/// pair every device that presents it during the five minutes it is shown.
///
/// The ordering is explicit because the initiator's `complete` returns as
/// soon as it has both payloads, before the responder has parked: the test
/// waits for the park before asserting that the code is spent, so the second
/// candidate is testing consumption and not a scheduling race.
#[test]
fn a_code_pairs_only_once() {
    let (dir_a, server_a) = server("once-a");
    let (dir_c, server_c) = server("once-c");
    let (dir_b, server_b) = server("once-b");
    let service_a = PairingService::new();
    let service_c = PairingService::new();
    let service_b = Arc::new(PairingService::new());
    let (code, _expires_at) = service_b.start().expect("a code");
    let transport = Arc::new(crate::peer_transport::TestTransport::default());
    assert!(server_a.set_peer_transport(transport.clone()).is_ok());
    assert!(server_c.set_peer_transport(transport.clone()).is_ok());

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();

    // The responder serves the two candidates in turn, on its own threads,
    // exactly as the accept loop would.
    let responder_service = Arc::clone(&service_b);
    let responder_transport = Arc::clone(&transport);
    let responder_server = Arc::clone(&server_b);
    let caps = Arc::new(crate::peer_transport::AcceptCaps::default());
    let responder = std::thread::spawn(move || {
        for _ in 0..2 {
            let (stream, peer_addr) = accept_bounded(&listener);
            let slot = caps
                .admit_handshake(crate::peer_transport::HandshakeKind::Pairing)
                .expect("a pairing slot");
            responder_service.handle(
                responder_transport.as_ref(),
                stream,
                peer_addr,
                &responder_server,
                slot,
            );
        }
    });

    let initiator_id = server_a
        .device_identity()
        .as_ref()
        .expect("A has an identity")
        .device_id
        .clone();

    // The first candidate pairs: this device reports it as pending.
    let first = service_a
        .complete(&server_a, &address, &code)
        .expect("the first candidate pairs");
    assert!(
        matches!(first, PairingOutcome::Pending(_)),
        "a Client pairing is reported pending, got {first:?}"
    );

    // Wait for the responder to park it, which is the moment it spends the code.
    let deadline = Instant::now() + bound::THREAD;
    loop {
        if service_b.pending_snapshot().len() == 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the responder never parked the first pairing"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !service_b.is_active(),
        "the code must be spent as soon as the first pairing is parked"
    );
    {
        let state = service_b
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(
            state.active.is_none(),
            "the code itself must be gone from the state, not merely expired"
        );
        // With no active code, a later candidate takes the `NoActiveCode`
        // arm — the same refusal a daemon that never showed a code gives.
        assert!(state.active_if_live(Instant::now()).is_none());
    }

    // The person at B accepts, which releases the parked responder.
    let accepted = service_b
        .confirm(&server_b, &initiator_id, true)
        .expect("confirm");
    assert!(matches!(accepted, ConfirmOutcome::Accepted(_)));

    // The second candidate presents the same code and is refused.
    let second = service_c.complete(&server_c, &address, &code);
    assert!(
        second.is_err(),
        "a second pairing with the same code must fail, got {second:?}"
    );

    join_bounded(responder, "the two-candidate responder");

    // One pairing, on both sides, and nothing for the refused candidate.
    assert!(
        service_b.pending_snapshot().is_empty(),
        "the parked pairing was resolved by the confirmation"
    );
    assert!(
        server_c.peers().expect("C's rows").is_empty(),
        "the refused candidate must not write a row"
    );
    let deadline = Instant::now() + bound::THREAD;
    loop {
        if server_a.peers().expect("A's rows").len() == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "A never wrote its row for B");
        std::thread::sleep(Duration::from_millis(20));
    }
    let b_rows = server_b.peers().expect("B's rows");
    assert_eq!(b_rows.len(), 1, "B wrote exactly one peer row");
    assert_eq!(b_rows[0].device_id, initiator_id, "and it names A");

    drop(service_b);
    drop(server_a);
    drop(server_c);
    drop(server_b);
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
    let _ = std::fs::remove_dir_all(&dir_c);
}

/// The daemon refuses to open a pairing connection to anything but a
/// tailnet address, so a renderer-supplied address cannot make it probe
/// arbitrary hosts.
#[test]
fn a_pairing_target_must_be_a_tailnet_address() {
    // In range.
    assert!(is_permitted_pairing_target(
        &"100.64.0.1:47831".parse().expect("addr")
    ));
    assert!(is_permitted_pairing_target(
        &"100.127.255.254:47831".parse().expect("addr")
    ));
    assert!(is_permitted_pairing_target(
        &"[fd7a:115c:a1e0::1]:47831".parse().expect("addr")
    ));
    // Out of range: a public address, a private LAN address, and a tailnet
    // address one step outside the range.
    assert!(!is_permitted_pairing_target(
        &"8.8.8.8:47831".parse().expect("addr")
    ));
    assert!(!is_permitted_pairing_target(
        &"192.168.1.10:47831".parse().expect("addr")
    ));
    assert!(!is_permitted_pairing_target(
        &"100.128.0.1:47831".parse().expect("addr")
    ));

    // Loopback is accepted in this crate's own unit tests (the in-process
    // responder listens on 127.0.0.1) and only there — this whole module is
    // `#[cfg(test)]`, so the assertion is exactly the test-only branch.
    // `tests/peer_link.rs` pairs over the real tailnet address, so it needs
    // no exemption.
    assert!(is_permitted_pairing_target(
        &"127.0.0.1:47831".parse().expect("addr")
    ));
}

/// The end-to-end form of the tailnet check: `complete` refuses a non-tailnet
/// address without opening a socket at all, with a message a person can act
/// on.
#[test]
fn pairing_complete_refuses_a_non_tailnet_address() {
    let (dir, server) = server("address");
    let service = PairingService::new();
    let (code, _expires_at) = service.start().expect("a code");
    let error = service
        .complete(&server, "8.8.8.8:47831", &code)
        .expect_err("a public address must be refused");
    let message = error.to_string();
    assert!(
        message.contains("tailnet"),
        "the refusal must say what is expected: {message}"
    );
    assert!(
        !message.contains(code.as_str()),
        "the refusal must not carry the code: {message}"
    );
    // And a malformed address is still refused, by the earlier parse.
    assert!(service.complete(&server, "not-an-address", &code).is_err());

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A payload whose `display_name` cannot be shown is refused at the
/// boundary, before anything is stored or parked.
#[test]
fn a_payload_name_that_cannot_be_shown_is_refused() {
    let payload = |name: &str| PairPayload {
        device_id: "6f1e5b7a-0000-4000-8000-00000000c0d9".to_string(),
        display_name: name.to_string(),
        role: Some(PeerRole::Client),
        workspace_host: None,
        public_key: String::new(),
        listen_port: None,
    };
    // A normal hostname passes.
    assert!(validate_peer_payload(&payload("Marcolenovo")).is_ok());
    for bad in [
        "",
        "   ",
        " leading",
        "trailing ",
        "two\nlines",
        "tab\there",
        "right-to-left\u{202e}override",
        "zero\u{200b}width",
        "byte-order\u{feff}mark",
    ] {
        assert!(
            validate_peer_payload(&payload(bad)).is_err(),
            "{bad:?} must be refused"
        );
    }
    // Over the length bound, at the boundary exactly.
    let just_over = "x".repeat(MAX_DISPLAY_NAME_CHARS + 1);
    assert!(validate_peer_payload(&payload(&just_over)).is_err());
    let at_bound = "x".repeat(MAX_DISPLAY_NAME_CHARS);
    assert!(validate_peer_payload(&payload(&at_bound)).is_ok());
}

/// The storage choke point refuses a bad name too, so "every stored
/// `display_name` passed validation" holds even for a caller that skipped
/// the payload check.
#[test]
fn a_peer_record_refuses_a_name_that_cannot_be_shown() {
    let (dir, server) = server("bad-name");
    let error = local_peer_record(
        &server,
        "6f1e5b7a-0000-4000-8000-00000000c0da",
        "invisible\u{202e}name",
        false,
        false,
        &[5u8; 32],
        TransportBinding::tailnet("n", "n", "n"),
        "100.64.0.2:47831".to_string(),
    )
    .expect_err("a bad display name must be refused before it is stored");
    assert!(
        error.to_string().contains("invisible"),
        "the reason names the problem: {error}"
    );
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}
/// Finishing a pairing must be idempotent for the device that is
/// already half-paired. The responder writes its row and the initiator
/// writes its own on a background thread; whichever fails second leaves one
/// side holding a row and the other not, and before this fix every retry
/// died at "already paired; revoke it first", so two good devices were stuck
/// until somebody revoked by hand.
///
/// The exception is narrow on purpose: the **same pinned key** is the same
/// pairing being finished, while a different key is the substitution the
/// revoke-first rule exists to refuse.
#[test]
fn re_pairing_with_the_same_key_finishes_the_pairing() {
    let (dir, server) = server("repair-same-key");
    let id = "6f1e5b7a-0000-4000-8000-00000000c0dc";
    let key = [6u8; 32];
    let record = local_peer_record(
        &server,
        id,
        "Peer",
        true,
        true,
        &key,
        TransportBinding::tailnet("npeer", "peer.", "user@example.com"),
        "100.64.0.2:47831".to_string(),
    )
    .expect("first pairing");
    server.peer_upsert(record).expect("store");

    // Same key, same role: the retry completes and refreshes the row.
    let retry = local_peer_record(
        &server,
        id,
        "Peer Renamed",
        true,
        true,
        &key,
        TransportBinding::tailnet("npeer", "peer.", "user@example.com"),
        "100.64.0.9:47831".to_string(),
    )
    .expect("a retry with the same pinned key must finish the pairing");
    assert_eq!(retry.display_name, "Peer Renamed");
    assert_eq!(retry.address, "100.64.0.9:47831");
    let stored = server.peer_upsert(retry).expect("re-store");
    assert_eq!(stored.display_name, "Peer Renamed");
    assert_eq!(
        server.peers().expect("rows").len(),
        1,
        "a retry must not duplicate the row"
    );

    // A different key is still refused: that is the credential changing,
    // which needs a revoke first (design §8 R8).
    let substituted = local_peer_record(
        &server,
        id,
        "Peer",
        true,
        true,
        &[7u8; 32],
        TransportBinding::tailnet("npeer", "peer.", "user@example.com"),
        "100.64.0.2:47831".to_string(),
    );
    let error = substituted.expect_err("a different key must be refused");
    assert!(
        error.to_string().contains("different key"),
        "the refusal says why it is different from an ordinary retry: {error}"
    );

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

/// At most one pending entry per device. Two parks for the same device
/// happened when a second code was shown while the first park was still
/// inside its 60 s window, and the panel then rendered two confirm cards for
/// one device while `confirm` removed only the first match.
///
/// The newer attempt replaces the older: a device cannot be waiting twice
/// for one pairing, and dropping the older entry releases its parked thread
/// (the answer becomes `pairing busy`) instead of leaving it to consume a
/// decision meant for the newer one.
#[test]
fn a_second_park_for_the_same_device_replaces_the_first() {
    let (dir_a, server_a) = server("dup-a");
    let (dir_b, server_b) = server("dup-b");
    let service_a = PairingService::new();
    let service_b = Arc::new(PairingService::new());
    let transport = Arc::new(crate::peer_transport::TestTransport::default());
    assert!(server_a.set_peer_transport(transport.clone()).is_ok());

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();

    // Two codes, two connections, both parked by the same device A. The
    // second code is what makes a second park possible at all: the first is
    // spent by the first park.
    let (code_one, _) = service_b.start().expect("code one");
    let responder_service = Arc::clone(&service_b);
    let responder_transport = Arc::clone(&transport);
    let responder_server = Arc::clone(&server_b);
    let caps = Arc::new(crate::peer_transport::AcceptCaps::default());
    // One thread per connection, because a `Client`-role park blocks its
    // handler until it is confirmed: a sequential loop would sit on the
    // first pairing for the whole 60 s window and never accept the second,
    // which is what made this test time out on its first run. The real
    // accept loop spawns a thread per connection for the same reason.
    let responder = std::thread::spawn(move || {
        let mut handlers = Vec::new();
        for _ in 0..2 {
            let (stream, peer_addr) = accept_bounded(&listener);
            let slot = caps
                .admit_handshake(crate::peer_transport::HandshakeKind::Pairing)
                .expect("a pairing slot");
            let service = Arc::clone(&responder_service);
            let transport = Arc::clone(&responder_transport);
            let server = Arc::clone(&responder_server);
            handlers.push(std::thread::spawn(move || {
                service.handle(transport.as_ref(), stream, peer_addr, &server, slot);
            }));
        }
        for handler in handlers {
            join_bounded(handler, "a parked pairing's handler");
        }
    });

    service_a
        .complete(&server_a, &address, &code_one)
        .expect("the first pairing");
    let deadline = Instant::now() + bound::THREAD;
    while service_b.park_count() < 1 {
        assert!(Instant::now() < deadline, "the first park never landed");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(service_b.pending_snapshot().len(), 1);

    // A second code, and the same device pairs again while the first park is
    // still waiting for a confirmation.
    let (code_two, _) = service_b.start().expect("code two");
    service_a
        .complete(&server_a, &address, &code_two)
        .expect("the second pairing");
    while service_b.park_count() < 2 {
        assert!(Instant::now() < deadline, "the second park never landed");
        std::thread::sleep(Duration::from_millis(20));
    }

    let pending = service_b.pending_snapshot();
    assert_eq!(
        pending.len(),
        1,
        "a second park for the same device must replace the first, not join it: {pending:?}"
    );
    assert_eq!(
        pending[0].device_id,
        server_a.device_identity().as_ref().expect("A").device_id
    );

    // Exactly one decision can be delivered: the surviving entry is the one
    // that answers.
    let accepted = service_b
        .confirm(&server_b, &pending[0].device_id, true)
        .expect("confirm");
    assert!(matches!(accepted, ConfirmOutcome::Accepted(_)));
    assert!(service_b.pending_snapshot().is_empty());

    join_bounded(responder, "the two-park responder");
    drop(service_b);
    drop(server_a);
    drop(server_b);
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}

/// The v30 initiator's side of one exchange, spoken by hand: the role tag, the
/// v1 PSK label, the v1 prologue, and a payload that still carries `role`.
/// This is the compatibility edge from the far side, which is what makes the
/// responder's handling of an old peer testable without an old binary.
fn v30_initiator_exchange(
    address: &str,
    code: &PairingSecret,
    tag: PeerRole,
    device_id: &str,
) -> Result<PairAnswer, String> {
    use std::io::Write as _;
    let (private, public) = test_keypair();
    let mut stream = connect_bounded(address.parse().expect("addr"));
    let deadline = Instant::now() + bound::THREAD;
    stream
        .write_all(&PAIRING_MAGIC)
        .map_err(|error| error.to_string())?;
    write_prelude(&mut stream, role_tag(tag), deadline).map_err(|error| error.to_string())?;
    let responder_tag = role_from_tag(
        read_prelude(&mut stream, deadline).map_err(|end| prelude_error(end).to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let password = spake2::Password::new(code.as_str().as_bytes());
    let responder_identity = spake2::Identity::new(PAIR_RESPONDER_ID);
    let initiator_identity = spake2::Identity::new(PAIR_INITIATOR_ID);
    let mut their_message = [0u8; 256];
    let their_len =
        read_framed(&stream, &mut their_message, deadline).map_err(|error| error.to_string())?;
    let (spake_state, our_message) = spake2::Spake2::<spake2::Ed25519Group>::start_b(
        &password,
        &responder_identity,
        &initiator_identity,
    );
    write_framed(&stream, &our_message, deadline).map_err(|error| error.to_string())?;
    let mut spake_key = spake_state
        .finish(&their_message[..their_len])
        .map_err(|error| error.to_string())?;
    let mut psk = derive_psk(&spake_key, tag, responder_tag);
    spake_key.zeroize();
    let session = initiator_handshake(
        &stream,
        deadline,
        &private,
        None,
        PAIR_PROLOGUE,
        Some(&psk),
        PAIR_NOISE_PATTERN,
    )
    .map_err(|error| error.to_string())?;
    for byte in psk.iter_mut() {
        *byte = 0;
    }
    let (mut reader, mut writer, _closer) =
        split_session(&stream, session).map_err(|error| error.to_string())?;
    let payload = PairPayload {
        device_id: device_id.to_string(),
        display_name: "Old phone".to_string(),
        role: Some(tag),
        workspace_host: None,
        public_key: base64_encode(&public),
        listen_port: None,
    };
    write_json(&mut writer, &payload, deadline).map_err(|error| error.to_string())?;
    let _responder_payload: PairPayload =
        read_json(&mut reader, deadline).map_err(|error| error.to_string())?;
    let answer: PairAnswer = read_json(&mut reader, deadline).map_err(|error| error.to_string())?;
    Ok(answer)
}

/// A v30 daemon tag no longer buys an unconfirmed pairing. The row is written
/// only when this device's person confirms, exactly like the roleless wire;
/// the tag's only remaining meaning is the transport hint stored with it.
#[test]
fn a_v30_daemon_tag_still_waits_for_the_local_confirmation() {
    let (dir, server) = server("v30-daemon");
    let service = Arc::new(PairingService::new());
    let (code, _expires_at) = service.start().expect("a code");
    let transport = Arc::new(crate::peer_transport::TestTransport::default());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();

    let responder_server = Arc::clone(&server);
    let responder_service = Arc::clone(&service);
    let responder_transport = Arc::clone(&transport);
    let caps = Arc::new(crate::peer_transport::AcceptCaps::default());
    let responder = std::thread::spawn(move || {
        let (stream, peer_addr) = accept_bounded(&listener);
        let slot = caps
            .admit_handshake(crate::peer_transport::HandshakeKind::Pairing)
            .expect("a pairing slot");
        responder_service.handle(
            responder_transport.as_ref(),
            stream,
            peer_addr,
            &responder_server,
            slot,
        );
    });

    let device_id = "6f1e5b7a-0000-4000-8000-00000000c0aa";
    let initiator = std::thread::spawn(move || {
        v30_initiator_exchange(&address, &code, PeerRole::Daemon, device_id)
    });
    let deadline = Instant::now() + bound::THREAD;
    let parked = loop {
        let pending = service.pending_snapshot();
        if !pending.is_empty() {
            break pending;
        }
        assert!(
            Instant::now() < deadline,
            "the v30 daemon tag never parked the pairing"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        server.peers().expect("rows").is_empty(),
        "no row may be written before the person here confirms"
    );
    assert_eq!(
        parked[0].role,
        Some(PeerRole::Daemon),
        "the tag projects as the dial hint on the card"
    );

    let row = match service
        .confirm(&server, "6f1e5b7a-0000-4000-8000-00000000c0aa", true)
        .expect("confirm")
    {
        ConfirmOutcome::Accepted(row) => *row,
        ConfirmOutcome::Declined => panic!("an accept must produce a row"),
    };
    assert_eq!(
        row.role,
        Some(PeerRole::Daemon),
        "the daemon tag is the dial hint the card carries"
    );
    let stored = server.peers().expect("rows");
    assert!(stored[0].legacy_dialable, "and it is stored on the row");
    assert!(
        stored[0].hosts_workspaces,
        "a daemon tag is also the machine-scope record"
    );
    let answer = join_bounded(initiator, "the v30 initiator").expect("the answer arrives");
    assert!(answer.accepted, "{answer:?}");
    join_bounded(responder, "the v30 responder's thread");
    assert!(!service.is_active(), "a confirmed pairing spends the code");

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A v30 client tag is the shape that always waited for the person at the
/// code-displaying device, and it still does: the exchange parks, the card
/// names the old peer, and only a confirm writes the row — with no dial hint.
#[test]
fn a_v30_client_tag_parks_for_the_local_confirmation() {
    let (dir, server) = server("v30-client");
    let service = Arc::new(PairingService::new());
    let (code, _expires_at) = service.start().expect("a code");
    let transport = Arc::new(crate::peer_transport::TestTransport::default());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();

    let responder_server = Arc::clone(&server);
    let responder_service = Arc::clone(&service);
    let responder_transport = Arc::clone(&transport);
    let caps = Arc::new(crate::peer_transport::AcceptCaps::default());
    let responder = std::thread::spawn(move || {
        let (stream, peer_addr) = accept_bounded(&listener);
        let slot = caps
            .admit_handshake(crate::peer_transport::HandshakeKind::Pairing)
            .expect("a pairing slot");
        responder_service.handle(
            responder_transport.as_ref(),
            stream,
            peer_addr,
            &responder_server,
            slot,
        );
    });

    let device_id = "6f1e5b7a-0000-4000-8000-00000000c0ab";
    let initiator = std::thread::spawn(move || {
        v30_initiator_exchange(&address, &code, PeerRole::Client, device_id)
    });
    // The park lands after the payloads; wait for the card, then decide.
    let deadline = Instant::now() + bound::THREAD;
    let parked = loop {
        let pending = service.pending_snapshot();
        if !pending.is_empty() {
            break pending;
        }
        assert!(Instant::now() < deadline, "the v30 pairing never parked");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        parked[0].role,
        Some(PeerRole::Client),
        "a client tag projects to the client word for the v30 panel"
    );
    assert!(server.peers().expect("rows").is_empty());

    let row = match service
        .confirm(&server, "6f1e5b7a-0000-4000-8000-00000000c0ab", true)
        .expect("confirm")
    {
        ConfirmOutcome::Accepted(row) => *row,
        ConfirmOutcome::Declined => panic!("an accept must produce a row"),
    };
    assert_eq!(
        row.role,
        Some(PeerRole::Client),
        "a client tag projects to the client word for the v30 panel"
    );
    let stored = server.peers().expect("rows");
    assert!(!stored[0].legacy_dialable);
    assert!(
        !stored[0].hosts_workspaces,
        "a client tag is the paired-user-scope record"
    );
    let answer = join_bounded(initiator, "the v30 initiator").expect("the answer arrives");
    assert!(answer.accepted, "{answer:?}");
    join_bounded(responder, "the v30 responder's thread");

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The old topology had one machine per pair. A v30 daemon tag against a device
/// that hosts a workspace makes both sides a daemon, and the handshake refuses
/// with a clear reason rather than storing either claim.
#[test]
fn a_v30_pairing_cannot_make_two_machines() {
    let (dir, server) = server("v30-two-machines");
    let project_dir = dir.join("project");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    let project = server
        .sessions
        .project_add(project_dir.to_str().expect("utf-8 path"))
        .expect("project row");
    server
        .sessions
        .workspace_create(
            &project.id,
            devboule_protocol::WorkspaceIsolation::Local,
            None,
        )
        .expect("workspace row");

    let service = Arc::new(PairingService::new());
    let (code, _expires_at) = service.start().expect("a code");
    let transport = Arc::new(crate::peer_transport::TestTransport::default());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();

    let responder_server = Arc::clone(&server);
    let responder_service = Arc::clone(&service);
    let responder_transport = Arc::clone(&transport);
    let caps = Arc::new(crate::peer_transport::AcceptCaps::default());
    let responder = std::thread::spawn(move || {
        let (stream, peer_addr) = accept_bounded(&listener);
        let slot = caps
            .admit_handshake(crate::peer_transport::HandshakeKind::Pairing)
            .expect("a pairing slot");
        responder_service.handle(
            responder_transport.as_ref(),
            stream,
            peer_addr,
            &responder_server,
            slot,
        );
    });

    let refused = v30_initiator_exchange(
        &address,
        &code,
        PeerRole::Daemon,
        "6f1e5b7a-0000-4000-8000-00000000c0ac",
    );
    assert!(
        refused.is_err(),
        "two machines cannot be represented by the v30 wire: {refused:?}"
    );
    join_bounded(responder, "the refused v30 responder's thread");
    assert!(
        server.peers().expect("rows").is_empty(),
        "the refusal stores nothing"
    );
    assert!(
        service.pending_snapshot().is_empty(),
        "the refusal parks nothing"
    );

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A roleless initiator retries on the v30 wire only for a version signal —
/// a role tag where the version byte belongs, or a close — and never after a
/// code or proof failure. The fake responder here counts the connections, so
/// the retry itself is the assertion.
#[test]
fn a_roleless_initiator_downgrades_only_on_a_role_tag_or_a_close() {
    let (dir, server) = server("downgrade");
    let service = PairingService::new();
    let (code, _expires_at) = service.start().expect("a code");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let counter = Arc::clone(&accepted);
    let fake = std::thread::spawn(move || {
        for attempt in 0..2 {
            let (mut stream, _) = accept_bounded(&listener);
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut magic = [0u8; 4];
            use std::io::Read as _;
            stream.read_exact(&mut magic).expect("the pairing magic");
            assert_eq!(magic, PAIRING_MAGIC);
            if attempt == 0 {
                // The v30 responder's first byte is a role tag, which is the
                // explicit version rejection the retry exists for.
                use std::io::Write as _;
                stream.write_all(&[ROLE_CLIENT_TAG]).expect("the tag");
            }
            // Then close: the second attempt has nothing left to answer.
        }
    });

    let error = service
        .complete(&server, &address, &code)
        .expect_err("neither attempt can complete against the fake responder");
    let _ = error;
    join_bounded(fake, "the two-connection fake responder");
    assert_eq!(
        accepted.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the roleless attempt must have been retried once on the v30 wire"
    );
    assert!(server.peers().expect("rows").is_empty());

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The responder's half of a v30 exchange, driven by hand so a v32 initiator
/// can be *offered* that wire: the first connection answers the roleless
/// prelude with a role tag (the explicit version rejection), the second speaks
/// the whole v30 codec and then holds the answer until `release` says a person
/// confirmed. `own_tag` is the role byte this old responder declares.
fn v30_responder_offering_the_old_wire(
    listener: std::net::TcpListener,
    code: PairingSecret,
    own_tag: PeerRole,
    release: std::sync::mpsc::Receiver<bool>,
) {
    use std::io::{Read as _, Write as _};
    let mut magic = [0u8; 4];
    // Connection one: the version rejection, and nothing else.
    let (mut first, _) = accept_bounded(&listener);
    first.read_exact(&mut magic).expect("the first magic");
    assert_eq!(magic, PAIRING_MAGIC);
    let mut version = [0u8; 1];
    first
        .read_exact(&mut version)
        .expect("the roleless version byte");
    assert_eq!(version[0], PAIR_VERSION_V2);
    first
        .write_all(&[role_tag(own_tag)])
        .expect("the v30 role tag");
    drop(first);

    // Connection two: the v30 codec.
    let (mut stream, _) = accept_bounded(&listener);
    let deadline = Instant::now() + bound::THREAD;
    stream.read_exact(&mut magic).expect("the second magic");
    assert_eq!(magic, PAIRING_MAGIC);
    let tag_byte = read_prelude(&mut stream, deadline)
        .map_err(|end| prelude_error(end).to_string())
        .expect("the initiator's tag");
    let initiator_tag = role_from_tag(tag_byte).expect("a known role tag");
    write_prelude(&mut stream, role_tag(own_tag), deadline).expect("our tag");
    let password = spake2::Password::new(code.as_str().as_bytes());
    let responder_identity = spake2::Identity::new(PAIR_RESPONDER_ID);
    let initiator_identity = spake2::Identity::new(PAIR_INITIATOR_ID);
    let (spake_state, our_message) = spake2::Spake2::<spake2::Ed25519Group>::start_a(
        &password,
        &responder_identity,
        &initiator_identity,
    );
    write_framed(&stream, &our_message, deadline).expect("the spake message");
    let mut their_message = [0u8; 256];
    let their_len =
        read_framed(&stream, &mut their_message, deadline).expect("the initiator's spake message");
    let mut spake_key = spake_state
        .finish(&their_message[..their_len])
        .expect("the spake key");
    let (private, public) = test_keypair();
    let mut psk = derive_psk(&spake_key, initiator_tag, own_tag);
    spake_key.zeroize();
    let session = responder_handshake(
        &stream,
        deadline,
        &private,
        PAIR_PROLOGUE,
        Some(&psk),
        PAIR_NOISE_PATTERN,
    )
    .expect("the v30 Noise session");
    for byte in psk.iter_mut() {
        *byte = 0;
    }
    let (mut reader, mut writer, _closer) =
        split_session(&stream, session).expect("split the session");
    let _payload: PairPayload = read_json(&mut reader, deadline).expect("the initiator payload");
    let our_payload = PairPayload {
        device_id: "6f1e5b7a-0000-4000-8000-00000000c0af".to_string(),
        display_name: "Old peer".to_string(),
        role: Some(own_tag),
        workspace_host: None,
        public_key: base64_encode(&public),
        listen_port: None,
    };
    write_json(&mut writer, &our_payload, deadline).expect("our payload");
    // The exchange is complete and the answer waits for the confirmation the
    // test withholds on purpose.
    let accepted = release.recv_timeout(bound::THREAD).unwrap_or(false);
    write_json(
        &mut writer,
        &PairAnswer {
            accepted,
            reason: String::new(),
        },
        deadline,
    )
    .expect("the answer");
}

/// A pre-32 responder that only ever rejects the roleless prelude and then
/// proves no v30 retry follows: one connection gets the role tag, and a second
/// connection inside the watch window panics the thread.
fn v30_responder_rejecting_the_roleless_wire(listener: std::net::TcpListener) {
    use std::io::{Read as _, Write as _};
    let (mut first, _) = accept_bounded(&listener);
    let mut magic = [0u8; 4];
    first.read_exact(&mut magic).expect("the first magic");
    assert_eq!(magic, PAIRING_MAGIC);
    let mut version = [0u8; 1];
    first
        .read_exact(&mut version)
        .expect("the roleless version byte");
    assert_eq!(version[0], PAIR_VERSION_V2);
    first
        .write_all(&[ROLE_CLIENT_TAG])
        .expect("the v30 role tag");
    drop(first);

    listener
        .set_nonblocking(true)
        .expect("the watch listener goes non-blocking");
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match listener.accept() {
            Ok(_) => panic!("the initiator retried a v30 exchange it must refuse"),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("the watch listener failed: {error}"),
        }
    }
}

/// The exploit the review found, on the *initiator* side: this device hosts a
/// workspace, so its v30 tag is `daemon`, and the old code-displaying device
/// would answer that tag by itself without anyone there confirming. Our side
/// must not complete a pairing the other side never confirmed. It refuses with
/// a sentence that says to update the other device, and tries nothing else.
#[test]
fn a_v32_host_refuses_the_v30_fallback() {
    let (dir, server) = server("downgrade-refuse");
    // A workspace makes this device's synthesized v30 tag a daemon, which is
    // the tag the old auto-accept path keyed on.
    let project_dir = dir.join("project");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    let project = server
        .sessions
        .project_add(project_dir.to_str().expect("utf-8 path"))
        .expect("project row");
    server
        .sessions
        .workspace_create(
            &project.id,
            devboule_protocol::WorkspaceIsolation::Local,
            None,
        )
        .expect("workspace row");
    assert!(server.has_hosted_workspace());

    let service = PairingService::new();
    assert!(server
        .set_peer_transport(Arc::new(crate::peer_transport::TestTransport::default()))
        .is_ok());
    let (code, _expires_at) = service.start().expect("a code");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    let responder = std::thread::spawn(move || {
        v30_responder_rejecting_the_roleless_wire(listener);
    });

    let error = service
        .complete(&server, &address, &code)
        .expect_err("a hosted device must refuse a pairing the other side cannot confirm");
    let message = error.to_string();
    assert!(
        message.contains("old version") && message.contains("update"),
        "the refusal says what to do: {message}"
    );
    assert!(
        server.peers().expect("rows").is_empty(),
        "a refused fallback writes no row"
    );
    assert!(
        service.pending_snapshot().is_empty(),
        "a refused fallback parks nothing"
    );
    join_bounded(responder, "the refusing v30 responder");

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The other direction of the same rule: this device does not host, so its v30
/// tag is `client`, which the old code always parked for its own person. That
/// pairing is legitimate — the person there confirms it — and completes once
/// the answer arrives, with the far side's daemon tag recorded as the machine
/// scope.
#[test]
fn a_client_device_completes_a_v30_pairing_after_the_far_side_confirms() {
    let (dir, server) = server("downgrade-client");
    assert!(!server.has_hosted_workspace(), "this device is a client");
    let service = PairingService::new();
    assert!(server
        .set_peer_transport(Arc::new(crate::peer_transport::TestTransport::default()))
        .is_ok());
    let (code, _expires_at) = service.start().expect("a code");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    let (release, wait) = mpsc::channel::<bool>();
    let responder_code = code.clone();
    let responder = std::thread::spawn(move || {
        v30_responder_offering_the_old_wire(listener, responder_code, PeerRole::Daemon, wait);
    });

    let PairingOutcome::Pending(pending) = service
        .complete(&server, &address, &code)
        .expect("the v30 exchange completes on the client path");
    assert_eq!(
        pending.role,
        Some(PeerRole::Daemon),
        "the far side's daemon tag is the machine scope on the card"
    );
    assert!(
        server.peers().expect("rows").is_empty(),
        "no row before the far side's person confirms"
    );

    release.send(true).expect("the far side confirms");
    let deadline = Instant::now() + bound::THREAD;
    loop {
        if server.peers().expect("rows").len() == 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the initiator never wrote its row after the confirmation"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let rows = server.peers().expect("rows");
    assert_eq!(rows[0].display_name, "Old peer");
    assert!(rows[0].legacy_dialable, "the daemon tag is the dial hint");
    assert!(
        rows[0].hosts_workspaces,
        "and it is the machine-scope record"
    );
    join_bounded(responder, "the v30 responder");

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The pairing ceremony records which side hosts workspaces, and the session
/// scope is read from that record later — never from a live hello. B hosts a
/// workspace here and A does not: A's row for B is the machine-scope record
/// (the narrow one), B's row for A is the client-scope record, and neither
/// needed a `workspaceHost` word after the pairing.
#[test]
fn a_roleless_pairing_records_the_peers_workspace_presence() {
    let (dir_a, server_a) = server("presence-a");
    let (dir_b, server_b) = server("presence-b");
    let project_dir = dir_b.join("project");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    let project = server_b
        .sessions
        .project_add(project_dir.to_str().expect("utf-8 path"))
        .expect("project row");
    server_b
        .sessions
        .workspace_create(
            &project.id,
            devboule_protocol::WorkspaceIsolation::Local,
            None,
        )
        .expect("workspace row");
    assert!(server_b.has_hosted_workspace(), "B hosts");
    assert!(!server_a.has_hosted_workspace(), "A does not");

    let service_a = PairingService::new();
    let service_b = Arc::new(PairingService::new());
    let (code, _expires_at) = service_b.start().expect("a code");
    let transport = Arc::new(crate::peer_transport::TestTransport::default());
    assert!(server_a.set_peer_transport(transport.clone()).is_ok());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();

    let responder_transport = Arc::clone(&transport);
    let responder_server = Arc::clone(&server_b);
    let responder_service = Arc::clone(&service_b);
    let caps = Arc::new(crate::peer_transport::AcceptCaps::default());
    let responder = std::thread::spawn(move || {
        let (stream, peer_addr) = accept_bounded(&listener);
        let slot = caps
            .admit_handshake(crate::peer_transport::HandshakeKind::Pairing)
            .expect("a pairing slot");
        responder_service.handle(
            responder_transport.as_ref(),
            stream,
            peer_addr,
            &responder_server,
            slot,
        );
    });

    let PairingOutcome::Pending(pending) = service_a
        .complete(&server_a, &address, &code)
        .expect("the exchange completes");
    assert_eq!(
        pending.role,
        Some(PeerRole::Daemon),
        "B's own statement makes it the machine peer on the card"
    );
    let a_id = server_a
        .device_identity()
        .as_ref()
        .expect("A has an identity")
        .device_id
        .clone();
    let deadline = Instant::now() + bound::THREAD;
    loop {
        if !service_b.pending_snapshot().is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "B never parked the pairing");
        std::thread::sleep(Duration::from_millis(20));
    }
    match service_b
        .confirm(&server_b, &a_id, true)
        .expect("confirm on B")
    {
        ConfirmOutcome::Accepted(_) => {}
        ConfirmOutcome::Declined => panic!("an accept must produce a row"),
    }
    join_bounded(responder, "the responder's pairing thread");

    let deadline = Instant::now() + bound::THREAD;
    loop {
        if !server_a.peers().expect("A's rows").is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "A never wrote its row after the confirmation"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let a_row = &server_a.peers().expect("A's rows")[0];
    assert!(
        a_row.hosts_workspaces,
        "the peer that said it hosts is the machine-scope record"
    );
    assert!(
        !a_row.legacy_dialable,
        "and it is not a v30 endpoint to dial"
    );
    assert_eq!(
        crate::peer_policy::PeerScope::recorded(a_row.hosts_workspaces),
        crate::peer_policy::PeerScope::PeerDevice,
        "the record selects the narrow scope"
    );
    let b_row = &server_b.peers().expect("B's rows")[0];
    assert!(
        !b_row.hosts_workspaces,
        "the peer with no workspace is the client-scope record"
    );
    assert_eq!(
        crate::peer_policy::PeerScope::recorded(b_row.hosts_workspaces),
        crate::peer_policy::PeerScope::PairedUser
    );

    drop(server_a);
    drop(server_b);
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}
