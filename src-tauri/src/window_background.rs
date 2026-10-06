//! The native window's startup background: the colour behind the webview
//! before the frontend paints its first frame.
//!
//! One responsibility: painting the native layers with the theme's ground at
//! startup, so a dark launch never shows the native white between window
//! creation and the first webview paint. On Windows both layers are painted;
//! on macOS the webview layer has no setter and on mobile background colours
//! are unsupported, so there the call only reaches the window layer — it
//! stays harmless everywhere.

use tauri::Manager;
use tauri::Theme;

/// Copies of `--ground-app` in `src/styles/tokens.css`; the
/// `nativeWindowBackground` frontend test fails if they drift.
const LIGHT_GROUND: (u8, u8, u8, u8) = (0xF4, 0xF3, 0xF0, 255);
const DARK_GROUND: (u8, u8, u8, u8) = (0x14, 0x14, 0x14, 255);

/// Maps an OS theme answer onto the startup ground. Anything but an
/// explicit light — dark, an unreadable OS theme, a future variant — is the
/// dark ground, agreeing with the config's `backgroundColor`, which chose
/// the value that can never show the reported white.
fn ground_for_theme(theme: Option<Theme>) -> (u8, u8, u8, u8) {
    match theme {
        Some(Theme::Light) => LIGHT_GROUND,
        Some(_) | None => DARK_GROUND,
    }
}

/// Paints the main window's native layers before it shows. The stored theme
/// choice lives in the webview's storage, which does not exist yet, so the
/// OS theme is the answer; the frontend corrects it once it reads the
/// stored preference.
pub fn paint_startup_background(app: &tauri::App) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let theme = match window.theme() {
        Ok(theme) => Some(theme),
        Err(error) => {
            eprintln!("devboule: OS theme unreadable, keeping the dark startup ground: {error}");
            None
        }
    };
    let (red, green, blue, alpha) = ground_for_theme(theme);
    if let Err(error) = window.set_background_color(Some((red, green, blue, alpha).into())) {
        eprintln!("devboule: native startup background could not be painted: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_os_theme_paints_the_dark_ground() {
        assert_eq!(ground_for_theme(Some(Theme::Dark)), DARK_GROUND);
    }

    #[test]
    fn light_os_theme_paints_the_light_ground() {
        assert_eq!(ground_for_theme(Some(Theme::Light)), LIGHT_GROUND);
    }

    #[test]
    fn unreadable_os_theme_keeps_the_dark_ground() {
        assert_eq!(ground_for_theme(None), DARK_GROUND);
    }
}
