//! The end-to-end smoke's CDP port, declared the way Tauri passes browser
//! arguments instead of by environment: measured on the GitHub runner, the
//! `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` variable never reached the browser
//! (0 of the run's 6 WebView2 processes carried the flag), while an explicit
//! `additionalBrowserArgs` is what the webview reads.
//!
//! Compiled only under the off-by-default `e2e-cdp` feature and applied only
//! when `DEVBOULE_E2E_CDP_PORT` names a port, so a release or installer build
//! contains neither the code nor the effect.

use tauri::Context;
use tauri::Runtime;

/// The browser arguments wry passes when a window names none
/// (`wry-0.55.1/src/webview2/mod.rs:294-296`). An explicit value **replaces**
/// those instead of adding to them, so this road has to carry them itself —
/// they turn off the mini menu, the PDF overlay UI and SmartScreen.
const WRY_DEFAULT_BROWSER_ARGS: &str =
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";

/// A port from the raw environment value, when it is one.
pub(crate) fn parse_port(raw: Option<&str>) -> Option<u16> {
    raw?.trim().parse().ok()
}

/// What a window's arguments become: whatever it already carried — or wry's
/// own default when it carried nothing — with the debug port appended.
pub(crate) fn with_cdp_port(existing: Option<&str>, port: u16) -> String {
    let base = existing
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(WRY_DEFAULT_BROWSER_ARGS);
    format!("{base} --remote-debugging-port={port}")
}

/// The arguments to declare, or `None` when this run is not the smoke's — the
/// window config is then left exactly as the app wrote it.
pub(crate) fn declared_args(existing: Option<&str>, raw_port: Option<&str>) -> Option<String> {
    Some(with_cdp_port(existing, parse_port(raw_port)?))
}

/// Point every configured window at the smoke's debug port, when this launch
/// named one.
pub(crate) fn declare_cdp_port<R: Runtime>(context: &mut Context<R>) {
    let raw_port = std::env::var("DEVBOULE_E2E_CDP_PORT").ok();
    for window in &mut context.config_mut().app.windows {
        if let Some(args) = declared_args(
            window.additional_browser_args.as_deref(),
            raw_port.as_deref(),
        ) {
            window.additional_browser_args = Some(args);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_existing_arguments_and_appends_the_port() {
        assert_eq!(
            with_cdp_port(Some("--foo --bar"), 9336),
            "--foo --bar --remote-debugging-port=9336"
        );
    }

    #[test]
    fn keeps_wrys_default_features_when_the_window_names_none() {
        let composed = with_cdp_port(None, 9336);
        assert!(composed.starts_with(WRY_DEFAULT_BROWSER_ARGS), "{composed}");
        assert!(
            composed.ends_with(" --remote-debugging-port=9336"),
            "{composed}"
        );
    }

    #[test]
    fn a_blank_argument_string_is_the_default_too() {
        assert_eq!(with_cdp_port(Some("   "), 9336), with_cdp_port(None, 9336));
    }

    #[test]
    fn nothing_is_declared_without_the_environment_variable() {
        assert_eq!(declared_args(Some("--foo"), None), None);
        assert_eq!(declared_args(None, None), None);
    }

    #[test]
    fn a_value_that_is_not_a_port_is_ignored() {
        assert_eq!(parse_port(Some("not-a-port")), None);
        assert_eq!(parse_port(Some("")), None);
        assert_eq!(parse_port(Some("9336")), Some(9336));
        assert_eq!(parse_port(Some(" 9336 ")), Some(9336));
    }

    #[test]
    fn the_declared_arguments_are_the_composition() {
        assert_eq!(
            declared_args(Some("--foo"), Some("9336")),
            Some("--foo --remote-debugging-port=9336".to_string())
        );
    }
}
