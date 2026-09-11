//! Slice 1a end-to-end: two real daemons, a real tailnet link.
//!
//! Everything in S1–S6 is covered without Tailscale; this file is the one test
//! that needs a running tailscaled, because the listener binds real tailnet
//! addresses and the binding check calls `whois`. It skips with a clear message
//! when the listener is not up, so a CI runner without Tailscale reports why
//! rather than failing.
//!
//! The test drives two separate daemon processes over their named pipes for
//! control (pairing, device lists, revocation), and acts as a **peer** itself
//! over Noise for the assertions that only exist on the peer path: the
//! `Daemon`-role projection, the capability refusals, the audit rows, and the
//! revocation drop. Acting as the peer means reading daemon B's long-term
//! static key out of its own file secret store, which is exactly what a real
//! peer holds.
//!
//! The pipe is spoken raw (`connect_pipe` + `Framed`) rather than through
//! `DaemonClient`: this test sends `ClientMessage` variants that
//! `DaemonClient` has no typed method for yet (the device RPCs are slice 1b's
//! to add on the client side), and a raw frame keeps the test independent of
//! that work.

#![cfg(windows)]

use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_daemon::{
    connect_pipe, initiator_handshake, split_session, Framed, RuntimePaths, PEER_NOISE_PATTERN,
    PEER_PROLOGUE,
};
use devboule_protocol::{ClientHello, ClientMessage, DaemonMessage, ErrorCode, OwnerId, PeerRole};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

fn daemon_bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_devboule-daemon") {
        return PathBuf::from(path);
    }
    if let Some(path) = option_env!("CARGO_BIN_EXE_devboule-daemon") {
        return PathBuf::from(path);
    }
    panic!(
        "CARGO_BIN_EXE_devboule-daemon was not provided by Cargo; refusing to \
         guess a target directory binary (a stale one would test the past)"
    )
}

fn unique_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "devboule peer {tag} {}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("runtime directory");
    dir
}

fn hello(name: &str) -> ClientHello {
    let sid = devboule_daemon::current_user_sid().expect("current user SID");
    ClientHello::m3a(
        OwnerId::new(sid, format!("peer-{name}-{}", std::process::id())).expect("owner"),
        "devboule-peer-test",
    )
}

const RECV_TIMEOUT: Duration = Duration::from_secs(10);

/// A raw protocol conversation over the daemon's named pipe.
struct Pipe {
    framed: Framed,
    next_id: AtomicU64,
}

impl Pipe {
    fn open(paths: &RuntimePaths, name: &str) -> Result<Self, String> {
        let file = connect_pipe(&paths.pipe_name).map_err(|error| format!("connect: {error}"))?;
        let framed = Framed::new(file);
        framed
            .send(&ClientMessage::Hello(hello(name)))
            .map_err(|error| format!("hello: {error}"))?;
        match framed.recv_timeout::<DaemonMessage>(RECV_TIMEOUT) {
            Ok(DaemonMessage::Hello(_)) => {}
            Ok(other) => return Err(format!("expected hello, got {other:?}")),
            Err(error) => return Err(format!("hello reply: {error}")),
        }
        Ok(Self {
            framed,
            next_id: AtomicU64::new(1),
        })
    }

    fn request(&self, message: ClientMessage) -> Result<DaemonMessage, String> {
        let name = message.name();
        self.framed
            .send(&message)
            .map_err(|error| format!("{name} send: {error}"))?;
        self.framed
            .recv_timeout(RECV_TIMEOUT)
            .map_err(|error| format!("{name} recv: {error}"))
    }

    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Send one device request and require a specific reply shape.
    fn expect(&self, message: ClientMessage) -> DaemonMessage {
        self.request(message)
            .unwrap_or_else(|error| panic!("request failed: {error}"))
    }
}

