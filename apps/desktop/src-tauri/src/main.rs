// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(debug_assertions))]
use human_panic::setup_panic;


mod commands;

fn main() {
    #[cfg(not(debug_assertions))]
    setup_panic!();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("locust=info".parse().unwrap()),
        )
        .init();

    // Create production backend state
    let state = locust_server::create_app_state();
    let state_for_server = state.clone();

    // Pick an available port for the embedded server
    let port = portpicker::pick_unused_port().unwrap_or(7842);

    // Start the backend server in a background thread
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        rt.block_on(async {
            tracing::info!("Starting embedded server on port {}", port);
            if let Err(e) = locust_server::start_server(state_for_server, port).await {
                tracing::error!("Server error: {}", e);
            }
        });
    });

    // Give the server a moment to start
    std::thread::sleep(std::time::Duration::from_millis(300));

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(commands::AppStateWrapper(state))
        .manage(commands::ServerPort(port))
        .invoke_handler(tauri::generate_handler![
            commands::get_server_port,
            commands::pick_game_folder,
            commands::open_project,
            commands::get_formats,
            commands::get_providers,
            commands::get_stats,
            commands::get_strings,
            commands::patch_string,
            commands::start_translation,
            commands::cancel_translation,
            commands::run_validation,
            commands::run_inject,
            commands::get_config,
            commands::save_config,
            commands::get_backups,
            commands::get_glossary,
            commands::add_glossary_entry,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
