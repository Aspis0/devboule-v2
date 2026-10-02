//! Handing one web link to the system browser. The URL is parsed here, not
//! trusted from the page: only http(s) without credentials may reach the OS
//! handler, and the webview never navigates to anything.

use devboule_protocol::ErrorCode;
use tauri::Url;
use tauri_plugin_opener::OpenerExt;

use super::error::CommandError;

const NOT_A_WEB_URL: &str = "only http and https links can be opened";
const WITH_CREDENTIALS: &str = "a link that carries credentials is never opened";
const BROWSER_FAILED: &str = "the system browser could not be opened";

/// The link click's other half: the page's own `preventDefault` stops the
/// navigation, and this hands the URL to the default browser instead.
#[tauri::command]
pub fn open_external_url(app: tauri::AppHandle, url: String) -> Result<(), CommandError> {
    let parsed = Url::parse(&url)
        .map_err(|_| CommandError::new(ErrorCode::InvalidRequest, NOT_A_WEB_URL))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(CommandError::new(ErrorCode::InvalidRequest, NOT_A_WEB_URL));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            WITH_CREDENTIALS,
        ));
    }
    app.opener()
        .open_url(parsed.to_string(), None::<&str>)
        .map_err(|_| CommandError::new(ErrorCode::Io, BROWSER_FAILED))
}
