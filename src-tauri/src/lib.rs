mod backend;
mod client;
mod oracle;

use tauri::Manager;

pub use backend::session::{validate_session_id, Session, SessionEvent, SessionKind};

#[tauri::command]
fn app_identity(app: tauri::AppHandle) -> String {
    app.package_info().name.to_owned()
}

pub fn run() {
    tauri::Builder::default()
        .manage(client::DaemonBridge::start())
        .manage(oracle::OracleRuntime::from_environment())
        .invoke_handler(tauri::generate_handler![
            app_identity,
            client::daemon_status,
            backend::session::session_create,
            backend::session::session_attach,
            backend::session::session_detach,
            backend::session::session_send,
            backend::session::session_resize,
            backend::session::session_close,
            backend::session::sessions_list,
            oracle::oracle_status,
            oracle::oracle_doctor,
            oracle::oracle_stats,
            oracle::oracle_index_start,
            oracle::oracle_watch_start,
            oracle::oracle_watch_stop,
            oracle::oracle_files,
            oracle::oracle_ask,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Devboule")
        .run(|app_handle, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                let daemon = app_handle.state::<client::DaemonBridge>();
                daemon.shutdown();
            }
        })
}
