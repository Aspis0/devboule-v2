//! One session's Codex home: an empty directory the child runs in, so the tool
//! lane it can see is this daemon's broker and not whatever the human's own
//! `~/.codex` configures (measured: a session started here offered the user's
//! own MCP servers, and the agent reached for one of those instead of the
//! broker's `browser_new_tab`).
//!
//! The lifecycle mirrors Orca's managed Codex home
//! (`refs/orca-src/src/main/codex-accounts/codex-managed-home-lifecycle.ts`,
//! Apache-2.0): a marker file that lets cleanup *prove* a directory is ours
//! before removing it, credentials imported from the real home rather than
//! cached by the daemon, and a removal that keeps the home instead of risking a
//! half-deleted credential file. Their Electron and WSL specifics have no
//! counterpart here.
//!
//! Two facts shape what the home does not carry. The user's `config.toml` is
//! never handed over: nothing in this daemon reads it, no feature depends on a
//! custom model provider (the delivered model is judged against the catalog the
//! handshake brings back), and editing TOML to strip it would be a text edit
//! that can mangle nested tables. And the home lives as long as the session
//! ROW rather than as long as one child: `thread/resume` loads the thread from
//! disk under the home the running child used, so a home deleted when the child
//! ended would take the conversation with it — which is why [`sweep`], not the
//! teardown, is where a home goes.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::thread::sleep;
#[cfg(windows)]
use std::time::Duration;

/// The prefix every managed home carries under the runtime dir, so the sweep
/// can find them without a journal read.
const HOME_PREFIX: &str = "codex-home-";

/// The file whose content is the session id this home belongs to. Cleanup
/// refuses to delete a directory whose marker does not name the session whose
/// home it is removing, which is what keeps a mistyped or foreign path from
/// taking a real home — or any other directory — with it.
const MARKER_NAME: &str = "devboule-managed-home";

/// Where the credentials live in a Codex home. Copied at every spawn from the
/// profile the child itself resolves (an overridden `USERPROFILE` is not that
/// profile — see `codex_command_catalog::home_dir`), so a home without one is
/// the profile's own state, not a copy that missed it.
const AUTH_NAME: &str = "auth.json";

/// Windows releases a handle late — antivirus, the search indexer, a child that
/// has only just exited — so a recursive removal can fail once and succeed a
/// moment later. Eight attempts 150 ms apart, the bound Orca settled on; a
/// removal that still fails is kept, not retried forever.
#[cfg(windows)]
const REMOVE_ATTEMPTS: usize = 8;
#[cfg(windows)]
const REMOVE_RETRY_DELAY: Duration = Duration::from_millis(150);

/// The home for `session_id`, created on first spawn and reused after that.
///
/// A resumed generation must land on the same directory as the generation that
/// wrote the rollout, so the search is by marker rather than by name: the
/// directory carries a uuid (nothing a session id can spell is trusted as a
/// path component) and the marker is what says whose home it is.
pub(crate) fn create(
    runtime_dir: &Path,
    session_id: &str,
    credentials_from: &Path,
) -> io::Result<PathBuf> {
    let home = match existing_home(runtime_dir, session_id) {
        Some(home) => home,
        None => {
            let home = runtime_dir.join(format!("{HOME_PREFIX}{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&home)?;
            fs::write(home.join(MARKER_NAME), format!("{session_id}\n"))?;
            home
        }
    };
    // A fresh copy per spawn, not a fill-in: the child may refresh its own
    // token inside this home, and the next spawn starts from the real one.
    import_auth(credentials_from, &home)?;
    Ok(home)
}

/// The home this row already owns, if any. `None` for a row whose home is gone
/// — never minted, or swept once its row stopped existing.
fn existing_home(runtime_dir: &Path, session_id: &str) -> Option<PathBuf> {
    homes(runtime_dir).find(|home| marker_session(home).as_deref() == Some(session_id))
}

/// Remove every managed home that claims `session_id`.
///
/// Called when the row itself is gone. A session whose child ended stays
/// resumable, and that resume reads the rollout off this disk, so the child
/// ending is not the moment its home may go.
pub(crate) fn remove_for_session(runtime_dir: &Path, session_id: &str) {
    for home in homes(runtime_dir) {
        remove(&home, session_id);
    }
}

/// Remove the managed homes whose row the journal no longer holds: the rows a
/// crash left behind, and the rows deleted while the daemon was not running.
///
/// Returns how many went. Best effort by design — a home nobody can name is a
/// few megabytes on disk, and a removal that guesses takes a credential file
/// with it.
pub(crate) fn sweep(runtime_dir: &Path, live_sessions: &HashSet<String>) -> usize {
    homes(runtime_dir)
        .filter_map(|home| marker_session(&home).map(|session_id| (home, session_id)))
        .filter(|(_, session_id)| !live_sessions.contains(session_id))
        .filter(|(home, session_id)| remove(home, session_id))
        .count()
}

/// Every managed home under the runtime dir, whether or not its marker reads.
fn homes(runtime_dir: &Path) -> impl Iterator<Item = PathBuf> {
    let entries = fs::read_dir(runtime_dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(HOME_PREFIX))
                })
                .collect::<Vec<PathBuf>>()
        })
        .unwrap_or_default();
    entries.into_iter()
}

