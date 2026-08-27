use super::*;

#[test]
fn push_capped_drops_oldest_bytes() {
    let mut ring = VecDeque::new();
    push_capped(&mut ring, b"abcd", 8);
    push_capped(&mut ring, b"efghij", 8);
    assert_eq!(ring.iter().copied().collect::<Vec<_>>(), b"cdefghij");
}

#[test]
fn push_capped_large_chunk_keeps_only_tail() {
    let mut ring = VecDeque::new();
    push_capped(&mut ring, b"hello", 16);
    push_capped(&mut ring, b"0123456789abcdefXYZ", 8);
    assert_eq!(ring.iter().copied().collect::<Vec<_>>(), b"bcdefXYZ");
}

#[test]
fn push_capped_zero_cap_is_empty() {
    let mut ring = VecDeque::new();
    push_capped(&mut ring, b"abc", 0);
    assert!(ring.is_empty());
}

#[test]
fn validate_session_id_accepts_safe_ids_and_rejects_smuggling() {
    assert!(validate_session_id("session-123-1").is_ok());
    assert!(validate_session_id("a.b_c-2").is_ok());
    assert!(validate_session_id(&"x".repeat(64)).is_ok());
    assert!(validate_session_id("").is_err());
    assert!(validate_session_id(&"x".repeat(65)).is_err());
    assert!(validate_session_id("../other").is_err());
    assert!(validate_session_id("session id").is_err());
    assert!(validate_session_id("a:b").is_err());
}

#[test]
fn cursor_replay_is_strictly_after_last_seen_sequence() {
    let mut scrollback = Scrollback::default();
    scrollback.push(1, b"one");
    scrollback.push(2, b"two");
    scrollback.push(3, b"three");
    assert_eq!(
        scrollback.replay_after(Some(1)),
        vec![
            SessionEvent::Output {
                seq: 2,
                data: "two".to_string()
            },
            SessionEvent::Output {
                seq: 3,
                data: "three".to_string()
            },
        ]
    );
    assert_eq!(scrollback.replay_after(None).len(), 3);
}

#[test]
fn session_event_uses_a_type_tag() {
    let output = serde_json::to_value(SessionEvent::Output {
        seq: 7,
        data: "hi".to_string(),
    })
    .unwrap();
    assert_eq!(output["type"], "output");
    assert_eq!(output["seq"], 7);
    let exit = serde_json::to_value(SessionEvent::Exit { code: Some(0) }).unwrap();
    assert_eq!(exit["type"], "exit");
    assert_eq!(exit["code"], 0);
}

#[test]
fn attach_replay_and_live_output_share_one_ordered_stream() {
    let runtime = SessionRuntime::new();
    runtime.publish_output("before");
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_for_channel = Arc::clone(&received);
    let channel = Channel::new(move |body| {
        let event: SessionEvent = body.deserialize()?;
        received_for_channel.lock().unwrap().push(event);
        Ok(())
    });
    runtime.attach(Some(0), channel);
    runtime.publish_output("after");
    assert_eq!(
        *received.lock().unwrap(),
        vec![
            SessionEvent::Output {
                seq: 1,
                data: "before".to_string()
            },
            SessionEvent::Output {
                seq: 2,
                data: "after".to_string()
            },
        ]
    );
}

#[test]
fn attaching_twice_replaces_the_old_view_and_delivers_once() {
    let runtime = SessionRuntime::new();
    let first_received = Arc::new(Mutex::new(Vec::new()));
    let second_received = Arc::new(Mutex::new(Vec::new()));
    let first_for_channel = Arc::clone(&first_received);
    let second_for_channel = Arc::clone(&second_received);
    let first = Channel::new(move |body| {
        let event: SessionEvent = body.deserialize()?;
        first_for_channel.lock().unwrap().push(event);
        Ok(())
    });
    let second = Channel::new(move |body| {
        let event: SessionEvent = body.deserialize()?;
        second_for_channel.lock().unwrap().push(event);
        Ok(())
    });

    runtime.attach(None, first);
    runtime.attach(None, second);
    assert_eq!(runtime.subscriber_count(), 1);
    runtime.publish_output("once");

    assert!(first_received.lock().unwrap().is_empty());
    assert_eq!(
        *second_received.lock().unwrap(),
        vec![SessionEvent::Output {
            seq: 1,
            data: "once".to_string()
        }]
    );
}

