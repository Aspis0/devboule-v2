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
    CREATE_INTENT, LINK_TARGET_MISSING, MAX_EDITABLE_FILE_BYTES, PARENT_MISSING, READ_ONLY,
    TOO_LARGE, UNSTATABLE,
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
        false,
    );

    match result {
        WorkspaceFileWriteResult::Written {
            modified_at: _,
            warning: None,
            size,
            revision,
        } => {
            // A clean write preserves everything and says nothing: the
            // warning only rides failures (pinned separately).
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
        false,
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
        true,
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

    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "a.txt",
        b"new\n",
        None,
        None,
        false,
    );

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
        true,
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
        false,
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
        create: false,
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
        false,
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
        false,
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
        false,
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

    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "sub",
        b"x",
        Some(0),
        None,
        false,
    );
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
        false,
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
        write_app_file(&missing, b"new\n", None, None, true),
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
        write_app_file("relative/path.txt", b"x", None, None, false),
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
            create: false,
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
            create: false,
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
            create: false,
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
                create: false,
            }
        ),
        PeerDecision::Deny(_)
    ));
}

// The agent/tools boundary moved to the broker's own tests
// (`mcp_broker/file_edit_boundary_tests.rs`): a call-graph pin plus the
// served surface over the real HTTP road, instead of this name scan.

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
    let result = write_workspace_file(&dir.root, dir.workspace_id(), weird, b"x", None, None, true);
    assert!(
        matches!(
            &result,
            WorkspaceFileWriteResult::Error { error } if error == UNSTATABLE
        ),
        "{result:?}"
    );
}

#[test]
fn an_app_write_lands_in_the_target_and_keeps_the_link() {
    let dir = Dir::fresh("app-write-link");
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

    let opened = open_app_file(&link.to_string_lossy());
    let (_, modified_at, revision) = ready_of(opened.version.as_ref().expect("version"));
    let result = write_app_file(
        &link.to_string_lossy(),
        b"through\n",
        Some(modified_at),
        revision.as_deref(),
        false,
    );
    assert!(
        matches!(result, WorkspaceFileWriteResult::Written { .. }),
        "{result:?}"
    );
    // The bytes reached the target, and the link is still a link: the
    // write replaced content, never the entry.
    assert_eq!(std::fs::read(&target).expect("read back"), b"through\n");
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("link stat")
            .file_type()
            .is_symlink(),
        "the link must survive the write"
    );
}

#[test]
fn a_dangling_link_is_an_error_never_a_create() {
    let dir = Dir::fresh("app-dangling");
    let missing = dir.root.join("gone.txt");
    let link = dir.root.join("link.txt");
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&missing, &link);
    #[cfg(not(windows))]
    let linked = std::os::unix::fs::symlink(&missing, &link);
    if linked.is_err() {
        return;
    }
    let spelling = link.to_string_lossy().into_owned();

    // Not an empty editor over a "missing" file: the link names
    // something that is not there.
    let file = open_app_file(&spelling);
    assert_eq!(file.status, WorkspaceFileContentStatus::Refused);
    assert_eq!(file.error.as_deref(), Some(LINK_TARGET_MISSING));

    // Not a missing version either, so no create can arm on it.
    assert!(matches!(
        version_app_file(&spelling),
        WorkspaceFileVersion::Error { .. }
    ));
    assert!(matches!(
        write_app_file(&spelling, b"x", None, None, true),
        WorkspaceFileWriteResult::Error { .. }
    ));
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("link stat")
            .file_type()
            .is_symlink(),
        "a failed write must not touch the link"
    );
}

#[test]
fn a_read_only_file_refuses_the_write_with_its_sentence() {
    let dir = Dir::fresh("readonly");
    dir.write("a.txt", "one\n");
    let target = dir.root.join("a.txt");
    let previous = std::fs::metadata(&target).expect("stat").permissions();
    let mut permissions = previous.clone();
    permissions.set_readonly(true);
    std::fs::set_permissions(&target, permissions).expect("readonly");

    // The file still opens: it looks editable, which is exactly why the
    // save must name its refusal.
    let file = open_workspace_file(&dir.root, dir.workspace_id(), "a.txt");
    assert_eq!(file.status, WorkspaceFileContentStatus::Ok);
    let (_, modified_at, revision) = ready_of(file.version.as_ref().expect("version"));

    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "a.txt",
        b"two\n",
        Some(modified_at),
        revision.as_deref(),
        false,
    );
    assert!(
        matches!(
            &result,
            WorkspaceFileWriteResult::Error { error } if error == READ_ONLY
        ),
        "{result:?}"
    );
    // Restored before the asserts so the temp dir below always cleans
    // up, on Windows as well as Unix.
    std::fs::set_permissions(&target, previous).expect("writable");
    assert_eq!(
        std::fs::read(&target).expect("read back"),
        b"one\n",
        "a refused write moves no byte"
    );
}

