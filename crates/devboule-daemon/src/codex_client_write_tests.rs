//! What the Codex writer's flush says about a write that failed: a closed
//! stdin put no byte on the wire, and that has to survive the `io::Error` a
//! `Write` hands back, because the send path reads it to decide whether the
//! message may be offered again.

use std::io::Write;
use std::sync::{Arc, Mutex};

use super::super::session_items::transport_was_closed;
use super::command_test_support::writer_on;

#[test]
fn a_flush_on_a_closed_codex_stdin_reports_a_closed_transport() {
    let mut writer = writer_on(Arc::new(Mutex::new(None)));
    writer.write_all(b"a queued message").expect("buffered");

    let error = writer.flush().expect_err("there is no stdin to write to");

    assert!(
        transport_was_closed(&error),
        "a closed stdin must stay the typed marker, not prose: {error}"
    );
}
