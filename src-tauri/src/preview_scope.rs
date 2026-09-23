//! The asset-protocol concession the config cannot spell: the `previews`
//! folder of the runtime directory **this process** resolves.
//!
//! `tauri.conf.json` concedes `$CACHE/Devboule/previews/*` — the default
//! runtime dir, fixed at build time. The daemon stages wherever
//! `RuntimePaths::from_env` says, and that function prefers
//! `DEVBOULE_RUNTIME_DIR` — the same override this app's own client
//! resolves (`client/mod.rs`) and the spawn passes on (`spawn.rs`), so
//! with the variable set the copies land in a folder the static scope does
//! not reach: every preview would answer 403 while the folder the scope
//! *does* concede stays empty. The scope API has no `disallow_file`, but
//! it has this: a concession added at start, beside the static one,
//! over the directory the process will actually use.
//!
//! What is granted is one folder and the files directly inside it — never
//! a subfolder, never a sibling, never a workspace — and the folder need
//! not exist yet when this runs: `push_pattern` canonicalizes the longest
//! prefix that exists and re-appends the tail (`tauri/scope/fs.rs`), so
//! the pattern is the same shape whether the daemon has started or not.

use std::path::Path;
use tauri::Manager;

/// Concede the `previews` folder of the runtime dir **this process**
/// resolves — the entry [`crate::run`] calls at start, so the override and
/// the default end up conceded alike.
pub fn concede_runtime_previews<R: tauri::Runtime>(app: &impl Manager<R>) {
    match devboule_daemon::RuntimePaths::from_env() {
        Ok(paths) => concede_previews_of(app, &paths.dir),
        Err(error) => {
            eprintln!("devboule: preview scope could not resolve the runtime dir: {error}")
        }
    }
}

/// The concession itself, over one runtime directory — the door the
/// integration test drives with a directory that is not this
/// environment's default, which is the whole case the static scope misses.
///
/// `recursive: false` is the whole width of it: the folder and the files
/// directly inside — the flat folder this daemon writes
/// (`workspace_file_preview`) — with no subfolder and nothing above it.
/// A failure is printed, never swallowed silently: an unconceded folder
/// means every preview answers 403, and the operator should read why.
pub fn concede_previews_of<R: tauri::Runtime>(app: &impl Manager<R>, runtime_dir: &Path) {
    let previews = runtime_dir.join("previews");
    if let Err(error) = app.asset_protocol_scope().allow_directory(&previews, false) {
        eprintln!(
            "devboule: preview scope could not concede {}: {error}",
            previews.display()
        );
    }
}
