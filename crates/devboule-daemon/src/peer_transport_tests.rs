//! Tests for the peer transport: the Noise handshake, framing and revocation.

use super::*;
use crate::secret_store::InMemoryStore;
use std::net::TcpListener as StdTcpListener;

/// Every wall-clock bound a test in this module relies on. Named so a
/// reader can see, per test, what stops it hanging.
mod bound {
    use std::time::Duration;

    /// A test that waits for a spawned thread must not wait forever: a bug
    /// on the far side would otherwise hang the whole suite.
    pub const THREAD: Duration = Duration::from_secs(20);
    /// A client that connects to a listener the test itself bound.
    pub const CONNECT: Duration = Duration::from_secs(5);
    /// The longest a socket read in a test may block.
    pub const READ: Duration = Duration::from_secs(20);
}

/// Join a thread with a wall-clock bound, panicking if it is still running
/// at `bound::THREAD`. `JoinHandle::join` has no timeout, so the wait is a
/// poll; `join` itself then returns immediately.
fn join_bounded<T>(handle: std::thread::JoinHandle<T>, what: &str) -> T {
    let deadline = Instant::now() + bound::THREAD;
    while !handle.is_finished() {
        assert!(
            Instant::now() < deadline,
            "{what} did not finish within {:?}",
            bound::THREAD
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    match handle.join() {
        Ok(value) => value,
        Err(_) => panic!("{what} panicked"),
    }
}

/// Accept one loopback connection with a wall-clock bound. A blocking
/// `accept()` with no bound is how a test hangs the suite when the client
/// side fails before connecting.
fn accept_bounded(listener: &StdTcpListener) -> (TcpStream, SocketAddr) {
    listener
        .set_nonblocking(true)
        .expect("the test listener goes non-blocking");
    let deadline = Instant::now() + bound::CONNECT;
    loop {
        match listener.accept() {
            Ok((stream, addr)) => {
                // Windows: an accepted socket inherits the listening
                // socket's non-blocking mode, so every later read would
                // fail with `WSAEWOULDBLOCK` rather than block. The
                // accepted connection must be blocking, which is what the
                // deadline-based reads in these tests assume.
                stream
                    .set_nonblocking(false)
                    .expect("the accepted socket is blocking");
                return (stream, addr);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "the client never connected within {:?}",
                    bound::CONNECT
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept failed: {error}"),
        }
    }
}

/// Connect to a listener the test bound, with a wall-clock bound.
fn connect_bounded(address: SocketAddr) -> TcpStream {
    TcpStream::connect_timeout(&address, bound::CONNECT).expect("connect within the bound")
}

fn keypair(pattern: &str) -> (Vec<u8>, Vec<u8>) {
    let params = pattern
        .parse::<snow::params::NoiseParams>()
        .expect("params");
    let pair = snow::Builder::new(params)
        .generate_keypair()
        .expect("keypair");
    (pair.private, pair.public)
}

/// The pairing pattern must parse and `.psk(3, …)` must be the position the
/// parser validates. The proof is a completed handshake: if the two sides
/// disagreed on the PSK or its position, the transport keys would differ
/// and the first message would fail its tag.
#[test]
fn pairing_pattern_parses_and_psk_three_is_where_the_transcript_agrees() {
    let psk = [7u8; 32];
    let (responder_private, _) = keypair(PAIR_NOISE_PATTERN);
    let (initiator_private, _) = keypair(PAIR_NOISE_PATTERN);

    // In memory, no sockets and no threads: this test is about which PSK
    // *position* the pattern consumes, and neither a socket nor a thread
    // can add anything to that answer while both can hang it.
    let build = |side_is_initiator: bool, private: &[u8], psk: Option<(u8, &[u8; 32])>| {
        let params = PAIR_NOISE_PATTERN
            .parse::<snow::params::NoiseParams>()
            .expect("params");
        let builder = snow::Builder::new(params)
            .local_private_key(private)
            .expect("local key")
            .prologue(PAIR_PROLOGUE)
            .expect("prologue");
        let builder = match psk {
            Some((location, key)) => builder.psk(location, key).expect("psk accepted"),
            None => builder,
        };
        if side_is_initiator {
            builder.build_initiator().expect("initiator")
        } else {
            builder.build_responder().expect("responder")
        }
    };

    // The same position on both sides completes and produces the payload.
    let initiator = build(true, &initiator_private, Some((PAIR_PSK_LOCATION, &psk)));
    let responder = build(false, &responder_private, Some((PAIR_PSK_LOCATION, &psk)));
    assert_eq!(
        drive_handshake(initiator, responder).expect("agreeing sides"),
        b"payload"
    );

    // The responder's key at position 2 instead of 3 leaves the pattern's
    // own `Psk(3)` token unsatisfied. That is the functional proof that 3
    // — the position `derive_psk` uses — is the one this pattern expects.
    let initiator = build(true, &initiator_private, Some((PAIR_PSK_LOCATION, &psk)));
    let responder = build(false, &responder_private, Some((2, &psk)));
    let wrong_position = drive_handshake(initiator, responder)
        .expect_err("a PSK outside the pattern's position cannot satisfy it");
    assert!(
        format!("{wrong_position}").to_lowercase().contains("psk"),
        "the failure must name the PSK: {wrong_position}"
    );

    // And two different keys at the right position do not agree.
    let other_psk = [8u8; 32];
    let initiator = build(true, &initiator_private, Some((PAIR_PSK_LOCATION, &psk)));
    let responder = build(
        false,
        &responder_private,
        Some((PAIR_PSK_LOCATION, &other_psk)),
    );
    assert!(
        drive_handshake(initiator, responder).is_err(),
        "different PSK material must not complete the handshake"
    );
}

/// Drive one in-memory `XXpsk3` exchange to completion. Returns the
/// initiator's first transport payload on success.
///
/// `XX` is exactly three handshake messages (`-> e`; `<- e, ee, s, es`;
/// `-> s, se`), driven by index rather than by `is_my_turn`: that flag
/// describes the pattern's next token, not which side speaks next, and
/// using it here produced a `NotTurnToWrite` on the transport write.
fn drive_handshake(
    mut initiator: snow::HandshakeState,
    mut responder: snow::HandshakeState,
) -> Result<Vec<u8>, snow::Error> {
    let mut message = [0u8; MAX_NOISE_MESSAGE];
    let mut plaintext = [0u8; MAX_NOISE_MESSAGE];

    let written = initiator.write_message(&[], &mut message)?;
    responder.read_message(&message[..written], &mut plaintext)?;
    let written = responder.write_message(&[], &mut message)?;
    initiator.read_message(&message[..written], &mut plaintext)?;
    let written = initiator.write_message(&[], &mut message)?;
    responder.read_message(&message[..written], &mut plaintext)?;

    assert!(
        initiator.is_handshake_finished() && responder.is_handshake_finished(),
        "XXpsk3 is three messages"
    );
    let mut initiator = initiator.into_transport_mode()?;
    let mut responder = responder.into_transport_mode()?;
    let written = initiator.write_message(b"payload", &mut message)?;
    let read = responder.read_message(&message[..written], &mut plaintext)?;
    Ok(plaintext[..read].to_vec())
}

/// A pinned key is enforced by `initiator_handshake`, not merely handed to
/// snow: `remote_public_key` seeds the expected key and plain `XX`
/// overwrites that seed with whatever the responder presents, so without
/// the check a dial pinned to one key completes happily against another.
/// The control dial with the true key proves the refusal is the key.
#[test]
fn an_initiator_refuses_a_key_that_is_not_the_one_it_pinned() {
    let (responder_private, responder_public) = keypair(PEER_NOISE_PATTERN);
    let (initiator_private, _) = keypair(PEER_NOISE_PATTERN);
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr");

    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (stream, _) = accept_bounded(&listener);
            let _ = responder_handshake(
                &stream,
                Instant::now() + bound::READ,
                &responder_private,
                PEER_PROLOGUE,
                None,
                PEER_NOISE_PATTERN,
            );
        }
    });

    // One bit flipped, so the refusal is the comparison and not a length
    // or encoding error on a garbage key.
    let mut impostor = responder_public.clone();
    impostor[31] ^= 0x01;
    let stream = connect_bounded(address);
    let error = initiator_handshake(
        &stream,
        Instant::now() + bound::READ,
        &initiator_private,
        Some(&impostor),
        PEER_PROLOGUE,
        None,
        PEER_NOISE_PATTERN,
    )
    .expect_err("a far end presenting another key must fail the handshake");
    assert!(
        error.to_string().contains("pinned key"),
        "the refusal names the pin: {error}"
    );

    let stream = connect_bounded(address);
    initiator_handshake(
        &stream,
        Instant::now() + bound::READ,
        &initiator_private,
        Some(&responder_public),
        PEER_PROLOGUE,
        None,
        PEER_NOISE_PATTERN,
    )
    .expect("the same dial with the pinned key succeeds");
    server.join().expect("responder");
}

