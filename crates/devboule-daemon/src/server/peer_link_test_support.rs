//! A loopback Noise responder that keeps serving, so a held link's tests have
//! something to hold a link to. Test-only weight, kept out of the production
//! files: the handshake helper it reuses is `peer_transport`'s, not a copy.

use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_protocol::{
    Capability, ClientMessage, DaemonHello, DaemonMessage, Project, SessionEvent,
    SessionEventEnvelope, WireError, Workspace, WorkspaceIsolation, PROTOCOL_MIN_VERSION,
    PROTOCOL_VERSION,
};

use crate::framing::Framed;
use crate::journal::PeerRecord;
use crate::peer_transport::{
    responder_handshake, split_session, HANDSHAKE_DEADLINE, PEER_NOISE_PATTERN, PEER_PROLOGUE,
};

/// One X25519 keypair: the responder holds the private half and the stored row
/// pins the public half, which is the shape a real pairing writes.
pub(crate) fn pinned_keypair() -> snow::Keypair {
    snow::Builder::new(PEER_NOISE_PATTERN.parse().expect("pattern"))
        .generate_keypair()
        .expect("keypair")
}

/// The `peers` row a held link dials through.
pub(crate) fn host_row(address: String, pinned_public: &[u8]) -> PeerRecord {
    PeerRecord {
        device_id: "b".to_string(),
        display_name: "peer b".to_string(),
        legacy_dialable: true,
        hosts_workspaces: true,
        public_key: pinned_public.to_vec(),
        paired_by_user: None,
        binding_kind: "tailscale".to_string(),
        binding_stable_id: None,
        binding_node_name: None,
        binding_login_name: None,
        address,
        paired_at: 0,
        revoked_at: None,
        caps: vec!["view".to_string(), "admin".to_string()],
    }
}

/// The far end of a held link, and the knobs a test turns on it.
pub(crate) struct Responder {
    pub(crate) address: SocketAddr,
    /// How many Noise handshakes really completed. One dial per reconnect is
    /// what proves a reconnect did or did not happen.
    pub(crate) handshakes: Arc<AtomicUsize>,
    /// How many accepted sockets reached their end: one close per link close.
    pub(crate) closes: Arc<AtomicUsize>,
    /// How many `Hello` frames arrived, counted before the hello answer is
    /// delayed. A test uses it to know the worker is inside its handshake.
    pub(crate) hellos: Arc<AtomicUsize>,
    pub(crate) answers_pings: Arc<AtomicBool>,
    /// Takes a list request and does not answer it, so a read parks in the
    /// worker. This is how a test reaches the link's one-read-at-a-time bound.
    pub(crate) holds_reads: Arc<AtomicBool>,
    hello_delay_ms: Arc<AtomicU64>,
    /// Answers the first read with a reply carrying **another** request's id,
    /// then the right one: a link that returned it would hand a caller a
    /// stranger's rows.
    answer_with_wrong_id: Arc<AtomicBool>,
    /// The refusal every read gets, once set: the far side saying no, with its
    /// own reason.
    pub(crate) refusal: Arc<Mutex<Option<WireError>>>,
    /// Frames the far side pushes on its own, drained on the serve loop's next
    /// poll. This is how a test makes the host announce a workspace change.
    pushes: Arc<Mutex<Vec<DaemonMessage>>>,
    stop: mpsc::Sender<()>,
}

impl Responder {
    pub(crate) fn set_refusal(&self, refusal: WireError) {
        *self
            .refusal
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(refusal);
    }

    /// Hold the daemon's hello answer for `delay`, so the worker is provably
    /// still inside its handshake while a test queues a read.
    pub(crate) fn hold_hello(&self, delay: Duration) {
        self.hello_delay_ms
            .store(delay.as_millis() as u64, Ordering::SeqCst);
    }

    pub(crate) fn answer_with_wrong_id(&self) {
        self.answer_with_wrong_id.store(true, Ordering::SeqCst);
    }

    pub(crate) fn hold_reads(&self) {
        self.holds_reads.store(true, Ordering::SeqCst);
    }

    pub(crate) fn stop(&self) {
        let _ = self.stop.send(());
    }

    /// Queue one relayed session event for the serve loop to send on the
    /// subscription the attach opened.
    pub(crate) fn push_session_event(&self, subscription_id: u64, data: &str) {
        self.pushes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(DaemonMessage::SubscriptionEvent {
                subscription_id,
                envelope: SessionEventEnvelope {
                    session_id: "session-1".to_string(),
                    generation: 1,
                    transcript_seq: None,
                    event: SessionEvent::Output {
                        seq: 1,
                        data: data.to_string(),
                    },
                },
            });
    }

    /// Queue one workspace-change push for the serve loop to send.
    pub(crate) fn push_workspace_changed(&self, revision: u64) {
        self.push_workspace_changed_from("b", revision);
    }

