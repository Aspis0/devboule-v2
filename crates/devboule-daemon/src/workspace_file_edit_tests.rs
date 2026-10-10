//! The editor's read and write road over real folders in `%TEMP%`:
//! opens with BOM flags and versions, creates from missing, conflicts on
//! stale stamps, refuses binaries and over-cap bytes in both directions,
//! writes atomically, and keeps the human's outside-workspace road on the
//! real file (links followed) while the workspace road refuses link
//! escapes. The last two cases pin the agent/peer boundary: the new frames
//! are denied to every peer-shaped connection, and the MCP broker serves
//! no tool that writes a file.
//!
//! Covers the port of Paseo's
//! `packages/server/src/server/file-explorer/service.ts`
//! (`writeExplorerFile`, `getExplorerFileVersion`, `readExplorerFile`)
//! in [`super`] (Apache-2.0, Copyright (c) 2025-present Mohamed Boudra).

use std::path::PathBuf;

use devboule_protocol::{
    WorkspaceFileContentStatus, WorkspaceFileVersion, WorkspaceFileWriteResult,
};

use super::{
    classify_target, expand_user_path, open_app_file, open_workspace_file, version_app_file,
    version_workspace_file, write_app_file, write_workspace_file, TargetState, BINARY,
    MAX_EDITABLE_FILE_BYTES, PARENT_MISSING, TOO_LARGE, UNSTATABLE,
};
use crate::workspace_git_diff::NOT_A_FILE;

struct Dir {
    root: PathBuf,
}

impl Dir {
    fn fresh(label: &str) -> Self {
        Self {
            root: crate::test_dirs::test_temp_dir(&format!("devboule-file-edit-{label}")),
        }
    }

    fn write(&self, relative: &str, contents: impl AsRef<[u8]>) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent directory");
        }
        std::fs::write(path, contents).expect("write");
    }

    fn workspace_id(&self) -> &str {
        "workspace-for-test"
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn ready_of(version: &WorkspaceFileVersion) -> (u64, i64, Option<String>) {
    match version {
        WorkspaceFileVersion::Ready {
            size,
            modified_at,
            revision,
            ..
        } => (*size, *modified_at, revision.clone()),
        other => panic!("expected a ready version, got {other:?}"),
    }
}

#[test]
fn an_open_hands_back_text_with_its_version() {
    let dir = Dir::fresh("open");
    dir.write("a.txt", "one\n");

    let file = open_workspace_file(&dir.root, dir.workspace_id(), "a.txt");

    assert_eq!(file.status, WorkspaceFileContentStatus::Ok);
    assert_eq!(file.content.as_deref(), Some("one\n"));
    assert_eq!(file.has_bom, Some(false));
    assert_eq!(file.size, Some(4));
    assert_eq!(file.error, None);
    let (size, _, revision) = ready_of(file.version.as_ref().expect("version"));
    assert_eq!(size, 4);
    assert!(revision.is_some(), "a write needs a revision to echo");
}

#[test]
fn a_write_with_the_fresh_version_lands_and_restamps() {
    let dir = Dir::fresh("write");
    dir.write("a.txt", "one\n");
    let opened = open_workspace_file(&dir.root, dir.workspace_id(), "a.txt");
    let (_, modified_at, revision) = ready_of(opened.version.as_ref().expect("version"));

    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "a.txt",
        b"two\n",
        Some(modified_at),
        revision.as_deref(),
    );

    match result {
        WorkspaceFileWriteResult::Written {
            modified_at: _,
            size,
            revision,
        } => {
            assert_eq!(size, 4);
            assert!(!revision.is_empty());
        }
        other => panic!("expected written, got {other:?}"),
    }
    assert_eq!(
        std::fs::read(dir.root.join("a.txt")).expect("read back"),
        b"two\n"
    );
}

