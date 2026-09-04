mod backend;
mod client;
mod oracle;
mod plugins;

use tauri::Manager;

pub use backend::session::{validate_session_id, Session, SessionEvent, SessionKind};

#[tauri::command]
fn app_identity(app: tauri::AppHandle) -> String {
    app.package_info().name.to_owned()
}

pub fn run() {
    let builder = plugins::assets::register(tauri::Builder::default());
    builder
        .manage(client::DaemonBridge::start())
        .manage(oracle::OracleRuntime::from_environment())
        // The asset server refuses everything until this exists, so it is
        // managed before any window can ask for a plugin file.
        .manage(plugins::PluginRegistry::default())
        .manage(plugins::rpc::PluginRuntime::default())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
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
            // Start the installer as soon as Oracle has a configured root. The
            // command status exposes its progress when the panel is opened.
            if let Err(error) = runtime.start_model_download_for_startup() {
                eprintln!(
                    "devboule: Oracle model download did not start: {}",
                    error.message
                );
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_identity,
            client::daemon_status,
            backend::session::session_create,
            backend::session::session_attach,
            backend::session::session_detach,
            backend::session::session_send,
            backend::session::session_permission_respond,
            backend::session::session_resize,
            backend::session::session_close,
            backend::journal::journal_usage,
            backend::journal::journal_retention_get,
            backend::journal::journal_retention_set,
            backend::journal::session_delete,
            backend::session::sessions_list,
            backend::session::sessions_watch,
            backend::session::sessions_unwatch,
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
            plugins::plugins_list,
            plugins::plugins_rescan,
            plugins::plugin_install,
            plugins::rpc::plugin_backend_ensure,
            plugins::rpc::plugin_backend_stop,
            plugins::rpc::plugin_invoke,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Devboule")
        .run(|app_handle, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                let oracle = app_handle.state::<oracle::OracleRuntime>();
                oracle.shutdown();
                let daemon = app_handle.state::<client::DaemonBridge>();
                daemon.shutdown();
                app_handle.state::<plugins::rpc::PluginRuntime>().stop_all();
            }
        })
}