/// Wait for the daemon's pipe to accept a hello. Bounded: a daemon that never
/// comes up fails the test rather than hanging it.
fn wait_until_pipe(paths: &RuntimePaths, tag: &str) -> Pipe {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut last = String::new();
    while Instant::now() < deadline {
        match Pipe::open(paths, tag) {
            Ok(pipe) => return pipe,
            Err(error) => last = error,
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("daemon {tag} did not come up: {last}");
}

/// One daemon process: its runtime dir, its pipe, and its captured stderr.
struct Peer {
    dir: PathBuf,
    child: Option<Child>,
    stderr: Arc<Mutex<String>>,
    pipe: Pipe,
}

impl Peer {
    /// Spawn with a fixed peer port and the file secret store, and drain
    /// stderr on its own thread so a full pipe buffer can never block the
    /// daemon.
    fn spawn(tag: &'static str, port: u16) -> Self {
        let dir = unique_dir(tag);
        let paths = RuntimePaths::from_dir(&dir);
        let mut command = Command::new(daemon_bin());
        command
            .env("DEVBOULE_RUNTIME_DIR", &dir)
            .env("DEVBOULE_PEER_PORT", port.to_string())
            .env("DEVBOULE_SECRET_STORE", "file")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().expect("spawn daemon");
        let stderr = Arc::new(Mutex::new(String::new()));
        if let Some(mut pipe) = child.stderr.take() {
            let sink = Arc::clone(&stderr);
            std::thread::spawn(move || {
                use std::io::Read;
                let mut buffer = [0u8; 4096];
                // Bounded: a daemon that never closes stderr must not keep this
                // test alive. The child is killed in `Drop` regardless.
                let deadline = Instant::now() + Duration::from_secs(120);
                while Instant::now() < deadline {
                    match pipe.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => sink
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .push_str(&String::from_utf8_lossy(&buffer[..read])),
                    }
                }
            });
        }
        let pipe = wait_until_pipe(&paths, tag);
        Self {
            dir,
            child: Some(child),
            stderr,
            pipe,
        }
    }

    fn self_info(&self) -> devboule_protocol::SelfInfo {
        match self
            .pipe
            .expect(ClientMessage::DevicesList { id: self.pipe.id() })
        {
            DaemonMessage::Devices { self_info, .. } => self_info,
            other => panic!("expected Devices, got {other:?}"),
        }
    }

    /// Where a peer should dial this daemon. `None` when the listener is not up
    /// (no Tailscale), which is how the test decides to skip.
    fn peer_address(&self) -> Option<String> {
        let info = self.self_info();
        if info.addresses.is_empty() || info.port == 0 {
            return None;
        }
        Some(format!("{}:{}", info.addresses[0], info.port))
    }

    fn stderr_contents(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// This daemon's long-term Noise static private key, read out of its own
    /// secret store. A peer holds exactly this.
    fn static_private(&self) -> [u8; 32] {
        let path = self.dir.join("secrets").join("noise-static.bin");
        let bytes = std::fs::read(&path).expect("the daemon's stored static key");
        assert_eq!(
            bytes.len(),
            34,
            "envelope is 2 version bytes + 32 key bytes"
        );
        assert_eq!(&bytes[..2], &[0x00, 0x01], "envelope version");
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes[2..]);
        key
    }

    fn audit_rows(&self) -> Vec<(String, String)> {
        let connection = rusqlite::Connection::open(self.dir.join("journal.db")).expect("journal");
        let mut statement = connection
            .prepare("SELECT action, outcome FROM audit ORDER BY id")
            .expect("prepare");
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows")
    }

    /// The stored peer rows, as raw columns. Read straight from the journal so
    /// the assertion does not depend on a projection that is itself under test.
    fn peer_rows(&self) -> Vec<(String, String, Option<i64>)> {
        let connection = rusqlite::Connection::open(self.dir.join("journal.db")).expect("journal");
        let mut statement = connection
            .prepare("SELECT device_id, role, revoked_at FROM peers ORDER BY device_id")
            .expect("prepare");
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows")
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A raw Noise peer: the steady-state connection a paired device makes.
struct NoisePeer {
    framed: Framed,
}

impl NoisePeer {
    fn connect(address: &str, static_private: &[u8; 32]) -> Result<Self, String> {
        let addr: SocketAddr = address.parse().map_err(|error| format!("{error}"))?;
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
            .map_err(|error| format!("connect: {error}"))?;
        let session = initiator_handshake(
            &stream,
            Instant::now() + Duration::from_secs(10),
            static_private,
            None,
            PEER_PROLOGUE,
            None,
            PEER_NOISE_PATTERN,
        )
        .map_err(|error| format!("handshake: {error}"))?;
        let (reader, writer) =
            split_session(&stream, session).map_err(|error| error.to_string())?;
        Ok(Self {
            framed: Framed::from_stream(reader, writer),
        })
    }

    /// Complete the hello exactly as the app does, so the daemon sees a normal
    /// post-handshake client.
    fn hello(&self) -> Result<(), String> {
        self.framed
            .send(&ClientMessage::Hello(hello("remote")))
            .map_err(|error| format!("{error}"))?;
        match self.framed.recv_timeout::<DaemonMessage>(RECV_TIMEOUT) {
            Ok(DaemonMessage::Hello(_)) => Ok(()),
            Ok(other) => Err(format!("expected hello, got {other:?}")),
            Err(error) => Err(format!("{error}")),
        }
    }

    fn request(&self, message: ClientMessage) -> Result<DaemonMessage, String> {
        self.framed
            .send(&message)
            .map_err(|error| format!("{error}"))?;
        self.framed
            .recv_timeout(RECV_TIMEOUT)
            .map_err(|error| format!("{error}"))
    }
}

/// Poll `DevicesList` until the daemon holds `count` rows. The initiator of a
/// `Client` pairing writes its row on its own thread once the far side
/// answers, so the row appears shortly after the confirm rather than at once.
fn wait_for_row_count(peer: &Peer, count: usize) -> Vec<(String, String, Option<i64>)> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let rows = peer.peer_rows();
        if rows.len() >= count {
            return rows;
        }
        assert!(
            Instant::now() < deadline,
            "expected {count} peer rows, saw {}",
            rows.len()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Assert that a JSON object carries no usable value under `key`.
fn assert_absent_or_empty(json: &serde_json::Value, key: &str, context: &str) {
    match json.get(key) {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::String(value)) if value.is_empty() => {}
        Some(serde_json::Value::Array(values)) if values.is_empty() => {}
        Some(other) => panic!("{context} leaks {key}: {other}"),
    }
}

#[test]
fn two_daemons_pair_over_the_tailnet_and_a_peer_is_restricted() {
    let _guard = lock_tests();

    let a = Peer::spawn("a", 47831);
    let b = Peer::spawn("b", 47832);

    let (Some(address_a), Some(address_b)) = (a.peer_address(), b.peer_address()) else {
        eprintln!(
            "SKIP peer_link: this machine has no reachable tailnet address (Tailscale not \
             running), so the remote listener is disabled and only the local-only paths \
             remain, which the unit suites already cover"
        );
        return;
    };

    let a_self = a.self_info();
    let b_self = b.self_info();
    assert_ne!(
        a_self.device_id, b_self.device_id,
        "distinct device identities"
    );
    assert_eq!(a_self.key_fingerprint.len(), 32, "a hex32 fingerprint");
    assert!(
        !a_self.public_key.is_empty(),
        "self_info carries the public key"
    );

    // ---- pairing one: A displays a Client code, B types it, A confirms -----
    //
    // Only two daemons are used, so both roles are exercised on the same pair:
    // the design refuses to re-pair a device that already has a live row
    // (§8 R8, F-19), so the `Daemon` pairing below happens only after a revoke.
    // Two daemons rather than three is also forced by the tailnet: every daemon
    // on this host shares one tailnet address, and the pre-Noise filter matches
    // peers by address, so a third daemon's connection would be mistaken for
    // the second one's.
    let id = a.pipe.id();
    let code = match a.pipe.expect(ClientMessage::PairingStart {
        id,
        role: PeerRole::Client,
    }) {
        DaemonMessage::PairingCode { code, .. } => code,
        other => panic!("expected PairingCode, got {other:?}"),
    };
    let id = b.pipe.id();
    match b.pipe.expect(ClientMessage::PairingComplete {
        id,
        address: address_a.clone(),
        code,
        role: PeerRole::Client,
    }) {
        // B's own role is Client, so it learns that A must confirm.
        DaemonMessage::PairingPending { peer, .. } => {
            assert_eq!(peer.role, PeerRole::Client);
            assert_eq!(peer.device_id, a_self.device_id);
            assert!(!peer.key_fingerprint.is_empty());
        }
        other => panic!("expected PairingPending for a Client pairing, got {other:?}"),
    }

    // A learns about the request by polling `DevicesList`: slice 1a has no push
    // channel, so this is the only way the card can appear.
    let parking = match a
        .pipe
        .expect(ClientMessage::DevicesList { id: a.pipe.id() })
    {
        DaemonMessage::Devices { pending, .. } => pending,
        other => panic!("expected Devices, got {other:?}"),
    };
    assert_eq!(parking.len(), 1, "exactly one pairing is parked at A");
    assert_eq!(
        parking[0].device_id, b_self.device_id,
        "the parked entry is keyed by the device that typed the code"
    );
    assert_eq!(parking[0].role, PeerRole::Client);

    let id = a.pipe.id();
    match a.pipe.expect(ClientMessage::PairingConfirm {
        id,
        device_id: parking[0].device_id.clone(),
        accept: true,
    }) {
        DaemonMessage::PeerUpdated { peer, .. } => {
            assert_eq!(peer.device_id, b_self.device_id);
            assert_eq!(peer.role, PeerRole::Client);
        }
        other => panic!("expected PeerUpdated on confirm, got {other:?}"),
    }

    // A holds its row now; B writes its own once A's answer reaches it, on the
    // thread that did not block the caller.
    let a_rows = a.peer_rows();
    assert_eq!(a_rows.len(), 1);
    assert_eq!(a_rows[0].0, b_self.device_id, "A's row names B");
    assert_eq!(a_rows[0].1, "client");
    let b_rows = wait_for_row_count(&b, 1);
    assert_eq!(b_rows[0].0, a_self.device_id, "B's row names A");

    // ---- a peer connects over Noise, and the hello is accepted -------------
    let peer = NoisePeer::connect(&address_a, &b.static_private())
        .expect("a paired peer completes the Noise handshake");
    peer.hello().expect("the hello inside Noise is accepted");

    // ---- an allowed read writes no audit row ------------------------------
    for id in 0..20 {
        match peer.request(ClientMessage::Ping { id }) {
            Ok(DaemonMessage::Pong { .. }) => {}
            other => panic!("expected Pong, got {other:?}"),
        }
    }
    let rows = a.audit_rows();
    assert!(
        !rows.iter().any(|(action, _)| action == "Ping"),
        "20 allowed pings must not write an audit row: {rows:?}"
    );

    // ---- Status and Shutdown are refused, each with one denied audit row ---
    for request in [
        ClientMessage::Status { id: 100 },
        ClientMessage::Shutdown { id: 101 },
    ] {
        let name = request.name().to_string();
        match peer.request(request) {
            Ok(DaemonMessage::Error(error)) => assert_eq!(
                error.code,
                ErrorCode::CapabilityNotSupported,
                "{name} must be refused"
            ),
            other => panic!("expected CapabilityNotSupported for {name}, got {other:?}"),
        }
        let rows = a.audit_rows();
        assert!(
            rows.iter()
                .any(|(action, outcome)| action == &name && outcome == "denied"),
            "{name} must leave one denied audit row: {rows:?}"
        );
    }

    // ---- the Client-role device list withholds the pairing user's SID ------
    match peer.request(ClientMessage::DevicesList { id: 200 }) {
        Ok(DaemonMessage::Devices {
            self_info, peers, ..
        }) => {
            let self_json = serde_json::to_value(&self_info).expect("json");
            for leaked in ["addresses", "remote"] {
                assert_absent_or_empty(&self_json, leaked, "a Client peer's self_info");
            }
            assert!(
                !peers.is_empty(),
                "a Client peer sees the devices it may drive"
            );
            for row in &peers {
                let json = serde_json::to_value(row).expect("json");
                assert_absent_or_empty(&json, "pairedByUser", "a Client peer's peer row");
                assert!(
                    json.get("deviceId").is_some(),
                    "the row still identifies the device: {json}"
                );
            }
        }
        other => panic!("expected Devices, got {other:?}"),
    }

    // ---- a plaintext connection is closed, not answered --------------------
    let mut plaintext =
        TcpStream::connect_timeout(&address_a.parse().expect("addr"), Duration::from_secs(5))
            .expect("connect");
    plaintext
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    {
        use std::io::Write;
        let _ = plaintext.write_all(b"{\"type\":\"hello\"}\n");
    }
    let mut sink = [0u8; 64];
    let read = {
        use std::io::Read;
        plaintext.read(&mut sink)
    };
    assert!(
        matches!(read, Ok(0) | Err(_)),
        "a plaintext hello must not be answered, got {read:?}"
    );

    // ---- revocation closes the live connection -----------------------------
    let id = a.pipe.id();
    match a.pipe.expect(ClientMessage::PeerRevoke {
        id,
        device_id: b_self.device_id.clone(),
    }) {
        DaemonMessage::PeerUpdated { peer, .. } => assert!(
            peer.revoked_at.is_some(),
            "the updated row reports the revocation"
        ),
        other => panic!("expected PeerUpdated on revoke, got {other:?}"),
    }
    assert!(
        a.peer_rows()
            .iter()
            .any(|(device, _, revoked)| device == &b_self.device_id && revoked.is_some()),
        "A's row for B is revoked"
    );
    let after = peer.request(ClientMessage::Ping { id: 999 });
    assert!(
        after.is_err(),
        "a revoked peer's connection must be closed, got {after:?}"
    );
    drop(peer);
    let refused = match NoisePeer::connect(&address_a, &b.static_private()) {
        Err(_) => true,
        Ok(peer) => peer.hello().is_err(),
    };
    assert!(refused, "a revoked peer must not be able to reconnect");

    // ---- re-pairing after a revoke, this time as a Daemon peer -------------
    //
    // The row must be gone from both sides before a code is shown, which is the
    // rule the UI states ("revoke it first").
    let id = b.pipe.id();
    match b.pipe.expect(ClientMessage::PeerRevoke {
        id,
        device_id: a_self.device_id.clone(),
    }) {
        DaemonMessage::PeerUpdated { .. } => {}
        other => panic!("expected PeerUpdated on B's revoke, got {other:?}"),
    }
    let id = b.pipe.id();
    let code = match b.pipe.expect(ClientMessage::PairingStart {
        id,
        role: PeerRole::Daemon,
    }) {
        DaemonMessage::PairingCode { code, .. } => code,
        other => panic!("expected PairingCode, got {other:?}"),
    };
    let id = a.pipe.id();
    match a.pipe.expect(ClientMessage::PairingComplete {
        id,
        // B displayed the code, so this is B's address.
        address: address_b.clone(),
        code,
        role: PeerRole::Daemon,
    }) {
        // A Daemon pairing needs no local confirmation, so it completes on the
        // spot.
        DaemonMessage::PairingDone { peer, .. } => {
            assert_eq!(peer.device_id, b_self.device_id);
            assert_eq!(peer.role, PeerRole::Daemon);
        }
        other => panic!("expected PairingDone for a Daemon pairing, got {other:?}"),
    }
    assert_eq!(
        wait_for_row_count(&a, 1)[0].1,
        "daemon",
        "the re-pair after a revoke carries the new role"
    );

    // ---- neither daemon's stderr leaks an identity or a key ----------------
    for (daemon, name) in [(&a, "A"), (&b, "B")] {
        let stderr = daemon.stderr_contents();
        assert!(
            !stderr.to_ascii_lowercase().contains("panicked"),
            "{name} stderr contains a panic:\n{stderr}"
        );
        for device in [&a_self.device_id, &b_self.device_id] {
            assert!(
                !stderr.contains(device.as_str()),
                "{name} stderr leaks a device id:\n{stderr}"
            );
        }
        for key in [&a_self.public_key, &b_self.public_key] {
            assert!(
                !stderr.contains(key.as_str()),
                "{name} stderr leaks a public key:\n{stderr}"
            );
        }
    }
}