#[test]
fn a_stale_write_conflicts_with_the_fresh_version() {
    let dir = Dir::fresh("stale");
    dir.write("a.txt", "one\n");
    let opened = open_workspace_file(&dir.root, dir.workspace_id(), "a.txt");
    let (_, modified_at, revision) = ready_of(opened.version.as_ref().expect("version"));

    // An outside change moves the stamp first.
    dir.write("a.txt", "outside\n");
    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "a.txt",
        b"local\n",
        Some(modified_at),
        revision.as_deref(),
    );

    match result {
        WorkspaceFileWriteResult::Conflict { version } => {
            let (size, _, _) = ready_of(&version);
            assert_eq!(size, 8, "the conflict names the disk's bytes, not ours");
        }
        other => panic!("expected conflict, got {other:?}"),
    }
    // The refused bytes never touched the disk.
    assert_eq!(
        std::fs::read(dir.root.join("a.txt")).expect("read back"),
        b"outside\n"
    );
}

#[test]
fn a_missing_file_opens_empty_and_the_first_save_creates_it() {
    let dir = Dir::fresh("create");
    std::fs::create_dir_all(dir.root.join("sub")).expect("parent");

    let file = open_workspace_file(&dir.root, dir.workspace_id(), "sub/new.txt");
    assert_eq!(file.status, WorkspaceFileContentStatus::Ok);
    assert_eq!(file.content.as_deref(), Some(""));
    assert!(matches!(
        file.version,
        Some(WorkspaceFileVersion::Missing { .. })
    ));

    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "sub/new.txt",
        b"born\n",
        None,
        None,
    );
    assert!(
        matches!(result, WorkspaceFileWriteResult::Written { .. }),
        "the first save creates: {result:?}"
    );
    assert_eq!(
        std::fs::read(dir.root.join("sub/new.txt")).expect("read back"),
        b"born\n"
    );
}

#[test]
fn a_create_names_no_version_but_finds_a_file_conflicts() {
    let dir = Dir::fresh("create-races");
    dir.write("a.txt", "there\n");

    let result = write_workspace_file(&dir.root, dir.workspace_id(), "a.txt", b"new\n", None, None);

    assert!(
        matches!(result, WorkspaceFileWriteResult::Conflict { .. }),
        "a create over an existing file is a conflict, never an overwrite: {result:?}"
    );
}

#[test]
fn a_create_without_a_parent_is_an_error_not_a_tree() {
    let dir = Dir::fresh("no-parent");

    let file = open_workspace_file(&dir.root, dir.workspace_id(), "absent/new.txt");
    assert_eq!(file.status, WorkspaceFileContentStatus::Refused);
    assert_eq!(file.error.as_deref(), Some(PARENT_MISSING));

    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "absent/new.txt",
        b"new\n",
        None,
        None,
    );
    assert!(
        matches!(
            &result,
            WorkspaceFileWriteResult::Error { error } if error == PARENT_MISSING
        ),
        "parents are never made silently: {result:?}"
    );
    assert!(!dir.root.join("absent").exists());
}

#[test]
fn binary_bytes_are_refused_in_both_directions() {
    let dir = Dir::fresh("binary");
    dir.write("a.bin", b"\0binary");
    // A binary still versions (stat-only), for the conflict road.
    let (_, modified_at, revision) = ready_of(&version_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "a.bin",
    ));

    // The open refuses...
    let file = open_workspace_file(&dir.root, dir.workspace_id(), "a.bin");
    assert_eq!(file.status, WorkspaceFileContentStatus::Refused);
    assert_eq!(file.error.as_deref(), Some(BINARY));

    // ...and so does a write that names the fresh version.
    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "a.bin",
        b"text\n",
        Some(modified_at),
        revision.as_deref(),
    );
    assert!(
        matches!(
            &result,
            WorkspaceFileWriteResult::Error { error } if error == BINARY
        ),
        "{result:?}"
    );
}

#[test]
fn undecodable_bytes_are_binary_not_lossy_text() {
    let dir = Dir::fresh("invalid-utf8");
    dir.write("a.txt", [0x66, 0x6f, 0xff, 0x6f]);

    let file = open_workspace_file(&dir.root, dir.workspace_id(), "a.txt");
    assert_eq!(file.status, WorkspaceFileContentStatus::Refused);
    assert_eq!(file.error.as_deref(), Some(BINARY));
}