#[test]
fn a_three_hundred_kib_frame_round_trips_through_a_noise_session() {
    let (responder_private, _) = keypair(PEER_NOISE_PATTERN);
    let (initiator_private, _) = keypair(PEER_NOISE_PATTERN);
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr");

    // One long NDJSON line, well past the single-message ceiling, so the
    // frame really is split and reassembled.
    let mut frame = vec![b'x'; 300 * 1024];
    frame[0] = b'{';
    let expected = frame.clone();

    let server = std::thread::spawn(move || {
        let (stream, _) = accept_bounded(&listener);
        let deadline = Instant::now() + bound::READ;
        let session = responder_handshake(
            &stream,
            deadline,
            &responder_private,
            PEER_PROLOGUE,
            None,
            PEER_NOISE_PATTERN,
        )
        .expect("responder");
        let (mut reader, _writer, _closer) = split_session(&stream, session).expect("split");
        let mut collected = Vec::new();
        while !collected.ends_with(b"\n") {
            let mut chunk = [0u8; 8192];
            let read = reader
                .read_plaintext(&mut chunk, Some(deadline))
                .expect("read");
            assert_ne!(read, 0, "stream closed early");
            collected.extend_from_slice(&chunk[..read]);
        }
        collected.pop();
        collected
    });

    let stream = connect_bounded(address);
    let deadline = Instant::now() + bound::READ;
    let session = initiator_handshake(
        &stream,
        deadline,
        &initiator_private,
        None,
        PEER_PROLOGUE,
        None,
        PEER_NOISE_PATTERN,
    )
    .expect("initiator");
    let (_reader, mut writer, _closer) = split_session(&stream, session).expect("split");
    writer.write_frame(&frame, Some(deadline)).expect("write");
    frame.clear();

    let received = join_bounded(server, "the 300 KiB peer reader");
    assert_eq!(received.len(), expected.len());
    assert_eq!(received, expected);
}

#[test]
fn a_reserved_flags_bit_closes_the_stream() {
    // Driven at the reader directly: the writer never sets a reserved bit,
    // so this is the hostile case a buggy or hostile peer would produce.
    let (responder_private, _) = keypair(PEER_NOISE_PATTERN);
    let (initiator_private, _) = keypair(PEER_NOISE_PATTERN);
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr");
    let server = std::thread::spawn(move || {
        let (stream, _) = accept_bounded(&listener);
        let deadline = Instant::now() + bound::READ;
        let session = responder_handshake(
            &stream,
            deadline,
            &responder_private,
            PEER_PROLOGUE,
            None,
            PEER_NOISE_PATTERN,
        )
        .expect("responder");
        let (mut reader, _, _) = split_session(&stream, session).expect("split");
        let mut chunk = [0u8; 64];
        reader.read_plaintext(&mut chunk, Some(deadline)).is_err()
    });
    let stream = connect_bounded(address);
    let deadline = Instant::now() + bound::READ;
    let mut state = initiator_handshake(
        &stream,
        deadline,
        &initiator_private,
        None,
        PEER_PROLOGUE,
        None,
        PEER_NOISE_PATTERN,
    )
    .expect("initiator");
    let mut message = vec![0u8; 64];
    let written = state
        .write_message(&[0x02, b'h', b'i'], &mut message)
        .expect("write");
    write_framed(&stream, &message[..written], deadline).expect("send");
    assert!(
        join_bounded(server, "the reserved-flags peer reader"),
        "a reserved flags bit must close the stream"
    );
}