#[test]
fn detach_stops_delivery_but_keeps_the_ring() {
    let runtime = SessionRuntime::new();
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_for_channel = Arc::clone(&received);
    runtime.attach(
        None,
        Channel::new(move |body| {
            let event: SessionEvent = body.deserialize()?;
            received_for_channel.lock().unwrap().push(event);
            Ok(())
        }),
    );
    runtime.publish_output("before-detach");
    runtime.detach();
    runtime.publish_output("while-detached");

    assert_eq!(runtime.subscriber_count(), 0);
    assert_eq!(
        *received.lock().unwrap(),
        vec![SessionEvent::Output {
            seq: 1,
            data: "before-detach".to_string()
        }]
    );
    assert_eq!(
        runtime.snapshot().0,
        vec![
            (1, "before-detach".to_string()),
            (2, "while-detached".to_string()),
        ]
    );
}

#[test]
fn detach_then_attach_replays_the_full_retained_ring() {
    let runtime = SessionRuntime::new();
    runtime.publish_output("first");
    runtime.attach(
        None,
        Channel::new(|body| {
            let _: SessionEvent = body.deserialize()?;
            Ok(())
        }),
    );
    runtime.detach();
    runtime.publish_output("second");
    let expected = runtime.snapshot().0;
    let replayed = Arc::new(Mutex::new(Vec::new()));
    let replayed_for_channel = Arc::clone(&replayed);
    runtime.attach(
        None,
        Channel::new(move |body| {
            replayed_for_channel
                .lock()
                .unwrap()
                .push(body.deserialize()?);
            Ok(())
        }),
    );
    let actual: Vec<(u64, String)> = replayed
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { seq, data } => Some((*seq, data.clone())),
            SessionEvent::Exit { .. } => None,
        })
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(runtime.subscriber_count(), 1);
}

#[test]
fn detaching_an_unknown_session_is_a_clean_error() {
    assert_eq!(
        detach_session(&SessionState::new(), "session-missing"),
        Err("No session with that id.".to_string())
    );
}

#[cfg(windows)]
fn answer_test_dsr(state: &SessionState, id: &str) {
    let writer = {
        let map = state.inner.lock().unwrap();
        Arc::clone(&map.get(id).unwrap().writer)
    };
    let mut writer = writer.lock().unwrap();
    writer.write_all(b"\x1b[1;1R").unwrap();
    writer.flush().unwrap();
}

#[cfg(windows)]
fn start_test_dsr_pump(
    state: &SessionState,
    id: &str,
) -> (Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    let writer = {
        let map = state.inner.lock().unwrap();
        Arc::clone(&map.get(id).unwrap().writer)
    };
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        while !stop_for_thread.load(Ordering::Acquire) {
            if let Ok(mut writer) = writer.lock() {
                let _ = writer.write_all(b"\x1b[1;1R");
                let _ = writer.flush();
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    });
    (stop, handle)
}

#[cfg(windows)]
fn stop_test_dsr_pump(stop: Arc<AtomicBool>, handle: std::thread::JoinHandle<()>) {
    stop.store(true, Ordering::Release);
    let _ = handle.join();
}

#[test]
fn ring_never_exceeds_256_kibibytes() {
    let runtime = SessionRuntime::new();
    runtime.publish_output(&"x".repeat(RING_CAPACITY / 2));
    runtime.publish_output(&"y".repeat(RING_CAPACITY));
    let (_, bytes) = runtime.snapshot();
    assert_eq!(bytes, RING_CAPACITY);
    assert_eq!(
        runtime.peak_ring_bytes.load(Ordering::Relaxed),
        RING_CAPACITY
    );
}

// Real PTY tests are Windows-gated and ignored by default because ConPTY
// integration needs a desktop-capable runner. Run with `--ignored` locally.
#[cfg(windows)]
fn test_state() -> SessionState {
    SessionState::new()
}