/// The cap that matters is the frame's, not the file's: the worst this
/// road allows must still encode — with the envelope around it — well
/// under [`devboule_protocol::MAX_FRAME_BYTES`]. An over-cap reply would
/// not answer but take the connection down with it (`Framed::send`
/// refuses the frame and the loop detaches), so this pins the margin
/// instead of trusting the arithmetic.
#[test]
fn the_cap_fits_the_worst_frame_with_margin() {
    use devboule_protocol::{DaemonMessage, MAX_FRAME_BYTES};

    let dir = Dir::fresh("frame");
    // A quote escapes as two wire bytes, the worst text this road opens
    // can do (control bytes that escape to six are binary by the sniff
    // above and never open). The write direction takes any bytes verbatim,
    // so it is measured with those below.
    dir.write("worst.txt", vec![b'"'; MAX_EDITABLE_FILE_BYTES as usize]);
    let file = open_workspace_file(&dir.root, dir.workspace_id(), "worst.txt");
    assert_eq!(
        file.status,
        WorkspaceFileContentStatus::Ok,
        "the cap is inclusive: exactly at it, the file still opens"
    );
    let encoded = serde_json::to_vec(&DaemonMessage::WorkspaceFileOpened { id: 1, file })
        .expect("the opened reply must encode");
    assert!(
        (encoded.len() as u64) < MAX_FRAME_BYTES as u64 - 128 * 1024,
        "worst-case open frame {} leaves no margin under {}",
        encoded.len(),
        MAX_FRAME_BYTES
    );

    // The write direction rides the same frame cap from the app side —
    // and takes bytes verbatim, so the six-byte escapes count here.
    let request = devboule_protocol::ClientMessage::WorkspaceFileWrite {
        id: 2,
        workspace_id: dir.workspace_id().to_string(),
        path: "worst.txt".to_string(),
        content: "\u{1}".repeat(MAX_EDITABLE_FILE_BYTES as usize),
        expected_modified_at: Some(0),
        expected_revision: None,
    };
    let encoded = serde_json::to_vec(&request).expect("the write must encode");
    assert!(
        (encoded.len() as u64) < MAX_FRAME_BYTES as u64 - 128 * 1024,
        "worst-case write frame {} leaves no margin under {}",
        encoded.len(),
        MAX_FRAME_BYTES
    );
}

#[test]
fn the_cap_holds_in_both_directions() {
    let dir = Dir::fresh("cap");
    dir.write("big.txt", vec![b'a'; MAX_EDITABLE_FILE_BYTES as usize + 1]);

    let file = open_workspace_file(&dir.root, dir.workspace_id(), "big.txt");
    assert_eq!(file.status, WorkspaceFileContentStatus::Refused);
    assert_eq!(file.error.as_deref(), Some(TOO_LARGE));

    dir.write("small.txt", "one\n");
    let opened = open_workspace_file(&dir.root, dir.workspace_id(), "small.txt");
    let (_, modified_at, revision) = ready_of(opened.version.as_ref().expect("version"));
    let big = vec![b'a'; MAX_EDITABLE_FILE_BYTES as usize + 1];
    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "small.txt",
        &big,
        Some(modified_at),
        revision.as_deref(),
    );
    assert!(
        matches!(
            &result,
            WorkspaceFileWriteResult::Error { error } if error == TOO_LARGE
        ),
        "{result:?}"
    );
}

#[test]
fn a_bom_round_trips_byte_for_byte() {
    let dir = Dir::fresh("bom");
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(b"hi\n");
    dir.write("a.txt", bytes);

    let file = open_workspace_file(&dir.root, dir.workspace_id(), "a.txt");
    assert_eq!(file.has_bom, Some(true));
    assert_eq!(file.content.as_deref(), Some("hi\n"));

    let (_, modified_at, revision) = ready_of(file.version.as_ref().expect("version"));
    // The model restores the BOM inside the content it sends (Paseo's
    // `serializedContent`); the daemon writes bytes verbatim.
    let mut next = vec![0xEF, 0xBB, 0xBF];
    next.extend_from_slice("hi!\n".as_bytes());
    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "a.txt",
        &next,
        Some(modified_at),
        revision.as_deref(),
    );
    assert!(
        matches!(result, WorkspaceFileWriteResult::Written { .. }),
        "{result:?}"
    );
    assert_eq!(
        std::fs::read(dir.root.join("a.txt")).expect("read back"),
        next
    );
}

