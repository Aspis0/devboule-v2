mod backend;

use tauri::Manager;

pub use backend::session::{
    kill_all_on_exit, push_capped, validate_session_id, PtyCommand, Session, SessionEvent,
    SessionKind, SessionState,
};

#[tauri::command]
fn app_identity(app: tauri::AppHandle) -> String {
    app.package_info().name.to_owned()
}

pub fn run() {
    tauri::Builder::default()
        .manage(SessionState::new())
        .invoke_handler(tauri::generate_handler![
            app_identity,
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
                let sessions = app_handle.state::<SessionState>();
                kill_all_on_exit(&sessions);
            }
        })
}
