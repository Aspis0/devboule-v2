mod backend;
mod client;
mod single_instance;

use tauri::Manager;

pub use backend::session::{validate_session_id, Session, SessionEvent, SessionKind};

#[tauri::command]
fn app_identity(app: tauri::AppHandle) -> String {
    app.package_info().name.to_owned()
}

pub fn run() {
    // Desktop single-instance behavior: a second Devboule brings the running
    // window to the front and exits 0. This guards one window per session;
    // the single-daemon guarantee lives in the daemon's own lock.
    let _app_instance = match single_instance::acquire() {
        single_instance::StartupInstance::Acquired(guard) => guard,
        single_instance::StartupInstance::AlreadyRunning => {
            eprintln!(
                "Devboule is already running; bringing the existing window to the front."
            );
            if !single_instance::focus_existing_window() {
                single_instance::notify_already_running();
            }
            return;
        }
    };

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