#[test]
fn line_endings_travel_verbatim() {
    let dir = Dir::fresh("crlf");
    dir.write("a.txt", "one\r\ntwo\r\n");

    let file = open_workspace_file(&dir.root, dir.workspace_id(), "a.txt");
    assert_eq!(file.content.as_deref(), Some("one\r\ntwo\r\n"));

    let (_, modified_at, revision) = ready_of(file.version.as_ref().expect("version"));
    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "a.txt",
        b"one\r\ntwo\r\nthree\r\n",
        Some(modified_at),
        revision.as_deref(),
    );
    assert!(
        matches!(result, WorkspaceFileWriteResult::Written { .. }),
        "{result:?}"
    );
    assert_eq!(
        std::fs::read(dir.root.join("a.txt")).expect("read back"),
        b"one\r\ntwo\r\nthree\r\n"
    );
}

#[test]
fn a_folder_is_not_a_file_on_either_road() {
    let dir = Dir::fresh("folder");
    std::fs::create_dir_all(dir.root.join("sub")).expect("folder");

    let file = open_workspace_file(&dir.root, dir.workspace_id(), "sub");
    assert_eq!(file.status, WorkspaceFileContentStatus::Refused);
    assert_eq!(file.error.as_deref(), Some(NOT_A_FILE));

    let result = write_workspace_file(&dir.root, dir.workspace_id(), "sub", b"x", Some(0), None);
    assert!(
        matches!(
            &result,
            WorkspaceFileWriteResult::Error { error } if error == NOT_A_FILE
        ),
        "{result:?}"
    );
}

#[test]
fn the_version_poll_tracks_the_disk() {
    let dir = Dir::fresh("version");
    dir.write("a.txt", "one\n");

    let before = version_workspace_file(&dir.root, dir.workspace_id(), "a.txt");
    let (size, _, revision) = ready_of(&before);
    assert_eq!(size, 4);

    dir.write("a.txt", "one and more\n");
    let after = version_workspace_file(&dir.root, dir.workspace_id(), "a.txt");
    let (size, _, next_revision) = ready_of(&after);
    assert_eq!(size, 13);
    assert_ne!(revision, next_revision, "the poll must move with the disk");

    std::fs::remove_file(dir.root.join("a.txt")).expect("delete");
    assert!(matches!(
        version_workspace_file(&dir.root, dir.workspace_id(), "a.txt"),
        WorkspaceFileVersion::Missing { .. }
    ));
}

#[test]
fn a_link_escape_inside_the_workspace_is_refused() {
    let dir = Dir::fresh("link");
    dir.write("real.txt", "real\n");
    #[cfg(windows)]
    let linked =
        std::os::windows::fs::symlink_file(dir.root.join("real.txt"), dir.root.join("link.txt"));
    #[cfg(not(windows))]
    let linked = std::os::unix::fs::symlink(dir.root.join("real.txt"), dir.root.join("link.txt"));
    if linked.is_err() {
        // No privilege to create links in this environment (Windows needs
        // Developer Mode): the walk's refusal is covered by the unit's own
        // symlink cases, not by a link this test could not make.
        return;
    }

    let file = open_workspace_file(&dir.root, dir.workspace_id(), "link.txt");
    assert_eq!(file.status, WorkspaceFileContentStatus::Refused);
    assert!(
        file.error.is_some(),
        "a link escape must not open: {file:?}"
    );
}

// ---------------------------------------------------------------------------
// The human's own files: absolute paths on this machine.
// ---------------------------------------------------------------------------