#[test]
fn concurrent_read_and_write_on_one_session_do_not_deadlock() {
    let (responder_private, _) = keypair(PEER_NOISE_PATTERN);
    let (initiator_private, _) = keypair(PEER_NOISE_PATTERN);
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr");
    let (client_read_all_tx, client_read_all_rx) = std::sync::mpsc::channel::<()>();

    let server = std::thread::spawn(move || {
        let (stream, _) = accept_bounded(&listener);
        let deadline = Instant::now() + bound::READ;
        let session = responder_handshake(
            &stream,
            deadline,
            &responder_private,
            PEER_PROLOGUE,
            None,
            PEER_NOISE_PATTERN,
        )
        .expect("responder");
        let (mut reader, mut writer, _closer) = split_session(&stream, session).expect("split");
        let writer_thread = std::thread::spawn(move || {
            for _ in 0..64 {
                writer
                    .write_frame(b"pong", Some(deadline))
                    .expect("write pong");
            }
        });
        let mut seen = 0usize;
        let mut chunk = [0u8; 4096];
        while seen < 64 {
            let read = reader
                .read_plaintext(&mut chunk, Some(deadline))
                .expect("read");
            seen += chunk[..read].iter().filter(|byte| **byte == b'\n').count();
        }
        join_bounded(writer_thread, "the server's concurrent writer");
        // The 320 pong bytes are in this socket's send buffer, not in the
        // peer's. A close here can be aborted (RST) on Windows and take the
        // peer's whole response buffer with it, so this socket outlives the
        // peer's read.
        client_read_all_rx
            .recv_timeout(bound::THREAD)
            .expect("the client must have read every pong before this socket closes");
        seen
    });

    let stream = connect_bounded(address);
    let deadline = Instant::now() + bound::READ;
    let session = initiator_handshake(
        &stream,
        deadline,
        &initiator_private,
        None,
        PEER_PROLOGUE,
        None,
        PEER_NOISE_PATTERN,
    )
    .expect("initiator");
    let (mut reader, mut writer, _closer) = split_session(&stream, session).expect("split");
    let writer_thread = std::thread::spawn(move || {
        for _ in 0..64 {
            writer
                .write_frame(b"ping", Some(deadline))
                .expect("write ping");
        }
    });
    let mut chunk = [0u8; 4096];
    let mut read_bytes = 0usize;
    while read_bytes < 64 * 5 {
        // A peer close arrives as `Err`, never as `Ok(0)`: `read_plaintext`
        // reserves zero for an empty buffer, so the loop body has no
        // early-close value to test.
        let read = match reader.read_plaintext(&mut chunk, Some(deadline)) {
            Ok(read) => read,
            Err(error) => {
                // Release the server so its own outcome is reported here
                // instead of abandoned with its `JoinHandle`.
                let _ = client_read_all_tx.send(());
                let seen = join_bounded(server, "the concurrent peer reader");
                panic!(
                    "client read failed after {read_bytes} bytes: {error}; the server saw {seen} frames"
                );
            }
        };
        read_bytes += read;
    }
    client_read_all_tx
        .send(())
        .expect("the server thread is still waiting");
    join_bounded(writer_thread, "the concurrent peer writer");
    assert_eq!(join_bounded(server, "the concurrent peer reader"), 64);
}

#[test]
fn the_token_bucket_bursts_then_refills_at_the_sustained_rate() {
    let start = Instant::now();
    let mut bucket = TokenBucket::new(start);
    // 50 in one instant: the whole burst, and not one more.
    for index in 0..RATE_BURST as usize {
        assert!(bucket.take(start), "burst token {index}");
    }
    assert!(
        !bucket.take(start),
        "the 51st request in a burst is refused"
    );
    // 20/s sustained: 100 ms buys two more.
    assert!(bucket.take(start + Duration::from_millis(100)));
    assert!(bucket.take(start + Duration::from_millis(100)));
    assert!(!bucket.take(start + Duration::from_millis(100)));
    // Refilling the whole burst from empty takes BURST/SUSTAINED = 2.5 s
    // from the last take, which is what the sleep below measures.
    let drained_at = start + Duration::from_millis(100);
    let later = drained_at + Duration::from_millis(2_600);
    for index in 0..RATE_BURST as usize {
        assert!(bucket.take(later), "refilled token {index}");
    }
    assert!(!bucket.take(later));
}

#[test]
fn the_accept_caps_bound_total_per_source_and_churn() {
    let caps = Arc::new(AcceptCaps::default());
    let now = Instant::now();
    let ip: IpAddr = "100.64.0.9".parse().expect("ip");
    let mut guards = Vec::new();
    for _ in 0..MAX_REMOTE_CONNECTIONS_PER_SOURCE {
        guards.push(caps.admit_source(ip, now).expect("per-source cap has room"));
    }
    assert!(
        caps.admit_source(ip, now).is_err(),
        "the fifth simultaneous connection from one source is refused"
    );
    let other: IpAddr = "100.64.0.10".parse().expect("ip");
    assert!(
        caps.admit_source(other, now).is_ok(),
        "another source is unaffected"
    );
    drop(guards);

    // Churn: ten accepts per minute per source, granted or not.
    let mut accepted = 0;
    for _ in 0..MAX_ACCEPTS_PER_SOURCE_PER_MINUTE * 2 {
        if let Ok(guard) = caps.admit_source(other, now) {
            accepted += 1;
            drop(guard);
        }
    }
    assert_eq!(accepted, MAX_ACCEPTS_PER_SOURCE_PER_MINUTE - 1);
}