/// An edited hidden file stays hidden and keeps its creation time: the
/// temp carries the target's identity across the rename instead of
/// inheriting the directory's.
#[cfg(windows)]
#[test]
fn a_replace_keeps_attributes_and_creation_time() {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};

    let dir = Dir::fresh("identity");
    dir.write("a.txt", "one\n");
    let target = dir.root.join("a.txt");
    let wide: Vec<u16> = target.as_os_str().encode_wide().chain([0]).collect();
    assert_ne!(
        unsafe { SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_HIDDEN) },
        0,
        "setup: hide the file"
    );
    let created_before = std::fs::metadata(&target)
        .expect("stat")
        .created()
        .expect("creation time");

    let file = open_workspace_file(&dir.root, dir.workspace_id(), "a.txt");
    let (_, modified_at, revision) = ready_of(file.version.as_ref().expect("version"));
    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "a.txt",
        b"two\n",
        Some(modified_at),
        revision.as_deref(),
        false,
    );
    assert!(
        matches!(result, WorkspaceFileWriteResult::Written { .. }),
        "{result:?}"
    );

    let after = std::fs::metadata(&target).expect("stat");
    assert_ne!(
        after.file_attributes() & FILE_ATTRIBUTE_HIDDEN,
        0,
        "hidden survives the replace"
    );
    assert_eq!(
        after.created().expect("creation time"),
        created_before,
        "creation time survives the replace"
    );
}

/// The temp takes the target's owner and group across the rename.
#[cfg(unix)]
#[test]
fn a_replace_keeps_owner() {
    use std::os::unix::fs::MetadataExt;
    let dir = Dir::fresh("owner");
    dir.write("a.txt", "one\n");
    let target = dir.root.join("a.txt");
    let before = std::fs::metadata(&target).expect("stat");

    let file = open_workspace_file(&dir.root, dir.workspace_id(), "a.txt");
    let (_, modified_at, revision) = ready_of(file.version.as_ref().expect("version"));
    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "a.txt",
        b"two\n",
        Some(modified_at),
        revision.as_deref(),
        false,
    );
    assert!(
        matches!(result, WorkspaceFileWriteResult::Written { .. }),
        "{result:?}"
    );

    let after = std::fs::metadata(&target).expect("stat");
    assert_eq!((after.uid(), after.gid()), (before.uid(), before.gid()));
}

#[test]
fn write_failures_name_their_class() {
    use std::io;
    let sentence = super::io_sentence(&io::Error::from(io::ErrorKind::PermissionDenied));
    assert_eq!(sentence, "permission denied");
    let sentence = super::io_sentence(&io::Error::from(io::ErrorKind::StorageFull));
    assert_eq!(sentence, "the disk is full");
    let sentence = super::io_sentence(&io::Error::from(io::ErrorKind::ResourceBusy));
    assert_eq!(sentence, "the file is in use");
    // The opaque fallback stays for everything unclassified — and still
    // names no path.
    let sentence = super::io_sentence(&io::Error::from(io::ErrorKind::BrokenPipe));
    assert_eq!(sentence, "the file could not be written");

    #[cfg(windows)]
    {
        assert_eq!(
            super::io_sentence(&io::Error::from_raw_os_error(5)),
            "permission denied"
        );
        assert_eq!(
            super::io_sentence(&io::Error::from_raw_os_error(32)),
            "the file is in use"
        );
        assert_eq!(
            super::io_sentence(&io::Error::from_raw_os_error(112)),
            "the disk is full"
        );
    }
    #[cfg(unix)]
    {
        assert_eq!(
            super::io_sentence(&io::Error::from_raw_os_error(13)),
            "permission denied"
        );
        assert_eq!(
            super::io_sentence(&io::Error::from_raw_os_error(28)),
            "the disk is full"
        );
        assert_eq!(
            super::io_sentence(&io::Error::from_raw_os_error(30)),
            "the filesystem is read-only"
        );
    }
}

#[test]
fn temp_names_sanitize_readonly_characters() {
    assert_eq!(super::temp_stem(std::path::Path::new("a.txt")), "a.txt");
    assert_eq!(
        super::temp_stem(std::path::Path::new("file.txt:stream")),
        "file.txt_stream"
    );
    assert_eq!(
        super::temp_stem(std::path::Path::new("a/b?c*.txt")),
        "b_c_.txt"
    );
}

