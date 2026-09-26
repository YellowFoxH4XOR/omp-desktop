mod commands;
mod dto;
mod error;
mod git;
mod harness;
pub mod rpc;
mod sessions;
mod state;
mod store;
mod threads;
mod util;
mod watcher;

use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| util::home_dir().join(".pidesk"));
            let database = store::db_path(&data_dir);
            let store = Arc::new(store::Store::open(&database).map_err(|error| {
                std::io::Error::other(format!(
                    "Could not open πDesk metadata at {}: {error}",
                    database.display()
                ))
            })?);
            // Processes from a previous run are gone; reflect that before UI loads.
            store.mark_all_threads_disconnected().map_err(|error| {
                std::io::Error::other(format!("Could not restore thread status: {error}"))
            })?;
            let state = state::AppState::new(app.handle().clone(), store);
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::detect_harnesses,
            commands::install_harness,
            commands::harness_install_commands,
            commands::list_projects,
            commands::add_project,
            commands::remove_project,
            commands::list_threads,
            commands::create_thread,
            commands::open_thread,
            commands::stop_thread,
            commands::get_runtime_stats,
            commands::restart_thread,
            commands::send_prompt,
            commands::abort_thread,
            commands::set_thread_model,
            commands::set_thread_effort,
            commands::get_models,
            commands::get_effort_levels,
            commands::get_usage,
            commands::rename_thread,
            commands::set_thread_flags,
            commands::respond_ui,
            commands::git_status,
            commands::git_file,
            commands::git_revert_file,
            commands::git_write_if_unchanged,
            commands::open_changed_file,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app.try_state::<state::AppState>() {
                    let threads = state.threads.clone();
                    let registry = state.registry.clone();
                    let _ = tauri::async_runtime::block_on(async move {
                        registry.shutdown().await;
                        threads.shutdown_all().await;
                    });
                }
            }
        });
}