#[test]
fn an_absolute_path_opens_and_writes_the_real_file() {
    let dir = Dir::fresh("app");
    dir.write("note.txt", "hello\n");
    let absolute = dir.root.join("note.txt").to_string_lossy().into_owned();

    let file = open_app_file(&absolute);
    assert_eq!(file.status, WorkspaceFileContentStatus::Ok);
    assert_eq!(file.content.as_deref(), Some("hello\n"));

    let (_, modified_at, revision) = ready_of(file.version.as_ref().expect("version"));
    let result = write_app_file(
        &absolute,
        b"edited\n",
        Some(modified_at),
        revision.as_deref(),
    );
    assert!(
        matches!(result, WorkspaceFileWriteResult::Written { .. }),
        "{result:?}"
    );
    assert_eq!(
        std::fs::read(dir.root.join("note.txt")).expect("read back"),
        b"edited\n"
    );
}

#[test]
fn an_app_create_needs_its_parent_like_the_workspace_one() {
    let dir = Dir::fresh("app-parent");
    let missing = dir
        .root
        .join("absent")
        .join("new.txt")
        .to_string_lossy()
        .into_owned();

    let file = open_app_file(&missing);
    assert_eq!(file.status, WorkspaceFileContentStatus::Refused);
    assert_eq!(file.error.as_deref(), Some(PARENT_MISSING));

    assert!(matches!(
        write_app_file(&missing, b"new\n", None, None),
        WorkspaceFileWriteResult::Error { .. }
    ));
}

#[test]
fn a_relative_app_path_is_refused_not_joined() {
    let file = open_app_file("relative/path.txt");
    assert_eq!(file.status, WorkspaceFileContentStatus::Refused);

    assert!(matches!(
        version_app_file("relative/path.txt"),
        WorkspaceFileVersion::Error { .. }
    ));
    assert!(matches!(
        write_app_file("relative/path.txt", b"x", None, None),
        WorkspaceFileWriteResult::Error { .. }
    ));
}

#[test]
fn tilde_expands_to_the_home_folder() {
    let expanded = expand_user_path("~/.config/pubvia/anthropic.env");
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .expect("a home folder to expand against");
    assert_eq!(
        expanded,
        home.join(".config/pubvia/anthropic.env"),
        "the owner's own example path must resolve under home"
    );
    assert_eq!(expand_user_path("~"), home);
}

#[test]
fn an_app_link_opens_the_real_file() {
    let dir = Dir::fresh("app-link");
    dir.write("real.txt", "real\n");
    let target = dir.root.join("real.txt");
    let link = dir.root.join("link.txt");
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&target, &link);
    #[cfg(not(windows))]
    let linked = std::os::unix::fs::symlink(&target, &link);
    if linked.is_err() {
        return;
    }

    // The human's road follows the link: the real file opens, where the
    // workspace road above refuses the same spelling.
    let file = open_app_file(&link.to_string_lossy());
    assert_eq!(file.status, WorkspaceFileContentStatus::Ok);
    assert_eq!(file.content.as_deref(), Some("real\n"));
}

// ---------------------------------------------------------------------------
// The boundary: peers and agents gain nothing.
// ---------------------------------------------------------------------------

