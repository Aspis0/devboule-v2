mod backend;
mod client;

use tauri::Manager;

pub use backend::session::{validate_session_id, Session, SessionEvent, SessionKind};

#[tauri::command]
fn app_identity(app: tauri::AppHandle) -> String {
    app.package_info().name.to_owned()
}

pub fn run() {
    tauri::Builder::default()
        .manage(client::DaemonBridge::start())
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