#[test]
fn the_two_handshake_budgets_are_separate() {
    let caps = Arc::new(AcceptCaps::default());
    let mut noise = Vec::new();
    for _ in 0..MAX_NOISE_HANDSHAKES_IN_FLIGHT {
        noise.push(
            caps.admit_handshake(HandshakeKind::Noise)
                .expect("noise slot"),
        );
    }
    assert!(
        caps.admit_handshake(HandshakeKind::Noise).is_err(),
        "the noise budget is enforced"
    );
    // The pairing budget is untouched by a noise flood: an unpaired source
    // can never spend a paired peer's slot, and vice versa.
    assert!(caps.admit_handshake(HandshakeKind::Pairing).is_ok());
    drop(noise);
    assert!(caps.admit_handshake(HandshakeKind::Noise).is_ok());
}

#[test]
fn the_peer_table_matches_addresses_and_pinned_keys_exactly() {
    let row = PeerRecord {
        device_id: "dev-1".to_string(),
        display_name: "Host".to_string(),
        role: "daemon".to_string(),
        public_key: vec![3u8; 32],
        paired_by_user: Some("S-1-5-21-1".to_string()),
        binding_kind: "tailnet".to_string(),
        binding_stable_id: Some("nstable".to_string()),
        binding_node_name: None,
        binding_login_name: None,
        address: "100.64.0.1:47831".to_string(),
        paired_at: 1,
        revoked_at: None,
        caps: vec!["view".to_string()],
    };
    let revoked = PeerRecord {
        revoked_at: Some(2),
        device_id: "dev-2".to_string(),
        public_key: vec![4u8; 32],
        ..row.clone()
    };
    let table = PeerTable::from_rows(vec![row.clone(), revoked]);
    // Every row is kept: the address of a revoked peer must be reported as
    // `Revoked`, which is a different fact from `UnknownSource`, and that
    // needs the row to still be here.
    assert_eq!(table.rows().len(), 2);
    assert!(table
        .by_address(&"100.64.0.1".parse().expect("ip"))
        .is_some());
    assert!(
        table
            .by_address(&"100.64.0.10".parse().expect("ip"))
            .is_none(),
        "addresses compare numerically, not by prefix"
    );
    // A revoked row is not reachable by its address (which is shared with
    // `dev-2`) and its key cannot authenticate.
    assert!(table.revoked_address(&"100.64.0.1".parse().expect("ip")));
    assert!(!table.revoked_address(&"100.64.0.9".parse().expect("ip")));
    assert!(table.by_static_key(&[3u8; 32]).is_some());
    assert!(
        table.by_static_key(&[4u8; 32]).is_none(),
        "a revoked peer's pinned key must not authenticate"
    );
    assert!(table.by_static_key(&[3u8; 31]).is_none());
}

/// The addresses the tailnet filter accepts, as text. Shared with the
/// composition property below so the two tables cannot drift.
const TAILNET_INSIDE: &[&str] = &[
    "100.64.0.1",
    "100.127.255.254",
    "fd7a:115c:a1e0::1",
    "fd7a:115c:a1e0:ffff::9",
];

#[test]
fn only_tailnet_ranges_are_treated_as_tailnet_sources() {
    for inside in TAILNET_INSIDE {
        assert!(
            is_tailnet_address(&inside.parse().expect("ip")),
            "{inside} is inside a tailnet range"
        );
    }
    for outside in [
        "100.63.255.255",
        "100.128.0.1",
        "127.0.0.1",
        "10.0.0.1",
        "::1",
    ] {
        assert!(
            !is_tailnet_address(&outside.parse().expect("ip")),
            "{outside} is not a tailnet address"
        );
    }
}

/// Every address a row may record is composed by
/// [`compose_peer_address`], so the composed text must parse back into
/// the very address it was composed from — brackets and all. A test that
/// only used `127.0.0.1` let the unbracketed IPv6 form through.
#[test]
fn every_tailnet_address_composes_into_a_parseable_peer_address() {
    const PORT: u16 = 47831;
    for text in TAILNET_INSIDE {
        let ip: IpAddr = text.parse().expect("ip");
        let composed = compose_peer_address(ip, PORT);
        assert_eq!(
            composed.parse::<SocketAddr>(),
            Ok(SocketAddr::new(ip, PORT)),
            "{composed} must parse back into the address it was composed from"
        );
    }
}

#[test]
fn the_pre_noise_filter_separates_revoked_from_unknown_from_off_tailnet() {
    let row = PeerRecord {
        device_id: "dev-1".to_string(),
        display_name: "Host".to_string(),
        role: "daemon".to_string(),
        public_key: vec![3u8; 32],
        paired_by_user: None,
        binding_kind: "tailnet".to_string(),
        binding_stable_id: Some("nstable".to_string()),
        binding_node_name: None,
        binding_login_name: None,
        address: "100.64.0.1:47831".to_string(),
        paired_at: 1,
        revoked_at: None,
        caps: vec!["view".to_string()],
    };
    let revoked = PeerRecord {
        revoked_at: Some(2),
        device_id: "dev-2".to_string(),
        address: "100.64.0.2:47831".to_string(),
        ..row.clone()
    };
    let table = PeerTable::from_rows(vec![row, revoked]);
    let check = |address: &str| {
        Tailnet.pre_noise_filter(&format!("{address}:5000").parse().expect("addr"), &table)
    };
    assert!(check("100.64.0.1").is_ok(), "a live peer address passes");
    assert_eq!(check("100.64.0.2"), Err(RejectReason::Revoked));
    assert_eq!(check("100.64.0.3"), Err(RejectReason::UnknownSource));
    assert_eq!(check("127.0.0.1"), Err(RejectReason::NotATailnetAddress));
}

#[test]
fn the_pattern_constants_are_the_ones_the_design_names() {
    let (peer, pair) = reference_patterns();
    assert_eq!(peer, "Noise_XX_25519_ChaChaPoly_BLAKE2s");
    assert_eq!(pair, "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s");
    assert_ne!(PEER_PROLOGUE, PAIR_PROLOGUE);
    assert_eq!(PAIR_PSK_LOCATION, 3);
}

