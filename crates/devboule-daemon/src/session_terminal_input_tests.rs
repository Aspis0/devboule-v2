//! The terminal-input tests, moved whole out of `session_tests.rs` lines
//! 2261-2383 (at `6d29bc3`): several observers sending complete inputs
//! concurrently through one writer without interleaving, and only the resize
//! owner being allowed to resize the terminal. Every line below is
//! byte-identical to its text there apart from this header;
//! `BytewiseRecordingWriter` is promoted to `pub(super)` for this move, and the
//! other fixtures come from the provider's own imports.

use super::tests::{
    insert_live, insert_live_agent_with_writer, test_owner, tmp_delete_registry,
    BytewiseRecordingWriter,
};
use super::*;

#[test]
fn multiple_observers_can_send_complete_inputs_concurrently() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-multi-writer", "process-multi-writer");
    let written = Arc::new(Mutex::new(Vec::new()));
    let session_id = "s.multi-writer.1";
    let first_text = "first observer input\n".repeat(32);
    let second_text = "second observer input\n".repeat(32);
    let start = Arc::new(Barrier::new(3));
    let first_write = Arc::new(Barrier::new(2));
    insert_live_agent_with_writer(
        &registry,
        session_id,
        owner.clone(),
        Box::new(BytewiseRecordingWriter {
            bytes: Arc::clone(&written),
            first_write: Arc::clone(&first_write),
            first_write_seen: AtomicBool::new(false),
        }),
    );
    let first = ConnHandle::new(1);
    let second = ConnHandle::new(2);
    registry
        .attach_with_subscription(session_id, 101, None, &first, &owner, true)
        .expect("first observer attaches");
    registry
        .attach_with_subscription(session_id, 202, None, &second, &owner, true)
        .expect("second observer attaches");

    let first_registry = registry.clone();
    let first_start = Arc::clone(&start);
    let first_owner = owner.clone();
    let first_session_id = session_id.to_string();
    let first_handle = std::thread::spawn(move || {
        first_start.wait();
        first_registry
            .send_with_subscription(
                &first_session_id,
                101,
                &first_text,
                &[],
                &[],
                &first_owner,
                &first,
            )
            .expect("first input");
        first_text
    });
    let second_registry = registry.clone();
    let second_start = Arc::clone(&start);
    let second_owner = owner.clone();
    let second_session_id = session_id.to_string();
    let second_handle = std::thread::spawn(move || {
        second_start.wait();
        second_registry
            .send_with_subscription(
                &second_session_id,
                202,
                &second_text,
                &[],
                &[],
                &second_owner,
                &second,
            )
            .expect("second input");
        second_text
    });
    start.wait();
    first_write.wait();
    let first_text = first_handle.join().expect("first sender joins");
    let second_text = second_handle.join().expect("second sender joins");
    let received = written.lock().expect("writer").clone();
    let first_then_second = [first_text.as_bytes(), second_text.as_bytes()].concat();
    let second_then_first = [second_text.as_bytes(), first_text.as_bytes()].concat();
    assert!(
        received == first_then_second || received == second_then_first,
        "concurrent inputs were interleaved"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_resize_owner_can_resize_terminal() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-resize-owner", "process-resize-owner");
    let session_id = "s.resize-owner.1";
    insert_live(&registry, session_id, owner.clone());
    let first = ConnHandle::new(1);
    let second = ConnHandle::new(2);
    registry
        .attach_with_subscription(session_id, 101, None, &first, &owner, false)
        .expect("first observer attaches");
    registry
        .attach_with_subscription(session_id, 202, None, &second, &owner, false)
        .expect("second observer attaches");
    registry
        .claim_resize_with_subscription(session_id, 101, &owner, &first)
        .expect("first observer claims resize control");

    let error = registry
        .resize_with_subscription(session_id, 202, 100, 30, &owner, &second)
        .expect_err("non-owner resize must be rejected");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("resize control"));
    registry
        .resize_with_subscription(session_id, 101, 100, 30, &owner, &first)
        .expect("resize owner can resize");
    let runtime = registry.runtime(session_id).expect("runtime");
    assert_eq!(
        runtime
            .stream
            .lock()
            .expect("stream")
            .screen
            .as_ref()
            .expect("screen")
            .dimensions(),
        (100, 30)
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
