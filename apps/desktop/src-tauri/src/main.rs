// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::Context;
#[cfg(not(debug_assertions))]
use human_panic::setup_panic;
use tauri_plugin_dialog::DialogExt;

mod commands;
mod self_test;

fn write_startup_note(message: &str) {
    use std::io::Write;

    eprintln!("{message}");
    // Release builds have no console. Leave a diagnostic even if Tauri itself
    // cannot initialize the native dialog. Never overwrite an existing note.
    let path = std::env::temp_dir().join(format!("locust-startup-{}.log", std::process::id()));
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        let _ = writeln!(file, "{message}");
        eprintln!("Startup diagnostic: {}", path.display());
    }
}

fn exit_with_startup_error(error: anyhow::Error) -> ! {
    let message = format!("Locust could not start.\n\n{error:#}");
    write_startup_note(&message);

    // The plugin needs an app handle, but no webview or parent window. Create
    // only the error dialog; the normal window must not open on failed startup.
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let handle = app.handle().clone();
            app.dialog()
                .message(message)
                .title("Locust startup error")
                .kind(tauri_plugin_dialog::MessageDialogKind::Error)
                .show(move |_| handle.exit(1));
            Ok(())
        })
        .run(context);
    if let Err(error) = result {
        eprintln!("Cannot show startup error dialog: {error}");
    }
    std::process::exit(1);
}

struct Backend {
    state: std::sync::Arc<locust_server::AppState>,
    port: u16,
    stop: tokio::sync::oneshot::Sender<()>,
    thread: std::thread::JoinHandle<anyhow::Result<()>>,
}

impl Backend {
    fn shutdown(self) -> anyhow::Result<()> {
        let _ = self.stop.send(());
        self.thread
            .join()
            .map_err(|_| anyhow::anyhow!("Embedded server thread panicked"))?
    }
}

fn start_backend() -> anyhow::Result<Backend> {
    start_backend_with_state(locust_server::create_app_state()?)
}

fn start_backend_with_state(
    state: std::sync::Arc<locust_server::AppState>,
) -> anyhow::Result<Backend> {
    let state_for_server = state.clone();

    // Keep the socket bound while handing it to the backend. Probing a free
    // port and reopening it races other processes and other Locust instances.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").context(
        "Cannot bind the embedded server on 127.0.0.1:0. Check loopback availability and socket permissions.",
    )?;
    listener
        .set_nonblocking(true)
        .context("Cannot configure the embedded server socket on 127.0.0.1")?;
    let port = listener
        .local_addr()
        .context("Cannot read the embedded server address on 127.0.0.1")?
        .port();

    // Finish all fallible runtime/socket initialization before opening a window.
    let rt = tokio::runtime::Runtime::new()
        .context("Cannot create the embedded server runtime. Check available system resources.")?;
    let listener = {
        let _guard = rt.enter();
        tokio::net::TcpListener::from_std(listener).with_context(|| {
            format!("Cannot initialize the embedded server socket on 127.0.0.1:{port}")
        })?
    };
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let thread = std::thread::Builder::new()
        .name("locust-server".into())
        .spawn(move || {
            rt.block_on(async {
                tracing::info!("Starting embedded server on port {}", port);
                // Self-test requests finish and close their connections before
                // this signal. Drop the idle serve future, then the runtime,
                // releasing the listener and state before joining the thread.
                tokio::select! {
                    result = locust_server::start_server_with_listener(state_for_server, listener) => {
                        if let Err(error) = &result {
                            tracing::error!("Server error: {}", error);
                        }
                        result
                    }
                    _ = stopped => Ok(()),
                }
            })
        })
        .context("Cannot start the embedded server thread. Check available system resources.")?;
    Ok(Backend {
        state,
        port,
        stop,
        thread,
    })
}

fn main() {
    // Branch before panic dialogs, tracing output, or any Tauri/GTK setup.
    // A self-test failure must never wait for a native error dialog.
    if std::env::args_os().any(|arg| arg == "--self-test") {
        std::process::exit(self_test::run());
    }

    #[cfg(not(debug_assertions))]
    setup_panic!();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("locust=info".parse().unwrap()),
        )
        .init();

    let backend = start_backend().unwrap_or_else(|error| exit_with_startup_error(error));

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(commands::AppStateWrapper(backend.state.clone()))
        .manage(commands::ServerPort(backend.port))
        .invoke_handler(tauri::generate_handler![
            commands::get_server_port,
            commands::text_fit,
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
            commands::preview_import_translations,
            commands::apply_import_preview,
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
        .unwrap_or_else(|error| {
            write_startup_note(&format!("Cannot run Locust desktop application: {error}"));
            std::process::exit(1);
        });
    if let Err(error) = backend.shutdown() {
        write_startup_note(&format!("Cannot stop the embedded server: {error:#}"));
        std::process::exit(1);
    }
}
