use crate::dto::{
    ChangesSummary, GitFile, HarnessInstallation, HarnessKind, ModelInfo, Project, SessionSnapshot,
    SessionState, Thread, ThreadDeletePreview, UiResponse, Usage,
};
use crate::error::{cmd_err, AppError, CmdResult};
use crate::git;
use crate::state::AppState;
use crate::util;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;

static ACTIVE_INSTALL: Mutex<Option<HarnessKind>> = Mutex::new(None);

/// Sync Tauri commands run on the main thread and freeze the window while
/// they work. Filesystem, git, and SQLite scans go to the blocking pool.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> crate::error::AppResult<T> + Send + 'static,
) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| format!("Background task failed: {error}"))?
        .map_err(cmd_err)
}

struct ActiveInstallGuard;

impl ActiveInstallGuard {
    fn acquire(kind: HarnessKind) -> Result<Self, AppError> {
        let mut active = ACTIVE_INSTALL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active.is_some() {
            return Err(AppError::new(
                "Another harness installation is already in progress. Wait for it to finish, then try again.",
            ));
        }
        *active = Some(kind);
        Ok(Self)
    }
}

impl Drop for ActiveInstallGuard {
    fn drop(&mut self) {
        *ACTIVE_INSTALL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

// ---------------------------------------------------------------------
// Harness detection / install
// ---------------------------------------------------------------------

#[tauri::command]
pub async fn detect_harnesses(state: State<'_, AppState>) -> CmdResult<Vec<HarnessInstallation>> {
    Ok(state.registry.detect().await)
}

#[tauri::command]
pub async fn install_harness(state: State<'_, AppState>, kind: HarnessKind) -> CmdResult<()> {
    let _active_install = ActiveInstallGuard::acquire(kind).map_err(cmd_err)?;
    state
        .registry
        .install(kind, state.app.clone())
        .await
        .map_err(cmd_err)
}

/// Display copies of the fixed install commands, built from the same source
/// as the executed argv so the UI text cannot drift from what actually runs.
#[tauri::command]
pub fn harness_install_commands() -> Vec<crate::dto::HarnessInstallCommand> {
    crate::harness::install_commands()
}

// ---------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------

#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> CmdResult<Vec<Project>> {
    state.store.list_projects().map_err(cmd_err)
}

#[tauri::command]
pub async fn add_project(
    state: State<'_, AppState>,
    path: String,
    harness: HarnessKind,
) -> CmdResult<Project> {
    let store = state.store.clone();
    blocking(move || add_project_inner(&store, &path, harness)).await
}

fn add_project_inner(
    store: &crate::store::Store,
    path: &str,
    harness: HarnessKind,
) -> crate::error::AppResult<Project> {
    let p = PathBuf::from(path);
    if !p.exists() {
        return Err(AppError::new("That directory does not exist."));
    }
    if !p.is_dir() {
        return Err(AppError::new("Projects must be directories, not files."));
    }
    let resolved = util::resolve_path(&p);
    let is_git = git::is_repo(&resolved);
    let preferred = harness;
    let display_name = resolved
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| resolved.to_string_lossy().to_string());
    store.add_project(
        &resolved.to_string_lossy(),
        &display_name,
        preferred,
        is_git,
    )
}

#[tauri::command]
pub async fn remove_project(state: State<'_, AppState>, project_id: String) -> CmdResult<()> {
    let _revocation = state
        .intern
        .begin_project_removal(&project_id)
        .map_err(cmd_err)?;
    state.intern.settle().await;
    // Stop any live threads first; metadata only — never touches files.
    let threads = state.store.list_threads(&project_id).map_err(cmd_err)?;
    for t in &threads {
        state.threads.stop(&t.id).await.map_err(cmd_err)?;
    }
    state.store.remove_project(&project_id).map_err(cmd_err)
}

// ---------------------------------------------------------------------
// Threads
// ---------------------------------------------------------------------

#[tauri::command]
pub async fn list_threads(
    state: State<'_, AppState>,
    project_id: String,
) -> CmdResult<Vec<Thread>> {
    let store = state.store.clone();
    let threads = state.threads.clone();
    blocking(move || {
        let _ = store.touch_project(&project_id);
        threads.list_threads(&project_id)
    })
    .await
}

