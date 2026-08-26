#[tauri::command]
fn app_identity(app: tauri::AppHandle) -> String {
    app.package_info().name.to_owned()
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![app_identity])
        .run(tauri::generate_context!())
        .expect("error while running Devboule");
}
