//! Two real daemons, a real tailnet link.
//!
//! This file is the suite's only tailscale-bound test: the listener binds
//! real tailnet addresses and the binding check calls `whois`. It skips with
//! a clear message when the listener is not up, so a CI runner without
//! Tailscale reports why rather than failing.
//!
//! The test drives two separate daemon processes over their named pipes for
//! control (pairing, device lists, revocation), and acts as a **peer** itself
//! over Noise for the assertions that only exist on the peer path: the
//! device projection, what the grant opens and what narrowing it closes, the
//! audit rows, and the
//! revocation drop. Acting as the peer means reading daemon B's long-term
//! static key out of its own file secret store, which is exactly what a real
//! peer holds.
//!
//! The pipe is spoken raw (`connect_pipe` + `Framed`) rather than through
//! `DaemonClient`: this test sends `ClientMessage` variants that
//! `DaemonClient` has no typed method for yet — the device RPCs among them —
//! and a raw frame keeps the test independent of the client's typed wrappers.

#![cfg(windows)]

use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_daemon::{
    connect_pipe, dial_peer, initiator_handshake, split_session, Framed, RuntimePaths,
    PEER_NOISE_PATTERN, PEER_PROLOGUE,
};
use devboule_protocol::{
    AgentMessageState, ClientHello, ClientMessage, DaemonMessage, ErrorCode, OwnerId, SessionEvent,
    SessionKind, UserMessageKind, WorkspaceIsolation,
};

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

/// Two ports that nothing is listening on right now.
///
/// The default 47831 is usually taken by the developer's own running app, and a
/// port that is already bound makes the daemon disable its listener and this
/// test skip — a false green on the only machine it runs on. Binding an
/// ephemeral port and releasing it gives a number that is free at probe time;
/// the window between probe and bind is microseconds, and the skip message above
/// now says which daemon failed to bind if it closes.
fn two_free_ports() -> (u16, u16) {
    let probe = || -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("probe a free port");
        listener.local_addr().expect("probe address").port()
    };
    let first = probe();
    let second = loop {
        let candidate = probe();
        if candidate != first {
            break candidate;
        }
    };
    (first, second)
}

fn unique_dir(tag: &str) -> PathBuf {
    devboule_daemon::test_dirs::test_temp_dir(&format!("devboule peer {tag}"))
}