/// The session id a managed home claims, or `None` for a directory carrying no
/// marker we can read — which is exactly the case removal refuses.
fn marker_session(home: &Path) -> Option<String> {
    let session_id = fs::read_to_string(home.join(MARKER_NAME))
        .ok()?
        .trim()
        .to_string();
    (!session_id.is_empty()).then_some(session_id)
}

/// Delete one managed home tree, and only where its marker names `session_id`.
///
/// The marker check is the whole safety argument: every path this file is
/// reached with is composed here, but a directory that arrives without a
/// marker naming the session in hand is somebody else's and is left exactly
/// where it is.
fn remove(home: &Path, session_id: &str) -> bool {
    if marker_session(home).as_deref() != Some(session_id) {
        return false;
    }
    match remove_tree(home) {
        Ok(()) => true,
        // A kept home is a recoverable leak; a credential file this process
        // cannot finish deleting is not, so this reports instead of forcing
        // anything.
        Err(error) => {
            eprintln!(
                "kept the Codex home {}: {error} (its credentials stay readable there)",
                home.display()
            );
            false
        }
    }
}

#[cfg(windows)]
fn remove_tree(home: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::remove_dir_all(home) {
            Ok(()) => return Ok(()),
            // Already gone: the outcome this call wanted.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) if is_transient_lock(&error) && attempt < REMOVE_ATTEMPTS => {
                attempt += 1;
                sleep(REMOVE_RETRY_DELAY);
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(not(windows))]
fn remove_tree(home: &Path) -> io::Result<()> {
    match fs::remove_dir_all(home) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        outcome => outcome,
    }
}

/// Whether an error is Windows releasing a handle late rather than a real
/// refusal. Only those are worth another attempt: `ERROR_SHARING_VIOLATION`,
/// `ERROR_DIR_NOT_EMPTY` and `ERROR_ACCESS_DENIED` are the three a
/// still-running descendant produces.
#[cfg(windows)]
fn is_transient_lock(error: &io::Error) -> bool {
    const TRANSIENT: &[i32] = &[32, 145, 5];
    TRANSIENT.contains(&error.raw_os_error().unwrap_or(0))
}

/// Copy `auth.json` from the human's home into the managed one, atomically.
///
/// The source is the profile the child resolves for itself, so this misses the
/// credential only when the profile has none — and then an absent source is not
/// a failure: a host that authenticates through `OPENAI_API_KEY` has no file to
/// copy, and refusing the spawn there would break the one case where the child
/// was never going to read it. Anything else — a locked or unreadable source —
/// is an error, because that error is what a human reads as "no credentials
/// here, run codex login" when they have already logged in.
fn import_auth(credentials_from: &Path, managed_home: &Path) -> io::Result<()> {
    let bytes = match fs::read(credentials_from.join(AUTH_NAME)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    crate::atomic::atomic_write(&managed_home.join(AUTH_NAME), &bytes)
}

#[cfg(test)]
#[path = "codex_managed_home_tests.rs"]
mod tests;
