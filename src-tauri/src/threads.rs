use crate::dto::{
    AgentInfo, BackendEvent, ContextUsage, HarnessCapabilities, HarnessKind, ModelInfo,
    SessionSnapshot, SessionState, Thread, TokenCounts, Usage,
};
use crate::error::{AppError, AppResult};
use crate::git;
use crate::harness::HarnessRegistry;
use crate::rpc::{normalize_outgoing_frame, RpcClient, RpcHandlers};
use crate::sessions;
use crate::store::{Store, ThreadRow};
use crate::util;
use crate::watcher::WatcherManager;
use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::process::Command;
use tokio::sync::Notify;

/// Idle threads are suspended after this long without activity.
const IDLE_SUSPEND: Duration = Duration::from_secs(15 * 60);
/// OMP ready handshake timeout.
const READY_TIMEOUT_SECS: u64 = 30;
/// Pi readiness = first get_state response.
const PI_READY_TIMEOUT_SECS: u64 = 45;
/// Login flows can wait on user interaction.
const LOGIN_TIMEOUT_SECS: u64 = 600;

pub struct LiveThread {
    client: Arc<RpcClient>,
    last_activity: Mutex<Instant>,
    streaming: AtomicBool,
    failed: AtomicBool,
    active_subagents: Mutex<std::collections::HashSet<String>>,
    /// Ids of interactive extension_ui_request frames (select, confirm,
    /// input, editor) still waiting on an extension_ui_response.
    pending_ui_requests: Mutex<std::collections::HashSet<String>>,
    /// Ids of fire-and-forget extension_ui_request frames (open_url, widgets)
    /// that never expect an extension_ui_response.
    ui_fire_and_forget: Mutex<std::collections::HashSet<String>>,
    /// Signalled on every RPC event, command, and busy-state change so the
    /// idle watch can re-evaluate without polling.
    activity: Notify,
    /// Per-process delayed-suspension task; aborted when this process is
    /// stopped or replaced so a stale deadline can't kill its successor.
    idle_watch: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl LiveThread {
    /// Record activity and wake the idle watch to re-evaluate its deadline.
    fn note_activity(&self) {
        *self.last_activity.lock() = Instant::now();
        self.activity.notify_one();
    }

    /// Whether the process is doing work that must not be suspended:
    /// streaming a turn, waiting on a UI response, or running subagents.
    fn is_busy(&self) -> bool {
        self.streaming.load(Ordering::SeqCst)
            || !self.pending_ui_requests.lock().is_empty()
            || !self.active_subagents.lock().is_empty()
    }

    /// Abort this process's idle watch (stop/restart/exit paths).
    fn cancel_idle_watch(&self) {
        if let Some(task) = self.idle_watch.lock().take() {
            task.abort();
        }
    }
}
fn merge_agents(mut current: Vec<AgentInfo>, history: Vec<AgentInfo>) -> Vec<AgentInfo> {
    let mut seen: std::collections::HashSet<String> =
        current.iter().map(|agent| agent.id.clone()).collect();
    for agent in history {
        if seen.insert(agent.id.clone()) {
            current.push(agent);
        }
    }
    current
}
/// Link saved top-level subagents back to their parent task call so the
/// delegation card remains useful after resuming a session.
fn attach_task_metadata(agents: &mut [AgentInfo], messages: &[Value]) {
    let by_id: HashMap<String, usize> = agents
        .iter()
        .enumerate()
        .map(|(index, agent)| (agent.id.clone(), index))
        .collect();
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(content) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("toolCall")
                || block.get("name").and_then(Value::as_str) != Some("task")
            {
                continue;
            }
            let call_id = block.get("id").and_then(Value::as_str);
            let Some(tasks) = block
                .get("arguments")
                .and_then(|args| args.get("tasks"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for task in tasks {
                let Some(name) = task.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let Some(&index) = by_id.get(name) else {
                    continue;
                };
                let agent = &mut agents[index];
                if agent.parent_tool_call_id.is_none() {
                    agent.parent_tool_call_id = call_id.map(str::to_owned);
                }
                if agent.role.is_none() {
                    agent.role = task.get("agent").and_then(Value::as_str).map(str::to_owned);
                }
                if agent.task.is_none() {
                    agent.task = task.get("task").and_then(Value::as_str).map(str::to_owned);
                }
            }
        }
    }
}

pub struct ThreadManager {
    store: Arc<Store>,
    registry: Arc<HarnessRegistry>,
    watcher: Arc<WatcherManager>,
    app: AppHandle,
    live: Arc<Mutex<HashMap<String, Arc<LiveThread>>>>,
    /// Threads currently being spawned (prevents double-spawn races).
    spawning: Mutex<std::collections::HashSet<String>>,
}

impl ThreadManager {
    pub fn new(
        store: Arc<Store>,
        registry: Arc<HarnessRegistry>,
        watcher: Arc<WatcherManager>,
        app: AppHandle,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            registry,
            watcher,
            app,
            live: Arc::new(Mutex::new(HashMap::new())),
            spawning: Mutex::new(std::collections::HashSet::new()),
        })
    }
    fn touch_activity(&self, thread_id: &str) {
        if let Some(l) = self.live.lock().get(thread_id) {
            l.note_activity();
        }
    }

    fn set_streaming(&self, thread_id: &str, streaming: bool) {
        if let Some(l) = self.live.lock().get(thread_id) {
            l.streaming.store(streaming, Ordering::SeqCst);
            l.note_activity();
        }
    }

    fn set_status(&self, thread_id: &str, status: &str) {
        let _ = self.store.update_thread_status(thread_id, status);
    }

    // ------------------------------------------------------------------
    // Thread listing / creation
    // ------------------------------------------------------------------

    /// Merge harness session files on disk with app metadata.
    pub fn list_threads(&self, project_id: &str) -> AppResult<Vec<Thread>> {
        let project = self.store.get_project(project_id)?;
        let project_path = PathBuf::from(&project.path);
        let mut rows = self.store.list_threads(project_id)?;
        let mut seen_files: std::collections::HashSet<String> = rows
            .iter()
            .map(|r| r.session_file.clone())
            .filter(|s| !s.is_empty())
            .collect();
        for kind in [HarnessKind::Omp, HarnessKind::Pi] {
            for scanned in sessions::scan_sessions(kind, &project_path) {
                if seen_files.contains(&scanned.session_file) {
                    continue;
                }
                seen_files.insert(scanned.session_file.clone());
                let id = uuid::Uuid::new_v4().to_string();
                let row = self.store.upsert_thread(
                    &id,
                    project_id,
                    kind,
                    &scanned.session_id,
                    &scanned.session_file,
                    &scanned.cwd,
                    &scanned.title,
                    "idle",
                    None,
                    Some(&scanned.created_at),
                )?;
                rows.push(row);
            }
        }
        // Reflect live status for running threads.
        let live = self.live.lock();
        let mut out: Vec<Thread> = rows
            .into_iter()
            .map(|mut r| {
                if let Some(l) = live.get(&r.id) {
                    if l.streaming.load(Ordering::SeqCst) {
                        r.status = "active".to_string();
                    }
                }
                r.into_dto()
            })
            .collect();
        out.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(a.archived.cmp(&b.archived))
                .then(b.last_viewed_at.cmp(&a.last_viewed_at))
        });
        Ok(out)
    }

    /// Create a new thread (and harness session) in a project.
    /// `isolated` requests an app-owned git worktree for the thread.
    pub fn create_thread(
        &self,
        project_id: &str,
        harness: HarnessKind,
        isolated: bool,
    ) -> AppResult<Thread> {
        let project = self.store.get_project(project_id)?;
        let project_path = PathBuf::from(&project.path);
        let existing = self.store.list_threads(project_id)?;
        let live = self.live.lock();
        let live_peer = existing.iter().any(|row| {
            live.get(&row.id)
                .is_some_and(|process| !process.client.is_exited() && row.cwd == project.path)
        });
        let running_peer = existing.iter().any(|row| {
            live.get(&row.id).is_some_and(|process| {
                !process.client.is_exited() && process.is_busy() && row.cwd == project.path
            })
        });
        drop(live);
        let git_repo = (isolated || live_peer) && git::is_repo(&project_path);
        if running_peer && !git_repo {
            return Err(AppError::new(
                "Another thread is running in this directory. Wait for it to finish; concurrent threads need a Git worktree for isolation.",
            ));
        }
        // UI status can lag a worker; Git threads get a separate worktree
        // whenever another live process already owns the project checkout.
        let isolated = isolated || (live_peer && git_repo);
        let id = uuid::Uuid::new_v4().to_string();
        let mut cwd = project_path.clone();
        let mut worktree_path = None;
        if isolated {
            if !git_repo {
                return Err(AppError::new(
                    "Isolated threads need a Git repository; this project is not one.",
                ));
            }
            let dest = util::home_dir()
                .join(".omp-desktop")
                .join("worktrees")
                .join(&id);
            git::create_worktree(&project_path, &dest)?;
            cwd = dest.clone();
            worktree_path = Some(dest.to_string_lossy().to_string());
        }
        let row = self.store.upsert_thread(
            &id,
            project_id,
            harness,
            "",
            "",
            &cwd.to_string_lossy(),
            "",
            "idle",
            worktree_path.as_deref(),
            None,
        )?;
        Ok(row.into_dto())
    }

    // ------------------------------------------------------------------
    // Process lifecycle
    // ------------------------------------------------------------------

    /// Spawn (or resume) the harness process for a thread. Returns the live
    /// handle. Idempotent: returns the existing process when already running.
    pub async fn ensure_running(&self, thread_id: &str) -> AppResult<Arc<LiveThread>> {
        if let Some(l) = self.live.lock().get(thread_id) {
            if !l.client.is_exited() {
                l.note_activity();
                return Ok(l.clone());
            }
        }
        // Serialize concurrent spawns for the same thread.
        let already_spawning = {
            let mut spawning = self.spawning.lock();
            if spawning.contains(thread_id) {
                true
            } else {
                spawning.insert(thread_id.to_string());
                false
            }
        };
        if already_spawning {
            // Another caller is spawning; wait for it to finish.
            for _ in 0..200 {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if let Some(l) = self.live.lock().get(thread_id) {
                    return Ok(l.clone());
                }
                if !self.spawning.lock().contains(thread_id) {
                    break;
                }
            }
            return Err(AppError::new(
                "The session is still starting. Try again in a moment.",
            ));
        }
        let result = self.spawn(thread_id).await;
        self.spawning.lock().remove(thread_id);
        result
    }

    async fn spawn(&self, thread_id: &str) -> AppResult<Arc<LiveThread>> {
        let row = self.store.get_thread(thread_id)?;
        let kind = row.harness;
        let cwd = PathBuf::from(&row.cwd);
        if !cwd.is_dir() {
            return Err(AppError::new(format!(
                "The working directory {} no longer exists.",
                cwd.display()
            )));
        }
        let exe = self.registry.executable_path(kind).await?;
        let mut args: Vec<String> = vec!["--mode".into(), "rpc".into()];
        let session_file = row.session_file.clone();
        let can_resume = !session_file.is_empty() && Path::new(&session_file).is_file();
        match kind {
            HarnessKind::Omp => {
                if can_resume {
                    args.push("--resume".into());
                    args.push(session_file.clone());
                }
            }
            HarnessKind::Pi => {
                if can_resume {
                    args.push("--session".into());
                    args.push(session_file.clone());
                }
            }
        }
        let mut command = Command::new(&exe);
        if let Some(shell_env) = util::login_shell_env() {
            for (key, value) in shell_env {
                if !matches!(key.as_str(), "PWD" | "SHLVL" | "_") && std::env::var_os(key).is_none()
                {
                    command.env(key, value);
                }
            }
        }
        let mut child = command
            .args(&args)
            .current_dir(&cwd)
            .env("PATH", util::merged_path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                AppError::new(format!(
                    "Could not start {} at {}: {e}",
                    kind.display_name(),
                    exe.display()
                ))
            })?;
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let tid = thread_id.to_string();
        let app = self.app.clone();
        let store = self.store.clone();
        let live_map = self.live.clone();
        let watcher = self.watcher.clone();

        // OMP readiness resolves on the `ready` frame.
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Value>();
        let ready_tx = Mutex::new(Some(ready_tx));

        let tid_ev = tid.clone();
        let tid_exit = tid.clone();
        let live_ev = live_map.clone();
        let store_ev = store.clone();
        let app_ev = app.clone();
        // Slot filled with this spawn's client so a stale exit handler can't
        // evict a newer process for the same thread.
        let client_slot: Arc<Mutex<Option<Arc<RpcClient>>>> = Arc::new(Mutex::new(None));
        let client_slot_exit = client_slot.clone();
        let handlers = RpcHandlers {
            on_event: Box::new(move |frame: Value| {
                let ftype = frame.get("type").and_then(Value::as_str).unwrap_or("");
                if let Some(l) = live_ev.lock().get(&tid_ev) {
                    match ftype {
                        "agent_start" => {
                            l.streaming.store(true, Ordering::SeqCst);
                            l.failed.store(false, Ordering::SeqCst);
                            let _ = store_ev.update_thread_status(&tid_ev, "active");
                        }
                        "agent_end" => {
                            if frame.get("isTerminal") != Some(&Value::Bool(false))
                                && frame.get("willRetry") != Some(&Value::Bool(true))
                            {
                                l.streaming.store(false, Ordering::SeqCst);
                                let status = if !l.active_subagents.lock().is_empty() {
                                    "active"
                                } else if l.failed.load(Ordering::SeqCst) {
                                    "failed"
                                } else {
                                    "completed"
                                };
                                let _ = store_ev.update_thread_status(&tid_ev, status);
                            }
                        }
                        "agent_settled" => {
                            l.streaming.store(false, Ordering::SeqCst);
                            let status = if !l.active_subagents.lock().is_empty() {
                                "active"
                            } else if l.failed.load(Ordering::SeqCst) {
                                "failed"
                            } else {
                                "completed"
                            };
                            let _ = store_ev.update_thread_status(&tid_ev, status);
                        }
                        "message_end" => {
                            if frame
                                .get("message")
                                .and_then(|message| message.get("stopReason"))
                                .and_then(Value::as_str)
                                == Some("error")
                            {
                                l.failed.store(true, Ordering::SeqCst);
                            }
                        }
                        "response"
                            if frame.get("success").and_then(Value::as_bool) == Some(false)
                                && matches!(
                                    frame.get("command").and_then(Value::as_str),
                                    Some("prompt" | "abort_and_prompt" | "steer" | "follow_up")
                                ) =>
                        {
                            l.streaming.store(false, Ordering::SeqCst);
                            l.failed.store(true, Ordering::SeqCst);
                            let _ = store_ev.update_thread_status(&tid_ev, "failed");
                        }
                        "subagent_lifecycle" => {
                            if let Some(payload) = frame.get("payload") {
                                if let Some(id) = payload.get("id").and_then(Value::as_str) {
                                    let mut active = l.active_subagents.lock();
                                    if payload.get("status").and_then(Value::as_str)
                                        == Some("started")
                                    {
                                        active.insert(id.to_string());
                                    } else {
                                        active.remove(id);
                                    }
                                }
                            }
                            if !l.streaming.load(Ordering::SeqCst) {
                                let status = if !l.active_subagents.lock().is_empty() {
                                    "active"
                                } else if l.failed.load(Ordering::SeqCst) {
                                    "failed"
                                } else {
                                    "completed"
                                };
                                let _ = store_ev.update_thread_status(&tid_ev, status);
                            }
                        }
                        "extension_ui_request" => {
                            // Fire-and-forget UI requests (open_url, widgets,
                            // notifications) have no pending dialog to answer.
                            let method = frame.get("method").and_then(Value::as_str).unwrap_or("");
                            if matches!(method, "select" | "confirm" | "input" | "editor") {
                                if let Some(id) = frame.get("id").and_then(Value::as_str) {
                                    l.pending_ui_requests.lock().insert(id.to_string());
                                }
                                let _ = store_ev.update_thread_status(&tid_ev, "waiting");
                            }
                            if method == "cancel" {
                                // The harness withdrew a request; resolve the
                                // target locally so it can't block suspension.
                                let target = frame
                                    .get("targetId")
                                    .and_then(Value::as_str)
                                    .or_else(|| frame.get("id").and_then(Value::as_str));
                                if let Some(target) = target {
                                    l.pending_ui_requests.lock().remove(target);
                                    l.ui_fire_and_forget.lock().remove(target);
                                }
                            }
                            if matches!(
                                method,
                                "open_url"
                                    | "setWidget"
                                    | "setStatus"
                                    | "setTitle"
                                    | "notify"
                                    | "cancel"
                            ) {
                                if let Some(id) = frame.get("id").and_then(Value::as_str) {
                                    let mut set = l.ui_fire_and_forget.lock();
                                    if set.len() >= 4096 {
                                        set.clear();
                                    }
                                    set.insert(id.to_string());
                                }
                            }
                        }
                        _ => {}
                    }
                    // Signal after state mutations so a woken idle watch
                    // observes the fresh busy flags.
                    l.note_activity();
                }
                let _ = app_ev.emit(
                    "desktop-event",
                    BackendEvent::Rpc {
                        thread_id: tid_ev.clone(),
                        frame: normalize_outgoing_frame(frame),
                    },
                );
            }),
            on_exit: Box::new(move |code: Option<i32>, stderr: String, expected: bool| {
                // An old process must not change the status or watcher of a
                // replacement spawned for the same thread.
                let Some(mine) = client_slot_exit.lock().clone() else {
                    return;
                };
                let mut map = live_map.lock();
                if !map
                    .get(&tid_exit)
                    .is_some_and(|l| Arc::ptr_eq(&l.client, &mine))
                {
                    return;
                }
                if let Some(l) = map.remove(&tid_exit) {
                    l.cancel_idle_watch();
                }
                drop(map);
                watcher.unwatch(&tid_exit);
                let status = if expected { "idle" } else { "disconnected" };
                let _ = store.update_thread_status(&tid_exit, status);
                let _ = app.emit(
                    "desktop-event",
                    BackendEvent::Exited {
                        thread_id: tid_exit.clone(),
                        code,
                        stderr,
                        expected,
                    },
                );
            }),
            on_ready: match kind {
                HarnessKind::Omp => Some(Box::new(move |frame: Value| {
                    if let Some(tx) = ready_tx.lock().take() {
                        let _ = tx.send(frame);
                    }
                }) as Box<dyn Fn(Value) + Send + Sync>),
                HarnessKind::Pi => None,
            },
        };
        let client = Arc::new(RpcClient::attach(child, stdout, stderr, handlers));
        *client_slot.lock() = Some(client.clone());
        let live = Arc::new(LiveThread {
            client: client.clone(),
            last_activity: Mutex::new(Instant::now()),
            streaming: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            active_subagents: Mutex::new(std::collections::HashSet::new()),
            pending_ui_requests: Mutex::new(std::collections::HashSet::new()),
            ui_fire_and_forget: Mutex::new(std::collections::HashSet::new()),
            activity: Notify::new(),
            idle_watch: Mutex::new(None),
        });
        if let Some(prev) = self.live.lock().insert(tid.clone(), live.clone()) {
            prev.cancel_idle_watch();
        }
        Self::start_idle_watch(
            &live,
            &tid,
            self.live.clone(),
            self.store.clone(),
            IDLE_SUSPEND,
        );
        self.watcher.watch(&tid, &cwd);

        // Handshake per harness.
        match kind {
            HarnessKind::Omp => {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(READY_TIMEOUT_SECS),
                    ready_rx,
                )
                .await
                {
                    Ok(Ok(_)) => {}
                    _ => {
                        let tail = client.stderr_tail().await;
                        client.shutdown().await;
                        live.cancel_idle_watch();
                        self.live.lock().remove(&tid);
                        return Err(AppError::new(format!(
                            "OMP did not become ready.{}",
                            if tail.trim().is_empty() {
                                String::new()
                            } else {
                                format!(" {}", tail.trim())
                            }
                        )));
                    }
                }
                // Negotiate protocol v2 (enables chunked frames).
                let _ = client
                    .call(
                        "negotiate_protocol",
                        Map::from_iter([("protocolVersion".into(), json!(2))]),
                    )
                    .await;
                // Subscribe to subagent lifecycle/progress events.
                let _ = client
                    .call(
                        "set_subagent_subscription",
                        Map::from_iter([("level".into(), json!("progress"))]),
                    )
                    .await;
            }
            HarnessKind::Pi => {
                // No ready frame: get_state doubles as the readiness probe.
                match tokio::time::timeout(
                    std::time::Duration::from_secs(PI_READY_TIMEOUT_SECS),
                    client.call("get_state", Map::new()),
                )
                .await
                {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        client.shutdown().await;
                        live.cancel_idle_watch();
                        self.live.lock().remove(&tid);
                        return Err(e);
                    }
                    Err(_) => {
                        let tail = client.stderr_tail().await;
                        client.shutdown().await;
                        live.cancel_idle_watch();
                        self.live.lock().remove(&tid);
                        return Err(AppError::new(format!(
                            "Pi did not become ready.{}",
                            if tail.trim().is_empty() {
                                String::new()
                            } else {
                                format!(" {}", tail.trim())
                            }
                        )));
                    }
                }
            }
        }
        // Adopt the real session identity once known.
        if let Ok(state) = client.call("get_state", Map::new()).await {
            self.adopt_state(&row, &state)?;
        }
        let _ = self.store.update_thread_status(&tid, "idle");
        let _ = self.store.touch_thread(&tid);
        Ok(live)
    }

    /// Persist session id/file learned from get_state.
    fn adopt_state(&self, row: &ThreadRow, state: &Value) -> AppResult<()> {
        let session_id = state.get("sessionId").and_then(Value::as_str).unwrap_or("");
        let session_file = state
            .get("sessionFile")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !session_id.is_empty() || !session_file.is_empty() {
            self.store
                .update_thread_session(&row.id, session_id, session_file)?;
        }
        Ok(())
    }

    /// Graceful stop: SIGTERM → SIGKILL; emits `exited` with expected=true.
    pub async fn stop(&self, thread_id: &str) -> AppResult<()> {
        let live = self.live.lock().get(thread_id).cloned();
        if let Some(l) = live {
            l.cancel_idle_watch();
            l.client.expect_exit();
            l.client.shutdown().await;
        }
        self.set_status(thread_id, "idle");
        Ok(())
    }

    /// Stop then spawn again (crash recovery / user restart).
    pub async fn restart(&self, thread_id: &str) -> AppResult<Arc<LiveThread>> {
        self.stop(thread_id).await.ok();
        self.ensure_running(thread_id).await
    }

    fn live_client(&self, thread_id: &str) -> Option<Arc<RpcClient>> {
        self.live.lock().get(thread_id).map(|l| l.client.clone())
    }

    /// Client for a thread, spawning the process when needed.
    async fn client_for(&self, thread_id: &str) -> AppResult<Arc<RpcClient>> {
        Ok(self.ensure_running(thread_id).await?.client.clone())
    }

    // ------------------------------------------------------------------
    // Snapshot + queries
    // ------------------------------------------------------------------

    pub async fn snapshot(&self, thread_id: &str) -> AppResult<SessionSnapshot> {
        let live = self.ensure_running(thread_id).await?;
        let row = self.store.get_thread(thread_id)?;
        let client = live.client.clone();
        let state = client.call("get_state", Map::new()).await?;
        self.adopt_state(&row, &state)?;
        let messages = Self::fetch_messages(&client, row.harness).await?;
        let models = self.fetch_models(&client).await;
        let levels = self.fetch_levels(&client).await;
        let stats = client.call("get_session_stats", Map::new()).await.ok();
        let live_agents = self.fetch_agents(&client, row.harness).await;
        let row = self.store.get_thread(thread_id)?;
        let mut agents = if row.harness == HarnessKind::Omp {
            merge_agents(live_agents, sessions::historical_agents(&row.session_file))
        } else {
            live_agents
        };
        if row.harness == HarnessKind::Omp {
            attach_task_metadata(&mut agents, &messages);
        }
        let harness = row.harness;
        Ok(SessionSnapshot {
            thread: row.into_dto(),
            messages,
            state: build_session_state(&state, stats.as_ref()),
            models,
            levels,
            capabilities: HarnessCapabilities::for_kind(harness),
            agents,
        })
    }

    async fn fetch_messages(client: &Arc<RpcClient>, kind: HarnessKind) -> AppResult<Vec<Value>> {
        match kind {
            HarnessKind::Omp => {
                let mut out = Vec::new();
                let mut cursor: Option<String> = None;
                let mut cursors = std::collections::HashSet::new();
                loop {
                    let mut args = Map::from_iter([("limit".into(), json!(256))]);
                    if let Some(c) = &cursor {
                        args.insert("cursor".into(), json!(c));
                    }
                    let data = match client.call("get_messages_page", args).await {
                        Ok(data) => data,
                        Err(_) => {
                            // A busy or stale cursor invalidates every partial page.
                            // Ask the harness for one best-effort snapshot instead.
                            let snapshot = client.call("get_messages", Map::new()).await?;
                            let messages = snapshot
                                .get("messages")
                                .and_then(Value::as_array)
                                .ok_or_else(|| {
                                    AppError::new("OMP returned invalid message history.")
                                })?;
                            return Ok(messages.clone());
                        }
                    };
                    let messages = data
                        .get("messages")
                        .and_then(Value::as_array)
                        .ok_or_else(|| AppError::new("OMP returned an invalid message page."))?;
                    out.extend(messages.iter().cloned());
                    match data.get("nextCursor").and_then(Value::as_str) {
                        Some(next) if !next.is_empty() => {
                            if !cursors.insert(next.to_string()) {
                                return Err(AppError::new(
                                    "OMP repeated a message-history cursor.",
                                ));
                            }
                            cursor = Some(next.to_string());
                        }
                        _ => return Ok(out),
                    }
                }
            }
            HarnessKind::Pi => {
                let snapshot = client.call("get_messages", Map::new()).await?;
                let messages = snapshot
                    .get("messages")
                    .and_then(Value::as_array)
                    .ok_or_else(|| AppError::new("Pi returned invalid message history."))?;
                Ok(messages.clone())
            }
        }
    }

    async fn fetch_models(&self, client: &Arc<RpcClient>) -> Vec<ModelInfo> {
        client
            .call("get_available_models", Map::new())
            .await
            .ok()
            .and_then(|d| d.get("models").and_then(Value::as_array).cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(value_to_model)
            .collect()
    }

    async fn fetch_levels(&self, client: &Arc<RpcClient>) -> Vec<String> {
        client
            .call("get_available_thinking_levels", Map::new())
            .await
            .ok()
            .and_then(|d| d.get("levels").and_then(Value::as_array).cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    }

    async fn fetch_agents(&self, client: &Arc<RpcClient>, kind: HarnessKind) -> Vec<AgentInfo> {
        if kind != HarnessKind::Omp {
            return Vec::new();
        }
        client
            .call("get_subagents", Map::new())
            .await
            .ok()
            .and_then(|d| d.get("subagents").and_then(Value::as_array).cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(value_to_agent)
            .collect()
    }

    // ------------------------------------------------------------------
    // Commands
    // ------------------------------------------------------------------

    pub async fn send_prompt(&self, thread_id: &str, message: &str, mode: &str) -> AppResult<()> {
        let row = self.store.get_thread(thread_id)?;
        let project = self.store.get_project(&row.project_id)?;
        if row.cwd == project.path {
            let peers = self.store.list_threads(&row.project_id)?;
            let live = self.live.lock();
            if peers.iter().any(|peer| {
                peer.id != thread_id
                    && peer.cwd == row.cwd
                    && live
                        .get(&peer.id)
                        .is_some_and(|process| !process.client.is_exited() && process.is_busy())
            }) {
                return Err(AppError::new(
                    "Another thread is working in this checkout. Create a new isolated thread to avoid mixing their changes.",
                ));
            }
        }
        let client = self.client_for(thread_id).await?;
        let command = match mode {
            "steer" => "steer",
            "follow_up" => "follow_up",
            _ => "prompt",
        };
        let args = Map::from_iter([("message".into(), json!(message))]);
        client.call(command, args).await?;
        self.set_status(thread_id, "active");
        self.touch_activity(thread_id);
        Ok(())
    }

    pub async fn abort(&self, thread_id: &str) -> AppResult<()> {
        let Some(client) = self.live_client(thread_id) else {
            return Ok(()); // nothing running
        };
        client.call("abort", Map::new()).await?;
        self.set_streaming(thread_id, false);
        self.set_status(thread_id, "waiting");
        Ok(())
    }

    pub async fn set_model(
        &self,
        thread_id: &str,
        provider: &str,
        model_id: &str,
    ) -> AppResult<SessionState> {
        let client = self.client_for(thread_id).await?;
        client
            .call(
                "set_model",
                Map::from_iter([
                    ("provider".into(), json!(provider)),
                    ("modelId".into(), json!(model_id)),
                ]),
            )
            .await?;
        self.refresh_state(thread_id, &client).await
    }

    pub async fn set_effort(&self, thread_id: &str, level: &str) -> AppResult<SessionState> {
        let client = self.client_for(thread_id).await?;
        client
            .call(
                "set_thinking_level",
                Map::from_iter([("level".into(), json!(level))]),
            )
            .await?;
        self.refresh_state(thread_id, &client).await
    }

    async fn refresh_state(
        &self,
        thread_id: &str,
        client: &Arc<RpcClient>,
    ) -> AppResult<SessionState> {
        let state = client.call("get_state", Map::new()).await?;
        let stats = client.call("get_session_stats", Map::new()).await.ok();
        if let Ok(row) = self.store.get_thread(thread_id) {
            self.adopt_state(&row, &state)?;
        }
        Ok(build_session_state(&state, stats.as_ref()))
    }

    pub async fn get_models(&self, thread_id: &str) -> AppResult<Vec<ModelInfo>> {
        let client = self.client_for(thread_id).await?;
        Ok(self.fetch_models(&client).await)
    }

    pub async fn get_levels(&self, thread_id: &str) -> AppResult<Vec<String>> {
        let client = self.client_for(thread_id).await?;
        Ok(self.fetch_levels(&client).await)
    }

    pub async fn get_usage(&self, thread_id: &str) -> AppResult<Usage> {
        let client = self.client_for(thread_id).await?;
        let stats = client.call("get_session_stats", Map::new()).await?;
        Ok(build_usage(&stats))
    }

    pub async fn respond_ui(
        &self,
        thread_id: &str,
        request_id: &str,
        response: crate::dto::UiResponse,
    ) -> AppResult<()> {
        // Fire-and-forget requests (open_url, widgets) never expect a response;
        // the UI only acknowledges them locally.
        if let Some(l) = self.live.lock().get(thread_id) {
            if l.ui_fire_and_forget.lock().remove(request_id) {
                return Ok(());
            }
            l.pending_ui_requests.lock().remove(request_id);
            l.note_activity();
        }
        let Some(client) = self.live_client(thread_id) else {
            return Err(AppError::new(
                "The session is no longer running; the request has expired.",
            ));
        };
        let mut frame = Map::from_iter([
            ("type".into(), json!("extension_ui_response")),
            ("id".into(), json!(request_id)),
        ]);
        if let Some(v) = response.value {
            frame.insert("value".into(), json!(v));
        }
        if let Some(c) = response.confirmed {
            frame.insert("confirmed".into(), json!(c));
        }
        if let Some(c) = response.cancelled {
            frame.insert("cancelled".into(), json!(c));
        }
        client.send(Value::Object(frame)).await?;
        self.set_status(thread_id, "active");
        Ok(())
    }

    pub async fn get_subagents(&self, thread_id: &str) -> AppResult<Vec<AgentInfo>> {
        let row = self.store.get_thread(thread_id)?;
        if row.harness != HarnessKind::Omp {
            return Ok(Vec::new());
        }
        let history = sessions::historical_agents(&row.session_file);
        let current = if let Some(client) = self.live_client(thread_id) {
            self.fetch_agents(&client, row.harness).await
        } else {
            Vec::new()
        };
        Ok(merge_agents(current, history))
    }

    pub async fn get_subagent_messages(
        &self,
        thread_id: &str,
        agent_id: &str,
    ) -> AppResult<Vec<Value>> {
        let row = self.store.get_thread(thread_id)?;
        if row.harness != HarnessKind::Omp {
            return Err(AppError::new(
                "Subagent transcripts are not supported by this harness.",
            ));
        }
        if !row.session_file.is_empty() {
            if let Ok(messages) = sessions::read_agent_messages(&row.session_file, agent_id) {
                return Ok(messages);
            }
        }
        let client = self.client_for(thread_id).await?;
        let data = client
            .call(
                "get_subagent_messages",
                Map::from_iter([("subagentId".into(), json!(agent_id))]),
            )
            .await?;
        Ok(data
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    pub async fn get_login_providers(
        &self,
        thread_id: &str,
    ) -> AppResult<Vec<crate::dto::LoginProvider>> {
        let row = self.store.get_thread(thread_id)?;
        if row.harness != HarnessKind::Omp {
            return Err(AppError::new(
                "Provider login is not supported by this harness.",
            ));
        }
        let client = self.client_for(thread_id).await?;
        let data = client.call("get_login_providers", Map::new()).await?;
        Ok(data
            .get("providers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|p| {
                Some(crate::dto::LoginProvider {
                    id: p.get("id").and_then(Value::as_str)?.to_string(),
                    name: p
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    available: p.get("available").and_then(Value::as_bool).unwrap_or(false),
                    authenticated: p
                        .get("authenticated")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
            })
            .collect())
    }

    /// Start an OAuth login flow. `extension_ui_request` frames (open_url,
    /// input) flow to the UI through the normal rpc event channel; the UI
    /// answers via respond_ui. Never auto-opens URLs.
    pub async fn login_provider(&self, thread_id: &str, provider_id: &str) -> AppResult<()> {
        let row = self.store.get_thread(thread_id)?;
        if row.harness != HarnessKind::Omp {
            return Err(AppError::new(
                "Provider login is not supported by this harness.",
            ));
        }
        let client = self.client_for(thread_id).await?;
        client
            .call_with_timeout(
                "login",
                Map::from_iter([("providerId".into(), json!(provider_id))]),
                LOGIN_TIMEOUT_SECS,
            )
            .await?;
        Ok(())
    }

    pub fn thread_cwd(&self, thread_id: &str) -> AppResult<PathBuf> {
        let row = self.store.get_thread(thread_id)?;
        Ok(PathBuf::from(row.cwd))
    }

    // ------------------------------------------------------------------
    // Idle suspension
    // ------------------------------------------------------------------

    /// Arm the per-process delayed suspension for one live thread. The task
    /// sleeps until the activity deadline or an activity signal — never on a
    /// fixed interval — and re-checks busy state before shutting down.
    fn start_idle_watch(
        live: &Arc<LiveThread>,
        thread_id: &str,
        live_map: Arc<Mutex<HashMap<String, Arc<LiveThread>>>>,
        store: Arc<Store>,
        idle_for: Duration,
    ) {
        let tid = thread_id.to_string();
        let watched = live.clone();
        let task = tauri::async_runtime::spawn(async move {
            loop {
                if watched.is_busy() {
                    // Streaming, a pending dialog, or active subagents: wait
                    // for the next state change instead of a timer.
                    watched.activity.notified().await;
                    continue;
                }
                let elapsed = watched.last_activity.lock().elapsed();
                let Some(remaining) = idle_for.checked_sub(elapsed) else {
                    // Suspend only if this process is still the live one; a
                    // stale watch must never kill a replacement.
                    let current = live_map
                        .lock()
                        .get(&tid)
                        .is_some_and(|l| Arc::ptr_eq(l, &watched));
                    if !current || watched.client.is_exited() {
                        return;
                    }
                    if watched.is_busy() || watched.last_activity.lock().elapsed() < idle_for {
                        continue;
                    }
                    watched.client.expect_exit();
                    watched.client.shutdown().await;
                    let _ = store.update_thread_status(&tid, "idle");
                    return;
                };
                if remaining.is_zero() {
                    continue;
                }
                // One delayed wake per idle stretch; activity resets it.
                if tokio::time::timeout(remaining, watched.activity.notified())
                    .await
                    .is_err()
                {
                    continue; // deadline reached: re-evaluate
                }
            }
        });
        *live.idle_watch.lock() = Some(task);
    }

    /// Stop every live process (app shutdown).
    pub async fn shutdown_all(&self) {
        let lives: Vec<Arc<LiveThread>> = self.live.lock().values().cloned().collect();
        for l in lives {
            l.cancel_idle_watch();
            l.client.expect_exit();
            l.client.shutdown().await;
        }
    }
}

// ----------------------------------------------------------------------
// Value → DTO mapping
// ----------------------------------------------------------------------

fn value_to_model(v: &Value) -> Option<ModelInfo> {
    Some(ModelInfo {
        provider: v.get("provider").and_then(Value::as_str)?.to_string(),
        id: v.get("id").and_then(Value::as_str)?.to_string(),
        name: v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        context_window: v.get("contextWindow").and_then(Value::as_u64),
        reasoning: v.get("reasoning").and_then(Value::as_bool),
    })
}

fn build_session_state(state: &Value, stats: Option<&Value>) -> SessionState {
    SessionState {
        session_id: state
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        session_file: state
            .get("sessionFile")
            .and_then(Value::as_str)
            .map(String::from),
        session_name: state
            .get("sessionName")
            .and_then(Value::as_str)
            .map(String::from),
        model: state.get("model").and_then(value_to_model),
        thinking_level: state
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .map(String::from),
        is_streaming: state
            .get("isStreaming")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        context_usage: stats
            .and_then(|s| s.get("contextUsage"))
            .map(build_context_usage),
    }
}

fn build_context_usage(v: &Value) -> ContextUsage {
    ContextUsage {
        tokens: v.get("tokens").and_then(Value::as_u64),
        context_window: v.get("contextWindow").and_then(Value::as_u64).unwrap_or(0),
        percent: v.get("percent").and_then(Value::as_f64),
    }
}

fn build_usage(stats: &Value) -> Usage {
    let t = stats.get("tokens").cloned().unwrap_or(Value::Null);
    let num = |k: &str| t.get(k).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        tokens: TokenCounts {
            input: num("input"),
            output: num("output"),
            cache_read: num("cacheRead"),
            cache_write: num("cacheWrite"),
            total: num("total"),
        },
        cost: stats.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
        context_usage: stats.get("contextUsage").map(build_context_usage),
    }
}

fn value_to_agent(v: &Value) -> Option<AgentInfo> {
    let id = v.get("id").and_then(Value::as_str)?.to_string();
    let progress = v.get("progress");
    let status_raw = v
        .get("status")
        .and_then(Value::as_str)
        .or_else(|| progress.and_then(|p| p.get("status").and_then(Value::as_str)))
        .unwrap_or("running");
    let status = match status_raw {
        "started" | "running" => "running",
        "waiting" => "waiting",
        "completed" | "done" | "finished" => "completed",
        "failed" | "error" => "failed",
        "aborted" | "killed" | "cancelled" => "aborted",
        "parked" => "parked",
        "pending" => "pending",
        _ => "running",
    }
    .to_string();
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_else(|| id.rsplit('.').next().unwrap_or(&id))
        .to_string();
    let task = v
        .get("assignment")
        .and_then(Value::as_str)
        .or_else(|| v.get("task").and_then(Value::as_str))
        .or_else(|| v.get("description").and_then(Value::as_str))
        .map(String::from);
    let activity = progress
        .and_then(|p| {
            p.get("lastIntent")
                .and_then(Value::as_str)
                .or_else(|| p.get("activity").and_then(Value::as_str))
                .or_else(|| p.get("message").and_then(Value::as_str))
                .or_else(|| p.get("description").and_then(Value::as_str))
        })
        .map(String::from);
    let tokens = progress.and_then(|p| {
        p.get("tokens")
            .and_then(Value::as_u64)
            .or_else(|| p.get("totalTokens").and_then(Value::as_u64))
    });
    let context_tokens = progress.and_then(|p| {
        p.get("contextTokens").and_then(Value::as_u64).or_else(|| {
            p.get("contextUsage")
                .and_then(|c| c.get("tokens").and_then(Value::as_u64))
        })
    });
    let context_window = progress.and_then(|p| {
        p.get("contextWindow").and_then(Value::as_u64).or_else(|| {
            p.get("contextUsage")
                .and_then(|c| c.get("contextWindow").and_then(Value::as_u64))
        })
    });
    let parent_id = id
        .rfind('.')
        .map(|i| id[..i].to_string())
        .filter(|s| !s.is_empty());
    Some(AgentInfo {
        id,
        parent_id,
        parent_tool_call_id: v
            .get("parentToolCallId")
            .and_then(Value::as_str)
            .map(String::from),
        name,
        role: v
            .get("agent")
            .and_then(Value::as_str)
            .or_else(|| v.get("agentSource").and_then(Value::as_str))
            .map(String::from),
        task,
        status,
        model: progress
            .and_then(|p| {
                p.get("resolvedModelIdentity")
                    .and_then(Value::as_str)
                    .or_else(|| p.get("resolvedModel").and_then(Value::as_str))
                    .or_else(|| p.get("model").and_then(Value::as_str))
            })
            .map(String::from),
        effort: progress
            .and_then(|p| {
                p.get("resolvedThinkingLevel")
                    .and_then(Value::as_str)
                    .or_else(|| p.get("effort").and_then(Value::as_str))
            })
            .map(String::from),
        activity,
        tokens,
        context_tokens,
        context_window,
        cost: progress.and_then(|p| p.get("cost").and_then(Value::as_f64)),
        duration_ms: progress.and_then(|p| {
            p.get("durationMs")
                .and_then(Value::as_u64)
                .or_else(|| p.get("elapsedMs").and_then(Value::as_u64))
        }),
        tool_count: progress.and_then(|p| p.get("toolCount").and_then(Value::as_u64)),
        session_file: v
            .get("sessionFile")
            .and_then(Value::as_str)
            .map(String::from),
        worktree_path: v
            .get("worktreePath")
            .and_then(Value::as_str)
            .map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resumed_delegation_links_saved_agent_to_task_call() {
        let mut agents =
            vec![value_to_agent(&json!({"id":"Explorer","status":"completed"})).unwrap()];
        let messages = vec![json!({"role":"assistant","content":[{
            "type":"toolCall","id":"call-1","name":"task",
            "arguments":{"tasks":[{"name":"Explorer","agent":"scout","task":"Inspect auth"}]}
        }]})];
        attach_task_metadata(&mut agents, &messages);
        assert_eq!(agents[0].parent_tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(agents[0].role.as_deref(), Some("scout"));
        assert_eq!(agents[0].task.as_deref(), Some("Inspect auth"));
    }

    #[test]
    fn rpc_agent_snapshot_keeps_identity_and_current_activity() {
        let snapshot = json!({
            "id":"Backend.DatabaseExpert",
            "agent":"scout",
            "agentSource":"bundled",
            "status":"running",
            "progress":{
                "id":"Backend.DatabaseExpert",
                "status":"running",
                "lastIntent":"Reading schema.rs",
                "resolvedModelIdentity":"openai/gpt-small",
                "resolvedThinkingLevel":"high",
                "tokens":2048,
                "contextTokens":3100,
                "contextWindow":200000,
                "toolCount":3
            }
        });
        let agent = value_to_agent(&snapshot).unwrap();
        assert_eq!(agent.name, "DatabaseExpert");
        assert_eq!(agent.role.as_deref(), Some("scout"));
        assert_eq!(agent.parent_id.as_deref(), Some("Backend"));
        assert_eq!(agent.activity.as_deref(), Some("Reading schema.rs"));
        assert_eq!(agent.model.as_deref(), Some("openai/gpt-small"));
        assert_eq!(agent.effort.as_deref(), Some("high"));
        assert_eq!(agent.tokens, Some(2048));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_page_discards_prior_rows_before_fallback_snapshot() {
        use std::process::Stdio;
        let script = r#"IFS= read -r first
printf '%s\n' '{"type":"response","id":"1","command":"get_messages_page","success":true,"data":{"messages":[{"role":"user","content":"old"}],"nextCursor":"cursor"}}'
IFS= read -r second
printf '%s\n' '{"type":"response","id":"2","command":"get_messages_page","success":false,"code":"stale_cursor","error":"Snapshot changed"}'
IFS= read -r third
printf '%s\n' '{"type":"response","id":"3","command":"get_messages","success":true,"data":{"messages":[{"role":"user","content":"fresh"}]}}'
"#;
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let client = Arc::new(RpcClient::attach(
            child,
            stdout,
            stderr,
            RpcHandlers {
                on_event: Box::new(|_| {}),
                on_exit: Box::new(|_, _, _| {}),
                on_ready: None,
            },
        ));
        let messages = ThreadManager::fetch_messages(&client, HarnessKind::Omp)
            .await
            .unwrap();
        assert_eq!(messages, vec![json!({"role":"user","content":"fresh"})]);
    }

    // ---- idle suspension ----

    fn spawn_cat() -> Arc<RpcClient> {
        let mut child = Command::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn cat");
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        Arc::new(RpcClient::attach(
            child,
            stdout,
            stderr,
            RpcHandlers {
                on_event: Box::new(|_| {}),
                on_exit: Box::new(|_, _, _| {}),
                on_ready: None,
            },
        ))
    }

    fn live_cat() -> Arc<LiveThread> {
        Arc::new(LiveThread {
            client: spawn_cat(),
            last_activity: Mutex::new(Instant::now()),
            streaming: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            active_subagents: Mutex::new(std::collections::HashSet::new()),
            pending_ui_requests: Mutex::new(std::collections::HashSet::new()),
            ui_fire_and_forget: Mutex::new(std::collections::HashSet::new()),
            activity: Notify::new(),
            idle_watch: Mutex::new(None),
        })
    }

    fn test_store() -> Arc<Store> {
        Arc::new(Store::open(Path::new(":memory:")).expect("open in-memory test store"))
    }

    fn live_map_with(
        tid: &str,
        live: &Arc<LiveThread>,
    ) -> Arc<Mutex<HashMap<String, Arc<LiveThread>>>> {
        Arc::new(Mutex::new(HashMap::from([(tid.to_string(), live.clone())])))
    }

    async fn wait_exited(client: &Arc<RpcClient>) -> bool {
        for _ in 0..200 {
            if client.is_exited() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        client.is_exited()
    }

    #[tokio::test]
    async fn idle_watch_suspends_after_deadline() {
        let live = live_cat();
        let map = live_map_with("t1", &live);
        ThreadManager::start_idle_watch(&live, "t1", map, test_store(), Duration::from_millis(50));
        assert!(wait_exited(&live.client).await);
        assert!(live.client.expected_exit());
    }

    #[tokio::test]
    async fn idle_watch_waits_out_streaming_then_suspends() {
        let live = live_cat();
        live.streaming.store(true, Ordering::SeqCst);
        let map = live_map_with("t2", &live);
        ThreadManager::start_idle_watch(&live, "t2", map, test_store(), Duration::from_millis(50));
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!live.client.is_exited());
        live.streaming.store(false, Ordering::SeqCst);
        live.note_activity();
        assert!(wait_exited(&live.client).await);
    }

    #[tokio::test]
    async fn idle_watch_waits_out_pending_ui_request() {
        let live = live_cat();
        live.pending_ui_requests.lock().insert("req-1".to_string());
        let map = live_map_with("t3", &live);
        ThreadManager::start_idle_watch(&live, "t3", map, test_store(), Duration::from_millis(50));
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!live.client.is_exited());
        live.pending_ui_requests.lock().remove("req-1");
        live.note_activity();
        assert!(wait_exited(&live.client).await);
    }

    #[tokio::test]
    async fn stale_idle_watch_cannot_kill_replacement() {
        let old = live_cat();
        let new = live_cat();
        let map = live_map_with("t4", &new);
        ThreadManager::start_idle_watch(&old, "t4", map, test_store(), Duration::from_millis(50));
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!old.client.is_exited());
        assert!(!new.client.is_exited());
    }

    #[tokio::test]
    async fn cancelled_idle_watch_never_suspends() {
        let live = live_cat();
        let map = live_map_with("t5", &live);
        ThreadManager::start_idle_watch(&live, "t5", map, test_store(), Duration::from_millis(50));
        live.cancel_idle_watch();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!live.client.is_exited());
    }
}
