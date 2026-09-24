//! The tray icon: Devboule's presence outside its window — the Windows
//! notification area, the macOS menu bar.
//!
//! One responsibility: the icon, its three-item menu, and keeping the
//! disabled status line roughly current. The quit confirmation itself lives
//! in `close_prompt`; left-click restore is all this module does to windows.

use std::time::Duration;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

use devboule_protocol::DaemonStatusBody;

use crate::client::DaemonBridge;
use crate::close_flow;

const STATUS_POLL_PERIOD: Duration = Duration::from_secs(5);
const STATUS_UNAVAILABLE: &str = "Daemon status unavailable";

pub(crate) fn build(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Devboule", true, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", "Starting", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &status, &quit])?;
    let builder = TrayIconBuilder::with_id("devboule")
        .menu(&menu)
        // Left click restores the window; only right click opens the menu.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main_window(app),
            // The same confirmation as closing the window: the tray is not a
            // way around it.
            "quit" => close_flow::confirm_quit(app.clone()),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    // The codegen default window icon (icons/icon.ico on Windows). Absent on
    // a platform with no icon configured, the tray still builds; the menu
    // bar item is how macOS shows it either way.
    let builder = match app.default_window_icon() {
        Some(icon) => builder.icon(icon.clone()),
        None => builder,
    };
    builder.build(app)?;
    spawn_status_poll(app.handle().clone(), status);
    Ok(())
}

/// Show, unminimize and focus the main window: the one thing left click and
/// "Open Devboule" both mean.
pub(crate) fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Keeps the disabled status line near the truth with one small Status
/// roundtrip per period. A Tauri 2 tray menu cannot be rebuilt at open
/// time, so the line is refreshed on a timer rather than on demand — close
/// enough for a glance, cheap enough to always run.
fn spawn_status_poll(app: tauri::AppHandle, status: MenuItem<tauri::Wry>) {
    let spawned = std::thread::Builder::new()
        .name("tray-status".into())
        .spawn(move || loop {
            std::thread::sleep(STATUS_POLL_PERIOD);
            let text = match app.try_state::<DaemonBridge>() {
                Some(bridge) => match bridge.client() {
                    Ok(client) => match client.status() {
                        Ok(body) => status_line(&body),
                        Err(_) => STATUS_UNAVAILABLE.to_string(),
                    },
                    Err(_) => STATUS_UNAVAILABLE.to_string(),
                },
                None => STATUS_UNAVAILABLE.to_string(),
            };
            let _ = status.set_text(text);
        });
    // If the poll thread cannot start, the menu still works with the
    // initial "Starting" line: the poll is cosmetic.
    let _ = spawned;
}

/// The disabled menu line, true to what the daemon said: agents and
/// terminals named separately when the daemon tells them apart, sessions
/// un-named when it does not, and a failed read never reported as a
/// stopped daemon.
fn status_line(body: &DaemonStatusBody) -> String {
    match (body.agents, body.terminals) {
        (Some(agents), Some(terminals)) => match (agents, terminals) {
            (0, 0) => "No agents or terminals running".to_string(),
            (agents, 0) => format!("{} running", plural_sessions(agents, "agent")),
            (0, terminals) => format!("{} running", plural_sessions(terminals, "terminal")),
            (agents, terminals) => format!(
                "{}, {} running",
                plural_sessions(agents, "agent"),
                plural_sessions(terminals, "terminal")
            ),
        },
        _ => match body.sessions {
            0 => "No sessions running".to_string(),
            1 => "1 session running".to_string(),
            sessions => format!("{sessions} sessions running"),
        },
    }
}

fn plural_sessions(count: u32, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        n => format!("{n} {noun}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(agents: Option<u32>, terminals: Option<u32>, sessions: u32) -> DaemonStatusBody {
        DaemonStatusBody {
            instance_id: "i".to_string(),
            protocol_version: 4,
            daemon_version: "0.0.0".to_string(),
            pid: 1,
            uptime_ms: 0,
            clients: 1,
            local_clients: 1,
            sessions,
            agents,
            terminals,
            capabilities: Vec::new(),
            peak_ring_bytes: 0,
            ring_evicted_bytes: 0,
            ring_dropped_frames: 0,
            journal_error: None,
            journal_stats: None,
            secret_store: None,
            remote: None,
        }
    }

    #[test]
    fn the_line_names_both_families_from_their_own_counts() {
        // An agent mid-startup: the lifecycle session counter already moved,
        // the registry split has not. The line trusts the split, never
        // sessions minus agents.
        let line = status_line(&body(Some(0), Some(0), 1));
        assert_eq!(
            line, "No agents or terminals running",
            "a starting agent is not a terminal: {line}"
        );
        let line = status_line(&body(Some(1), Some(2), 3));
        assert_eq!(line, "1 agent, 2 terminals running");
        let line = status_line(&body(Some(0), Some(1), 1));
        assert_eq!(line, "1 terminal running");
    }

    #[test]
    fn an_old_daemon_without_the_split_says_sessions() {
        let line = status_line(&body(None, None, 2));
        assert_eq!(line, "2 sessions running");
    }
}
