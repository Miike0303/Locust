// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(debug_assertions))]
use human_panic::setup_panic;

fn main() {
    #[cfg(not(debug_assertions))]
    setup_panic!();

    // Start the backend server in a background thread
    std::thread::spawn(|| {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        rt.block_on(async {
            let state = locust_server::create_test_state();
            if let Err(e) = locust_server::start_server(state, 7842).await {
                eprintln!("Server error: {}", e);
            }
        });
    });

    // Give the server a moment to start
    std::thread::sleep(std::time::Duration::from_millis(500));

    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