#[cfg(windows)]
fn attach_collecting(state: &SessionState, id: &str, received: Arc<Mutex<Vec<SessionEvent>>>) {
    let target = {
        let map = state.inner.lock().unwrap();
        Arc::clone(&map.get(id).unwrap().runtime)
    };
    let received_for_channel = Arc::clone(&received);
    target.attach(
        None,
        Channel::new(move |body| {
            received_for_channel
                .lock()
                .unwrap()
                .push(body.deserialize()?);
            Ok(())
        }),
    );
}

#[cfg(windows)]
#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn real_pty_spawn_read_resize_and_teardown() {
    let state = test_state();
    let id = "session-test-echo".to_string();
    let session = Session {
        id: id.clone(),
        workspace_id: None,
        kind: SessionKind::Terminal,
        title: "Terminal".to_string(),
    };
    let command = PtyCommand::new(
        "cmd.exe",
        vec!["/c".to_string(), "echo DEVBOULE_PTY_OK".to_string()],
        std::env::current_dir().unwrap(),
        Vec::new(),
    );
    spawn_session(&state, session, command).unwrap();
    // ConPTY can issue DSR (`ESC[6n`) before the viewer attaches. A real xterm
    // answers it through onData; this headless integration test does so here.
    answer_test_dsr(&state, &id);
    let (stop_dsr, dsr_thread) = start_test_dsr_pump(&state, &id);
    let received = Arc::new(Mutex::new(Vec::new()));
    attach_collecting(&state, &id, Arc::clone(&received));
    session_resize_inner(&state, &id, 100, 30).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if received.lock().unwrap().iter().any(|event| {
            matches!(event, SessionEvent::Output { data, .. } if data.contains("DEVBOULE_PTY_OK"))
        }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let saw_marker = received.lock().unwrap().iter().any(|event| {
        matches!(event, SessionEvent::Output { data, .. } if data.contains("DEVBOULE_PTY_OK"))
    });
    close_session(&state, &id);
    stop_test_dsr_pump(stop_dsr, dsr_thread);
    assert!(saw_marker);
    assert!(state.inner.lock().unwrap().is_empty());
}

#[cfg(windows)]
#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn real_pty_detach_keeps_session_buffers_output_and_close_reaps_child() {
    const MARKER: &str = "DEVBOULE_DETACH_BUFFER";
    let state = test_state();
    let id = "session-test-detach".to_string();
    let session = Session {
        id: id.clone(),
        workspace_id: None,
        kind: SessionKind::Terminal,
        title: "Terminal".to_string(),
    };
    let command = PtyCommand::new(
        "cmd.exe",
        vec!["/k".to_string()],
        std::env::current_dir().unwrap(),
        Vec::new(),
    );
    spawn_session(&state, session, command).unwrap();
    answer_test_dsr(&state, &id);
    let (stop_dsr, dsr_thread) = start_test_dsr_pump(&state, &id);
    let received = Arc::new(Mutex::new(Vec::new()));
    attach_collecting(&state, &id, Arc::clone(&received));
    let runtime = {
        let map = state.inner.lock().unwrap();
        Arc::clone(&map.get(&id).unwrap().runtime)
    };
    let received_before_detach = received.lock().unwrap().len();

    detach_session(&state, &id).unwrap();
    assert_eq!(runtime.subscriber_count(), 0);
    assert!(!runtime.reader_finished.load(Ordering::Acquire));
    assert!(!runtime.child_reaped.load(Ordering::Acquire));
    assert!(!state
        .inner
        .lock()
        .unwrap()
        .get(&id)
        .unwrap()
        .exited
        .load(Ordering::Acquire));

    let writer = {
        let map = state.inner.lock().unwrap();
        Arc::clone(&map.get(&id).unwrap().writer)
    };
    let mut writer = writer.lock().unwrap();
    writer
        .write_all(format!("echo {MARKER}\r\n").as_bytes())
        .unwrap();
    writer.flush().unwrap();
    drop(writer);

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if runtime
            .snapshot()
            .0
            .iter()
            .any(|(_, data)| data.contains(MARKER))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        runtime
            .snapshot()
            .0
            .iter()
            .any(|(_, data)| data.contains(MARKER)),
        "detached output was not retained in the ring"
    );
    assert_eq!(received.lock().unwrap().len(), received_before_detach);
    assert!(state.inner.lock().unwrap().contains_key(&id));
    assert!(!runtime.reader_finished.load(Ordering::Acquire));
    assert!(!runtime.child_reaped.load(Ordering::Acquire));

    let expected_replay = runtime.snapshot().0;
    let replayed = Arc::new(Mutex::new(Vec::new()));
    attach_collecting(&state, &id, Arc::clone(&replayed));
    let actual_replay: Vec<(u64, String)> = replayed
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { seq, data } => Some((*seq, data.clone())),
            SessionEvent::Exit { .. } => None,
        })
        .collect();
    assert_eq!(actual_replay, expected_replay);
    assert_eq!(runtime.subscriber_count(), 1);

    detach_session(&state, &id).unwrap();
    close_session(&state, &id);
    stop_test_dsr_pump(stop_dsr, dsr_thread);
    assert!(state.inner.lock().unwrap().is_empty());
    assert!(runtime.reader_finished.load(Ordering::Acquire));
    assert!(runtime.child_reaped.load(Ordering::Acquire));
}

