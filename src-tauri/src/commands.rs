use crate::dto::{
    AgentInfo, ChangesSummary, GitFile, HarnessInstallation, HarnessKind, LoginProvider, ModelInfo,
    Project, SessionSnapshot, SessionState, Thread, UiResponse, Usage,
};
use crate::error::{cmd_err, AppError, CmdResult};
use crate::git;
use crate::state::AppState;
use crate::util;
use serde_json::Value;
use std::path::PathBuf;
use tauri::State;

// ---------------------------------------------------------------------
// Harness detection / install
// ---------------------------------------------------------------------

#[tauri::command]
pub async fn detect_harnesses(state: State<'_, AppState>) -> CmdResult<Vec<HarnessInstallation>> {
    Ok(state.registry.detect().await)
}

#[tauri::command]
pub async fn set_executable_override(
    state: State<'_, AppState>,
    kind: HarnessKind,
    path: String,
) -> CmdResult<HarnessInstallation> {
    state
        .registry
        .set_override(kind, &path)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn install_harness(state: State<'_, AppState>, kind: HarnessKind) -> CmdResult<()> {
    let registry = state.registry.clone();
    let app = state.app.clone();
    // Run in the background; progress streams via install_progress events.
    tauri::async_runtime::spawn(async move {
        let _ = registry.install(kind, app).await;
    });
    Ok(())
}

// ---------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------

#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> CmdResult<Vec<Project>> {
    state.store.list_projects().map_err(cmd_err)
}

#[tauri::command]
pub fn add_project(
    state: State<'_, AppState>,
    path: String,
    harness: HarnessKind,
) -> CmdResult<Project> {
    add_project_inner(&state, &path, harness).map_err(cmd_err)
}

fn add_project_inner(
    state: &AppState,
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
    // Harness config markers influence the preferred harness: an explicit
    // .omp or .pi directory wins over the picker default.
    let preferred = if resolved.join(".omp").exists() {
        HarnessKind::Omp
    } else if resolved.join(".pi").exists() {
        HarnessKind::Pi
    } else {
        harness
    };
    let display_name = resolved
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| resolved.to_string_lossy().to_string());
    state.store.add_project(
        &resolved.to_string_lossy(),
        &display_name,
        preferred,
        is_git,
    )
}

#[tauri::command]
pub async fn remove_project(state: State<'_, AppState>, project_id: String) -> CmdResult<()> {
    // Stop any live threads first; metadata only — never touches files.
    let threads = state.store.list_threads(&project_id).map_err(cmd_err)?;
    for t in &threads {
        let _ = state.threads.stop(&t.id).await;
    }
    state.store.remove_project(&project_id).map_err(cmd_err)
}

// ---------------------------------------------------------------------
// Threads
// ---------------------------------------------------------------------

#[tauri::command]
pub fn list_threads(state: State<'_, AppState>, project_id: String) -> CmdResult<Vec<Thread>> {
    let _ = state.store.touch_project(&project_id);
    state.threads.list_threads(&project_id).map_err(cmd_err)
}

#[tauri::command]
pub fn create_thread(
    state: State<'_, AppState>,
    project_id: String,
    harness: HarnessKind,
    isolated: Option<bool>,
) -> CmdResult<Thread> {
    state
        .threads
        .create_thread(&project_id, harness, isolated.unwrap_or(false))
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn open_thread(
    state: State<'_, AppState>,
    thread_id: String,
) -> CmdResult<SessionSnapshot> {
    state.threads.snapshot(&thread_id).await.map_err(cmd_err)
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
// Subagents
// ---------------------------------------------------------------------

#[tauri::command]
pub async fn get_subagent_messages(
    state: State<'_, AppState>,
    thread_id: String,
    agent_id: String,
) -> CmdResult<Vec<Value>> {
    state
        .threads
        .get_subagent_messages(&thread_id, &agent_id)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn get_subagents(
    state: State<'_, AppState>,
    thread_id: String,
) -> CmdResult<Vec<AgentInfo>> {
    state
        .threads
        .get_subagents(&thread_id)
        .await
        .map_err(cmd_err)
}

// ---------------------------------------------------------------------
// Login providers
// ---------------------------------------------------------------------

#[tauri::command]
pub async fn get_login_providers(
    state: State<'_, AppState>,
    thread_id: String,
) -> CmdResult<Vec<LoginProvider>> {
    state
        .threads
        .get_login_providers(&thread_id)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn login_provider(
    state: State<'_, AppState>,
    thread_id: String,
    provider_id: String,
) -> CmdResult<()> {
    state
        .threads
        .login_provider(&thread_id, &provider_id)
        .await
        .map_err(cmd_err)
}

// ---------------------------------------------------------------------
// Git
// ---------------------------------------------------------------------

#[tauri::command]
pub fn git_status(state: State<'_, AppState>, thread_id: String) -> CmdResult<ChangesSummary> {
    let cwd = state.threads.thread_cwd(&thread_id).map_err(cmd_err)?;
    git::status(&cwd).map_err(cmd_err)
}

#[tauri::command]
pub fn git_file(state: State<'_, AppState>, thread_id: String, path: String) -> CmdResult<GitFile> {
    let cwd = state.threads.thread_cwd(&thread_id).map_err(cmd_err)?;
    git::file(&cwd, &path).map_err(cmd_err)
}

#[tauri::command]
pub fn git_revert_file(
    state: State<'_, AppState>,
    thread_id: String,
    path: String,
) -> CmdResult<()> {
    let cwd = state.threads.thread_cwd(&thread_id).map_err(cmd_err)?;
    git::revert_file(&cwd, &path).map_err(cmd_err)
}

#[tauri::command]
pub fn git_write_if_unchanged(
    state: State<'_, AppState>,
    thread_id: String,
    path: String,
    expected_hash: String,
    content: String,
) -> CmdResult<()> {
    let cwd = state.threads.thread_cwd(&thread_id).map_err(cmd_err)?;
    git::write_if_unchanged(&cwd, &path, &expected_hash, &content).map_err(cmd_err)
}