#[test]
fn the_peer_port_is_overridable_and_never_zero() {
    // No process environment is touched: the rule is a pure function, so
    // this test cannot race another test that reads the variable.
    assert_eq!(peer_port_from(Some("47832")), 47832);
    assert_eq!(peer_port_from(Some(" 47832 ")), 47832);
    assert_eq!(peer_port_from(Some("0")), DEFAULT_PEER_PORT);
    assert_eq!(peer_port_from(Some("not a port")), DEFAULT_PEER_PORT);
    assert_eq!(peer_port_from(Some("65536")), DEFAULT_PEER_PORT);
    assert_eq!(peer_port_from(Some("")), DEFAULT_PEER_PORT);
    assert_eq!(peer_port_from(None), DEFAULT_PEER_PORT);
}

#[test]
fn an_identity_can_be_created_for_the_transport_tests() {
    // Sanity check on the fixture S5's real test needs; the full identity
    // path is covered in `device_identity`.
    let dir = std::env::temp_dir().join(format!("devboule-peers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let paths = crate::paths::RuntimePaths::from_dir(&dir);
    let store = InMemoryStore::default();
    let identity = crate::device_identity::load_or_create(&paths, &store).expect("identity");
    assert_eq!(identity.private_key().len(), 32);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The accept loop's own behaviour for a source that is neither a peer nor
/// a pairing candidate: the socket is closed **before any read**, so a
/// client that never writes sees EOF at once rather than a 2 s wait.
///
/// `Tailnet`'s `pre_noise_filter` against a live-but-empty peer table is the
/// real decision; this drives it through the accept loop.
///
/// Bounds, all named: the read on the client socket is `bound::READ`, the
/// accept thread must stop within `bound::THREAD`, and the stop is signalled
/// on both the listener and the daemon state so neither mechanism can be
/// the one that hangs.
#[test]
fn an_unknown_source_is_closed_before_any_read() {
    // A transport that records every `pre_noise_filter` call and decides
    // from a table this test controls, so the accept loop's *ordering*
    // (filter first, then decide) is what is being observed rather than the
    // tailnet address range, which would refuse loopback for an unrelated
    // reason and make the test pass for the wrong one.
    struct Recording {
        calls: Mutex<Vec<SocketAddr>>,
        allow: bool,
    }
    impl PeerTransport for Recording {
        fn listen(
            &self,
            _paths: &crate::paths::RuntimePaths,
            stop: Arc<AtomicBool>,
        ) -> io::Result<PeerListener> {
            Tailnet::bind_peer_listener(&["127.0.0.1".parse().expect("ip")], 0, stop)
        }
        fn pre_noise_filter(
            &self,
            peer: &SocketAddr,
            _peers: &PeerTable,
        ) -> Result<(), RejectReason> {
            self.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(*peer);
            if self.allow {
                Ok(())
            } else {
                Err(RejectReason::UnknownSource)
            }
        }
        fn binding(&self, _peer: &SocketAddr) -> Result<TransportBinding, BindingError> {
            Ok(TransportBinding::tailnet("n", "n", "n"))
        }
    }

    // Case 1: the filter refuses, so the loop must close the socket
    // **before** reading anything from it.
    let refused = Arc::new(Recording {
        calls: Mutex::new(Vec::new()),
        allow: false,
    });
    let state = crate::server::ServerState::new("peer-accept".to_string());
    let stop = Arc::new(AtomicBool::new(false));
    let listener =
        Tailnet::bind_peer_listener(&["127.0.0.1".parse().expect("ip")], 0, Arc::clone(&stop))
            .expect("bind");
    let address = listener.addrs()[0];
    let transport: Arc<dyn PeerTransport> = refused.clone();
    let pairing: Arc<dyn PairingHook> = Arc::new(PairingDisabled);
    let accept_state = Arc::clone(&state);
    let accept = std::thread::spawn(move || {
        accept_peers(listener, transport, accept_state, pairing);
    });

    let stream = connect_bounded(address);
    stream
        .set_read_timeout(Some(bound::READ))
        .expect("read timeout");
    let started = Instant::now();
    let mut byte = [0u8; 1];
    // Never write: an unpaired source that says nothing must still be
    // closed, and the read below must return 0 rather than time out.
    let read = std::io::Read::read(&mut &stream, &mut byte);
    assert_eq!(
        read.expect("a closed socket reads as EOF, not as a timeout"),
        0,
        "the daemon must close an unknown source, not answer it"
    );
    assert!(
        started.elapsed() < PAIRING_PEEK_TIMEOUT,
        "closed immediately, without even the pairing peek budget: {:?}",
        started.elapsed()
    );
    // The filter ran, which is what makes the close its decision.
    assert_eq!(
        refused
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len(),
        1,
        "pre_noise_filter must be consulted exactly once"
    );

    // Case 2: the same source, same table-shape, with the filter allowing
    // it. Nothing is closed: the loop commits to the Noise path, and the
    // client's read blocks (no EOF) until its own deadline.
    stop.store(true, Ordering::SeqCst);
    state.stop_flag().store(true, Ordering::SeqCst);
    join_bounded(accept, "the peer accept loop");
    drop(stream);

    let allowed = Arc::new(Recording {
        calls: Mutex::new(Vec::new()),
        allow: true,
    });
    let state = crate::server::ServerState::new("peer-allow".to_string());
    let stop = Arc::new(AtomicBool::new(false));
    let listener =
        Tailnet::bind_peer_listener(&["127.0.0.1".parse().expect("ip")], 0, Arc::clone(&stop))
            .expect("bind");
    let address = listener.addrs()[0];
    let transport: Arc<dyn PeerTransport> = allowed.clone();
    let pairing: Arc<dyn PairingHook> = Arc::new(PairingDisabled);
    let accept_state = Arc::clone(&state);
    let accept = std::thread::spawn(move || {
        accept_peers(listener, transport, accept_state, pairing);
    });

    let stream = connect_bounded(address);
    // Wait until the loop has actually run the filter for this connection:
    // asserting the counter first removes the tick race, so the read below
    // tests the loop's decision and not its scheduling.
    let deadline = Instant::now() + Duration::from_secs(3);
    while allowed
        .calls
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_empty()
    {
        assert!(
            Instant::now() < deadline,
            "pre_noise_filter was never consulted for an accepted source"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // The loop's Noise path will not send anything until the client speaks,
    // so a short read deadline is what distinguishes "kept" from "closed".
    stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .expect("read timeout");
    let mut byte = [0u8; 1];
    let read = std::io::Read::read(&mut &stream, &mut byte);
    assert!(
        matches!(read, Err(ref error) if error.kind() == io::ErrorKind::WouldBlock
                || error.kind() == io::ErrorKind::TimedOut),
        "an accepted source must not be closed: expected a read timeout, got {read:?}"
    );
    assert_eq!(
        allowed
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len(),
        1,
        "pre_noise_filter must be consulted exactly once"
    );

    stop.store(true, Ordering::SeqCst);
    state.stop_flag().store(true, Ordering::SeqCst);
    join_bounded(accept, "the allowing peer accept loop");
}

/// A silent source must not be able to park a reader. This drives the
/// production path (`Framed` over a Noise session, which is what
/// `handle_client` reads through) against a peer that completes the
/// handshake and then says nothing, and asserts the read comes back inside
/// the caller's deadline instead of blocking.
#[test]
fn a_silent_peer_cannot_park_a_framed_read() {
    const DEADLINE: Duration = Duration::from_millis(400);
    let (responder_private, _) = keypair(PEER_NOISE_PATTERN);
    let (initiator_private, _) = keypair(PEER_NOISE_PATTERN);
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr");

    // The peer completes the handshake and then holds the socket open
    // without writing a byte.
    let quiet = std::thread::spawn(move || {
        let (stream, _) = accept_bounded(&listener);
        let handshake_deadline = Instant::now() + bound::READ;
        let session = responder_handshake(
            &stream,
            handshake_deadline,
            &responder_private,
            PEER_PROLOGUE,
            None,
            PEER_NOISE_PATTERN,
        )
        .expect("responder");
        let (_reader, _writer, _closer) = split_session(&stream, session).expect("split");
        // Hold the connection open past the reader's deadline.
        std::thread::sleep(DEADLINE + Duration::from_millis(300));
    });

    let stream = connect_bounded(address);
    let session = initiator_handshake(
        &stream,
        Instant::now() + bound::READ,
        &initiator_private,
        None,
        PEER_PROLOGUE,
        None,
        PEER_NOISE_PATTERN,
    )
    .expect("initiator");
    let (reader, writer, closer) = split_session(&stream, session).expect("split");
    let framed = crate::framing::Framed::from_stream(reader, writer, closer);

    let started = Instant::now();
    let outcome = framed.recv_timeout::<devboule_protocol::ClientMessage>(DEADLINE);
    let elapsed = started.elapsed();
    assert!(
        outcome.is_err(),
        "a reader must not be fed by a peer that sends nothing"
    );
    assert!(
        elapsed < bound::READ,
        "the read must return on its own deadline ({DEADLINE:?}), took {elapsed:?}"
    );
    join_bounded(quiet, "the silent peer");
}

/// The daemon's stop flag is the production path; the listener's is what a
/// caller that keeps only the listener can use. Both are asserted here so
/// neither can silently rot into "the loop never stops".
#[test]
fn the_accept_loop_stops_on_the_listener_flag_alone() {
    let state = crate::server::ServerState::new("peer-stop".to_string());
    let stop = Arc::new(AtomicBool::new(false));
    let listener =
        Tailnet::bind_peer_listener(&["127.0.0.1".parse().expect("ip")], 0, Arc::clone(&stop))
            .expect("bind");
    let shutdown = listener.stop_handle();
    let accept = std::thread::spawn(move || {
        accept_peers(
            listener,
            Arc::new(TestTransport::default()) as Arc<dyn PeerTransport>,
            state,
            Arc::new(PairingDisabled) as Arc<dyn PairingHook>,
        );
    });
    // It must still be running before the flag is raised: a loop that had
    // already exited would "stop" for the wrong reason.
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        !accept.is_finished(),
        "the accept loop must be running before the flag is raised"
    );
    let raised_at = Instant::now();
    shutdown.store(true, Ordering::SeqCst);

    // A tight bound of its own: `bound::THREAD` (20 s) would hide a loop
    // that only exits after several ticks. One tick is the design's
    // `HOUSEKEEPING_TICK` (1 s), so 3 s is a generous but real bound.
    const STOP_BUDGET: Duration = Duration::from_secs(3);
    let deadline = Instant::now() + STOP_BUDGET;
    while !accept.is_finished() {
        assert!(
            Instant::now() < deadline,
            "the accept loop did not stop within {STOP_BUDGET:?} of the flag being raised\
                 (last tick took {:?})",
            raised_at.elapsed()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = accept.join();
}

/// A transport that records every `pre_noise_filter` call and refuses, so
/// the pairing branch is reachable while the accept path still reads the
/// peer table for each connection.
#[derive(Default)]
struct RefusingTransport {
    calls: Mutex<Vec<SocketAddr>>,
}

impl RefusingTransport {
    fn calls(&self) -> Vec<SocketAddr> {
        self.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

impl PeerTransport for RefusingTransport {
    fn listen(
        &self,
        _paths: &crate::paths::RuntimePaths,
        stop: Arc<AtomicBool>,
    ) -> io::Result<PeerListener> {
        Tailnet::bind_peer_listener(&["127.0.0.1".parse().expect("ip")], 0, stop)
    }
    fn pre_noise_filter(&self, peer: &SocketAddr, _peers: &PeerTable) -> Result<(), RejectReason> {
        self.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(*peer);
        Err(RejectReason::UnknownSource)
    }
    fn binding(&self, _peer: &SocketAddr) -> Result<TransportBinding, BindingError> {
        Ok(TransportBinding::tailnet("n", "n", "n"))
    }
}

/// Active, so the pairing branch is reachable, and records each candidate
/// the accept path handed to it.
struct RecordingPairing {
    handled: Mutex<Vec<SocketAddr>>,
}

impl PairingHook for RecordingPairing {
    fn is_active(&self) -> bool {
        true
    }
    fn housekeeping(&self, _now: Instant) {}
    fn handle(
        &self,
        _transport: &dyn PeerTransport,
        stream: TcpStream,
        peer_addr: SocketAddr,
        _state: &Arc<ServerState>,
        _in_flight: HandshakeGuard,
    ) {
        self.handled
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(peer_addr);
        std::thread::sleep(Duration::from_millis(20));
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Start an accept loop on loopback with the given transport and hook, and
/// return the address plus what is needed to stop it.
fn spawn_accept_loop(
    transport: Arc<dyn PeerTransport>,
    pairing: Arc<dyn PairingHook>,
    tag: &str,
) -> (
    SocketAddr,
    Arc<crate::server::ServerState>,
    Arc<AtomicBool>,
    std::thread::JoinHandle<()>,
) {
    let state = crate::server::ServerState::new(tag.to_string());
    let stop = Arc::new(AtomicBool::new(false));
    let listener =
        Tailnet::bind_peer_listener(&["127.0.0.1".parse().expect("ip")], 0, Arc::clone(&stop))
            .expect("bind");
    let address = listener.addrs()[0];
    let accept_state = Arc::clone(&state);
    let accept = std::thread::spawn(move || {
        accept_peers(listener, transport, accept_state, pairing);
    });
    (address, state, stop, accept)
}

fn stop_accept_loop(state: &Arc<crate::server::ServerState>, stop: &Arc<AtomicBool>) {
    stop.store(true, Ordering::SeqCst);
    state.stop_flag().store(true, Ordering::SeqCst);
}

/// M1: the peek must not run on the accept thread. A connector that
/// completes the TCP handshake and then sends nothing parks inside
/// `peek_magic` for the whole peek budget; if that happened inline, a
/// second connector would not even be *accepted* until it expired.
///
/// The second client writes `DBP1`, so its own peek returns immediately and
/// its pairing handler is reached at once. The assertion is the delay: the
/// second client must be handled inside `PAIRING_PEEK_TIMEOUT`, which it
/// cannot be if the accept thread is parked on the first one.
#[test]
fn a_silent_connector_does_not_delay_the_next_one() {
    let transport = Arc::new(RefusingTransport::default());
    let pairing = Arc::new(RecordingPairing {
        handled: Mutex::new(Vec::new()),
    });
    let (address, state, stop, accept) =
        spawn_accept_loop(transport.clone(), pairing.clone(), "peer-slowloris");

    // First client: connects and says nothing, forever. It parks inside its
    // own peek for the whole `PAIRING_PEEK_TIMEOUT`.
    let silent = connect_bounded(address);
    std::thread::sleep(Duration::from_millis(150));

    // Second client: a real pairing candidate.
    let candidate = connect_bounded(address);
    {
        use std::io::Write;
        let mut candidate_writer = &candidate;
        candidate_writer
            .write_all(&PAIRING_MAGIC)
            .expect("write the pairing magic");
    }

    let started = Instant::now();
    // The bound is the peek budget rather than a wall clock of its own: the
    // property is "the second connector is not behind the silent one", and a
    // serial accept loop would handle it only *after* the silent client's
    // peek expired. A fixed one-second deadline turned the same property
    // into a machine-speed assertion and went red by 8 ms under load.
    let deadline = started + PAIRING_PEEK_TIMEOUT;
    loop {
        if !pairing
            .handled
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "a second connector was not handled within the peek budget while a silent one \
                 was parked; the accept thread is blocked on the peek ({:?} elapsed)",
            started.elapsed()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        started.elapsed() < PAIRING_PEEK_TIMEOUT,
        "the second connector was handled only after the silent one's peek budget expired: \
             {:?}",
        started.elapsed()
    );
    // The handler saw exactly the candidate, never the silent client.
    let handled = pairing
        .handled
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    assert_eq!(handled.len(), 1, "{handled:?}");
    assert_eq!(
        handled[0].port(),
        candidate.local_addr().expect("candidate addr").port(),
        "the handled connection is the candidate"
    );

    stop_accept_loop(&state, &stop);
    drop(silent);
    drop(candidate);
    join_bounded(accept, "the peer accept loop");
}

/// M1: the peer table is a cached read, not a journal round trip per
/// accepted socket. Every connection below goes through the same path a
/// real one does — the accept loop calls the accessor once per connection,
/// before the filter — so N filter calls with one load proves the cache.
#[test]
fn many_connects_share_one_peer_table_load() {
    // Bounded by the per-source cap, not by a number chosen here: all these
    // connections come from one address, and `admit_source` closes the ones
    // past `MAX_REMOTE_CONNECTIONS_PER_SOURCE` before the filter is ever
    // reached. The cap is the reason the count is what it is, so the
    // constant is the count — if the cap moves, this exercises the new
    // value instead of silently testing four connections.
    const CONNECTIONS: usize = MAX_REMOTE_CONNECTIONS_PER_SOURCE;
    let transport = Arc::new(RefusingTransport::default());
    // Inert: with no active code every connection is closed at once, so
    // nothing but the table read and the filter is exercised.
    let pairing: Arc<dyn PairingHook> = Arc::new(PairingDisabled);
    let (address, state, stop, accept) =
        spawn_accept_loop(transport.clone(), pairing, "peer-table-loads");

    // All the connections are opened first, so the accept loop drains a
    // queue instead of waiting a tick between each one.
    let mut clients = Vec::new();
    for _ in 0..CONNECTIONS {
        clients.push(connect_bounded(address));
    }

    let deadline = Instant::now() + Duration::from_secs(15);
    while transport.calls().len() < CONNECTIONS {
        assert!(
            Instant::now() < deadline,
            "only {} of {CONNECTIONS} connections reached the filter",
            transport.calls().len()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        state.peer_table_loads(),
        1,
        "{CONNECTIONS} connections must load the peer table once"
    );

    stop_accept_loop(&state, &stop);
    drop(clients);
    join_bounded(accept, "the peer accept loop");
}

/// A peer connection is a client for the idle exit, exactly as a pipe
/// connection is.
///
/// The reachable sequence: the app had a session, the session ended, the app
/// detached, and a paired device is still connected. `client_disconnected`
/// then sees `clients == 0 && sessions == 0` and arms the idle timer; one
/// second later the daemon begins shutdown and closes the peer's socket with
/// no shutdown frame. The peer path never took the slot, so nothing bumped
/// `idle_generation` to invalidate the armed timer.
///
/// The assertion is the daemon's own shutdown flag, not the counter: a counter
/// assert would also pass with the increment in the wrong branch. Without the
/// fix the timer fires inside the margin and this goes red.
#[test]
fn a_connected_peer_is_a_client_for_the_idle_exit() {
    use devboule_protocol::{ClientHello, ClientMessage, DaemonMessage, OwnerId};

    let transport = Arc::new(TestTransport::default());
    let (address, state, stop, accept) = spawn_accept_loop(
        transport,
        Arc::new(PairingDisabled) as Arc<dyn PairingHook>,
        "peer-counts-as-client",
    );

    // A paired device, in the shape pairing writes: the row pins the static
    // key the client proves, and the binding the test transport answers with.
    let (client_private, client_public) = keypair(PEER_NOISE_PATTERN);
    state
        .peer_upsert(PeerRecord {
            device_id: "phone".to_string(),
            display_name: "phone".to_string(),
            role: "client".to_string(),
            public_key: client_public,
            paired_by_user: None,
            binding_kind: "tailnet".to_string(),
            binding_stable_id: Some("nstable".to_string()),
            binding_node_name: None,
            binding_login_name: None,
            address: address.to_string(),
            paired_at: 1,
            revoked_at: None,
            caps: vec!["view".to_string()],
        })
        .expect("upsert the peer row");

    let stream = connect_bounded(address);
    let session = initiator_handshake(
        &stream,
        Instant::now() + bound::READ,
        &client_private,
        None,
        PEER_PROLOGUE,
        None,
        PEER_NOISE_PATTERN,
    )
    .expect("the peer handshake");
    let (reader, writer, closer) = split_session(&stream, session).expect("split");
    let framed = crate::framing::Framed::from_stream(reader, writer, closer);
    framed
        .send(&ClientMessage::Hello(ClientHello::m3a(
            OwnerId::new("peer_phone", "devboule-daemon").expect("owner"),
            "devboule-daemon",
        )))
        .expect("the peer hello");
    // The daemon's own hello is the barrier: it is written from inside
    // `handle_client`, so a slot taken before that call is held by now.
    match framed.recv_timeout::<DaemonMessage>(Duration::from_secs(5)) {
        Ok(DaemonMessage::Hello(_)) => {}
        other => panic!("the authenticated peer was not served: {other:?}"),
    }

    // The app connects and detaches with no session live: the transition to
    // zero clients that arms the idle timer. The slot comes from the same
    // admission the pipe accept loop takes (`ServerState::admit_client`).
    drop(state.admit_client().expect("the app is admitted"));

    std::thread::sleep(crate::IDLE_SHUTDOWN_GRACE + Duration::from_millis(400));
    assert!(
        !state.stop_flag().load(Ordering::SeqCst),
        "the daemon signalled shutdown while a paired device was connected and no session was \
         live (clients: {}); the peer connection was not counted",
        state.live_client_count()
    );

    drop(framed);
    drop(stream);
    stop_accept_loop(&state, &stop);
    join_bounded(accept, "the peer accept loop");
}

/// C11: while a code is active, an off-tailnet source must be closed
/// **without being peeked**.
///
/// The design's §7 condition 1 says a non-tailnet source is refused before
/// any byte is read. That held for non-pairing traffic but not for the
/// pairing branch, which peeked first and asked about the address only after
/// the exchange — so an off-tailnet host could have had four bytes read from
/// it, and a `DBP1` from it would have started a full SPAKE2+Noise exchange.
///
/// Read as the compiler sees it: `is_tailnet_or_test_loopback` takes
/// `&IpAddr` and returns a `bool`, and its `#[cfg(test)]` branch admits
/// loopback. A test therefore **cannot** use `127.0.0.1` for the off-tailnet
/// case — that address passes in a test build by design, because the crate's
/// own pairing tests run a responder there. So `dispatch_peer_connection` is
/// called directly with a fabricated off-tailnet `SocketAddr` over a real
/// loopback socket, which is the only socket a unit test can make.
///
/// "Without a peek" is timed, and the timing is a clean discriminator rather
/// than a guess: the client never writes, so `peek_magic` would block for the
/// whole `PAIRING_PEEK_TIMEOUT` before giving up. Closing in less than that
/// can only mean the peek was never entered.
#[test]
fn an_off_tailnet_source_with_a_code_active_is_closed_without_a_peek() {
    let transport = Arc::new(RefusingTransport::default());
    // Active, so the pairing branch — and therefore the gate — is reached.
    let pairing = Arc::new(RecordingPairing {
        handled: Mutex::new(Vec::new()),
    });
    let state = crate::server::ServerState::new("peer-off-tailnet".to_string());
    let caps = Arc::new(AcceptCaps::default());

    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr");
    let client = connect_bounded(address);
    let (stream, _) = accept_bounded(&listener);

    // A public address: neither a tailnet range nor loopback, so the gate
    // refuses it in a test build too.
    let off_tailnet: SocketAddr = "8.8.8.8:47831".parse().expect("addr");
    assert!(
        !is_tailnet_or_test_loopback(&off_tailnet.ip()),
        "the fixture must be off-tailnet for this test to mean anything"
    );
    assert!(
        pairing.is_active(),
        "a code must be active to reach the branch"
    );

    let started = Instant::now();
    dispatch_peer_connection(
        transport.as_ref(),
        &caps,
        pairing.as_ref(),
        stream,
        off_tailnet,
        &state,
    );
    let elapsed = started.elapsed();

    // Nothing was read: the peek would have burned the whole budget here.
    assert!(
        elapsed < PAIRING_PEEK_TIMEOUT,
        "an off-tailnet source must be closed before the peek budget is spent, took {elapsed:?}"
    );
    // And the pairing exchange was never entered.
    assert!(
        pairing
            .handled
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty(),
        "an off-tailnet source must never reach the pairing exchange"
    );
    // The source sees the close, and never a byte back.
    let mut sink = [0u8; 1];
    let read = {
        use std::io::Read;
        (&client).read(&mut sink)
    };
    assert_eq!(
        read.expect("a closed socket reads as EOF"),
        0,
        "the daemon must close an off-tailnet source, not answer it"
    );
}