/// Move a file's mtime 120 seconds into the past, past the sweep's
/// grace, so the test can plant litter instead of waiting a minute.
fn backdate_120s(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let raw = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("path");
        let old = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs() as libc::time_t
            - 120;
        let times = [
            libc::timespec {
                tv_sec: old,
                tv_nsec: 0,
            },
            libc::timespec {
                tv_sec: old,
                tv_nsec: 0,
            },
        ];
        // SAFETY: `raw` is NUL-terminated and alive for the call;
        // `AT_FDCWD` takes the path as given; the return is asserted.
        assert_eq!(
            unsafe { libc::utimensat(libc::AT_FDCWD, raw.as_ptr(), times.as_ptr(), 0) },
            0,
            "backdate"
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, GENERIC_WRITE};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, SetFileTime, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE, OPEN_EXISTING,
        };
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let old_100ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos() as u64
            / 100
            + 116_444_736_000_000_000
            - 120 * 10_000_000;
        let filetime = FILETIME {
            dwLowDateTime: (old_100ns & 0xFFFF_FFFF) as u32,
            dwHighDateTime: (old_100ns >> 32) as u32,
        };
        // SAFETY: handles checked against INVALID_HANDLE_VALUE and
        // closed; the FILETIME is a plain value.
        unsafe {
            let handle = CreateFileW(
                wide.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            );
            assert_ne!(
                handle,
                windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE,
                "open for backdate"
            );
            assert_ne!(
                SetFileTime(handle, std::ptr::null(), std::ptr::null(), &filetime),
                0,
                "backdate"
            );
            CloseHandle(handle);
        }
    }
}

#[test]
fn a_write_sweeps_only_its_own_stale_litter() {
    let dir = Dir::fresh("sweep");
    dir.write("a.txt", "one\n");
    // A kill between stage and rename would leave exactly these: one
    // past the grace (litter), one fresh (possibly in flight), one for
    // another file (not this write's business).
    let stale = dir.root.join(".a.txt.devboule-deadbeef.tmp");
    let fresh = dir.root.join(".a.txt.devboule-fresh.tmp");
    let other = dir.root.join(".b.txt.devboule-deadbeef.tmp");
    std::fs::write(&stale, b"litter").expect("plant");
    std::fs::write(&fresh, b"in flight").expect("plant");
    std::fs::write(&other, b"other").expect("plant");
    backdate_120s(&stale);
    backdate_120s(&other);

    let file = open_workspace_file(&dir.root, dir.workspace_id(), "a.txt");
    let (_, modified_at, revision) = ready_of(file.version.as_ref().expect("version"));
    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "a.txt",
        b"two\n",
        Some(modified_at),
        revision.as_deref(),
        false,
    );
    assert!(
        matches!(result, WorkspaceFileWriteResult::Written { .. }),
        "{result:?}"
    );

    assert!(!stale.exists(), "stale litter is swept");
    assert!(fresh.exists(), "a fresh temp may still be in flight");
    assert!(other.exists(), "another file's litter is not this write's");
    std::fs::remove_file(&fresh).expect("cleanup");
    std::fs::remove_file(&other).expect("cleanup");
}

#[test]
fn a_versionless_write_without_create_creates_nothing() {
    let dir = Dir::fresh("create-intent");
    std::fs::create_dir_all(dir.root.join("sub")).expect("parent");

    // The file is genuinely missing and the parent exists — but without
    // an explicit create intent the write is refused, so a stale or
    // replayed versionless frame can never conjure a file.
    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "sub/new.txt",
        b"born\n",
        None,
        None,
        false,
    );
    assert!(
        matches!(
            &result,
            WorkspaceFileWriteResult::Error { error } if error == CREATE_INTENT
        ),
        "{result:?}"
    );
    assert!(!dir.root.join("sub/new.txt").exists());
}

#[test]
fn relativize_keeps_relative_spellings_untouched() {
    use crate::workspace_git_support::relativize;
    let root = std::path::Path::new("/repo");
    assert_eq!(relativize(root, "sub/a.txt"), Some("sub/a.txt".to_string()));
    assert_eq!(
        relativize(root, "./sub/a.txt"),
        Some("./sub/a.txt".to_string())
    );
}