/// Recently opened threads across projects for the welcome screen. Unlike
/// `list_threads`, this neither rescans sessions nor touches project order.
#[tauri::command]
pub async fn list_recent_threads(
    state: State<'_, AppState>,
    limit: usize,
) -> CmdResult<Vec<Thread>> {
    let store = state.store.clone();
    blocking(move || {
        Ok(store
            .list_recent_threads(limit)?
            .into_iter()
            .map(|row| row.into_dto())
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn set_thread_mode(
    state: State<'_, AppState>,
    thread_id: String,
    mode: String,
) -> CmdResult<Thread> {
    state
        .threads
        .set_mode(&thread_id, &mode)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn create_thread(
    state: State<'_, AppState>,
    project_id: String,
    harness: HarnessKind,
    isolated: Option<bool>,
) -> CmdResult<Thread> {
    let threads = state.threads.clone();
    blocking(move || threads.create_thread(&project_id, harness, isolated.unwrap_or(false))).await
}

#[tauri::command]
pub async fn open_thread(
    state: State<'_, AppState>,
    thread_id: String,
) -> CmdResult<SessionSnapshot> {
    state.threads.snapshot(&thread_id).await.map_err(cmd_err)
}

/// Start a thread's Pi in the background (e.g. on hover) so the following
/// open finds a warm process. Returns immediately; failures surface on open.
#[tauri::command]
pub async fn prewarm_thread(state: State<'_, AppState>, thread_id: String) -> CmdResult<()> {
    let threads = state.threads.clone();
    tauri::async_runtime::spawn(async move {
        let _ = threads.ensure_running(&thread_id).await;
    });
    Ok(())
}

#[tauri::command]
pub async fn get_model_defaults() -> CmdResult<crate::dto::ModelDefaults> {
    blocking(|| crate::pi_settings::read_defaults(&util::pidesk_root())).await
}

#[tauri::command]
pub async fn set_default_model(
    provider: String,
    model_id: String,
) -> CmdResult<crate::dto::ModelDefaults> {
    blocking(move || {
        crate::pi_settings::set_default_model(&util::pidesk_root(), &provider, &model_id)
    })
    .await
}

#[tauri::command]
pub async fn set_default_thinking_level(level: String) -> CmdResult<crate::dto::ModelDefaults> {
    blocking(move || crate::pi_settings::set_default_thinking_level(&util::pidesk_root(), &level))
        .await
}

#[tauri::command]
pub async fn get_runtime_stats(state: State<'_, AppState>) -> CmdResult<crate::dto::RuntimeStats> {
    let threads = state.threads.clone();
    blocking(move || Ok(threads.runtime_stats())).await
}

#[tauri::command]
pub async fn thread_delete_preview(
    state: State<'_, AppState>,
    thread_id: String,
) -> CmdResult<ThreadDeletePreview> {
    let threads = state.threads.clone();
    blocking(move || threads.delete_preview(&thread_id)).await
}

#[tauri::command]
pub async fn delete_thread(
    state: State<'_, AppState>,
    thread_id: String,
    discard_changes: bool,
) -> CmdResult<()> {
    state
        .threads
        .delete_thread(&thread_id, discard_changes)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn stop_thread(state: State<'_, AppState>, thread_id: String) -> CmdResult<()> {
    state.threads.stop(&thread_id).await.map_err(cmd_err)
}

#[tauri::command]
pub async fn restart_thread(
    state: State<'_, AppState>,
    thread_id: String,
) -> CmdResult<SessionSnapshot> {
    state.threads.restart(&thread_id).await.map_err(cmd_err)?;
    state.threads.snapshot(&thread_id).await.map_err(cmd_err)
}

#[tauri::command]
pub async fn send_prompt(
    state: State<'_, AppState>,
    thread_id: String,
    message: String,
    mode: Option<String>,
) -> CmdResult<()> {
    let mode = mode.as_deref().unwrap_or("prompt");
    if !matches!(mode, "prompt" | "steer" | "follow_up") {
        return Err("Invalid message mode.".to_string());
    }
    if message.trim().is_empty() {
        return Err("Cannot send an empty message.".to_string());
    }
    state
        .threads
        .send_prompt(&thread_id, &message, mode)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn compact_thread(
    state: State<'_, AppState>,
    thread_id: String,
    instructions: Option<String>,
) -> CmdResult<()> {
    state
        .threads
        .compact(&thread_id, instructions.as_deref())
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn abort_thread(state: State<'_, AppState>, thread_id: String) -> CmdResult<()> {
    state.threads.abort(&thread_id).await.map_err(cmd_err)
}

#[tauri::command]
pub async fn set_thread_model(
    state: State<'_, AppState>,
    thread_id: String,
    provider: String,
    model_id: String,
) -> CmdResult<SessionState> {
    state
        .threads
        .set_model(&thread_id, &provider, &model_id)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn set_thread_effort(
    state: State<'_, AppState>,
    thread_id: String,
    level: String,
) -> CmdResult<SessionState> {
    state
        .threads
        .set_effort(&thread_id, &level)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn get_models(
    state: State<'_, AppState>,
    thread_id: String,
) -> CmdResult<Vec<ModelInfo>> {
    state.threads.get_models(&thread_id).await.map_err(cmd_err)
}

#[tauri::command]
pub async fn get_effort_levels(
    state: State<'_, AppState>,
    thread_id: String,
) -> CmdResult<Vec<String>> {
    state.threads.get_levels(&thread_id).await.map_err(cmd_err)
}

/// Files under the thread's working folder (all subfolders), for `@` mentions.
#[tauri::command]
pub async fn list_thread_files(
    state: State<'_, AppState>,
    thread_id: String,
) -> CmdResult<crate::dto::FileList> {
    let store = state.store.clone();
    blocking(move || {
        let cwd = PathBuf::from(store.get_thread(&thread_id)?.cwd);
        let (files, truncated) = crate::git::list_files(&cwd)?;
        Ok(crate::dto::FileList { files, truncated })
    })
    .await
}

#[tauri::command]
pub async fn get_usage(state: State<'_, AppState>, thread_id: String) -> CmdResult<Usage> {
    state.threads.get_usage(&thread_id).await.map_err(cmd_err)
}

#[tauri::command]
pub fn rename_thread(
    state: State<'_, AppState>,
    thread_id: String,
    title: String,
) -> CmdResult<Thread> {
    if title.trim().is_empty() {
        return Err("Title cannot be empty.".to_string());
    }
    state
        .store
        .rename_thread(&thread_id, title.trim())
        .map(|r| r.into_dto())
        .map_err(cmd_err)
}

#[tauri::command]
pub fn set_thread_flags(
    state: State<'_, AppState>,
    thread_id: String,
    pinned: Option<bool>,
    archived: Option<bool>,
) -> CmdResult<Thread> {
    state
        .store
        .set_thread_flags(&thread_id, pinned, archived)
        .map(|r| r.into_dto())
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn respond_ui(
    state: State<'_, AppState>,
    thread_id: String,
    request_id: String,
    response: UiResponse,
) -> CmdResult<()> {
    state
        .threads
        .respond_ui(&thread_id, &request_id, response)
        .await
        .map_err(cmd_err)
}

// ---------------------------------------------------------------------
// Pi packages (extensions)
// ---------------------------------------------------------------------

#[tauri::command]
pub async fn extensions_catalog(
    query: String,
    kind: String,
    sort: String,
    page: u32,
) -> CmdResult<crate::extensions::CatalogPage> {
    crate::extensions::catalog(&query, &kind, &sort, page)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn extensions_installed() -> CmdResult<Vec<crate::extensions::InstalledPackage>> {
    blocking(|| crate::extensions::installed(&util::pidesk_root())).await
}

#[tauri::command]
pub async fn extensions_check_updates() -> CmdResult<Vec<crate::extensions::InstalledPackage>> {
    crate::extensions::check_updates(&util::pidesk_root())
        .await
        .map_err(cmd_err)
}

/// Returns the canonical source that was installed.
#[tauri::command]
pub async fn extensions_install(state: State<'_, AppState>, source: String) -> CmdResult<String> {
    crate::extensions::install(&state.registry, &source)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn extensions_update(state: State<'_, AppState>, source: String) -> CmdResult<()> {
    crate::extensions::update(&state.registry, &source)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn extensions_remove(state: State<'_, AppState>, source: String) -> CmdResult<()> {
    crate::extensions::remove(&state.registry, &source)
        .await
        .map_err(cmd_err)
}

// ---------------------------------------------------------------------
// MCP servers (pi-mcp-adapter config in the private agent dir)
// ---------------------------------------------------------------------

#[tauri::command]
pub async fn mcp_overview() -> CmdResult<crate::mcp::McpOverview> {
    blocking(|| Ok(crate::mcp::overview(&util::pidesk_root()))).await
}

#[tauri::command]
pub async fn mcp_save_server(
    original: Option<String>,
    name: String,
    config: serde_json::Value,
) -> CmdResult<()> {
    blocking(move || {
        crate::mcp::save_server(&util::pidesk_root(), original.as_deref(), &name, config)
    })
    .await
}

#[tauri::command]
pub async fn mcp_remove_server(name: String) -> CmdResult<()> {
    blocking(move || crate::mcp::remove_server(&util::pidesk_root(), &name)).await
}

#[tauri::command]
pub async fn mcp_set_enabled(name: String, enabled: bool) -> CmdResult<()> {
    blocking(move || crate::mcp::set_enabled(&util::pidesk_root(), &name, enabled)).await
}

#[tauri::command]
pub async fn mcp_set_approve_tools(all: bool) -> CmdResult<()> {
    blocking(move || crate::mcp::set_approve_tools(&util::pidesk_root(), all)).await
}

#[tauri::command]
pub async fn mcp_save_raw(text: String) -> CmdResult<()> {
    blocking(move || crate::mcp::save_raw(&util::pidesk_root(), &text)).await
}

/// Copies only the servers the user picked; returns the names copied.
#[tauri::command]
pub async fn mcp_import(source: String, names: Vec<String>) -> CmdResult<Vec<String>> {
    blocking(move || crate::mcp::import(&util::pidesk_root(), &source, &names)).await
}

#[tauri::command]
pub async fn mcp_install_adapter(state: State<'_, AppState>) -> CmdResult<()> {
    crate::extensions::install(&state.registry, crate::mcp::ADAPTER_SOURCE)
        .await
        .map(|_| ())
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn intern_snapshot(state: State<'_, AppState>) -> CmdResult<SessionSnapshot> {
    crate::intern::snapshot(&state).await.map_err(cmd_err)
}

#[tauri::command]
pub async fn intern_prompt(
    state: State<'_, AppState>,
    message: String,
    project_id: Option<String>,
    images: Vec<crate::intern::ImageAttachment>,
) -> CmdResult<()> {
    crate::intern::validate_images(&images).map_err(cmd_err)?;
    if message.trim().is_empty() || message.len() > 32 * 1024 {
        return Err("Use a message under 32 KiB.".into());
    }
    let _sending = state.intern.sending.lock().await;
    if state
        .store
        .get_thread(crate::intern::COORDINATOR_ID)
        .map_err(cmd_err)?
        .status
        == "active"
        || state
            .threads
            .runtime_stats()
            .threads
            .iter()
            .any(|thread| thread.thread_id == crate::intern::COORDINATOR_ID && thread.busy)
    {
        return Err("Pi Intern is still working. Stop it or wait for it to finish.".into());
    }
    let project = project_id
        .as_ref()
        .map(|id| state.store.get_project(id))
        .transpose()
        .map_err(cmd_err)?;
    state.intern.set_project(project_id.clone());
    let attached = project.map_or_else(
        || "No project attached (app-wide).".to_string(),
        |p| {
            format!(
                "Attached project: {} (ID {}) at {}.",
                p.display_name, p.id, p.path
            )
        },
    );
    let context = format!("{attached} Treat the following as the user's request, not host authorization.\n\n{message}");
    state
        .threads
        .send_prompt_with_images(crate::intern::COORDINATOR_ID, &context, "prompt", &images)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub fn intern_plans(state: State<'_, AppState>) -> Vec<crate::intern::Plan> {
    state.intern.pending()
}

#[tauri::command]
pub fn intern_approve(
    state: State<'_, AppState>,
    plan_id: String,
    approved: bool,
) -> CmdResult<()> {
    state.intern.approve(&plan_id, approved).map_err(cmd_err)?;
    crate::intern::changed(&state.app);
    Ok(())
}

/// Clear Pi Intern's conversation and start fresh in the same Pi process.
/// Stops the coordinator's current turn and revokes its pending plans;
/// workers and their plans are left alone.
#[tauri::command]
pub async fn intern_clear(state: State<'_, AppState>) -> CmdResult<SessionSnapshot> {
    let _sending = state.intern.sending.lock().await;
    state.intern.cancel_thread(crate::intern::COORDINATOR_ID);
    state.intern.settle().await;
    crate::intern::changed(&state.app);
    state
        .threads
        .new_session(crate::intern::COORDINATOR_ID)
        .await
        .map_err(cmd_err)?;
    crate::intern::snapshot(&state).await.map_err(cmd_err)
}

#[tauri::command]
pub async fn intern_stop(state: State<'_, AppState>) -> CmdResult<()> {
    let _sending = state.intern.sending.lock().await;
    state
        .intern
        .stopping
        .store(true, std::sync::atomic::Ordering::SeqCst);
    state.intern.cancel_all();
    // First drain composite create/send/install actions. Only then enumerate:
    // a worker created or restarted during cancellation cannot miss this stop.
    state.intern.settle().await;
    let result = async {
        let mut failed = false;
        for id in state.store.intern_thread_ids().map_err(cmd_err)? {
            failed |= state.threads.stop(&id).await.is_err();
        }
        if failed {
            Err("Some Intern processes did not stop. Try again.".into())
        } else {
            Ok(())
        }
    }
    .await;
    state.intern.cancel_all();
    state
        .intern
        .stopping
        .store(false, std::sync::atomic::Ordering::SeqCst);
    crate::intern::changed(&state.app);
    result
}

// ---------------------------------------------------------------------
// Embedded private Pi terminal
// ---------------------------------------------------------------------

#[tauri::command]
pub async fn terminal_open(
    state: State<'_, AppState>,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
    output: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>,
) -> CmdResult<String> {
    let home = util::home_dir();
    let directory = if let Some(cwd) = cwd {
        let requested = std::fs::canonicalize(&cwd)
            .map_err(|_| "Project directory is unavailable.".to_string())?;
        let projects = state.store.list_projects().map_err(cmd_err)?;
        if !projects
            .iter()
            .any(|project| std::fs::canonicalize(&project.path).ok().as_ref() == Some(&requested))
        {
            return Err("Terminal directory must be a registered project.".into());
        }
        requested
    } else {
        home
    };
    state
        .terminals
        .open(cols, rows, &directory, output, &state.registry)
        .await
        .map_err(cmd_err)
}

/// Non-blocking: input is queued on the terminal's ordered writer thread.
#[tauri::command]
pub async fn terminal_write(state: State<'_, AppState>, id: String, data: String) -> CmdResult<()> {
    state.terminals.write(&id, &data).map_err(cmd_err)
}

#[tauri::command]
pub async fn terminal_ack(state: State<'_, AppState>, id: String, bytes: usize) -> CmdResult<()> {
    state.terminals.ack(&id, bytes);
    Ok(())
}

#[tauri::command]
pub async fn terminal_resize(
    state: State<'_, AppState>,
    id: String,
    cols: u16,
    rows: u16,
) -> CmdResult<()> {
    let terminals = state.terminals.clone();
    blocking(move || terminals.resize(&id, cols, rows)).await
}

#[tauri::command]
pub async fn terminal_close(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let terminals = state.terminals.clone();
    blocking(move || {
        terminals.close(&id);
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------
// Git
// ---------------------------------------------------------------------

#[tauri::command]
pub async fn git_status(
    state: State<'_, AppState>,
    thread_id: String,
) -> CmdResult<ChangesSummary> {
    let cwd = state.threads.thread_cwd(&thread_id).map_err(cmd_err)?;
    blocking(move || git::status(&cwd)).await
}

#[tauri::command]
pub async fn git_file(
    state: State<'_, AppState>,
    thread_id: String,
    path: String,
) -> CmdResult<GitFile> {
    let cwd = state.threads.thread_cwd(&thread_id).map_err(cmd_err)?;
    blocking(move || git::file(&cwd, &path)).await
}

#[tauri::command]
pub async fn git_revert_file(
    state: State<'_, AppState>,
    thread_id: String,
    path: String,
    expected_hash: Option<String>,
) -> CmdResult<()> {
    let cwd = state.threads.thread_cwd(&thread_id).map_err(cmd_err)?;
    blocking(move || git::revert_file(&cwd, &path, expected_hash.as_deref())).await
}

#[tauri::command]
pub async fn git_write_if_unchanged(
    state: State<'_, AppState>,
    thread_id: String,
    path: String,
    expected_hash: String,
    content: String,
) -> CmdResult<()> {
    let cwd = state.threads.thread_cwd(&thread_id).map_err(cmd_err)?;
    blocking(move || git::write_if_unchanged(&cwd, &path, &expected_hash, &content)).await
}

#[tauri::command]
pub async fn open_changed_file(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    thread_id: String,
    path: String,
) -> CmdResult<()> {
    let cwd = state.threads.thread_cwd(&thread_id).map_err(cmd_err)?;
    let abs = blocking(move || git::openable_path(&cwd, &path)).await?;
    tauri_plugin_opener::OpenerExt::opener(&app)
        .open_path(abs.to_string_lossy().to_string(), None::<&str>)
        .map_err(|error| cmd_err(AppError::new(format!("Could not open file: {error}"))))
}
