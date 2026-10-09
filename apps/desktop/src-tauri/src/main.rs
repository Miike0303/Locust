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

    // Keep the socket bound while handing it to the backend. Probing a free
    // port and reopening it races other processes and other Locust instances.
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind the embedded server");
    listener
        .set_nonblocking(true)
        .expect("Failed to configure embedded server socket");
    let port = listener
        .local_addr()
        .expect("Failed to read embedded server address")
        .port();

    // Start the backend server in a background thread
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        rt.block_on(async {
            tracing::info!("Starting embedded server on port {}", port);
            let listener = tokio::net::TcpListener::from_std(listener)
                .expect("Failed to initialize embedded server socket");
            if let Err(e) =
                locust_server::start_server_with_listener(state_for_server, listener).await
            {
                tracing::error!("Server error: {}", e);
            }
        });
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(commands::AppStateWrapper(state))
        .manage(commands::ServerPort(port))
        .invoke_handler(tauri::generate_handler![
            commands::get_server_port,
            commands::preflight_project_open,
            commands::resume_project,
            commands::open_project,
            commands::open_project_db,
            commands::get_formats,
            commands::get_providers,
            commands::get_stats,
            commands::get_strings,
            commands::get_string_facets,
            commands::list_injection_recordings,
            commands::run_pivot,
            commands::patch_string,
            commands::batch_patch_strings,
            commands::start_translation,
            commands::cancel_translation,
            commands::run_validation,
            commands::export_translations,
            commands::import_translations,
            commands::run_inject,
            commands::register_lang,
            commands::xai_auth_start,
            commands::xai_auth_poll,
            commands::get_config,
            commands::save_config,
            commands::get_backups,
            commands::get_glossary,
            commands::add_glossary_entry,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
