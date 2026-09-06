//! Tauri commands for per-surface settings persistence.
//!
//! Each surface (a panel that wants to remember its own state) stores one
//! opaque JSON document under
//! `<app_config_dir>/surface-settings/<surfaceId>.json`. The backend never
//! inspects the value's shape: it stores whatever JSON arrives verbatim and
//! hands it back unchanged.

use std::fmt::Display;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tauri::Manager;

use devboule_protocol::ErrorCode;

use crate::backend::error::CommandError;

const SURFACE_SETTINGS_DIR: &str = "surface-settings";
/// Serialized values above this are rejected. ~64 KB is ample for a panel's
/// saved state and keeps a runaway surface from filling the config directory.
const MAX_SURFACE_SETTINGS_BYTES: usize = 64 * 1024;
const MAX_SURFACE_ID_LEN: usize = 32;
/// Windows reserved device stems, denied in their exact extensionless form.
/// On modern Windows `con.json` is a regular file — the reserved-name rule
/// applies to the final path component without an extension — and the id is
/// only ever used as `<id>.json`, so this deny is not exploitable today. It
/// stays denied for defense-in-depth: older Windows releases and unusual
/// filesystem drivers have not always drawn the extension line the same way.
const WINDOWS_RESERVED_DEVICE_STEMS: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

fn invalid_request(message: impl Into<String>) -> CommandError {
    CommandError::new(ErrorCode::InvalidRequest, message)
}

fn internal_error(context: &str, error: impl Display) -> CommandError {
    CommandError::new(ErrorCode::Internal, format!("{context}: {error}"))
}

fn resolved_config_dir(app: &tauri::AppHandle) -> Result<PathBuf, CommandError> {
    app.path()
        .app_config_dir()
        .map_err(|error| internal_error("the application config directory is unavailable", error))
}

fn settings_path(config_dir: &Path, surface_id: &str) -> PathBuf {
    config_dir
        .join(SURFACE_SETTINGS_DIR)
        .join(format!("{surface_id}.json"))
}

/// `surfaceId` becomes a filename component, so it is restricted to
/// `^[a-z0-9-]{1,32}$`: lowercase letters, digits, and hyphens only — no dots,
/// slashes, or uppercase. Anything else is rejected before it touches a path.
fn validate_surface_id(surface_id: &str) -> Result<(), CommandError> {
    let valid = !surface_id.is_empty()
        && surface_id.len() <= MAX_SURFACE_ID_LEN
        && surface_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !WINDOWS_RESERVED_DEVICE_STEMS.contains(&surface_id);
    if valid {
        Ok(())
    } else {
        Err(invalid_request(format!(
            "Invalid surface id `{surface_id}`: it must be 1-32 characters of lowercase letters, digits, or hyphens."
        )))
    }
}

fn surface_settings_get_inner(
    config_dir: &Path,
    surface_id: &str,
) -> Result<Option<Value>, CommandError> {
    validate_surface_id(surface_id)?;
    let path = settings_path(config_dir, surface_id);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        // Missing and unreadable are the same signal to callers: fall back
        // to defaults. Only an unavailable config directory is an error.
        Err(_) => return Ok(None),
    };
    match serde_json::from_str(&raw) {
        Ok(value) => Ok(Some(value)),
        // Malformed JSON is reported as null, never as an error: a corrupt
        // file must not lock the surface out of its own panel.
        Err(_) => Ok(None),
    }
}

fn surface_settings_set_inner(
    config_dir: &Path,
    surface_id: &str,
    value: &Value,
) -> Result<(), CommandError> {
    validate_surface_id(surface_id)?;
    let raw = serde_json::to_vec_pretty(value)
        .map_err(|error| internal_error("serializing surface settings failed", error))?;
    if raw.len() > MAX_SURFACE_SETTINGS_BYTES {
        return Err(invalid_request(format!(
            "Surface settings for `{surface_id}` are {} bytes once serialized, over the {MAX_SURFACE_SETTINGS_BYTES}-byte (~64 KB) cap. Save less state.",
            raw.len()
        )));
    }
    let path = settings_path(config_dir, surface_id);
    let parent = path
        .parent()
        .ok_or_else(|| invalid_request("surface settings have no containing directory"))?;
    fs::create_dir_all(parent)
        .map_err(|error| internal_error("creating the surface settings directory failed", error))?;
    // Same atomic replacement as Oracle's preferences: write a temp file,
    // flush it, then rename over the target so a crash never leaves a
    // half-written document behind.
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
        internal_error("creating the temporary surface settings file failed", error)
    })?;
    temp.write_all(&raw).map_err(|error| {
        internal_error("writing the temporary surface settings file failed", error)
    })?;
    temp.as_file().sync_all().map_err(|error| {
        internal_error("flushing the temporary surface settings file failed", error)
    })?;
    temp.persist(&path).map_err(|error| {
        internal_error(
            "atomically replacing the surface settings file failed",
            error.error,
        )
    })?;
    Ok(())
}

