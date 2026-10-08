mod artifact_export;
mod attention_toast;
mod backend;
mod browser;
mod client;
mod close_flow;
mod close_prompt;
#[cfg(feature = "e2e-cdp")]
mod e2e_cdp;
#[cfg(test)]
mod nsis_template_pin;
mod oracle;
mod plugins;
mod preview_scope;
mod surface_settings;
mod tray;
mod window_background;

use tauri::Manager;

pub use backend::session::{validate_session_id, Session, SessionEvent, SessionKind};
// The asset-scope concession whose parameterized half the integration test
// drives (`tests/asset_scope.rs`); `run` reaches the resolving one through
// the module itself.
pub use preview_scope::concede_previews_of;

#[tauri::command]
fn app_identity(app: tauri::AppHandle) -> String {
    app.package_info().name.to_owned()
}

pub fn run() {
    // The smoke's CDP port, when this is an `e2e-cdp` build and the launch
    // named one; every other build reaches the builder with the config as it
    // was written (`e2e_cdp`).
    let mut context = tauri::generate_context!();
    #[cfg(feature = "e2e-cdp")]
    e2e_cdp::declare_cdp_port(&mut context);
    let builder = plugins::assets::register(tauri::Builder::default());
    builder
        .manage(client::DaemonBridge::start())
        .manage(oracle::OracleRuntime::from_environment())
        .manage(oracle::OracleEndpoint::default())
        // The asset server refuses everything until this exists, so it is
        // managed before any window can ask for a plugin file.
        .manage(plugins::PluginRegistry::default())
        .manage(plugins::rpc::PluginRuntime::default())
        // The registry every browser tab resolves through: which child
        // webview a tab owns, and the one profile directory they share.
        .manage(std::sync::Arc::new(
            browser::registry::BrowserRegistry::new(),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        // The opener behind the pencil's Reveal in folder target — its Rust
        // API only. The default builder injects a link-click script, and
        // letting it claim every `target="_blank"` anchor would change how
        // existing links behave, so the injected half stays off and the
        // reveal happens in the command, never from the page.
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .setup(|app| {
            // Before any other setup: the native layers must carry a ground
            // from the first presented frame, not the config's single colour.
            // The OS answer is a stand-in — the stored preference lives in
            // webview storage, so a stored-choice launch is corrected by the
            // frontend once it runs (known residual, see the U15 report).
            window_background::paint_startup_background(app);
            // The asset scope in tauri.conf.json concedes the DEFAULT
            // runtime dir's previews folder; the daemon stages where
            // `RuntimePaths::from_env` says — `DEVBOULE_RUNTIME_DIR`
            // included, the same rule this app's client and spawn use.
            // Concede the folder this process actually resolves, or an
            // override turns every preview into a 403 against a folder
            // nothing writes (see `preview_scope`).
            preview_scope::concede_runtime_previews(app);
            let runtime = app.state::<oracle::OracleRuntime>();
            match app.path().app_config_dir() {
                Ok(config_dir) => {
                    if let Err(error) = runtime.load_persisted_root(&config_dir) {
                        eprintln!(
                            "devboule: Oracle preferences could not be loaded: {}",
                            error.message
                        );
                    }
                }
                Err(error) => eprintln!("devboule: Oracle preferences unavailable: {error}"),
            }
            // The agent browser's destination policy, with whatever exact
            // host:port exceptions the person's settings file carries.
            let config_dir = app.path().app_config_dir().ok();
            app.manage(std::sync::Arc::new(
                browser::destination::DestinationPolicy::load(config_dir.as_deref()),
            ));
            // Start the installer as soon as Oracle has a configured root. The
            // command status exposes its progress when the panel is opened.
            if let Err(error) = runtime.start_model_download_for_startup() {
                eprintln!(
                    "devboule: Oracle model download did not start: {}",
                    error.message
                );
            }
            // The daemon reaches the semantic search over this endpoint; the
            // record is published only after the bind.
            let endpoint = app.state::<oracle::OracleEndpoint>();
            if let Err(error) = endpoint.start(app.handle().clone()) {
                eprintln!("devboule: Oracle endpoint did not start: {error}");
            }
            // The app lives outside its window from here on: the tray is
            // present while it runs, and closing the window is a decision,
            // not a quit.
            tray::build(app)?;
            // The browser host, once the daemon bridge is up: it registers for
            // as long as it holds a connection, and every (re)connect is
            // reported to it from there.
            let registry = app
                .state::<std::sync::Arc<browser::registry::BrowserRegistry>>()
                .inner()
                .clone();
            app.manage(browser::host::BrowserHost::start(
                app.state::<client::DaemonBridge>().shared(),
                app.handle().clone(),
                registry,
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_identity,
            attention_toast::attention_toast_show,
            client::daemon_status,
            client::daemon_restart,
            backend::session::session_create,
            backend::session::session_resume,
            backend::session::session_attach,
            backend::session::session_detach,
            backend::session::session_claim,
            backend::session::session_presence,
            backend::session::session_send,
            backend::session::session_queue_add,
            backend::session::session_queue_edit,
            backend::session::session_queue_remove,
            backend::session::session_queue_move,
            backend::session::session_queue_send_now,
            backend::session::session_deposit,
            backend::session::session_upload_begin,
            backend::session::session_upload_status,
            backend::session::session_upload_chunk,
            backend::session::session_upload_finish,
            backend::session::session_upload_abort,
            backend::session::session_attachment_delete,
            backend::session::session_attachment_read,
            backend::session::session_interrupt,
            backend::session::session_set_model,
            backend::session::session_set_mode,
            backend::session::session_set_name,
            backend::session::session_set_feature,
            backend::session::session_permission_respond,
            backend::session::session_resize,
            backend::session::session_close,
            backend::session::session_stop,
            backend::session::session_tasks,
            backend::journal::journal_usage,
            backend::journal::journal_retention_get,
            backend::journal::journal_retention_set,
            backend::journal::session_delete,
            backend::session::sessions_list,
            backend::session::daemon_diagnostics,
            backend::session::sessions_watch,
            backend::session::sessions_unwatch,
            backend::workspace::projects_list,
            backend::workspace::project_add,
            backend::workspace::workspaces_list,
            backend::workspace::workspace_create,
            backend::workspace::workspace_set_title,
            backend::workspace::workspace_delete,
            backend::workspace::workspace_git_status,
            backend::workspace::workspace_git_diff,
            backend::workspace::workspace_git_log,
            backend::workspace::workspace_git_stage,
            backend::workspace::workspace_git_unstage,
            backend::workspace::workspace_git_discard,
            backend::workspace::workspace_git_commit,
            backend::workspace::workspace_file_read,
            backend::workspace::workspace_file_preview_stage,
            backend::workspace::workspace_file_preview_unstage,
            backend::workspace::workspace_file_rename,
            backend::workspace::workspace_file_duplicate,
            backend::workspace::workspace_file_delete,
            backend::open_in_editor::workspace_file_open,
            backend::open_external::open_external_url,
            backend::editor_targets::editor_targets_list,
            backend::workspace::workspace_files_list,
            backend::devices::devices_list,
            backend::devices::pairing_start,
            backend::devices::pairing_complete,
            backend::devices::pairing_confirm,
            backend::devices::peer_revoke,
            backend::devices::peer_set_caps,
            backend::remote_hosts::remote_host_watch,
            backend::remote_hosts::remote_host_unwatch,
            backend::remote_hosts::remote_host_list,
            backend::tool_policy::tool_policy_get,
            backend::tool_policy::tool_policy_set,
            backend::agent_profiles::agent_profiles_get,
            backend::agent_profiles::agent_profiles_set,
            backend::delegation::delegation_get,
            backend::delegation::delegation_set,
            backend::provider_vocabulary::provider_vocabulary_get,
            backend::providers::providers_list,
            backend::providers::providers_refresh,
            backend::providers::providers_auth_check,
            backend::providers::provider_update,
            backend::providers::provider_set_enabled,
            oracle::oracle_workspace_get,
            oracle::oracle_workspace_set,
            oracle::oracle_model_download_start,
            oracle::oracle_model_download_cancel,
            oracle::oracle_index_cancel,
            oracle::oracle_status,
            oracle::oracle_doctor,
            oracle::oracle_stats,
            oracle::oracle_index_start,
            oracle::oracle_watch_start,
            oracle::oracle_watch_stop,
            oracle::oracle_files,
            oracle::oracle_ask,
            oracle::oracle_folder_status,
            oracle::oracle_ask_folder,
            surface_settings::surface_settings_get,
            surface_settings::surface_settings_set,
            artifact_export::artifact_write_file,
            plugins::plugins_list,
            plugins::plugins_rescan,
            plugins::plugin_install,
            plugins::rpc::plugin_backend_ensure,
            plugins::rpc::plugin_backend_stop,
            plugins::rpc::plugin_invoke,
            browser::browser_open,
            browser::browser_present,
            browser::browser_park,
            browser::browser_navigate,
            browser::browser_history,
            browser::browser_reload,
            browser::browser_close,
            browser::credentials::commands::saved_logins_list,
            browser::credentials::commands::saved_login_create,
            browser::credentials::commands::saved_login_update,
            browser::credentials::commands::saved_login_delete,
        ])
        .on_window_event(|window, event| {
            // Every close of the main window becomes a decision (hide, quit,
            // or ask).
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                close_flow::on_close_requested(window, api);
            }
        })
        .build(context)
        .expect("error while building Devboule")
        .run(|app_handle, event| match event {
            tauri::RunEvent::Exit => {
                let oracle = app_handle.state::<oracle::OracleRuntime>();
                oracle.shutdown();
                app_handle.state::<oracle::OracleEndpoint>().stop();
                // Before the bridge: this gives the daemon its host id back
                // while the connection that carries the frame is still open.
                app_handle.state::<browser::host::BrowserHost>().stop();
                let daemon = app_handle.state::<client::DaemonBridge>();
                daemon.shutdown();
                app_handle.state::<plugins::rpc::PluginRuntime>().stop_all();
                // A toast left in Action Center would open nothing after this; a crash can still leave one.
                #[cfg(windows)]
                attention_toast::clear_pending(app_handle);
            }
            // macOS: with the window closed into the menu bar, a dock click
            // is how the user comes back.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => tray::show_main_window(app_handle),
            // macOS: Cmd+Q and the app-menu Quit arrive here as a
            // user-requested exit (no code). The same confirmation decides;
            // a prevented exit leaves the app and its daemon up.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } => {
                api.prevent_exit();
                close_flow::confirm_quit(app_handle.clone());
            }
            _ => {}
        })
}
