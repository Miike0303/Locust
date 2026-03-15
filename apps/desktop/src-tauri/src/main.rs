// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(debug_assertions))]
use human_panic::setup_panic;

fn main() {
    #[cfg(not(debug_assertions))]
    setup_panic!();

    // For now, run as a simple app shell
    // Full Tauri integration will be added when tauri v2 crate is configured
    println!("Project Locust Desktop — starting...");

    // Start the backend server in a background thread
    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    rt.block_on(async {
        let state = locust_server::create_test_state();
        println!("Backend server starting on http://localhost:7842");
        if let Err(e) = locust_server::start_server(state, 7842).await {
            eprintln!("Server error: {}", e);
        }
    });
}