#[test]
fn every_new_frame_is_denied_to_every_peer() {
    use crate::peer_policy::{peer_allows, PeerDecision};
    use devboule_protocol::ClientMessage;

    // Whatever the paired device holds — even the full default set — the
    // editor and app-file frames are never its to send. The workspace trio
    // needs `admin` like the other file frames; the app-file and remote
    // trios are denied outright, like the open root and the host link.
    let admin = vec!["admin".to_string()];
    let request = |message: ClientMessage| peer_allows(&admin, &message);
    assert!(matches!(
        request(ClientMessage::WorkspaceFileWrite {
            id: 1,
            workspace_id: "w".to_string(),
            path: "a".to_string(),
            content: "x".to_string(),
            expected_modified_at: None,
            expected_revision: None,
        }),
        PeerDecision::Allow
    ));
    for message in [
        ClientMessage::AppFileOpen {
            id: 1,
            path: "/a".to_string(),
        },
        ClientMessage::AppFileVersion {
            id: 1,
            path: "/a".to_string(),
        },
        ClientMessage::AppFileWrite {
            id: 1,
            path: "/a".to_string(),
            content: "x".to_string(),
            expected_modified_at: None,
            expected_revision: None,
        },
        ClientMessage::RemoteHostFileOpen {
            id: 1,
            device_id: "d".to_string(),
            workspace_id: "w".to_string(),
            path: "a".to_string(),
        },
        ClientMessage::RemoteHostFileVersion {
            id: 1,
            device_id: "d".to_string(),
            workspace_id: "w".to_string(),
            path: "a".to_string(),
        },
        ClientMessage::RemoteHostFileWrite {
            id: 1,
            device_id: "d".to_string(),
            workspace_id: "w".to_string(),
            path: "a".to_string(),
            content: "x".to_string(),
            expected_modified_at: None,
            expected_revision: None,
        },
    ] {
        assert!(
            matches!(peer_allows(&admin, &message), PeerDecision::Deny(_)),
            "a peer must never reach {message:?}"
        );
    }
    // And without `admin` even the workspace write is shut.
    assert!(matches!(
        peer_allows(
            &[],
            &ClientMessage::WorkspaceFileWrite {
                id: 1,
                workspace_id: "w".to_string(),
                path: "a".to_string(),
                content: "x".to_string(),
                expected_modified_at: None,
                expected_revision: None,
            }
        ),
        PeerDecision::Deny(_)
    ));
}

#[test]
fn the_mcp_broker_serves_no_file_writing_tool() {
    // Agents reach the daemon through the broker's tools, never through
    // these frames — so the tool list is the boundary that matters. Any
    // tool whose name or description offers to write, edit or save a file
    // fails this test on purpose.
    for (name, description) in crate::provider_catalog::MCP_BROKER_TOOLS {
        let haystack = format!("{name} {description}").to_lowercase();
        for stem in [
            "file_write",
            "file_edit",
            "write_file",
            "edit_file",
            "save_file",
            "writefile",
        ] {
            assert!(
                !haystack.contains(stem),
                "the broker must serve no file-writing tool, found {stem} in {name}"
            );
        }
        assert!(
            !name.contains("file_open") && !name.contains("file_version"),
            "the broker must serve no editor frame, found {name}"
        );
    }
}

#[test]
fn the_stat_is_classified_not_booleanised() {
    let dir = Dir::fresh("classify");
    dir.write("there.txt", "x");
    assert!(matches!(
        classify_target(&dir.root.join("there.txt")),
        TargetState::Present(_)
    ));
    assert!(matches!(
        classify_target(&dir.root.join("absent.txt")),
        TargetState::Absent
    ));
    // No file can live behind an interior NUL: the stat fails with
    // anything but NotFound, which is exactly the unstatable class.
    assert!(matches!(
        classify_target(&dir.root.join("no\x00pe.txt")),
        TargetState::Unstatable
    ));
}

#[test]
fn an_unstatable_path_is_a_failed_check_not_a_missing_file() {
    let dir = Dir::fresh("unstatable");
    let weird = "no\x00pe.txt";

    // The open refuses instead of offering an empty editor over a file
    // the daemon cannot see.
    let file = open_workspace_file(&dir.root, dir.workspace_id(), weird);
    assert_eq!(file.status, WorkspaceFileContentStatus::Refused);
    assert_eq!(file.error.as_deref(), Some(UNSTATABLE));

    // The poll reports the failed check, never a missing version that
    // would arm a create.
    assert!(matches!(
        version_workspace_file(&dir.root, dir.workspace_id(), weird),
        WorkspaceFileVersion::Error { .. }
    ));

    // And a create aimed at it errors instead of replacing whatever is
    // behind the failed stat.
    let result = write_workspace_file(&dir.root, dir.workspace_id(), weird, b"x", None, None);
    assert!(
        matches!(
            &result,
            WorkspaceFileWriteResult::Error { error } if error == UNSTATABLE
        ),
        "{result:?}"
    );
}