/// Returns the parsed JSON document for `surfaceId`, or `null` when the file
/// is missing OR unreadable OR malformed JSON. Callers treat "never saved"
/// and "corrupt" identically, so none of those is an error.
#[tauri::command]
pub fn surface_settings_get(
    app: tauri::AppHandle,
    surface_id: String,
) -> Result<Option<Value>, CommandError> {
    surface_settings_get_inner(&resolved_config_dir(&app)?, &surface_id)
}

/// Stores `value` verbatim as pretty JSON. Rejects surface ids outside
/// `^[a-z0-9-]{1,32}$` and serialized values over the ~64 KB cap.
#[tauri::command]
pub fn surface_settings_set(
    app: tauri::AppHandle,
    surface_id: String,
    value: Value,
) -> Result<(), CommandError> {
    surface_settings_set_inner(&resolved_config_dir(&app)?, &surface_id, &value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn assert_invalid_request(error: CommandError) {
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn round_trips_a_value_through_set_and_get() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = temp.path().join("config");
        let value = json!({
            "layout": "split",
            "count": 3,
            "nested": { "flag": true, "note": null },
        });

        surface_settings_set_inner(&config, "design", &value).expect("set");
        let stored = fs::read_to_string(settings_path(&config, "design")).expect("stored file");
        assert!(
            stored.contains('\n'),
            "the stored document must be pretty-printed: {stored}"
        );

        let loaded = surface_settings_get_inner(&config, "design").expect("get");
        assert_eq!(loaded, Some(value));
    }

    #[test]
    fn get_on_a_missing_file_is_null() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = temp.path().join("config");

        let loaded = surface_settings_get_inner(&config, "design").expect("get");
        assert_eq!(loaded, None);
    }

    #[test]
    fn get_on_malformed_json_is_null() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = temp.path().join("config");
        let path = settings_path(&config, "design");
        fs::create_dir_all(path.parent().expect("settings parent")).expect("settings directory");
        fs::write(&path, "{not json at all").expect("garbage settings file");

        let loaded = surface_settings_get_inner(&config, "design").expect("get");
        assert_eq!(loaded, None);
    }

    #[test]
    fn rejects_invalid_surface_ids_on_set_and_get() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = temp.path().join("config");
        let value = json!({ "ok": true });
        let long = "a".repeat(33);

        for surface_id in [
            "../evil",
            "UPPER",
            "",
            long.as_str(),
            "a.b",
            // Windows reserved device stems, exact form only.
            "con",
            "nul",
            "com1",
            "lpt9",
        ] {
            let set_error =
                surface_settings_set_inner(&config, surface_id, &value).expect_err("set");
            assert_invalid_request(set_error);
            let get_error = surface_settings_get_inner(&config, surface_id).expect_err("get");
            assert_invalid_request(get_error);
        }
    }

    #[test]
    fn accepts_lookalikes_of_reserved_device_stems() {
        // The deny is exact-stem only: anything that is not precisely a
        // reserved name stays valid.
        let temp = tempfile::tempdir().expect("tempdir");
        let config = temp.path().join("config");
        let value = json!({ "ok": true });

        for surface_id in ["cons", "com0", "lpt10", "conx"] {
            surface_settings_set_inner(&config, surface_id, &value)
                .unwrap_or_else(|error| panic!("{surface_id} must be accepted: {error:?}"));
            let loaded = surface_settings_get_inner(&config, surface_id).expect("get lookalike");
            assert_eq!(loaded, Some(value.clone()));
        }
    }

    #[test]
    fn rejects_values_over_the_size_cap() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = temp.path().join("config");
        let value = json!({ "blob": "x".repeat(MAX_SURFACE_SETTINGS_BYTES) });

        let error = surface_settings_set_inner(&config, "design", &value).expect_err("set");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("65536"),
            "the error must name the cap: {}",
            error.message
        );
    }

    #[test]
    fn a_stale_temp_file_does_not_break_a_later_set() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = temp.path().join("config");
        let directory = config.join(SURFACE_SETTINGS_DIR);
        fs::create_dir_all(&directory).expect("settings directory");
        fs::write(directory.join(".tmpstale"), "leftover bytes").expect("stale temp file");

        let value = json!({ "after": "stale" });
        surface_settings_set_inner(&config, "design", &value).expect("set after stale temp");
        let loaded = surface_settings_get_inner(&config, "design").expect("get");
        assert_eq!(loaded, Some(value));
    }
}
