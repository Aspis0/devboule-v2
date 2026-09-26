//! The broker's own files on disk: protected writes and the stale-config sweep.

use std::fs;
use std::io;
use std::path::Path;

use serde_json::Value;

pub(super) const CONFIG_PREFIX: &str = "devboule-mcp-";

/// The one protected-bytes primitive every carrier writer uses (S4) now lives
/// in `crate::atomic` (P2: one writer, all callers). Both format wrappers below
/// go through it.
pub(super) fn write_protected_json(path: &Path, value: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    crate::atomic::write_protected_bytes(path, &bytes)
}

/// The second wrapper (S4): text carriers (the pi bridge in S5, the Codex TOML
/// in S6) go through the same primitive. The TOML parse-back before rename
/// that S6 needs is S6's hook on top of this; this step owns the helper it calls.
pub(crate) fn write_protected_str(path: &Path, text: &str) -> io::Result<()> {
    crate::atomic::write_protected_bytes(path, text.as_bytes())
}

pub(super) fn cleanup_stale_configs(runtime_dir: &Path) -> io::Result<()> {
    let Ok(entries) = fs::read_dir(runtime_dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // Legacy Codex homes (S6, retired): the `-c` carrier writes no home
        // any more, so whole trees by our name alone — never by content,
        // never outside our names — are leftovers from older builds, and
        // sweeping them keeps a crashed spawn's goals, sqlite and
        // `installation_id` from accumulating. At daemon start no live
        // session exists, so every match is an orphan by construction.
        if name.starts_with("devboule-codex-home-") {
            let _ = fs::remove_dir_all(&path);
            continue;
        }
        let stale = {
            // Our carriers, by our names: the Claude config plus the pi
            // permission/bridge files S5 writes beside it (and their temps).
            // These are the daemon's own file names, not provider dispatch —
            // no behaviour branches on them — so listing them here keeps the
            // provider dimension open while orphans from a dead daemon (or a
            // crashed spawn) cannot accumulate. At daemon start no live session
            // exists, so every match is an orphan by construction. Never match
            // by content, and never sweep a tree we do not own (the Codex home
            // dir in S6 gets its own owned-dir removal for the same reason).
            (name.starts_with(CONFIG_PREFIX) && (name.ends_with(".json") || name.ends_with(".tmp")))
                || (name.starts_with("devboule-pi-permissions-")
                    && (name.ends_with(".ts") || name.ends_with(".tmp")))
                || (name.starts_with("devboule-pi-bridge-")
                    && (name.ends_with(".ts") || name.ends_with(".tmp")))
        };
        if stale {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

pub(super) fn remove_file(path: Option<&Path>) {
    if let Some(path) = path {
        let _ = fs::remove_file(path);
    }
}