    /// The same push naming another device, for the frames a receiver must
    /// drop rather than act on.
    pub(crate) fn push_workspace_changed_from(&self, device_id: &str, revision: u64) {
        self.pushes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(DaemonMessage::HostWorkspaceChanged {
                device_id: device_id.to_string(),
                revision,
            });
    }
}

/// Start a responder that serves every request on one handshake until stopped.
///
/// `capabilities` is what the far daemon advertises, so a test can offer a
/// daemon that predates a frame and watch the link refuse before the request
/// leaves.
pub(crate) fn spawn(
    static_private: [u8; 32],
    capabilities: Vec<Capability>,
) -> (Responder, mpsc::Receiver<ClientMessage>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the responder");
    let address = listener.local_addr().expect("responder address");
    let handshakes = Arc::new(AtomicUsize::new(0));
    let closes = Arc::new(AtomicUsize::new(0));
    let hellos = Arc::new(AtomicUsize::new(0));
    let answers_pings = Arc::new(AtomicBool::new(true));
    let holds_reads = Arc::new(AtomicBool::new(false));
    let hello_delay_ms = Arc::new(AtomicU64::new(0));
    let answer_with_wrong_id = Arc::new(AtomicBool::new(false));
    let refusal = Arc::new(Mutex::new(None));
    let pushes = Arc::new(Mutex::new(Vec::new()));
    let (requests_tx, requests_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel();
    let knobs = (
        Arc::clone(&handshakes),
        Arc::clone(&closes),
        Arc::clone(&hellos),
        Arc::clone(&answers_pings),
        Arc::clone(&holds_reads),
        Arc::clone(&hello_delay_ms),
        Arc::clone(&answer_with_wrong_id),
        Arc::clone(&refusal),
        Arc::clone(&pushes),
    );
    std::thread::spawn(move || {
        // One connection and no more. A held link is one handshake, and a reconnect
        // is visible as a refused dial rather than a second one: the listener
        // goes away when the first socket does, so a link that closed cannot
        // quietly reappear on the next one.
        if let Ok((stream, _)) = listener.accept() {
            let Ok(session) = responder_handshake(
                &stream,
                Instant::now() + HANDSHAKE_DEADLINE,
                &static_private,
                PEER_PROLOGUE,
                None,
                PEER_NOISE_PATTERN,
            ) else {
                return;
            };
            let (
                thread_handshakes,
                thread_closes,
                thread_hellos,
                thread_answers_pings,
                thread_holds_reads,
                thread_hello_delay,
                thread_wrong_id,
                thread_refusal,
                thread_pushes,
            ) = knobs.clone();
            thread_handshakes.fetch_add(1, Ordering::SeqCst);
            let Ok((reader, writer, closer)) = split_session(&stream, session) else {
                return;
            };
            let framed = Framed::from_stream(reader, writer, closer);
            let Ok(ClientMessage::Hello(_)) = framed.recv_timeout(Duration::from_secs(10)) else {
                return;
            };
            thread_hellos.fetch_add(1, Ordering::SeqCst);
            let delay = Duration::from_millis(thread_hello_delay.load(Ordering::SeqCst));
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
            if framed
                .send(&DaemonMessage::Hello(DaemonHello {
                    protocol_version: PROTOCOL_VERSION,
                    min_protocol_version: PROTOCOL_MIN_VERSION,
                    daemon_version: "test".to_string(),
                    instance_id: "held-link-responder".to_string(),
                    pid: std::process::id(),
                    capabilities: capabilities.clone(),
                    workspace_host: None,
                }))
                .is_err()
            {
                return;
            }
            serve(
                &framed,
                &requests_tx,
                &Serving {
                    refusal: Arc::clone(&thread_refusal),
                    answers_pings: Arc::clone(&thread_answers_pings),
                    holds_reads: Arc::clone(&thread_holds_reads),
                    wrong_id: Arc::clone(&thread_wrong_id),
                    pushes: Arc::clone(&thread_pushes),
                },
                &stop_rx,
            );
            thread_closes.fetch_add(1, Ordering::SeqCst);
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    });
    (
        Responder {
            address,
            handshakes,
            closes,
            hellos,
            answers_pings,
            holds_reads,
            hello_delay_ms,
            answer_with_wrong_id,
            refusal,
            pushes,
            stop: stop_tx,
        },
        requests_rx,
    )
}

/// Whether a failed read means "nothing arrived yet" rather than "the far end
/// is gone".
///
/// A Noise stream carries its deadline on the socket, so its timeout arrives as
/// an `io::Error` of kind `TimedOut`/`WouldBlock`; a pipe reports the same
/// thing as the daemon's own `TimedOut`. Both are an idle poll and the loop
/// goes round again; anything else is a hang-up, which is how a test sees a
/// link close.
fn idle_poll(error: &crate::error::DaemonError) -> bool {
    match error {
        crate::error::DaemonError::TimedOut(_) => true,
        crate::error::DaemonError::Io(io) => matches!(
            io.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ),
        _ => false,
    }
}

/// The knobs the serve loop reads. Bundled so the loop's signature stays one
/// frame, one request channel, one knob set, one stop channel.
struct Serving {
    refusal: Arc<Mutex<Option<WireError>>>,
    answers_pings: Arc<AtomicBool>,
    holds_reads: Arc<AtomicBool>,
    wrong_id: Arc<AtomicBool>,
    pushes: Arc<Mutex<Vec<DaemonMessage>>>,
}

/// The serving loop. A `Ping` is answered only while `answers_pings` is set,
/// which is how a test makes a host stop answering without stopping the socket.
///
/// The read error is the loop's other exit: an idle poll goes round again,
/// while a hang-up ends it — and that end is what a test observes as the link's
/// close.
fn serve(
    framed: &Framed,
    requests: &mpsc::Sender<ClientMessage>,
    knobs: &Serving,
    stop: &mpsc::Receiver<()>,
) {
    loop {
        if stop.try_recv().is_ok() {
            return;
        }
        let queued = {
            let mut held = knobs
                .pushes
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            std::mem::take(&mut *held)
        };
        for frame in queued {
            if framed.send(&frame).is_err() {
                return;
            }
        }
        let message = match framed.recv_timeout::<ClientMessage>(Duration::from_millis(5)) {
            Ok(message) => message,
            Err(ref error) if idle_poll(error) => continue,
            Err(_) => return,
        };
        match message {
            ClientMessage::Ping { id } => {
                if knobs.answers_pings.load(Ordering::SeqCst) {
                    let _ = framed.send(&DaemonMessage::Pong { id, ts_ms: 1 });
                }
            }
            ClientMessage::SessionAttach {
                id,
                subscription_id,
                ..
            } => {
                if requests.send(message.clone()).is_err() {
                    return;
                }
                if framed
                    .send(&DaemonMessage::SessionAttached {
                        id,
                        subscription_id,
                        resume: None,
                    })
                    .is_err()
                {
                    return;
                }
                // One replayed event proves the relay; a test can push more.
                let _ = framed.send(&DaemonMessage::SubscriptionEvent {
                    subscription_id,
                    envelope: SessionEventEnvelope {
                        session_id: "session-1".to_string(),
                        generation: 1,
                        transcript_seq: None,
                        event: SessionEvent::Output {
                            seq: 1,
                            data: "replayed".to_string(),
                        },
                    },
                });
            }
            ClientMessage::SessionDetach { id, .. } => {
                if requests.send(message.clone()).is_err() {
                    return;
                }
                if framed.send(&DaemonMessage::Ok { id }).is_err() {
                    return;
                }
            }
            request => {
                if requests.send(request.clone()).is_err() {
                    return;
                }
                let Some(id) = request_id(&request) else {
                    return;
                };
                if knobs.holds_reads.load(Ordering::SeqCst) {
                    continue;
                }
                // One frame that belongs to another request goes out first: the
                // link must drop it rather than hand it to this caller.
                if knobs.wrong_id.swap(false, Ordering::SeqCst) {
                    let _ = framed.send(&list_reply(&request, id + 1_000));
                }
                let refusal = knobs
                    .refusal
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                let reply = match refusal {
                    Some(error) => DaemonMessage::Error(error.with_id(id)),
                    None => match list_reply(&request, id) {
                        Some(reply) => reply,
                        None => return,
                    },
                };
                if framed.send(&reply).is_err() {
                    return;
                }
            }
        }
    }
}

fn request_id(request: &ClientMessage) -> Option<u64> {
    match request {
        ClientMessage::ProjectsList { id }
        | ClientMessage::WorkspacesList { id, .. }
        | ClientMessage::SessionsList { id } => Some(*id),
        _ => None,
    }
}

fn list_reply(request: &ClientMessage, id: u64) -> Option<DaemonMessage> {
    Some(match request {
        ClientMessage::ProjectsList { .. } => DaemonMessage::Projects {
            id,
            projects: vec![Project {
                id: "far-project".to_string(),
                name: "far project".to_string(),
                path: "C:/far".to_string(),
            }],
        },
        ClientMessage::WorkspacesList { project_id, .. } => DaemonMessage::Workspaces {
            id,
            workspaces: vec![Workspace {
                id: "far-workspace".to_string(),
                project_id: project_id.clone(),
                title: "far workspace".to_string(),
                isolation: WorkspaceIsolation::Local,
                path: "C:/far/ws".to_string(),
            }],
        },
        ClientMessage::SessionsList { .. } => DaemonMessage::Sessions {
            id,
            sessions: Vec::new(),
        },
        _ => return None,
    })
}

/// Wait for `check` to hold, or fail with what it was still waiting for.
pub(crate) fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("{what} never happened");
}

/// The capabilities a current daemon advertises: the link refuses a read the
/// far end never agreed to speak.
pub(crate) fn current_capabilities() -> Vec<Capability> {
    use devboule_protocol::caps;
    vec![
        Capability::new(caps::PING),
        Capability::new(caps::SESSIONS),
        Capability::new(caps::JOURNAL),
        Capability::new(caps::REMOTE_HOSTS),
    ]
}