#[cfg(windows)]
fn session_resize_inner(
    state: &SessionState,
    id: &str,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let master = {
        let map = state.inner.lock().unwrap();
        Arc::clone(&map.get(id).unwrap().master)
    };
    let result = master
        .lock()
        .unwrap()
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string());
    result
}

#[cfg(windows)]
struct ChannelTransportMetrics {
    bytes: usize,
    wall: Duration,
    chunk_sizes: Vec<usize>,
    seq_reordered: bool,
    peak_ring_bytes: usize,
    teardown: Duration,
    child_reaped: bool,
    clean: bool,
}

#[cfg(windows)]
fn summarize_chunk_sizes(chunk_sizes: &[usize]) -> (usize, f64, usize) {
    let mut sorted = chunk_sizes.to_vec();
    sorted.sort_unstable();
    let min = *sorted.first().expect("transport produced no chunks");
    let max = *sorted.last().unwrap();
    let middle = sorted.len() / 2;
    let median = if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) as f64 / 2.0
    } else {
        sorted[middle] as f64
    };
    (min, median, max)
}

#[cfg(windows)]
fn benchmark_file_command(file_path: &Path) -> PtyCommand {
    // The test temp path is deliberately a simple filename. portable-pty
    // quotes each argv entry, while cmd.exe applies its own /c quoting rules.
    PtyCommand::new(
        "cmd.exe",
        vec!["/c".to_string(), format!("type {}", file_path.display())],
        std::env::current_dir().unwrap(),
        Vec::new(),
    )
}

