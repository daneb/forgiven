mod nexus;

use tauri::Manager as _;

#[tauri::command]
fn read_text_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![read_text_file])
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Spawn the Nexus UDS reader on the Tokio runtime that Tauri provides.
            nexus::spawn(app.handle().clone());

            // Position the window to a sensible default (right side of screen).
            if let Some(window) = app.get_webview_window("main") {
                if let Ok(monitor) = window.current_monitor() {
                    if let Some(monitor) = monitor {
                        let size = monitor.size();
                        let _ = window.set_position(tauri::PhysicalPosition::new(
                            size.width.saturating_sub(820) as i32,
                            60,
                        ));
                    }
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Forgiven Previewer");
}
