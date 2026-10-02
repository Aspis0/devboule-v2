//! Handing one web link to the system browser. The command opens a
//! caller-supplied http(s) URL from the main window only, and parses it here
//! rather than trusting the page: the canonical `http://` or `https://` form, no
//! credentials, and no control or whitespace byte a URL parser would strip.

use devboule_protocol::ErrorCode;
use tauri::Url;
use tauri_plugin_opener::OpenerExt;

use super::error::CommandError;

const MAIN_WINDOW_LABEL: &str = "main";

const NOT_THE_MAIN_WINDOW: &str = "only a link in the main window is opened";
const NOT_A_WEB_URL: &str = "only http and https links can be opened";
const WITH_CREDENTIALS: &str = "a link that carries credentials is never opened";
const TOO_LONG: &str = "a link this long is never opened";
const NOT_EXACT: &str = "a link with whitespace or control characters is never opened";
const BROWSER_FAILED: &str = "the system browser could not be opened";

/// The limit is on the raw text the command receives, not on the URL it opens
/// after normalizing; a caller that sends a normalized href checks that href.
pub(crate) const MAX_URL_LENGTH: usize = 8192;

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
    // `Url::parse` also accepts `https:/host` and `https:\\host`, which have no
    // `//` for the credential check below to anchor its authority to.
    let Some(after_prefix) = after_web_prefix(input) else {
        return Err(CommandError::new(ErrorCode::InvalidRequest, NOT_A_WEB_URL));
    };
    let parsed = Url::parse(input)
        .map_err(|_| CommandError::new(ErrorCode::InvalidRequest, NOT_A_WEB_URL))?;
    // The parser drops an empty user and password, so `https://@host/` only
    // shows in the raw authority; the page's predicate applies the same rule.
    if authority_has_userinfo_marker(after_prefix)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            WITH_CREDENTIALS,
        ));
    }
    Ok(parsed)
}

/// What follows a leading `http://` or `https://`, whatever the case of the scheme.
fn after_web_prefix(input: &str) -> Option<&str> {
    ["https://", "http://"].into_iter().find_map(|prefix| {
        let head = input.get(..prefix.len())?;
        head.eq_ignore_ascii_case(prefix)
            .then(|| &input[prefix.len()..])
    })
}

/// Whether the authority, the text up to the first `/`, `\`, `?` or `#`, holds an `@`.
fn authority_has_userinfo_marker(after_prefix: &str) -> bool {
    after_prefix
        .split(['/', '\\', '?', '#'])
        .next()
        .is_some_and(|authority| authority.contains('@'))
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