#[cfg(windows)]
fn run_channel_transport(
    state: &SessionState,
    id: &str,
    file_path: &Path,
    expected_file_bytes: usize,
    reader_mode: ReaderMode,
) -> ChannelTransportMetrics {
    let session = Session {
        id: id.to_string(),
        workspace_id: None,
        kind: SessionKind::Terminal,
        title: "Terminal".to_string(),
    };
    spawn_session_with_reader_mode(
        state,
        session,
        benchmark_file_command(file_path),
        reader_mode,
    )
    .unwrap();
    let runtime = {
        let map = state.inner.lock().unwrap();
        Arc::clone(&map.get(id).unwrap().runtime)
    };
    let writer_for_channel = {
        let map = state.inner.lock().unwrap();
        Arc::clone(&map.get(id).unwrap().writer)
    };
    let observed = Arc::new(Mutex::new((
        0usize,
        Vec::<usize>::new(),
        None::<u64>,
        false,
    )));
    let observed_for_channel = Arc::clone(&observed);
    let start = Instant::now();
    runtime.attach(
        None,
        Channel::new(move |body| {
            let event: SessionEvent = body.deserialize()?;
            if let SessionEvent::Output { seq, data } = event {
                if data.contains("\x1b[6n") {
                    let mut writer = writer_for_channel.lock().unwrap();
                    let _ = writer.write_all(b"\x1b[1;1R");
                    let _ = writer.flush();
                }
                let mut observed = observed_for_channel.lock().unwrap();
                let expected = observed.2.map_or(seq, |last| last + 1);
                if seq != expected {
                    observed.3 = true;
                }
                observed.2 = Some(seq);
                observed.0 += data.len();
                observed.1.push(data.len());
            }
            Ok(())
        }),
    );
    let (stop_dsr, dsr_thread) = start_test_dsr_pump(state, id);

    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if observed.lock().unwrap().0 >= expected_file_bytes {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let complete = observed.lock().unwrap().0 >= expected_file_bytes;
    if !complete {
        close_session(state, id);
        stop_test_dsr_pump(stop_dsr, dsr_thread);
        panic!(
            "channel transport did not finish: bytes={} expected_file_bytes={expected_file_bytes}",
            observed.lock().unwrap().0
        );
    }

    let wall = start.elapsed();
    let (bytes, chunk_sizes, seq_reordered) = {
        let observed = observed.lock().unwrap();
        (observed.0, observed.1.clone(), observed.3)
    };
    let peak_ring_bytes = runtime.peak_ring_bytes.load(Ordering::Relaxed);
    let close_start = Instant::now();
    close_session(state, id);
    let teardown = close_start.elapsed();
    stop_test_dsr_pump(stop_dsr, dsr_thread);
    let child_reaped = runtime.child_reaped.load(Ordering::Acquire);
    let clean = state.inner.lock().unwrap().is_empty()
        && runtime.reader_finished.load(Ordering::Acquire)
        && child_reaped;
    ChannelTransportMetrics {
        bytes,
        wall,
        chunk_sizes,
        seq_reordered,
        peak_ring_bytes,
        teardown,
        child_reaped,
        clean,
    }
}

#[cfg(windows)]
struct AtomicTransportMetrics {
    bytes: usize,
    wall: Duration,
    peak_ring_bytes: usize,
    teardown: Duration,
    child_reaped: bool,
    clean: bool,
}

#[cfg(windows)]
fn run_atomic_transport(
    state: &SessionState,
    id: &str,
    file_path: &Path,
    expected_file_bytes: usize,
) -> AtomicTransportMetrics {
    let counter = Arc::new(AtomicUsize::new(0));
    let session = Session {
        id: id.to_string(),
        workspace_id: None,
        kind: SessionKind::Terminal,
        title: "Terminal".to_string(),
    };
    spawn_session_with_reader_mode(
        state,
        session,
        benchmark_file_command(file_path),
        ReaderMode::AtomicByteCounter(Arc::clone(&counter)),
    )
    .unwrap();
    // B has no Channel callback to answer ConPTY's startup DSR query.
    answer_test_dsr(state, id);
    let (stop_dsr, dsr_thread) = start_test_dsr_pump(state, id);
    let runtime = {
        let map = state.inner.lock().unwrap();
        Arc::clone(&map.get(id).unwrap().runtime)
    };
    let start = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if counter.load(Ordering::Acquire) >= expected_file_bytes {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let bytes = counter.load(Ordering::Acquire);
    if bytes < expected_file_bytes {
        close_session(state, id);
        stop_test_dsr_pump(stop_dsr, dsr_thread);
        panic!(
            "atomic transport did not finish: bytes={bytes} expected_file_bytes={expected_file_bytes}"
        );
    }

    let wall = start.elapsed();
    let peak_ring_bytes = runtime.peak_ring_bytes.load(Ordering::Relaxed);
    let close_start = Instant::now();
    close_session(state, id);
    let teardown = close_start.elapsed();
    stop_test_dsr_pump(stop_dsr, dsr_thread);
    let child_reaped = runtime.child_reaped.load(Ordering::Acquire);
    let clean = state.inner.lock().unwrap().is_empty()
        && runtime.reader_finished.load(Ordering::Acquire)
        && child_reaped;
    AtomicTransportMetrics {
        bytes,
        wall,
        peak_ring_bytes,
        teardown,
        child_reaped,
        clean,
    }
}

#[cfg(windows)]
#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn real_pty_channel_flood_correctness() {
    const LINES: usize = 50_000;
    const PAYLOAD: &str = "DEVBOULE_LOAD_0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const DONE: &str = "DEVBOULE_LOAD_DONE";
    let state = test_state();
    let id = "session-test-load".to_string();
    let session = Session {
        id: id.clone(),
        workspace_id: None,
        kind: SessionKind::Terminal,
        title: "Terminal".to_string(),
    };
    let command = PtyCommand::new(
        "pwsh.exe",
        vec![
            "-NoLogo".to_string(),
            "-NoProfile".to_string(),
            "-Command".to_string(),
            format!("$line = '{PAYLOAD}'; 1..{LINES} | ForEach-Object {{ $line }}; '{DONE}'"),
        ],
        std::env::current_dir().unwrap(),
        Vec::new(),
    );
    spawn_session(&state, session, command).unwrap();
    answer_test_dsr(&state, &id);
    let (stop_dsr, dsr_thread) = start_test_dsr_pump(&state, &id);
    let observed = Arc::new(Mutex::new((0usize, 0usize, None::<u64>, false, false)));
    let observed_for_channel = Arc::clone(&observed);
    let writer_for_channel = {
        let map = state.inner.lock().unwrap();
        Arc::clone(&map.get(&id).unwrap().writer)
    };
    let runtime = {
        let map = state.inner.lock().unwrap();
        Arc::clone(&map.get(&id).unwrap().runtime)
    };
    runtime.attach(
        None,
        Channel::new(move |body| {
            let event: SessionEvent = body.deserialize()?;
            if let SessionEvent::Output { seq, data } = event {
                if data.contains("\x1b[6n") {
                    let mut writer = writer_for_channel.lock().unwrap();
                    let _ = writer.write_all(b"\x1b[1;1R");
                    let _ = writer.flush();
                }
                let mut observed = observed_for_channel.lock().unwrap();
                let expected = observed.2.map_or(seq, |last| last + 1);
                if seq != expected {
                    observed.3 = true;
                }
                if data.contains(DONE) {
                    observed.4 = true;
                }
                observed.2 = Some(seq);
                observed.0 += data.len();
                observed.1 += 1;
            }
            Ok(())
        }),
    );

    let start = Instant::now();
    let deadline = start + Duration::from_secs(60);
    while Instant::now() < deadline {
        if observed.lock().unwrap().4 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let wall = start.elapsed();
    if !observed.lock().unwrap().4 {
        close_session(&state, &id);
        stop_test_dsr_pump(stop_dsr, dsr_thread);
        panic!("load child did not emit its completion marker");
    }
    let (bytes, chunks, _, reordered, _) = *observed.lock().unwrap();
    let expected_bytes = LINES * PAYLOAD.len();
    let output_complete = bytes >= expected_bytes;
    let peak_ring_bytes = runtime.peak_ring_bytes.load(Ordering::Relaxed);
    let close_start = Instant::now();
    close_session(&state, &id);
    let teardown = close_start.elapsed();
    stop_test_dsr_pump(stop_dsr, dsr_thread);
    let clean = state.inner.lock().unwrap().is_empty()
        && runtime.reader_finished.load(Ordering::Acquire)
        && runtime.child_reaped.load(Ordering::Acquire);
    println!(
        "PTY_CORRECTNESS lines={LINES} expected_min_bytes={expected_bytes} bytes={bytes} chunks={chunks} wall_ms={} peak_ring_bytes={peak_ring_bytes} output_complete={output_complete} seq_reordered={reordered} child_reaped={} teardown_ms={} clean={clean}",
        wall.as_millis(),
        runtime.child_reaped.load(Ordering::Acquire),
        teardown.as_millis(),
    );
    assert!(
        output_complete,
        "the generator did not deliver its expected flood"
    );
    assert!(!reordered, "output sequence was dropped or reordered");
    assert!(peak_ring_bytes <= RING_CAPACITY);
    assert!(
        runtime.child_reaped.load(Ordering::Acquire),
        "child wait did not reap a status"
    );
    assert!(clean, "teardown left a session or reader thread");
}

#[cfg(windows)]
#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn real_pty_channel_file_transport_ab_benchmark() {
    const DATA_LINES: usize = 200_000;
    const PAYLOAD: &str = "DEVBOULE_TRANSPORT_0123456789abcdefghijklmnopqrstuvwxyz0123456789";

    let file_path = std::env::temp_dir().join(format!(
        "devboule-pty-transport-{}-{}.txt",
        std::process::id(),
        SESSION_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    {
        let file = std::fs::File::create(&file_path).unwrap();
        let mut file = std::io::BufWriter::new(file);
        for _ in 0..DATA_LINES {
            file.write_all(PAYLOAD.as_bytes()).unwrap();
            file.write_all(b"\r\n").unwrap();
        }
        file.flush().unwrap();
    }
    let expected_file_bytes = std::fs::metadata(&file_path).unwrap().len() as usize;

    // A and B use the same file, command line, PTY size, and 16 KiB reader.
    // A retains the production Channel path; B replaces publication with one
    // atomic raw-byte counter and therefore performs no serialization or send.
    let channel = run_channel_transport(
        &test_state(),
        "session-test-transport-channel",
        &file_path,
        expected_file_bytes,
        ReaderMode::Channel,
    );
    let atomic = run_atomic_transport(
        &test_state(),
        "session-test-transport-counter",
        &file_path,
        expected_file_bytes,
    );
    let (channel_min, channel_median, channel_max) = summarize_chunk_sizes(&channel.chunk_sizes);
    let channel_mib_s = channel.bytes as f64 / (1024.0 * 1024.0) / channel.wall.as_secs_f64();
    let atomic_mib_s = atomic.bytes as f64 / (1024.0 * 1024.0) / atomic.wall.as_secs_f64();
    println!(
        "PTY_AB scenario=channel bytes={} expected_file_bytes={expected_file_bytes} wall_ms={} mib_s={channel_mib_s:.2} messages={} messages_per_s={:.2} chunk_min={channel_min} chunk_median={channel_median:.1} chunk_max={channel_max} peak_ring_bytes={} seq_reordered={} child_reaped={} teardown_ms={} clean={}",
        channel.bytes,
        channel.wall.as_millis(),
        channel.chunk_sizes.len(),
        channel.chunk_sizes.len() as f64 / channel.wall.as_secs_f64(),
        channel.peak_ring_bytes,
        channel.seq_reordered,
        channel.child_reaped,
        channel.teardown.as_millis(),
        channel.clean,
    );
    println!(
        "PTY_AB scenario=atomic_counter bytes={} expected_file_bytes={expected_file_bytes} wall_ms={} mib_s={atomic_mib_s:.2} messages=n/a peak_ring_bytes={} seq_continuity=n/a child_reaped={} teardown_ms={} clean={}",
        atomic.bytes,
        atomic.wall.as_millis(),
        atomic.peak_ring_bytes,
        atomic.child_reaped,
        atomic.teardown.as_millis(),
        atomic.clean,
    );
    println!(
        "PTY_AB comparison atomic_over_channel_speedup={:.2}",
        atomic_mib_s / channel_mib_s
    );

    assert!(
        channel.bytes >= expected_file_bytes,
        "Channel output was truncated"
    );
    assert!(
        !channel.seq_reordered,
        "Channel output was dropped or reordered"
    );
    assert!(channel.peak_ring_bytes <= RING_CAPACITY);
    assert!(channel.child_reaped && channel.clean);
    assert!(
        atomic.bytes >= expected_file_bytes,
        "atomic output was truncated"
    );
    assert_eq!(atomic.peak_ring_bytes, 0);
    assert!(atomic.child_reaped && atomic.clean);

    println!("PTY_COALESCING skipped=atomic_counter_within_20_percent_of_channel");

    let _ = std::fs::remove_file(&file_path);
}