#[test]
fn relativize_maps_inside_absolute_spellings_and_refuses_the_rest() {
    use crate::workspace_git_support::relativize;
    // Platform-absolute paths throughout: a bare `/x` is drive-relative
    // on Windows, so absolutes are built by joining (absolute on every
    // platform) under the guarded fixture dir, never `std::env::temp_dir`.
    let dir = Dir::fresh("relativize-unit");
    let root = &dir.root;
    let inside = root.join("sub").join("a.txt");
    assert_eq!(
        relativize(&root, &inside.to_string_lossy()),
        Some("sub/a.txt".to_string())
    );
    let dotted = root.join(".").join("a.txt");
    assert_eq!(
        relativize(&root, &dotted.to_string_lossy()),
        Some("a.txt".to_string())
    );
    // A sibling that merely shares the prefix is outside.
    let sibling = root
        .with_file_name("devboule-relativize-sibling")
        .join("a.txt");
    assert_eq!(relativize(&root, &sibling.to_string_lossy()), None);
    // Above the root, with or without `..` help, is outside.
    let above = root.parent().expect("temp has a parent").join("other.txt");
    assert_eq!(relativize(&root, &above.to_string_lossy()), None);
    let dotdot = root.join("sub").join("..").join("other.txt");
    assert_eq!(relativize(&root, &dotdot.to_string_lossy()), None);
    // The folder itself relativizes empty, which confinement refuses.
    assert_eq!(
        relativize(&root, &root.to_string_lossy()),
        Some(String::new())
    );
}

#[test]
fn an_absolute_inside_spelling_opens_reads_and_writes() {
    let dir = Dir::fresh("absolute-inside");
    dir.write("sub/a.txt", "one\n");
    let absolute = dir
        .root
        .join("sub")
        .join("a.txt")
        .to_string_lossy()
        .into_owned();

    // The cwd-subdirectory case: the link resolved against
    // `<root>/sub`, opened as the file it names — never as the
    // root-relative lookalike, which a save could then create.
    let file = open_workspace_file(&dir.root, dir.workspace_id(), &absolute);
    assert_eq!(file.status, WorkspaceFileContentStatus::Ok);
    assert_eq!(file.content.as_deref(), Some("one\n"));
    let (_, modified_at, revision) = ready_of(file.version.as_ref().expect("version"));

    let version = version_workspace_file(&dir.root, dir.workspace_id(), &absolute);
    assert!(matches!(version, WorkspaceFileVersion::Ready { .. }));

    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        &absolute,
        b"two\n",
        Some(modified_at),
        revision.as_deref(),
        false,
    );
    assert!(
        matches!(result, WorkspaceFileWriteResult::Written { .. }),
        "{result:?}"
    );
    assert_eq!(
        std::fs::read(dir.root.join("sub/a.txt")).expect("read back"),
        b"two\n",
        "the write lands in the named file, and nothing is created beside it"
    );
    assert!(
        !dir.root.join("a.txt").exists(),
        "no root-relative lookalike may appear"
    );
}

#[test]
fn an_absolute_outside_spelling_is_refused_on_every_road() {
    let dir = Dir::fresh("absolute-outside");
    dir.write("sub/a.txt", "one\n");
    // A sibling of the root: inside no spelling of it.
    let outside = dir.root.join("..").join("elsewhere.txt");
    let spelling = outside.to_string_lossy().into_owned();

    let file = open_workspace_file(&dir.root, dir.workspace_id(), &spelling);
    assert_eq!(file.status, WorkspaceFileContentStatus::Refused);
    assert!(matches!(
        version_workspace_file(&dir.root, dir.workspace_id(), &spelling),
        WorkspaceFileVersion::Error { .. }
    ));
    assert!(matches!(
        write_workspace_file(
            &dir.root,
            dir.workspace_id(),
            &spelling,
            b"x",
            None,
            None,
            true
        ),
        WorkspaceFileWriteResult::Error { .. }
    ));
}

#[test]
fn identity_preservation_reports_instead_of_silence() {
    // A target that cannot even be statted names its shortfall: the
    // write would land the bytes on a temp whose ownership is unknown.
    let missing = std::path::Path::new("/devboule/never/there.txt");
    let temp = std::path::Path::new("/devboule/never/there.tmp");
    // Unix stats the owner first; Windows reads attributes first. Either
    // way a missing target reports instead of preserving silence.
    #[cfg(unix)]
    assert_eq!(
        super::preserve_identity(missing, temp),
        Some("the file's owner could not be read")
    );
    #[cfg(windows)]
    assert_eq!(
        super::preserve_identity(missing, temp),
        Some("the file's attributes could not be read")
    );
}

#[test]
fn a_clean_write_carries_no_warning() {
    let dir = Dir::fresh("no-warning");
    dir.write("a.txt", "one\n");
    let file = open_workspace_file(&dir.root, dir.workspace_id(), "a.txt");
    let (_, modified_at, revision) = ready_of(file.version.as_ref().expect("version"));
    let result = write_workspace_file(
        &dir.root,
        dir.workspace_id(),
        "a.txt",
        b"two\n",
        Some(modified_at),
        revision.as_deref(),
        false,
    );
    assert!(
        matches!(
            result,
            WorkspaceFileWriteResult::Written { warning: None, .. }
        ),
        "{result:?}"
    );
}
