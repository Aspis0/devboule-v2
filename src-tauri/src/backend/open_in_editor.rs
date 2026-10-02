//! Asking this machine to open one workspace file: the daemon resolves the
//! workspace id to its root, this side resolves the chosen target, contains
//! the file against that root, and launches — all inside one
//! off-window-thread wait, with the canonical path never leaving the process.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use devboule_protocol::ErrorCode;
use tauri::State;
use tauri_plugin_opener::OpenerExt;

use super::blocking::off_main_thread;
use super::editor_targets::{self, Launch, Resolved};
use super::error::CommandError;
use super::workspace::require_client;
use crate::client::DaemonBridge;

/// The one sentence per refusal, static like the daemon's own: an OS error's
/// own text names the path it failed on, and no path may reach the UI.
const MISSING: &str = "the file no longer exists in the workspace";
const ACCESS_DENIED: &str = "this machine denied access to this file";
const UNRESOLVED: &str = "the file could not be resolved";
const ROOT_GONE: &str = "the workspace folder is not on this machine";
const OUTSIDE: &str = "the file resolved outside the workspace folder";
const NOT_A_FILE: &str = "the target is not a regular file";
const NO_TARGET: &str = "the chosen editor is not installed on this machine";
const INSIDE_WORKSPACE: &str = "the chosen editor is inside this workspace";
const LAUNCH_FAILED: &str = "the editor could not be started";
const REVEAL_FAILED: &str = "the file manager could not be opened";

/// A file proven to be the workspace's: both paths canonical, the file
/// under the root, and a regular file — the identity the launch argv is
/// spelled from.
#[derive(Debug)]
pub(crate) struct Validated {
    pub root: PathBuf,
    pub file: PathBuf,
}

/// Closes every pre-existing link — symlinked root or child; a same-user
/// writer swapping the path after this line is the accepted residual.
pub(crate) fn validated_file(root: &Path, relative: &str) -> Result<Validated, CommandError> {
    let canonical_root = std::fs::canonicalize(root).map_err(root_stat_error)?;
    let canonical_file =
        std::fs::canonicalize(canonical_root.join(relative)).map_err(file_stat_error)?;
    if !canonical_file.starts_with(&canonical_root) {
        return Err(CommandError::new(ErrorCode::Io, OUTSIDE));
    }
    if !std::fs::metadata(&canonical_file)
        .map_err(file_stat_error)?
        .is_file()
    {
        return Err(CommandError::new(ErrorCode::Io, NOT_A_FILE));
    }
    Ok(Validated {
        root: canonical_root,
        file: canonical_file,
    })
}

/// A stat the workspace file could not answer, by the kind that matters:
/// gone and denied are different sentences.
pub(crate) fn file_stat_error(error: std::io::Error) -> CommandError {
    match error.kind() {
        ErrorKind::PermissionDenied => CommandError::new(ErrorCode::Io, ACCESS_DENIED),
        ErrorKind::NotFound => CommandError::new(ErrorCode::Io, MISSING),
        _ => CommandError::new(ErrorCode::Io, UNRESOLVED),
    }
}

/// The root's own stat: a folder this machine cannot see is the workspace's
/// availability question, not the file's.
fn root_stat_error(error: std::io::Error) -> CommandError {
    match error.kind() {
        ErrorKind::PermissionDenied => CommandError::new(ErrorCode::Io, ACCESS_DENIED),
        _ => CommandError::new(ErrorCode::WorkspaceUnavailable, ROOT_GONE),
    }
}

/// An executable inside the workspace it opens is refused: a checkout on
/// PATH could otherwise name itself and be launched by its own pencil.
pub(crate) fn trusted_program(program: &Path, canonical_root: &Path) -> Result<(), CommandError> {
    if program.starts_with(canonical_root) {
        return Err(CommandError::new(ErrorCode::Io, INSIDE_WORKSPACE));
    }
    Ok(())
}

pub(crate) fn open(
    app: &tauri::AppHandle,
    root: &Path,
    relative: &str,
    line: Option<u32>,
    target_id: &str,
) -> Result<(), CommandError> {
    let probe = editor_targets::Probe::from_env();
    let resolved = editor_targets::resolve(target_id, &probe)
        .ok_or_else(|| CommandError::new(ErrorCode::Io, NO_TARGET))?;
    let validated = validated_file(root, relative)?;
    if let Resolved::Executable { program, .. } = &resolved {
        trusted_program(program, &validated.root)?;
    }
    match editor_targets::launch_plan(&resolved, &validated.root, &validated.file, line) {
        Launch::Reveal => app
            .opener()
            .reveal_item_in_dir(&validated.file)
            .map_err(|_| CommandError::new(ErrorCode::Io, REVEAL_FAILED)),
        Launch::Spawn(plan) => editor_targets::spawn(&plan)
            .map_err(|_| CommandError::new(ErrorCode::Io, LAUNCH_FAILED)),
    }
}

/// Open one workspace file in a chosen editor — the pencil in the File and
/// Diff tab headers, with the diff's first hunk line where it has one.
/// `workspace_id` names the folder (the daemon resolves it; the frontend's
/// own path is display-only) and `path` stays relative: the root arrives
/// from the daemon, everything after it happens here, and neither the root
/// nor the canonical file crosses to the webview. The whole act — the
/// daemon round trip, the containment, the launch — leaves the window's
/// thread together through one `off_main_thread` call.
#[tauri::command]
pub async fn workspace_file_open(
    app: tauri::AppHandle,
    bridge: State<'_, DaemonBridge>,
    workspace_id: String,
    path: String,
    line: Option<u32>,
    target_id: String,
) -> Result<(), CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || {
        let root = client.workspace_open_root(&workspace_id)?;
        open(&app, Path::new(&root), &path, line, &target_id)
    })
    .await
}
