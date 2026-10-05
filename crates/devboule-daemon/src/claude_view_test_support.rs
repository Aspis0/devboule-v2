//! Fixtures shared by several claude_view suites: the probe-rooted view
//! and the measured init frame. Each is defined exactly once, here.

use std::path::PathBuf;

use serde_json::{json, Value};

use super::ClaudeView;

pub(super) fn view() -> ClaudeView {
    ClaudeView::new(Some(PathBuf::from(crate::test_support::FIXTURE_ROOT)))
}

// Reconstructed from recon/probes/claude-perm-probe2-allow-host.txt
// (CLI 2.1.260, 2026-09-05). Field names are the measured snake_case.
pub(super) fn init_frame() -> Value {
    json!({
        "type": "system",
        "subtype": "init",
        "cwd": r"C:\Users\gualt\AppData\Local\Temp\devboule-claude-perm2-allow-host-8r8qc09c",
        "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd",
        "tools": ["Agent", "Bash", "Read", "Edit", "Write"],
        "model": "claude-opus-5[1m]",
        "permissionMode": "default",
        "claude_code_version": "2.1.260"
    })
}