/// The ACP stub provider's argv, as the daemon reads `DEVBOULE_ACP_COMMAND`.
/// The delivery test's target session must be a real live agent; the stub is
/// built by the same `cargo test` invocation, so the test does not depend on
/// what this machine happens to have installed.
fn stub_command() -> String {
    let path = std::env::var("CARGO_BIN_EXE_devboule-acp-stub")
        .ok()
        .or_else(|| option_env!("CARGO_BIN_EXE_devboule-acp-stub").map(str::to_string))
        .unwrap_or_else(|| {
            panic!(
                "CARGO_BIN_EXE_devboule-acp-stub was not provided by Cargo; refusing to \
                 guess a target directory binary (a stale one would test the past)"
            )
        });
    serde_json::to_string(&[path]).expect("stub argv")
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

    /// Read frames until one satisfies `wanted`, or `deadline` passes: an
    /// attached session's events arrive on the same pipe as replies, so a
    /// test that waits for one cannot use `request`.
    fn recv_until<T>(
        &self,
        deadline: Instant,
        mut wanted: impl FnMut(&DaemonMessage) -> Option<T>,
    ) -> Option<T> {
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let frame = self.framed.recv_timeout::<DaemonMessage>(remaining).ok()?;
            if let Some(found) = wanted(&frame) {
                return Some(found);
            }
        }
        None
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
    /// Spawn with the given peer port and the file secret store, and drain
    /// stderr on its own thread so a full pipe buffer can never block the
    /// daemon.
    ///
    /// The port is chosen by the caller from a free-port probe rather than a
    /// fixed number: a developer running the app holds 47831 with a live
    /// daemon, and a fixed port would make this test skip (or fail) on exactly
    /// the machine it is meant to prove itself on.
    fn spawn(tag: &'static str, port: u16) -> Self {
        Self::spawn_with_env(tag, port, &[])
    }

    /// The same daemon carrying `extra_env` for its providers, so the
    /// delivery test points one at the stub without touching the process
    /// environment every other test in this binary inherits.
    fn spawn_with_env(tag: &'static str, port: u16, extra_env: &[(&str, &str)]) -> Self {
        let dir = unique_dir(tag);
        let paths = RuntimePaths::from_dir(&dir);
        let mut command = Command::new(daemon_bin());
        command
            .env("DEVBOULE_RUNTIME_DIR", &dir)
            .env("DEVBOULE_PEER_PORT", port.to_string())
            .env("DEVBOULE_SECRET_STORE", "file")
            .envs(extra_env.iter().copied())
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
    /// (no Tailscale), which is how the test decides to skip. Composed by
    /// `SocketAddr`, so an IPv6 tailnet address arrives bracketed.
    fn peer_address(&self) -> Option<String> {
        let info = self.self_info();
        if info.addresses.is_empty() || info.port == 0 {
            return None;
        }
        let ip: std::net::IpAddr = info.addresses[0].parse().expect("self address is an ip");
        Some(SocketAddr::new(ip, info.port).to_string())
    }

    fn stderr_contents(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// What this daemon says about its listener, for a skip message that
    /// distinguishes "no Tailscale" from "the port was taken".
    fn remote_label(&self) -> String {
        let info = self.self_info();
        if info.addresses.is_empty() {
            format!(
                "disabled ({}), port {}",
                info.remote
                    .as_ref()
                    .and_then(|remote| remote.reason.clone())
                    .unwrap_or_else(|| "no reason reported".to_string()),
                info.port
            )
        } else {
            format!("{:?} port {}", info.addresses, info.port)
        }
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
    fn peer_rows(&self) -> Vec<(String, bool, Option<i64>)> {
        let connection = rusqlite::Connection::open(self.dir.join("journal.db")).expect("journal");
        let mut statement = connection
            .prepare("SELECT device_id, legacy_dialable, revoked_at FROM peers ORDER BY device_id")
            .expect("prepare");
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows")
    }

    /// One peer row's pinned key and stored address, read straight from the
    /// journal: exactly the data the dial path resolves out of the row at
    /// dial time.
    fn stored_row_for(&self, device_id: &str) -> Option<(Vec<u8>, String)> {
        let connection = rusqlite::Connection::open(self.dir.join("journal.db")).expect("journal");
        let mut statement = connection
            .prepare("SELECT public_key, address FROM peers WHERE device_id = ?1")
            .expect("prepare");
        let mut rows = statement
            .query_map([device_id], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
            })
            .expect("query");
        rows.next().transpose().expect("row")
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
        let (reader, writer, closer) =
            split_session(&stream, session).map_err(|error| error.to_string())?;
        Ok(Self {
            framed: Framed::from_stream(reader, writer, closer),
        })
    }

    /// Complete the hello exactly as a dialing daemon does: a peer owner and
    /// the workspace presence a v32 peer must state, so the daemon sees a
    /// normal post-handshake peer.
    fn hello(&self) -> Result<(), String> {
        let sid = devboule_daemon::current_user_sid().expect("current user SID");
        let owner =
            OwnerId::new(sid, format!("peer-remote-{}", std::process::id())).expect("owner");
        self.framed
            .send(&ClientMessage::Hello(ClientHello::peer(
                owner,
                "devboule-peer-test",
                true,
            )))
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

/// Pair `a` to `b`, `b` displaying the code. Every pairing asks no role, and
/// the displaying device always parks it for its own person's confirmation, so
/// this confirms there and returns once both rows are written.
fn pair_devices(a: &Peer, b: &Peer, address_b: &str, b_device_id: &str) {
    let id = b.pipe.id();
    let code = match b.pipe.expect(ClientMessage::PairingStart { id }) {
        DaemonMessage::PairingCode { code, .. } => code,
        other => panic!("expected PairingCode, got {other:?}"),
    };
    let id = a.pipe.id();
    match a.pipe.expect(ClientMessage::PairingComplete {
        id,
        address: address_b.to_string(),
        code,
    }) {
        DaemonMessage::PairingPending { peer, .. } => {
            assert_eq!(peer.device_id, b_device_id);
            assert_eq!(
                peer.role, None,
                "a v32 pending row carries no v30 role projection"
            );
        }
        other => panic!("expected PairingPending for a roleless pairing, got {other:?}"),
    }
    // The displaying device parks after the exchange; poll its own list for the
    // card, then answer it. `pairing_complete` returns before the park has
    // landed, so a confirm that beats it is refused as UnknownPending.
    let deadline = Instant::now() + Duration::from_secs(15);
    let parking = loop {
        let pending = match b
            .pipe
            .expect(ClientMessage::DevicesList { id: b.pipe.id() })
        {
            DaemonMessage::Devices { pending, .. } => pending,
            other => panic!("expected Devices, got {other:?}"),
        };
        if !pending.is_empty() {
            break pending;
        }
        assert!(
            Instant::now() < deadline,
            "the displaying device never parked the pairing"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let id = b.pipe.id();
    match b.pipe.expect(ClientMessage::PairingConfirm {
        id,
        device_id: parking[0].device_id.clone(),
        accept: true,
    }) {
        DaemonMessage::PeerUpdated { .. } => {}
        other => panic!("expected PeerUpdated on confirm, got {other:?}"),
    }
    wait_for_row_count(a, 1);
    wait_for_row_count(b, 1);
}

fn wait_for_row_count(peer: &Peer, count: usize) -> Vec<(String, bool, Option<i64>)> {
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
fn two_daemons_pair_over_the_tailnet_and_the_grant_decides_what_the_peer_reaches() {
    let _guard = lock_tests();

    // Two ports nobody is using, rather than the defaults: the developer's own
    // app typically holds 47831, and a collided port would make this test skip
    // on the one machine where it matters.
    let (port_a, port_b) = two_free_ports();
    let a = Peer::spawn("a", port_a);
    let b = Peer::spawn("b", port_b);

    let (Some(address_a), Some(address_b)) = (a.peer_address(), b.peer_address()) else {
        // Not a silent skip: each daemon's own reason is printed, so a port
        // taken after the probe, a Tailscale that is not running, and a bug in
        // this test are distinguishable.
        eprintln!(
            "SKIP peer_link: no reachable tailnet address. A said {}; B said {}. Tailscale is \
             probably not running, and the unit suites cover every path that does not need it.",
            a.remote_label(),
            b.remote_label()
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

    // ---- pairing one: A displays a code, B types it, A confirms ------------
    //
    // Only two daemons are used, and the design refuses to re-pair a device
    // that already has a live row (§8 R8, F-19), so the re-pair below happens
    // only after a revoke. Two daemons rather than three is also forced by the
    // tailnet: every daemon on this host shares one tailnet address, and the
    // pre-Noise filter matches peers by address, so a third daemon's connection
    // would be mistaken for the second one's.
    let id = a.pipe.id();
    let code = match a.pipe.expect(ClientMessage::PairingStart { id }) {
        DaemonMessage::PairingCode { code, .. } => code,
        other => panic!("expected PairingCode, got {other:?}"),
    };
    let id = b.pipe.id();
    match b.pipe.expect(ClientMessage::PairingComplete {
        id,
        address: address_a.clone(),
        code,
    }) {
        // A roleless pairing is always parked, so B learns that A must confirm.
        DaemonMessage::PairingPending { peer, .. } => {
            assert_eq!(
                peer.role, None,
                "a v32 pending row carries no v30 role projection"
            );
            assert_eq!(peer.device_id, a_self.device_id);
            assert!(!peer.key_fingerprint.is_empty());
        }
        other => panic!("expected PairingPending for a roleless pairing, got {other:?}"),
    }

    // A learns about the request by polling `DevicesList`: there is no push
    // channel, so this is the only way the card can appear.
    // The park happens on A's side once B's request has crossed the link, so
    // one poll is not enough: ask again until the daemon's own answer names the
    // pairing, and fail if it never parks at all.
    let deadline = Instant::now() + Duration::from_secs(15);
    let parking = loop {
        let pending = match a
            .pipe
            .expect(ClientMessage::DevicesList { id: a.pipe.id() })
        {
            DaemonMessage::Devices { pending, .. } => pending,
            other => panic!("expected Devices, got {other:?}"),
        };
        if !pending.is_empty() {
            break pending;
        }
        assert!(
            Instant::now() < deadline,
            "A never parked the pairing it was asked to confirm"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(parking.len(), 1, "exactly one pairing is parked at A");
    assert_eq!(
        parking[0].device_id, b_self.device_id,
        "the parked entry is keyed by the device that typed the code"
    );
    assert_eq!(
        parking[0].role, None,
        "the v32 panel reads a pending row without a v30 role word"
    );

    let id = a.pipe.id();
    match a.pipe.expect(ClientMessage::PairingConfirm {
        id,
        device_id: parking[0].device_id.clone(),
        accept: true,
    }) {
        DaemonMessage::PeerUpdated { peer, .. } => {
            assert_eq!(peer.device_id, b_self.device_id);
            assert_eq!(
                peer.role, None,
                "a v32 PeerUpdated carries no v30 role projection"
            );
        }
        other => panic!("expected PeerUpdated on confirm, got {other:?}"),
    }

    // A holds its row now; B writes its own once A's answer reaches it, on the
    // thread that did not block the caller.
    let a_rows = a.peer_rows();
    assert_eq!(a_rows.len(), 1);
    assert_eq!(a_rows[0].0, b_self.device_id, "A's row names B");
    assert!(!a_rows[0].1, "a roleless pairing keeps no v30 dial hint");
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

    // ---- a paired device's list withholds the pairing user's SID ----------
    match peer.request(ClientMessage::DevicesList { id: 200 }) {
        Ok(DaemonMessage::Devices {
            self_info, peers, ..
        }) => {
            let self_json = serde_json::to_value(&self_info).expect("json");
            for leaked in ["addresses", "remote"] {
                assert_absent_or_empty(&self_json, leaked, "a paired device's self_info");
            }
            assert!(
                !peers.is_empty(),
                "a paired device sees the devices it may drive"
            );
            for row in &peers {
                let json = serde_json::to_value(row).expect("json");
                assert_absent_or_empty(&json, "pairedByUser", "a paired device's peer row");
                assert!(
                    json.get("deviceId").is_some(),
                    "the row still identifies the device: {json}"
                );
            }
        }
        other => panic!("expected Devices, got {other:?}"),
    }

    // ---- a paired device is a full client: the status body is served --------
    //
    // The row a pairing writes holds every capability (`PEER_DEFAULT_CAPS`,
    // the owner's decision of 2026-09-21), so the daemon's own status body —
    // pid, instance, counts, the secret-store selector — reaches the device
    // that just paired. The pid is compared with what the local pipe reads,
    // which is what makes this the daemon's own body and not a projection. It
    // is a read, so it writes no audit row: the same rule the twenty pings
    // above follow.
    let local_pid = match a.pipe.expect(ClientMessage::Status { id: a.pipe.id() }) {
        DaemonMessage::Status { body, .. } => body.pid,
        other => panic!("the local pipe reads the daemon's status body, got {other:?}"),
    };
    match peer.request(ClientMessage::Status { id: 100 }) {
        Ok(DaemonMessage::Status { id: 100, body }) => {
            assert_eq!(
                body.pid, local_pid,
                "the peer reads the daemon's own status body, not a projection"
            );
            assert!(
                body.secret_store.is_some(),
                "the whole body, selector included: {body:?}"
            );
        }
        other => panic!("a freshly paired device must be served Status, got {other:?}"),
    }
    let rows = a.audit_rows();
    assert!(
        !rows.iter().any(|(action, _)| action == "Status"),
        "an allowed read must not write an audit row: {rows:?}"
    );

    // ---- narrowing the grant takes it away, over the wire ------------------
    //
    // The capability set is read from the `peers` row when a connection is
    // established (`server/connection.rs`), so the owner's change applies to the
    // next connection. It also drops the live one, on that connection's own next
    // turn: a device must not keep a capability the row no longer grants
    // (`server/devices.rs`, `revoke_peer_connections`).
    let id = a.pipe.id();
    match a.pipe.expect(ClientMessage::PeerSetCaps {
        id,
        device_id: b_self.device_id.clone(),
        caps: vec!["view".to_string()],
    }) {
        DaemonMessage::PeerUpdated { peer, .. } => {
            assert_eq!(
                peer.caps,
                vec!["view".to_string()],
                "the row keeps the narrowing"
            )
        }
        other => panic!("expected PeerUpdated on set_caps, got {other:?}"),
    }
    assert!(
        peer.request(ClientMessage::Ping { id: 120 }).is_err(),
        "the live connection must not outlive the capability it was opened with"
    );

    // ---- and a device without `admin` is refused the same frames as before --
    //
    // The negative control the parity decision has to keep, on the connection
    // that reads the narrowed row: the administrative surface is refused with
    // the capability's own name, and each refusal leaves one denied audit row.
    let narrowed = NoisePeer::connect(&address_a, &b.static_private())
        .expect("the narrowed peer completes the Noise handshake");
    narrowed
        .hello()
        .expect("the hello inside Noise is accepted");
    for request in [
        ClientMessage::Status { id: 101 },
        ClientMessage::Shutdown { id: 102 },
    ] {
        let name = request.name().to_string();
        match narrowed.request(request) {
            Ok(DaemonMessage::Error(error)) => {
                assert_eq!(
                    error.code,
                    ErrorCode::CapabilityNotSupported,
                    "{name} must be refused without `admin`"
                );
                assert_eq!(
                    error.message, "capability 'admin' was not negotiated",
                    "the refusal names the capability the device lacks"
                );
            }
            other => panic!("expected CapabilityNotSupported for {name}, got {other:?}"),
        }
        let rows = a.audit_rows();
        assert!(
            rows.iter()
                .any(|(action, outcome)| action == &name && outcome == "denied"),
            "{name} must leave one denied audit row: {rows:?}"
        );
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
    let after = narrowed.request(ClientMessage::Ping { id: 999 });
    assert!(
        after.is_err(),
        "a revoked peer's connection must be closed, got {after:?}"
    );
    drop(narrowed);
    drop(peer);
    let refused = match NoisePeer::connect(&address_a, &b.static_private()) {
        Err(_) => true,
        Ok(peer) => peer.hello().is_err(),
    };
    assert!(refused, "a revoked peer must not be able to reconnect");

    // ---- re-pairing after a revoke -----------------------------------------
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
    let code = match b.pipe.expect(ClientMessage::PairingStart { id }) {
        DaemonMessage::PairingCode { code, .. } => code,
        other => panic!("expected PairingCode, got {other:?}"),
    };
    let id = a.pipe.id();
    match a.pipe.expect(ClientMessage::PairingComplete {
        id,
        // B displayed the code, so this is B's address.
        address: address_b.clone(),
        code,
    }) {
        DaemonMessage::PairingPending { peer, .. } => {
            assert_eq!(peer.device_id, b_self.device_id);
            assert_eq!(peer.role, None, "a v32 pending row carries no role word");
        }
        other => panic!("expected PairingPending for a roleless pairing, got {other:?}"),
    }
    // B parks the re-pair too, so confirm it there before both rows exist.
    let deadline = Instant::now() + Duration::from_secs(15);
    let parking = loop {
        let pending = match b
            .pipe
            .expect(ClientMessage::DevicesList { id: b.pipe.id() })
        {
            DaemonMessage::Devices { pending, .. } => pending,
            other => panic!("expected Devices, got {other:?}"),
        };
        if !pending.is_empty() {
            break pending;
        }
        assert!(Instant::now() < deadline, "B never parked the re-pair");
        std::thread::sleep(Duration::from_millis(50));
    };
    let id = b.pipe.id();
    match b.pipe.expect(ClientMessage::PairingConfirm {
        id,
        device_id: parking[0].device_id.clone(),
        accept: true,
    }) {
        DaemonMessage::PeerUpdated { .. } => {}
        other => panic!("expected PeerUpdated on confirm, got {other:?}"),
    }
    assert!(
        !wait_for_row_count(&a, 1)[0].1,
        "the re-pair after a revoke keeps no v30 dial hint"
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

/// The outbound dial: two real daemons pair, and then one of them dials the
/// other through the production dial path. The dialer's side of the call is
/// driven with daemon A's own static key and the row A's journal holds for B —
/// exactly the data the state-aware caller resolves at dial time — and B
/// answers through its real accept path: pre-Noise filter, pinned-key lookup,
/// binding check, gate, dispatch.
#[test]
fn a_paired_daemon_dials_its_peer_and_reads_the_reply() {
    let _guard = lock_tests();

    let (port_a, port_b) = two_free_ports();
    let a = Peer::spawn("dial-a", port_a);
    let b = Peer::spawn("dial-b", port_b);

    let (Some(_address_a), Some(address_b)) = (a.peer_address(), b.peer_address()) else {
        eprintln!(
            "SKIP peer_link dial: no reachable tailnet address. A said {}; B said {}.",
            a.remote_label(),
            b.remote_label()
        );
        return;
    };
    let a_self = a.self_info();
    let b_self = b.self_info();

    // The hello a dialing daemon speaks: it names itself the way a remote
    // peer is named on the wire, `peer_<device_id>`. The responder replaces
    // the owner with the identity the Noise handshake authenticated; nothing
    // authorizes on it.
    let dial_hello = ClientHello::peer(
        OwnerId::new(format!("peer_{}", a_self.device_id), "daemon").expect("owner"),
        "devboule-daemon",
        true,
    );

    // ---- pair as Daemon peers: a Daemon pairing needs no confirmation window
    pair_devices(&a, &b, &address_b, &b_self.device_id);

    let (pinned_key, stored_address) = a
        .stored_row_for(&b_self.device_id)
        .expect("A holds a row for B");
    assert_eq!(
        stored_address, address_b,
        "the row names B's tailnet address"
    );
    assert_eq!(pinned_key.len(), 32, "a pinned key is a Noise static key");

    // ---- the happy dial: one request, one reply ----------------------------
    let reply = dial_peer(
        &a.static_private(),
        &pinned_key,
        &stored_address,
        &dial_hello,
        &ClientMessage::SessionsList { id: 7 },
    )
    .unwrap_or_else(|error| {
        panic!(
            "a paired daemon dials its peer: {error}; B stderr: {}",
            b.stderr_contents()
        )
    });
    match reply {
        DaemonMessage::Sessions { id, sessions } => {
            assert_eq!(id, 7, "the reply carries the request id");
            assert!(
                sessions.is_empty(),
                "a fresh Daemon peer has created no sessions on B: {sessions:?}"
            );
        }
        other => panic!("expected Sessions, got {other:?}"),
    }

    // ---- a far end that does not match the pinned key ----------------------
    //
    // The key is flipped, not replaced with garbage, so the failure is the
    // handshake's key check and not a length or encoding refusal. There is no
    // fallback: the dial fails, full stop.
    let mut impostor_key = pinned_key.clone();
    impostor_key[31] ^= 0x01;
    let error = dial_peer(
        &a.static_private(),
        &impostor_key,
        &stored_address,
        &dial_hello,
        &ClientMessage::SessionsList { id: 8 },
    )
    .expect_err("a far end that presents the wrong key must fail the dial");
    assert_eq!(error.step(), "handshake", "{error}");
}

/// The direction yesterday's daemon could not do: the device that **displayed**
/// the code (the responder) dials the device that typed it. The responder's
/// row must record the initiator's advertised listener port — the port on the
/// accepted socket is the initiator's ephemeral source port and belongs to
/// nothing — and the dial through the production path must be answered.
#[test]
fn the_device_that_displayed_the_code_can_dial_its_peer_back() {
    let _guard = lock_tests();

    let (port_a, port_b) = two_free_ports();
    let a = Peer::spawn("back-a", port_a);
    let b = Peer::spawn("back-b", port_b);

    let (Some(address_a), Some(address_b)) = (a.peer_address(), b.peer_address()) else {
        eprintln!(
            "SKIP peer_link dial-back: no reachable tailnet address. A said {}; B said {}.",
            a.remote_label(),
            b.remote_label()
        );
        return;
    };
    let a_self = a.self_info();
    let b_self = b.self_info();

    // B displays the code, A types it: B is the responder.
    pair_devices(&a, &b, &address_b, &b_self.device_id);

    let (pinned_key, stored_address) = b
        .stored_row_for(&a_self.device_id)
        .expect("B holds a row for A");
    assert_eq!(
        stored_address, address_a,
        "the responder's row records the initiator's advertised listener port"
    );

    let dial_hello = ClientHello::peer(
        OwnerId::new(format!("peer_{}", b_self.device_id), "daemon").expect("owner"),
        "devboule-daemon",
        true,
    );
    let reply = dial_peer(
        &b.static_private(),
        &pinned_key,
        &stored_address,
        &dial_hello,
        &ClientMessage::SessionsList { id: 11 },
    )
    .unwrap_or_else(|error| {
        panic!(
            "the responder dials the initiator back: {error}; A stderr: {}",
            a.stderr_contents()
        )
    });
    match reply {
        DaemonMessage::Sessions { id, sessions } => {
            assert_eq!(id, 11, "the reply carries the request id");
            assert!(
                sessions.is_empty(),
                "a fresh Daemon peer has created no sessions on A: {sessions:?}"
            );
        }
        other => panic!("expected Sessions, got {other:?}"),
    }
}

/// A dial refuses an address that is not on the tailnet before anything
/// connects. The address comes out of a stored row, and a stored row is data
/// that could be wrong — so the dialer enforces the same footing the pairing
/// initiator does, on the way out. The decoy listener proves the refusal: a
/// dial that tried to connect would be accepted by it.
#[test]
fn a_dial_refuses_an_address_that_is_not_on_the_tailnet() {
    let _guard = lock_tests();

    let (port_a, port_b) = two_free_ports();
    let a = Peer::spawn("refuse-a", port_a);
    let b = Peer::spawn("refuse-b", port_b);

    let (Some(_address_a), Some(address_b)) = (a.peer_address(), b.peer_address()) else {
        eprintln!(
            "SKIP peer_link dial refusal: no reachable tailnet address. A said {}; B said {}.",
            a.remote_label(),
            b.remote_label()
        );
        return;
    };
    let a_self = a.self_info();
    let b_self = b.self_info();
    pair_devices(&a, &b, &address_b, &b_self.device_id);
    let (pinned_key, _stored_address) = a
        .stored_row_for(&b_self.device_id)
        .expect("A holds a row for B");
    let dial_hello = ClientHello::peer(
        OwnerId::new(format!("peer_{}", a_self.device_id), "daemon").expect("owner"),
        "devboule-daemon",
        true,
    );

    // A loopback address is well formed and something is even listening there
    // — and the dial must still refuse it, because it is not a tailnet
    // address. Whatever this machine's tailscaled offers, a stored row never
    // gets to point the daemon off the tailnet.
    let decoy = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the decoy listener");
    let decoy_port = decoy.local_addr().expect("decoy address").port();
    let error = dial_peer(
        &a.static_private(),
        &pinned_key,
        &format!("127.0.0.1:{decoy_port}"),
        &dial_hello,
        &ClientMessage::SessionsList { id: 9 },
    )
    .expect_err("a non-tailnet address must be refused before anything connects");
    assert_eq!(error.step(), "address", "{error}");
    decoy
        .set_nonblocking(true)
        .expect("decoy goes non-blocking");
    match decoy.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        accepted => panic!("the refused dial must not have connected anywhere: {accepted:?}"),
    }
}

/// The point of the slice, over two real daemons: A dials B with the frame
/// `devboule_send_message` sends for a paired device, B delivers the envelope
/// into a live session and answers `accepted`, and the same idempotency key a
/// second time is answered from the first receipt without a second delivery.
#[test]
fn a_paired_daemon_delivers_an_agent_message_and_replays_its_receipt_once() {
    let _guard = lock_tests();

    let (port_a, port_b) = two_free_ports();
    let a = Peer::spawn("send-a", port_a);
    let stub = stub_command();
    let b = Peer::spawn_with_env(
        "send-b",
        port_b,
        &[
            ("DEVBOULE_ACP_COMMAND", stub.as_str()),
            ("DEVBOULE_ACP_PROVIDER_ID", "devboule-acp-stub"),
        ],
    );

    let (Some(_address_a), Some(address_b)) = (a.peer_address(), b.peer_address()) else {
        eprintln!(
            "SKIP peer_link send: no reachable tailnet address. A said {}; B said {}.",
            a.remote_label(),
            b.remote_label()
        );
        return;
    };
    let a_self = a.self_info();
    let b_self = b.self_info();
    pair_devices(&a, &b, &address_b, &b_self.device_id);

    // ---- B grows a live agent the paired daemon may write into -------------
    // The wire refuses an agent with no workspace, so B gets a scratch one.
    let project_dir = devboule_daemon::test_dirs::test_temp_dir("peer-link workspace");
    std::fs::create_dir_all(&project_dir).expect("B's project dir");
    let project = match b.pipe.expect(ClientMessage::ProjectAdd {
        id: b.pipe.id(),
        path: project_dir.to_string_lossy().into_owned(),
    }) {
        DaemonMessage::Project { project, .. } => project,
        other => panic!("B must add its project: {other:?}"),
    };
    let workspace = match b.pipe.expect(ClientMessage::WorkspaceCreate {
        id: b.pipe.id(),
        project_id: project.id.clone(),
        isolation: WorkspaceIsolation::Local,
        branch: None,
    }) {
        DaemonMessage::Workspace { workspace, .. } => workspace,
        other => panic!("B must create its workspace: {other:?}"),
    };
    let created = b.pipe.expect(ClientMessage::SessionCreate {
        id: b.pipe.id(),
        workspace_id: Some(workspace.id.clone()),
        kind: SessionKind::Acp,
        provider: Some("devboule-acp-stub".to_string()),
        mode: None,
        display_name: None,
        idempotency_key: None,
        cols: None,
        rows: None,
    });
    let session_id = match created {
        DaemonMessage::Session { session, .. } => session.id,
        other => panic!(
            "expected Session on create, got {other:?}; B stderr: {}",
            b.stderr_contents()
        ),
    };

    // ---- watch the session's own event stream ------------------------------
    const SUBSCRIPTION: u64 = 1;
    match b.pipe.expect(ClientMessage::SessionAttach {
        id: b.pipe.id(),
        session_id: session_id.clone(),
        subscription_id: SUBSCRIPTION,
        from_cursor: None,
    }) {
        DaemonMessage::SessionAttached { .. } => {}
        other => panic!("expected SessionAttached, got {other:?}"),
    }

    // ---- A sends, then sends the same key again ----------------------------
    let (pinned_key, stored_address) = a
        .stored_row_for(&b_self.device_id)
        .expect("A holds a row for B");
    let dial_hello = ClientHello::peer(
        OwnerId::new(format!("peer_{}", a_self.device_id), "daemon").expect("owner"),
        "devboule-daemon",
        true,
    );
    for _ in 0..2 {
        let reply = dial_peer(
            &a.static_private(),
            &pinned_key,
            &stored_address,
            &dial_hello,
            &ClientMessage::AgentMessageSend {
                id: 0,
                from_session: "s.far.source".to_string(),
                to_session: session_id.clone(),
                text: "hello from the other machine".to_string(),
                idempotency_key: Some("peer-link-once".to_string()),
            },
        )
        .unwrap_or_else(|error| {
            panic!(
                "a paired daemon sends: {error}; B stderr: {}",
                b.stderr_contents()
            )
        });
        match reply {
            DaemonMessage::AgentMessageReceipt {
                state: AgentMessageState::Accepted,
                ..
            } => {}
            other => panic!(
                "expected an accepted receipt, got {other:?}; B stderr: {}",
                b.stderr_contents()
            ),
        }
    }

    // ---- the receiver published the incoming a2a envelope ------------------
    let delivered = b
        .pipe
        .recv_until(
            Instant::now() + Duration::from_secs(10),
            |frame| match frame {
                DaemonMessage::SubscriptionEvent {
                    subscription_id,
                    envelope,
                } if *subscription_id == SUBSCRIPTION => match &envelope.event {
                    SessionEvent::AgentUserMessage {
                        text, message_kind, ..
                    } if *message_kind == UserMessageKind::IncomingA2a => Some(text.clone()),
                    _ => None,
                },
                _ => None,
            },
        )
        .unwrap_or_else(|| {
            panic!(
                "B never published the incoming a2a; B stderr: {}",
                b.stderr_contents()
            )
        });
    assert!(
        delivered.contains("from_agent: peer:") && delivered.contains("/s.far.source"),
        "the envelope names the authenticated device and the far label: {delivered}"
    );

    // A second delivery would be a second `agent_user_message` report in B's
    // journal; the replay above must leave it at one. The journal writer is a
    // queue, so the count is read once the row appears and again after the
    // queue has had time to drain: both reads must say one.
    let count_incoming = || -> i64 {
        let connection = rusqlite::Connection::open(b.dir.join("journal.db")).expect("journal");
        connection
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id = ?1 AND kind = 'agent_report' \
                 AND CAST(payload AS TEXT) LIKE '%\"incoming_a2a\"%'",
                [&session_id],
                |row| row.get(0),
            )
            .expect("count B's incoming a2a rows")
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while count_incoming() == 0 {
        assert!(
            Instant::now() < deadline,
            "B never journaled the incoming a2a"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(count_incoming(), 1, "the replay is one delivery, not two");
}

/// One daemon creates a project and a workspace through its pipe, and the two
/// ids come back for the remote reads to look for.
fn create_project_and_workspace(peer: &Peer, tag: &str) -> (String, String) {
    let dir = peer.dir.join(format!("project-{tag}"));
    std::fs::create_dir_all(&dir).expect("project dir");
    let id = peer.pipe.id();
    let project = request_skipping_pushes(
        peer,
        ClientMessage::ProjectAdd {
            id,
            path: dir.to_string_lossy().into_owned(),
        },
        |frame| match frame {
            DaemonMessage::Project { project, .. } => Some(project.clone()),
            _ => None,
        },
    )
    .expect("the project row");
    let id = peer.pipe.id();
    let workspace = request_skipping_pushes(
        peer,
        ClientMessage::WorkspaceCreate {
            id,
            project_id: project.id.clone(),
            isolation: WorkspaceIsolation::Local,
            branch: None,
        },
        |frame| match frame {
            DaemonMessage::Workspace { workspace, .. } => Some(workspace.clone()),
            _ => None,
        },
    )
    .expect("the workspace row");
    (project.id, workspace.id)
}

/// Wait for a daemon's listener to be bound, then hand back its advertised
/// `ip:port`. The listener starts on its own thread at boot, so a test that
/// reads `SelfInfo` immediately after the pipe opens can see the port before
/// the bind lands.
fn wait_until_listening(peer: &Peer) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(address) = peer.peer_address() {
            return address;
        }
        assert!(
            Instant::now() < deadline,
            "the listener never came up: {}",
            peer.remote_label()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One peer row from this daemon's own DevicesList, or `None` when the row is
/// gone.
fn peer_row(peer: &Peer, device_id: &str) -> Option<devboule_protocol::PeerRow> {
    let id = peer.pipe.id();
    let peers = request_skipping_pushes(
        peer,
        ClientMessage::DevicesList { id },
        |frame| match frame {
            DaemonMessage::Devices { peers, .. } => Some(peers.clone()),
            _ => None,
        },
    )
    .expect("the device list");
    peers.into_iter().find(|row| row.device_id == device_id)
}

/// The loopback two-daemon test the design asks for: two real daemons on one
/// machine with separate data directories, identities and loopback ports,
/// paired, each watching the other. Each side reads the other's workspace
/// through the held link, each Devices panel reports the other online, a
/// workspace change is pushed with its revision, and dropping both links turns
/// both offline. No Tailscale, and it never skips: a failure to bind loopback
/// is a failure, not an excuse.
/// Send one request and read until `wanted` accepts a frame. A connection
/// that watches a host is interleaved with `RemoteHostStatus` pushes, so a
/// test that expects one reply must skip them rather than assume the next
/// frame is its answer.
fn request_skipping_pushes<T>(
    peer: &Peer,
    message: ClientMessage,
    wanted: impl FnMut(&DaemonMessage) -> Option<T>,
) -> Option<T> {
    peer.pipe.framed.send(&message).ok()?;
    peer.pipe
        .recv_until(Instant::now() + Duration::from_secs(30), wanted)
}

#[test]
fn two_daemons_on_loopback_see_each_others_workspaces_and_presence() {
    let _guard = lock_tests();

    let (port_a, port_b) = two_free_ports();
    let a = Peer::spawn_with_env("loop-a", port_a, &[("DEVBOULE_PEER_LOOPBACK", "1")]);
    let b = Peer::spawn_with_env("loop-b", port_b, &[("DEVBOULE_PEER_LOOPBACK", "1")]);
    let _address_a = wait_until_listening(&a);
    let address_b = wait_until_listening(&b);
    let a_self = a.self_info();
    let b_self = b.self_info();
    assert_ne!(a_self.device_id, b_self.device_id, "distinct identities");

    // ---- pair, confirmed on the displaying device --------------------------
    pair_devices(&a, &b, &address_b, &b_self.device_id);

    // ---- each creates a project and a workspace ---------------------------
    // Before either watches: a client that has just become a host is exactly
    // the device the links must discover, and no link exists yet to carry the
    // notification — which is why the dial cannot be gated on the record.
    let (a_project, a_workspace) = create_project_and_workspace(&a, "a");
    let (b_project, b_workspace) = create_project_and_workspace(&b, "b");

    // ---- each device watches the other ------------------------------------
    for (peer, other) in [(&a, &b_self.device_id), (&b, &a_self.device_id)] {
        let id = peer.pipe.id();
        let reply = request_skipping_pushes(
            peer,
            ClientMessage::RemoteHostWatch {
                id,
                device_id: other.clone(),
            },
            |frame| match frame {
                DaemonMessage::Ok { .. } => Some(()),
                _ => None,
            },
        );
        assert!(reply.is_some(), "the watch to {other} never answered");
    }
    for (peer, other) in [(&a, &b_self.device_id), (&b, &a_self.device_id)] {
        let online =
            peer.pipe.recv_until(
                Instant::now() + Duration::from_secs(30),
                |frame| match frame {
                    DaemonMessage::RemoteHostStatus {
                        device_id, state, ..
                    } if device_id == other
                        && *state == devboule_protocol::RemoteHostState::Online =>
                    {
                        Some(())
                    }
                    _ => None,
                },
            );
        assert!(online.is_some(), "the link to {other} never came online");
    }

    // ---- a change while the link is up reaches the watcher with its revision
    let (_b_project_two, _b_workspace_two) = create_project_and_workspace(&b, "b2");
    let revision = a.pipe.recv_until(
        Instant::now() + Duration::from_secs(30),
        |frame| match frame {
            DaemonMessage::RemoteHostStatus {
                device_id,
                revision: Some(revision),
                ..
            } if device_id == &b_self.device_id => Some(*revision),
            _ => None,
        },
    );
    assert!(
        revision.is_some(),
        "the host's workspace revision never reached the watcher
--- A stderr ---
{}
--- B stderr ---
{}",
        a.stderr_contents(),
        b.stderr_contents()
    );

    // ---- each reads the other's project and workspace over the link -------
    for (peer, other, project_id, workspace_id) in [
        (&a, &b_self.device_id, &b_project, &b_workspace),
        (&b, &a_self.device_id, &a_project, &a_workspace),
    ] {
        let id = peer.pipe.id();
        let projects = request_skipping_pushes(
            peer,
            ClientMessage::RemoteHostList {
                id,
                device_id: other.clone(),
                list: devboule_protocol::RemoteHostList::Projects,
            },
            |frame| match frame {
                DaemonMessage::RemoteHostList {
                    body: devboule_protocol::RemoteHostListBody::Projects { rows },
                    ..
                } => Some(rows.clone()),
                _ => None,
            },
        )
        .expect("the host's projects");
        assert!(
            projects.iter().any(|project| project.id == *project_id),
            "the host's project is in its own list: {projects:?}"
        );
        let id = peer.pipe.id();
        let workspaces = request_skipping_pushes(
            peer,
            ClientMessage::RemoteHostList {
                id,
                device_id: other.clone(),
                list: devboule_protocol::RemoteHostList::Workspaces {
                    project_id: project_id.clone(),
                },
            },
            |frame| match frame {
                DaemonMessage::RemoteHostList {
                    body: devboule_protocol::RemoteHostListBody::Workspaces { rows },
                    ..
                } => Some(rows.clone()),
                _ => None,
            },
        )
        .expect("the host's workspaces");
        assert!(
            workspaces
                .iter()
                .any(|workspace| workspace.id == *workspace_id),
            "the host's workspace is in its own list: {workspaces:?}"
        );
    }

    // ---- both Devices panels read the other online ------------------------
    for (peer, other) in [(&a, &b_self.device_id), (&b, &a_self.device_id)] {
        let row = peer_row(peer, other).expect("the peer row");
        assert!(
            row.online,
            "the Devices panel must read the other device online: {row:?}"
        );
        assert!(
            row.hosts_workspaces,
            "the peer hosts its own workspace and the record must say so"
        );
    }

    // ---- drop both links: the idle grace closes them, both read offline ---
    for (peer, other) in [(&a, &b_self.device_id), (&b, &a_self.device_id)] {
        let id = peer.pipe.id();
        let reply = request_skipping_pushes(
            peer,
            ClientMessage::RemoteHostUnwatch {
                id,
                device_id: other.clone(),
            },
            |frame| match frame {
                DaemonMessage::Ok { .. } => Some(()),
                _ => None,
            },
        );
        assert!(reply.is_some(), "the unwatch to {other} never answered");
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let a_online = peer_row(&a, &b_self.device_id).is_some_and(|row| row.online);
        let b_online = peer_row(&b, &a_self.device_id).is_some_and(|row| row.online);
        if !a_online && !b_online {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the links never closed: A sees B online={a_online}, B sees A online={b_online}"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Slice 3 end to end: a client device opens a remote agent's stream, sees its
/// events tagged with the host, keeps the stream while the host is away, and
/// resumes on a fresh subscription when the host comes back.
///
/// Only B hosts here. That is not an accident of the harness: a machine peer's
/// session scope is the sessions it created on the host, while a client's is
/// the paired user's — and opening the host's own agents is the client's act.
#[test]
fn a_client_attaches_to_a_remote_terminal_and_resumes_after_the_host_returns() {
    let _guard = lock_tests();

    let (port_a, port_b) = two_free_ports();
    let a = Peer::spawn_with_env("s3-attach-a", port_a, &[("DEVBOULE_PEER_LOOPBACK", "1")]);
    let b = Peer::spawn_with_env("s3-attach-b", port_b, &[("DEVBOULE_PEER_LOOPBACK", "1")]);
    let _address_a = wait_until_listening(&a);
    let address_b = wait_until_listening(&b);
    let a_self = a.self_info();
    let b_self = b.self_info();
    pair_devices(&a, &b, &address_b, &b_self.device_id);

    // B hosts a workspace and a terminal in it; A stays a client.
    let (_b_project, b_workspace) = create_project_and_workspace(&b, "b");
    let session = match b.pipe.expect(ClientMessage::SessionCreate {
        id: b.pipe.id(),
        workspace_id: Some(b_workspace),
        kind: SessionKind::Terminal,
        provider: None,
        mode: None,
        display_name: None,
        idempotency_key: None,
        cols: Some(80),
        rows: Some(24),
    }) {
        DaemonMessage::Session { session, .. } => session,
        other => panic!("expected Session, got {other:?}"),
    };

    // A watches B and opens the terminal's stream.
    let watch = request_skipping_pushes(
        &a,
        ClientMessage::RemoteHostWatch {
            id: a.pipe.id(),
            device_id: b_self.device_id.clone(),
        },
        |frame| match frame {
            DaemonMessage::Ok { .. } => Some(()),
            _ => None,
        },
    );
    assert!(watch.is_some(), "the watch never answered");
    let online = a.pipe.recv_until(
        Instant::now() + Duration::from_secs(30),
        |frame| match frame {
            DaemonMessage::RemoteHostStatus {
                device_id, state, ..
            } if device_id == &b_self.device_id
                && *state == devboule_protocol::RemoteHostState::Online =>
            {
                Some(())
            }
            _ => None,
        },
    );
    assert!(online.is_some(), "the link to B never came online");

    let attach = request_skipping_pushes(
        &a,
        ClientMessage::RemoteHostAttach {
            id: a.pipe.id(),
            device_id: b_self.device_id.clone(),
            session_id: session.id.clone(),
            subscription_id: 1,
        },
        |frame| match frame {
            DaemonMessage::Ok { .. } => Some("ok".to_string()),
            DaemonMessage::Error(error) => Some(error.message.clone()),
            _ => None,
        },
    );
    assert_eq!(attach.as_deref(), Some("ok"), "the attach must be accepted");

    // The transcript replays, tagged with B's device id and the subscription.
    let replayed = a.pipe.recv_until(
        Instant::now() + Duration::from_secs(30),
        |frame| match frame {
            DaemonMessage::RemoteHostEvent {
                device_id,
                session_id,
                subscription_id,
                envelope,
            } if device_id == &b_self.device_id
                && session_id == &session.id
                && *subscription_id == 1 =>
            {
                Some(envelope.clone())
            }
            _ => None,
        },
    );
    assert!(
        replayed.is_some(),
        "the remote event never reached the attaching client"
    );

    // The host goes away: give the lease back and wait for the row to say so.
    let unwatch = request_skipping_pushes(
        &a,
        ClientMessage::RemoteHostUnwatch {
            id: a.pipe.id(),
            device_id: b_self.device_id.clone(),
        },
        |frame| match frame {
            DaemonMessage::Ok { .. } => Some(()),
            _ => None,
        },
    );
    assert!(unwatch.is_some(), "the unwatch never answered");
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let online = peer_row(&a, &b_self.device_id).is_some_and(|row| row.online);
        if !online {
            break;
        }
        assert!(Instant::now() < deadline, "B never went offline");
        std::thread::sleep(Duration::from_millis(500));
    }

    // A fresh attach while the host is away is refused, not queued.
    let refused = request_skipping_pushes(
        &a,
        ClientMessage::RemoteHostAttach {
            id: a.pipe.id(),
            device_id: b_self.device_id.clone(),
            session_id: session.id.clone(),
            subscription_id: 2,
        },
        |frame| match frame {
            DaemonMessage::Ok { .. } => Some("ok".to_string()),
            DaemonMessage::Error(..) => Some("error".to_string()),
            _ => None,
        },
    );
    assert_eq!(
        refused.as_deref(),
        Some("error"),
        "an offline host cannot be attached"
    );

    // The host comes back: watch again and reattach with a fresh subscription,
    // which replays the transcript for the new stream.
    let watch = request_skipping_pushes(
        &a,
        ClientMessage::RemoteHostWatch {
            id: a.pipe.id(),
            device_id: b_self.device_id.clone(),
        },
        |frame| match frame {
            DaemonMessage::Ok { .. } => Some(()),
            _ => None,
        },
    );
    assert!(watch.is_some(), "the second watch never answered");
    let online = a.pipe.recv_until(
        Instant::now() + Duration::from_secs(30),
        |frame| match frame {
            DaemonMessage::RemoteHostStatus {
                device_id, state, ..
            } if device_id == &b_self.device_id
                && *state == devboule_protocol::RemoteHostState::Online =>
            {
                Some(())
            }
            _ => None,
        },
    );
    assert!(online.is_some(), "the link to B never came back");

    let attach = request_skipping_pushes(
        &a,
        ClientMessage::RemoteHostAttach {
            id: a.pipe.id(),
            device_id: b_self.device_id.clone(),
            session_id: session.id.clone(),
            subscription_id: 2,
        },
        |frame| match frame {
            DaemonMessage::Ok { .. } => Some("ok".to_string()),
            DaemonMessage::Error(error) => Some(error.message.clone()),
            _ => None,
        },
    );
    assert_eq!(
        attach.as_deref(),
        Some("ok"),
        "the reattach must be accepted"
    );

    let resumed = a.pipe.recv_until(
        Instant::now() + Duration::from_secs(30),
        |frame| match frame {
            DaemonMessage::RemoteHostEvent {
                device_id,
                session_id,
                subscription_id,
                ..
            } if device_id == &b_self.device_id
                && session_id == &session.id
                && *subscription_id == 2 =>
            {
                Some(())
            }
            _ => None,
        },
    );
    assert!(
        resumed.is_some(),
        "the reattached stream never replayed the transcript"
    );

    let _ = a_self;
}
