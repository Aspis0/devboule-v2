//! Wire contract for the in-app file editor (Paseo port, dialect 33).
//!
//! Paseo source: `packages/protocol/src/messages.ts` (`fs.file.subscribe`,
//! `fs.file.write`, `fs.file.update`, `FileVersion`, `FileWriteResult`)
//! and `packages/app/src/file-pane/editor/model.ts` (Apache-2.0,
//! Copyright (c) 2025-present Mohamed Boudra).
//!
//! The editor speaks three reads and one write, each in a workspace
//! spelling, an app-file spelling (an absolute or `~` path on this
//! machine, app-only), and a remote-host spelling (relayed over the held
//! peer link, human-originated, no confirmation card):
//!
//! - open: whole text up to 1 MiB with its BOM flag and version, or the
//!   refusal's sentence; a missing file opens empty and the first save
//!   creates it (Paseo answers ENOENT here — the missing arm is the fix).
//! - version: the poll the observation source refreshes from.
//! - write: `written` | `conflict` with the fresh version | `error`.
//!
//! A write that names no expected version is a create: it succeeds only
//! when the file is missing, and answers `conflict` with the ready version
//! when anything is already there — so a retry after an outside creator
//! lands on the conflict road, never on a silent overwrite.

use super::*;
use crate::PROTOCOL_VERSION;

#[test]
fn the_file_edit_wire_words_are_exact() {
    assert_eq!(
        serde_json::to_value(WorkspaceFileVersion::Missing {
            workspace_id: "w".to_string(),
            path: "a.ts".to_string(),
        })
        .expect("json"),
        serde_json::json!({"status": "missing", "workspaceId": "w", "path": "a.ts"}),
    );
    assert_eq!(
        serde_json::to_value(WorkspaceFileWriteResult::Written {
            modified_at: 7,
            size: 3,
            revision: "3:7".to_string(),
        })
        .expect("json"),
        serde_json::json!({"status": "written", "modifiedAt": 7, "size": 3, "revision": "3:7"}),
    );
}

#[test]
fn the_editor_frames_carry_their_ids_and_names() {
    let write = ClientMessage::WorkspaceFileWrite {
        id: 9,
        workspace_id: "w".to_string(),
        path: "a.ts".to_string(),
        content: "one".to_string(),
        expected_modified_at: Some(7),
        expected_revision: Some("3:7".to_string()),
        create: false,
    };
    assert_eq!(write.request_id(), Some(9));
    assert!(write.is_state_changing());

    let open = ClientMessage::WorkspaceFileOpen {
        id: 10,
        workspace_id: "w".to_string(),
        path: "a.ts".to_string(),
    };
    assert_eq!(open.request_id(), Some(10));
    assert!(!open.is_state_changing());

    let version = ClientMessage::WorkspaceFileVersion {
        id: 11,
        workspace_id: "w".to_string(),
        path: "a.ts".to_string(),
    };
    assert_eq!(version.request_id(), Some(11));
    assert!(!version.is_state_changing());

    let app_write = ClientMessage::AppFileWrite {
        id: 12,
        path: "~/.config/pubvia/anthropic.env".to_string(),
        content: "x".to_string(),
        expected_modified_at: None,
        expected_revision: None,
        create: false,
    };
    assert_eq!(app_write.request_id(), Some(12));
    assert!(app_write.is_state_changing());

    let remote_write = ClientMessage::RemoteHostFileWrite {
        id: 13,
        device_id: "d".to_string(),
        workspace_id: "w".to_string(),
        path: "a.ts".to_string(),
        content: "one".to_string(),
        expected_modified_at: Some(7),
        expected_revision: None,
        create: false,
    };
    assert_eq!(remote_write.request_id(), Some(13));
    assert!(remote_write.is_state_changing());
}

#[test]
fn the_file_edit_dialect_is_33() {
    assert_eq!(PROTOCOL_VERSION, 33);
}

#[test]
fn a_create_is_an_explicit_intent_on_the_wire() {
    // Absent on an old frame decodes as false: a versionless write
    // without it creates nothing.
    let decoded: ClientMessage = serde_json::from_value(serde_json::json!({
        "type": "workspace_file_write",
        "id": 1,
        "workspaceId": "w",
        "path": "a.ts",
        "content": "x",
    }))
    .expect("a v33 write decodes");
    assert!(
        matches!(
            decoded,
            ClientMessage::WorkspaceFileWrite { create: false, .. }
        ),
        "{decoded:?}"
    );
}
