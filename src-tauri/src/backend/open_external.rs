//! Handing one web link to the system browser. The URL is parsed here, not
//! trusted from the page: only the main window's own http(s) links, written
//! without credentials and without a byte a URL parser would normalize away,
//! may reach the OS handler.

use devboule_protocol::ErrorCode;
use tauri::Url;
use tauri_plugin_opener::OpenerExt;

use super::error::CommandError;

/// The window tauri.conf.json declares; only its links open.
const MAIN_WINDOW_LABEL: &str = "main";

const NOT_THE_MAIN_WINDOW: &str = "only a link in the main window is opened";
const NOT_A_WEB_URL: &str = "only http and https links can be opened";
const WITH_CREDENTIALS: &str = "a link that carries credentials is never opened";
const TOO_LONG: &str = "a link this long is never opened";
const NOT_EXACT: &str = "a link with whitespace or control characters is never opened";
const BROWSER_FAILED: &str = "the system browser could not be opened";

/// Past any link a user clicks, and the ceiling that keeps the parse and the
/// OS call bounded when the input is not a link at all.
pub(crate) const MAX_URL_LENGTH: usize = 8192;

/// The URL the OS may open, or the sentence for why it may not.
pub(crate) fn openable_url(input: &str) -> Result<Url, CommandError> {
    if input.len() > MAX_URL_LENGTH {
        return Err(CommandError::new(ErrorCode::InvalidRequest, TOO_LONG));
    }
    // `Url::parse` trims C0 controls and spaces at the ends and drops tabs and
    // newlines, so input that reads as one URL can open another.
    if input
        .chars()
        .any(|character| character.is_ascii_control() || character.is_whitespace())
    {
        return Err(CommandError::new(ErrorCode::InvalidRequest, NOT_EXACT));
    }
    let parsed = Url::parse(input)
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
    Ok(parsed)
}

/// The link click's other half: the page cancels the clicks it routes here, and
/// this hands the URL to the OS handler. A context-menu open reaches the webview
/// on its own and never arrives at this command.
#[tauri::command]
pub fn open_external_url(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    url: String,
) -> Result<(), CommandError> {
    if window.label() != MAIN_WINDOW_LABEL {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            NOT_THE_MAIN_WINDOW,
        ));
    }
    let parsed = openable_url(&url)?;
    app.opener()
        .open_url(parsed.to_string(), None::<&str>)
        .map_err(|_| CommandError::new(ErrorCode::Io, BROWSER_FAILED))
}
