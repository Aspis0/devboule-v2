//! The four commands the Settings page drives.
//!
//! None of them can answer with a password: each returns the vault's metadata
//! (`SavedLogin`, which has no field a password could be put in) or nothing at
//! all. The password goes in through `create` and `update` and into the OS
//! credential store, and the only function that reads one back is not in this
//! file — it is `Vault::password_for`, which is crate-internal and is there for
//! the fill path that lands with slice V2.
//!
//! Each command resolves this machine's own folder and hands the work to the
//! vault off the window's thread, because an OS credential store can block on a
//! locked vault of its own. A request the vault could not carry is refused
//! before it is copied onto that thread at all.

use std::path::PathBuf;

use tauri::Manager;

use devboule_protocol::ErrorCode;

use crate::backend::blocking::off_main_thread;
use crate::backend::error::CommandError;

use super::{metadata::SavedLogin, Refusal, Vault};

/// Every entry this machine has saved. Metadata only.
#[tauri::command]
pub async fn saved_logins_list(app: tauri::AppHandle) -> Result<Vec<SavedLogin>, CommandError> {
    let dir = local_folder(&app)?;
    off_main_thread(move || list(&Vault::in_dir(dir))).await
}

/// Save one login. Its password goes to the OS store before the row is listed.
#[tauri::command]
pub async fn saved_login_create(
    app: tauri::AppHandle,
    label: String,
    origins: Vec<String>,
    username: String,
    password: String,
) -> Result<SavedLogin, CommandError> {
    super::checked(&label, &origins, &username, Some(&password))?;
    let dir = local_folder(&app)?;
    off_main_thread(move || create(&Vault::in_dir(dir), &label, &origins, &username, &password))
        .await
}

/// Change one login. An absent `password` keeps the one already stored, which
/// is what the form's empty password field means.
#[tauri::command]
pub async fn saved_login_update(
    app: tauri::AppHandle,
    id: String,
    label: String,
    origins: Vec<String>,
    username: String,
    password: Option<String>,
) -> Result<SavedLogin, CommandError> {
    super::checked(&label, &origins, &username, password.as_deref())?;
    let dir = local_folder(&app)?;
    off_main_thread(move || {
        update(
            &Vault::in_dir(dir),
            &id,
            &label,
            &origins,
            &username,
            password.as_deref(),
        )
    })
    .await
}

/// Forget one login: its password out of the OS store first, then its row.
#[tauri::command]
pub async fn saved_login_delete(app: tauri::AppHandle, id: String) -> Result<(), CommandError> {
    let dir = local_folder(&app)?;
    off_main_thread(move || delete(&Vault::in_dir(dir), &id)).await
}

/// This machine's own folder, which is where the browser profile already lives
/// and therefore where a person expects to find what the app saved.
pub(super) fn local_folder(app: &tauri::AppHandle) -> Result<PathBuf, CommandError> {
    app.path().app_local_data_dir().map_err(|error| {
        CommandError::new(
            ErrorCode::Internal,
            format!("This app's data folder is unavailable: {error}."),
        )
    })
}

impl From<Refusal> for CommandError {
    /// What the person typed, or what this machine's own store said. The
    /// Settings form shows either sentence as it arrives.
    fn from(refusal: Refusal) -> Self {
        let code = match refusal {
            Refusal::Asked(_) => ErrorCode::InvalidRequest,
            Refusal::Failed(_) => ErrorCode::Internal,
        };
        CommandError::new(code, refusal.sentence())
    }
}

pub(crate) fn list(vault: &Vault) -> Result<Vec<SavedLogin>, CommandError> {
    vault.list().map_err(Into::into)
}

pub(crate) fn create(
    vault: &Vault,
    label: &str,
    origins: &[String],
    username: &str,
    password: &str,
) -> Result<SavedLogin, CommandError> {
    vault
        .create(label, origins, username, password)
        .map_err(Into::into)
}

pub(crate) fn update(
    vault: &Vault,
    id: &str,
    label: &str,
    origins: &[String],
    username: &str,
    password: Option<&str>,
) -> Result<SavedLogin, CommandError> {
    vault
        .update(id, label, origins, username, password)
        .map_err(Into::into)
}

pub(crate) fn delete(vault: &Vault, id: &str) -> Result<(), CommandError> {
    vault.delete(id).map_err(Into::into)
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
