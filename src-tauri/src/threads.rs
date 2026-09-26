use crate::dto::{
    BackendEvent, ContextUsage, HarnessCapabilities, HarnessKind, ModelInfo, SessionSnapshot,
    SessionState, Thread, TokenCounts, Usage,
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::process::Command;
use tokio::sync::{Mutex as AsyncMutex, Notify};

/// Idle threads are suspended after this long without activity.
const IDLE_SUSPEND: Duration = Duration::from_secs(15 * 60);
/// Pi readiness = first get_state response.
const PI_READY_TIMEOUT_SECS: u64 = 45;
/// Hard cap prevents an unbounded number of live harness process trees.
const MAX_LIVE_THREADS: usize = 16;
/// Aggregate budgets for Pi's monolithic message history.
const MAX_HISTORY_MESSAGES: usize = 16_384;
const MAX_HISTORY_BYTES: usize = 32 * 1024 * 1024;
/// Pi can acknowledge a prompt before its turn emits `agent_start`. The
/// checkout reservation is held this long waiting for the turn to appear.
const PI_PROMPT_SETTLE_SECS: u64 = 30;
/// Same-thread prompts are serialized so an older failed/reservation release
/// cannot tear down a newer accepted turn.

pub struct LiveThread {
    client: Arc<RpcClient>,
    /// Monotonic identity for this process incarnation. Event callbacks carry
    /// the generation they were created for and ignore later incarnations.
    generation: u64,
    last_activity: Mutex<Instant>,
    streaming: AtomicBool,
    failed: AtomicBool,
    /// Interactive extension_ui_request frames (select, confirm, input, or
    /// editor) still waiting on an extension_ui_response, mapped
    /// to the requested method so answers can be validated before sending.
    pending_ui_requests: Mutex<HashMap<String, String>>,
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

    /// Whether the process is doing work that must not be suspended.
    fn is_busy(&self) -> bool {
        self.streaming.load(Ordering::SeqCst) || !self.pending_ui_requests.lock().is_empty()
    }

    /// Abort this process's idle watch (stop/restart/exit paths).
    fn cancel_idle_watch(&self) {
        if let Some(task) = self.idle_watch.lock().take() {
            task.abort();
        }
    }
}
/// Serialization shared by prompts, stops, and idle suspension. The watcher's
/// delayed task runs without a `ThreadManager` handle, so the map lives in
/// static storage and every path takes the same entry. Prompt takes `prompt`,
/// stop/idle take `prompt` then `lifecycle` in the same order. Entries are
/// reaped when uncontended so both the manager map and this map stay
/// O(active threads).
fn shared_locks_map() -> &'static Mutex<HashMap<String, Arc<ThreadLocks>>> {
    static LOCKS: std::sync::LazyLock<Mutex<HashMap<String, Arc<ThreadLocks>>>> =
        std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
    &LOCKS
}

fn shared_locks(thread_id: &str) -> Arc<ThreadLocks> {
    shared_locks_map()
        .lock()
        .entry(thread_id.to_string())
        .or_insert_with(|| {
            Arc::new(ThreadLocks {
                lifecycle: AsyncMutex::new(()),
                prompt: AsyncMutex::new(()),
            })
        })
        .clone()
}

/// Drop a shared-lock entry when nobody else can be holding it. Every clone
/// happens under the map lock, so a concurrent holder blocks reaping and a
/// later caller transparently creates a fresh entry.
fn reap_shared_locks(thread_id: &str, locks: &Arc<ThreadLocks>) {
    let mut map = shared_locks_map().lock();
    if map
        .get(thread_id)
        .is_some_and(|entry| Arc::ptr_eq(entry, locks))
        && Arc::strong_count(locks) == 2
    {
        map.remove(thread_id);
    }
}

fn release_owner(owners: &Mutex<HashMap<String, String>>, cwd: &str, thread_id: &str) {
    let key = ThreadManager::checkout_key(cwd);
    let mut owners = owners.lock();
    if owners.get(&key).is_some_and(|owner| owner == thread_id) {
        owners.remove(&key);
    }
}

fn acquire_owner(owners: &Mutex<HashMap<String, String>>, cwd: &str, thread_id: &str) -> bool {
    let key = ThreadManager::checkout_key(cwd);
    let mut owners = owners.lock();
    if owners
        .get(&key)
        .is_some_and(|owner| owner.as_str() != thread_id)
    {
        return false;
    }
    owners.insert(key, thread_id.to_string());
    true
}

fn resume_arguments(session_id: &str, session_file: &str) -> AppResult<Vec<String>> {
    match (session_id, session_file) {
        ("", "") => Ok(Vec::new()),
        ("", _) | (_, "") => Err(AppError::new(
            "This thread's saved session mapping is incomplete. Restore its session file before reopening it.",
        )),
        (_, mapped) if Path::new(mapped).is_file() => Ok(vec![
            "--session".into(),
            util::private_session_file(Path::new(mapped))?
                .to_string_lossy()
                .into_owned(),
        ]),
        (id, mapped) => {
            // Pi reports its journal path before the first turn is written, so a
            // thread opened but never messaged has no file yet. `--session-id`
            // looks the id up in the private session dir (finding a moved
            // journal) and otherwise recreates the same session identity;
            // `--session <missing path>` would mint a different id instead.
            util::private_session_target(Path::new(mapped)).map_err(|_| {
                AppError::new(format!(
                    "The saved session file {mapped} is missing or has moved. Restore it before reopening this thread."
                ))
            })?;
            if !is_valid_session_id(id) {
                return Err(AppError::new(
                    "This thread's saved session id is invalid and cannot be reopened.",
                ));
            }
            Ok(vec!["--session-id".into(), id.to_string()])
        }
    }
}

/// Tells the model what πDesk's transcript can render. Passed as a flag rather
/// than written to `APPEND_SYSTEM.md`, which stays free for the user's own text.
const RENDERING_GUIDE: &str = "You are running inside πDesk, a desktop app that renders your replies as GitHub-flavored Markdown. \
When a diagram would help (architecture, flows, sequences, state machines, ER models), write it as a fenced ```mermaid code block; πDesk renders Mermaid as a real diagram. \
Prefer Mermaid over ASCII or box-drawing art, keep node labels short, and use plain code fences only for actual code or terminal output.";

fn spawn_arguments(session_dir: &Path) -> Vec<String> {
    // sessionDir is read before project trust, so --no-approve alone does
    // not isolate it. The explicit directory is authoritative for Pi.
    vec![
        "--mode".into(),
        "rpc".into(),
        "--no-approve".into(),
        "--session-dir".into(),
        session_dir.to_string_lossy().into_owned(),
        "--append-system-prompt".into(),
        RENDERING_GUIDE.into(),
    ]
}

/// Pi accepts letters, digits, `.`, `_`, and `-`; a leading alphanumeric keeps
/// the value from ever being parsed as a CLI flag.
fn is_valid_session_id(id: &str) -> bool {
    id.len() <= 128
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}
pub struct ThreadManager {
    store: Arc<Store>,
    registry: Arc<HarnessRegistry>,
    watcher: Arc<WatcherManager>,
    app: AppHandle,
    live: Arc<Mutex<HashMap<String, Arc<LiveThread>>>>,
    next_generation: AtomicU64,
    /// Exclusive owner of a non-isolated checkout. A reservation is made
    /// before a prompt starts and lives until that turn becomes terminal or
    /// its process exits.
    checkout_owners: Arc<Mutex<HashMap<String, String>>>,
    shutting_down: AtomicBool,
    live_admissions: Mutex<usize>,
}

struct ThreadLocks {
    lifecycle: AsyncMutex<()>,
    prompt: AsyncMutex<()>,
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
            next_generation: AtomicU64::new(1),
            checkout_owners: Arc::new(Mutex::new(HashMap::new())),
            shutting_down: AtomicBool::new(false),
            live_admissions: Mutex::new(0),
        })
    }

    fn admit_live(&self, thread_id: &str) -> AppResult<()> {
        let mut live = self.live.lock();
        live.retain(|_, process| !process.client.is_exited());
        if live.contains_key(thread_id) {
            return Ok(());
        }
        let mut admissions = self.live_admissions.lock();
        if live.len() + *admissions >= MAX_LIVE_THREADS {
            return Err(AppError::new(
                "The app has reached its live session limit. Stop a session and try again.",
            ));
        }
        *admissions += 1;
        Ok(())
    }

    fn finish_live_admission(&self) {
        let mut admissions = self.live_admissions.lock();
        *admissions = admissions.saturating_sub(1);
    }

    fn checkout_key(cwd: &str) -> String {
        util::resolve_path(Path::new(cwd))
            .to_string_lossy()
            .into_owned()
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

    fn live_client(&self, thread_id: &str) -> Option<Arc<RpcClient>> {
        self.live.lock().get(thread_id).map(|l| l.client.clone())
    }

    /// Client for a thread, spawning the process when needed.
    async fn client_for(&self, thread_id: &str) -> AppResult<Arc<RpcClient>> {
        Ok(self.ensure_running(thread_id).await?.client.clone())
    }

    /// Atomically acquire a checkout for an active turn. The owner remains
    /// reserved until that process reports terminal/idle state or exits.
    fn reserve_checkout(&self, cwd: &str, thread_id: &str) -> AppResult<()> {
        if acquire_owner(&self.checkout_owners, cwd, thread_id) {
            Ok(())
        } else {
            Err(AppError::new(
                "Another thread is working in this checkout. Create a new isolated thread to avoid mixing their changes.",
            ))
        }
    }

    fn release_checkout(&self, cwd: &str, thread_id: &str) {
        let key = Self::checkout_key(cwd);
        let mut owners = self.checkout_owners.lock();
        if owners.get(&key).is_some_and(|owner| owner == thread_id) {
            owners.remove(&key);
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
        let mut seen_sessions: std::collections::HashSet<String> = self
            .store
            .registered_session_files(project_id)?
            .into_iter()
            .collect();
        for scanned in sessions::scan_sessions(&project_path) {
            if !seen_sessions.insert(scanned.session_file.clone()) {
                continue;
            }
            let id = uuid::Uuid::new_v4().to_string();
            let row = self.store.upsert_thread(
                &id,
                project_id,
                HarnessKind::Pi,
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
        drop(live);

        // Serialize the peer decision with prompt reservations. Creation only
        // inserts metadata; ownership is acquired by the first prompt/turn.
        let checkout_key = Self::checkout_key(&project.path);
        let owners = self.checkout_owners.lock();
        let peer = live_peer || owners.contains_key(&checkout_key);
        let git_repo = (isolated || peer) && git::is_repo(&project_path);
        if peer && !git_repo {
            return Err(AppError::new(
                "Another thread is working in this directory. Wait for it to finish; concurrent threads need a Git worktree for isolation.",
            ));
        }
        let isolated = isolated || (peer && git_repo);
        let id = uuid::Uuid::new_v4().to_string();
        let mut cwd = project_path.clone();
        let mut worktree_path = None;
        if isolated {
            if !git_repo {
                return Err(AppError::new(
                    "Isolated threads need a Git repository; this project is not one.",
                ));
            }
            let dest = util::home_dir().join(".pidesk").join("worktrees").join(&id);
            git::create_worktree(&project_path, &dest)?;
            cwd = dest;
            worktree_path = Some(cwd.to_string_lossy().to_string());
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
    /// handle. Idempotent: returns the existing process when still running.
    pub async fn ensure_running(&self, thread_id: &str) -> AppResult<Arc<LiveThread>> {
        // Validate before inserting so repeated calls with unknown ids can
        // never grow the map; every clone happens under the map lock so an
        // uncontended entry can be reaped safely. Register the lifecycle
        // before checking the shutdown flag. This makes shutdown_all's
        // snapshot include every operation that passed the pre-shutdown
        // check, while the second check still prevents spawning once
        // shutdown has started.
        self.store.get_thread(thread_id)?;
        let locks = shared_locks(thread_id);
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(AppError::new("The app is shutting down."));
        }
        let _lifecycle = locks.lifecycle.lock().await;
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(AppError::new("The app is shutting down."));
        }
        if let Some(live) = Self::take_reusable_live(&self.live, thread_id) {
            live.note_activity();
            return Ok(live);
        }
        self.admit_live(thread_id)?;
        let result = self.spawn(thread_id).await;
        self.finish_live_admission();
        result
    }

    /// Return a running process, eagerly evicting a handle whose exit callback
    /// has not run yet. This closes the stop/restart reuse window.
    fn take_reusable_live(
        live: &Mutex<HashMap<String, Arc<LiveThread>>>,
        thread_id: &str,
    ) -> Option<Arc<LiveThread>> {
        let mut live = live.lock();
        let reusable = live
            .get(thread_id)
            .filter(|process| !process.client.is_exited())
            .cloned();
        if reusable.is_none() {
            live.remove(thread_id);
        }
        reusable
    }

    fn remove_generation(
        live: &Mutex<HashMap<String, Arc<LiveThread>>>,
        thread_id: &str,
        generation: u64,
    ) -> Option<Arc<LiveThread>> {
        let mut live = live.lock();
        if !live
            .get(thread_id)
            .is_some_and(|process| process.generation == generation)
        {
            return None;
        }
        let process = live.remove(thread_id)?;
        process.cancel_idle_watch();
        Some(process)
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
        let root = util::pidesk_root();
        let session_dir = util::session_dir_for(&cwd);
        util::ensure_private_directory(&root, &session_dir)?;
        let mut args = spawn_arguments(&session_dir);
        args.extend(resume_arguments(&row.session_id, &row.session_file)?);
        let mut command = Command::new(&exe);
        util::configure_private_command(&mut command, &root);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .args(&args)
            .current_dir(&cwd)
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
        let checkout_owners = self.checkout_owners.clone();

        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        let tid_ev = tid.clone();
        let tid_exit = tid.clone();
        let live_ev = live_map.clone();
        let store_ev = store.clone();
        let app_ev = app.clone();
        let checkout_owners_ev = checkout_owners.clone();
        let cwd_ev = cwd.to_string_lossy().into_owned();
        let cwd_exit = cwd_ev.clone();
        let cwd_cleanup = cwd_ev.clone();
        let checkout_owners_exit = checkout_owners.clone();
        // Events from an older process must not mutate or emit for a replacement.
        // The generation is captured before the client is attached.
        let handlers = RpcHandlers {
            on_event: Box::new(move |frame: Value| {
                let ftype = frame.get("type").and_then(Value::as_str).unwrap_or("");
                let Some(l) = live_ev
                    .lock()
                    .get(&tid_ev)
                    .filter(|l| l.generation == generation)
                    .cloned()
                else {
                    return;
                };
                // A stop/restart on another thread can replace the live process
                // between any two statements below. Re-check that this frame's
                // generation is still live immediately before every shared side
                // effect; a stale frame must never mutate its replacement.
                let is_current = || {
                    live_ev
                        .lock()
                        .get(&tid_ev)
                        .is_some_and(|current| Arc::ptr_eq(current, &l))
                };
                match ftype {
                    "agent_start" => {
                        l.streaming.store(true, Ordering::SeqCst);
                        l.failed.store(false, Ordering::SeqCst);
                        if is_current() {
                            let _ = store_ev.update_thread_status(&tid_ev, "active");
                        }
                    }
                    "agent_end" => {
                        // Pi can still compact or drain queued handlers after agent_end;
                        // agent_settled is the terminal event.
                        if is_current() {
                            l.note_activity();
                        }
                    }
                    "agent_settled" => {
                        l.streaming.store(false, Ordering::SeqCst);
                        let status = if l.failed.load(Ordering::SeqCst) {
                            "failed"
                        } else {
                            "completed"
                        };
                        if is_current() {
                            let _ = store_ev.update_thread_status(&tid_ev, status);
                        }
                        if status != "active" && is_current() {
                            release_owner(&checkout_owners_ev, &cwd_ev, &tid_ev);
                        }
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
                    "extension_ui_request" => {
                        let method = frame.get("method").and_then(Value::as_str).unwrap_or("");
                        if matches!(method, "select" | "confirm" | "input" | "editor") {
                            if let Some(id) = frame.get("id").and_then(Value::as_str) {
                                l.pending_ui_requests
                                    .lock()
                                    .insert(id.to_string(), method.to_string());
                            }
                            if is_current() {
                                let _ = store_ev.update_thread_status(&tid_ev, "waiting");
                            }
                        }
                        if method == "cancel" {
                            let target = frame
                                .get("targetId")
                                .and_then(Value::as_str)
                                .or_else(|| frame.get("id").and_then(Value::as_str));
                            if let Some(target) = target {
                                l.pending_ui_requests.lock().remove(target);
                                l.ui_fire_and_forget.lock().remove(target);
                            }
                            if l.pending_ui_requests.lock().is_empty()
                                && !l.streaming.load(Ordering::SeqCst)
                            {
                                if is_current() {
                                    let _ = store_ev.update_thread_status(&tid_ev, "completed");
                                }
                                if is_current() {
                                    release_owner(&checkout_owners_ev, &cwd_ev, &tid_ev);
                                }
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
                if is_current() {
                    l.note_activity();
                    let _ = app_ev.emit(
                        "desktop-event",
                        BackendEvent::Rpc {
                            thread_id: tid_ev.clone(),
                            frame: normalize_outgoing_frame(frame),
                        },
                    );
                }
            }),
            on_exit: Box::new(move |code: Option<i32>, stderr: String, expected: bool| {
                let mut map = live_map.lock();
                if !map
                    .get(&tid_exit)
                    .is_some_and(|l| l.generation == generation)
                {
                    return;
                }
                if let Some(l) = map.remove(&tid_exit) {
                    l.cancel_idle_watch();
                }
                drop(map);
                watcher.unwatch(&tid_exit);
                release_owner(&checkout_owners_exit, &cwd_exit, &tid_exit);
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
        };
        let client = Arc::new(RpcClient::attach_with_frame_limit(
            child,
            stdout,
            stderr,
            handlers,
            crate::rpc::MAX_PI_FRAME_BYTES,
        ));
        let live = Arc::new(LiveThread {
            client: client.clone(),
            generation,
            last_activity: Mutex::new(Instant::now()),
            streaming: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            pending_ui_requests: Mutex::new(HashMap::new()),
            ui_fire_and_forget: Mutex::new(std::collections::HashSet::new()),
            activity: Notify::new(),
            idle_watch: Mutex::new(None),
        });
        if let Some(prev) = self.live.lock().insert(tid.clone(), live.clone()) {
            prev.cancel_idle_watch();
        }
        // The watcher starts only after the readiness handshake succeeds.

        // Pi has no ready frame: get_state doubles as the readiness probe.
        match tokio::time::timeout(
            std::time::Duration::from_secs(PI_READY_TIMEOUT_SECS),
            client.call("get_state", Map::new()),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                client.shutdown().await;
                Self::remove_generation(&self.live, &tid, generation);
                release_owner(&checkout_owners, &cwd_cleanup, &tid);
                return Err(error);
            }
            Err(_) => {
                let tail = client.stderr_tail().await;
                client.shutdown().await;
                Self::remove_generation(&self.live, &tid, generation);
                release_owner(&checkout_owners, &cwd_cleanup, &tid);
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
        // Readiness is proven; only now may file events start flowing.
        self.watcher.watch(&tid, &cwd);
        Self::start_idle_watch(
            &live,
            &tid,
            self.live.clone(),
            self.store.clone(),
            self.checkout_owners.clone(),
            cwd.to_string_lossy().into_owned(),
            IDLE_SUSPEND,
        );
        let state = match client.call("get_state", Map::new()).await {
            Ok(state) => state,
            Err(error) => {
                client.shutdown().await;
                Self::remove_generation(&self.live, &tid, generation);
                self.watcher.unwatch(&tid);
                release_owner(&checkout_owners, &cwd_cleanup, &tid);
                return Err(error);
            }
        };
        if let Err(error) = self.adopt_state(&row, &state) {
            client.shutdown().await;
            Self::remove_generation(&self.live, &tid, generation);
            self.watcher.unwatch(&tid);
            release_owner(&checkout_owners, &cwd_cleanup, &tid);
            return Err(error);
        }
        let _ = self.store.update_thread_status(&tid, "idle");
        let _ = self.store.touch_thread(&tid);
        Ok(live)
    }

    /// Persist session identity learned from get_state without erasing a
    /// durable mapping when a harness omits an optional field.
    fn adopt_state(&self, row: &ThreadRow, state: &Value) -> AppResult<()> {
        let session_id = state.get("sessionId").and_then(Value::as_str).unwrap_or("");
        let session_file = state
            .get("sessionFile")
            .and_then(Value::as_str)
            .unwrap_or("");
        if session_id.is_empty() {
            return Err(AppError::new(
                "The harness returned an invalid empty session identity.",
            ));
        }
        if !session_file.is_empty() {
            util::private_session_target(Path::new(session_file))?;
        }
        if !row.session_id.is_empty() && !session_id.is_empty() && row.session_id != session_id {
            return Err(AppError::new(
                "The harness resumed a different session than the one mapped to this thread.",
            ));
        }
        // An unwritten journal gets a fresh timestamped path on every launch;
        // only a journal that exists on disk pins the mapping.
        if !row.session_file.is_empty()
            && !session_file.is_empty()
            && Path::new(&row.session_file).exists()
            && util::resolve_path(Path::new(&row.session_file))
                != util::resolve_path(Path::new(session_file))
        {
            return Err(AppError::new(
                "The harness resumed a different session file than the one mapped to this thread.",
            ));
        }
        self.store
            .update_thread_session(&row.id, session_id, session_file)
    }

    /// Graceful stop: SIGTERM → SIGKILL; emits `exited` with expected=true.
    /// The live entry is removed first so a failed stop never blocks a fresh
    /// spawn, but the checkout stays reserved and the status stays `active`
    /// until the harness is confirmed dead: callers (and `restart()`) must
    /// not treat an unconfirmed stop as success.
    pub async fn stop(&self, thread_id: &str) -> AppResult<()> {
        // Stopping an already-removed thread must still kill its process, so
        // a live handle bypasses store validation and goes straight to the
        // map; an unknown id without a handle fails closed with no insert.
        // Every clone happens under the map lock so uncontended entries can
        // be reaped safely when the stop completes.
        let locks = if self.live.lock().contains_key(thread_id) {
            shared_locks(thread_id)
        } else {
            self.store.get_thread(thread_id)?;
            shared_locks(thread_id)
        };
        let _prompt = locks.prompt.lock().await;
        let _lifecycle = locks.lifecycle.lock().await;
        let live = self.live.lock().remove(thread_id);
        let Some(live) = live else {
            if self.store.get_thread(thread_id).is_ok() {
                self.set_status(thread_id, "idle");
            }
            drop(_lifecycle);
            drop(_prompt);
            reap_shared_locks(thread_id, &locks);
            return Ok(());
        };
        live.cancel_idle_watch();
        live.client.expect_exit();
        live.client.shutdown().await;
        // Give the exit monitor a beat to observe the reaped child before
        // declaring the stop unconfirmed; the poll loop runs every ~25ms.
        for _ in 0..40 {
            if live.client.is_exited() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        if live.client.is_exited() {
            if let Ok(row) = self.store.get_thread(thread_id) {
                self.release_checkout(&row.cwd, thread_id);
                self.set_status(thread_id, "idle");
            }
            drop(_lifecycle);
            drop(_prompt);
            reap_shared_locks(thread_id, &locks);
            return Ok(());
        }
        // Termination unconfirmed: restore the handle so the session stays
        // cached and retryable, keep the checkout reserved, and fail loudly
        // instead of letting `restart()` spawn a second harness.
        self.live.lock().insert(thread_id.to_string(), live);
        if let Ok(row) = self.store.get_thread(thread_id) {
            let _ = self.reserve_checkout(&row.cwd, thread_id);
            self.set_status(thread_id, "active");
        }
        Err(AppError::new(
            "The harness process did not stop. It may still be running — try stopping again.",
        ))
    }
    /// Stop then spawn again (crash recovery / user restart). A stop that
    /// cannot confirm termination keeps the reservation and fails instead
    /// of spawning a second harness for the same session.
    pub async fn restart(&self, thread_id: &str) -> AppResult<Arc<LiveThread>> {
        self.stop(thread_id).await?;
        self.ensure_running(thread_id).await
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
        let messages = Self::fetch_messages(&client).await?;
        let models_result = client.call("get_available_models", Map::new()).await;
        let models = models_result
            .as_ref()
            .ok()
            .and_then(|d| d.get("models").and_then(Value::as_array))
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(value_to_model)
            .collect::<Vec<_>>();
        let levels_result = client
            .call("get_available_thinking_levels", Map::new())
            .await;
        let levels = levels_result
            .as_ref()
            .ok()
            .and_then(|d| d.get("levels").and_then(Value::as_array))
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|value| value.as_str().map(String::from))
            .collect::<Vec<_>>();
        let stats = client.call("get_session_stats", Map::new()).await.ok();
        let row = self.store.get_thread(thread_id)?;
        let mut capabilities = HarnessCapabilities::for_kind(HarnessKind::Pi);
        capabilities.model_switching = models_result.is_ok();
        capabilities.effort_levels = levels_result.is_ok();
        capabilities.context_usage = stats
            .as_ref()
            .is_some_and(|stats| stats.get("contextUsage").is_some());
        capabilities.token_usage = stats.is_some();
        Ok(SessionSnapshot {
            thread: row.into_dto(),
            messages,
            state: build_session_state(&state, stats.as_ref()),
            models,
            levels,
            capabilities,
        })
    }

    async fn fetch_messages(client: &Arc<RpcClient>) -> AppResult<Vec<Value>> {
        let snapshot = client.call("get_messages", Map::new()).await?;
        let messages = snapshot
            .get("messages")
            .and_then(Value::as_array)
            .ok_or_else(|| AppError::new("Pi returned invalid message history."))?;
        Self::check_history_budgets(messages)?;
        Ok(messages.clone())
    }

    fn check_history_budgets(messages: &[Value]) -> AppResult<()> {
        if messages.len() > MAX_HISTORY_MESSAGES {
            return Err(AppError::new(
                "The harness returned more message history than the app can hold. Ask it for a compacted view and try again.",
            ));
        }
        let mut bytes: usize = 0;
        for message in messages {
            let encoded = serde_json::to_vec(message)
                .map_err(|_| AppError::new("The harness returned invalid message history."))?;
            bytes = bytes.saturating_add(encoded.len());
            if bytes > MAX_HISTORY_BYTES {
                return Err(AppError::new(
                    "The harness returned more message history than the app can hold. Ask it for a compacted view and try again.",
                ));
            }
        }
        Ok(())
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

    // ------------------------------------------------------------------
    // Commands
    // ------------------------------------------------------------------

    pub async fn send_prompt(&self, thread_id: &str, message: &str, mode: &str) -> AppResult<()> {
        self.store.get_thread(thread_id)?;
        let locks = shared_locks(thread_id);
        let _prompt = locks.prompt.lock().await;
        let row = self.store.get_thread(thread_id)?;
        let key = Self::checkout_key(&row.cwd);
        let already_owned = self
            .checkout_owners
            .lock()
            .get(&key)
            .is_some_and(|owner| owner == thread_id);
        self.reserve_checkout(&row.cwd, thread_id)?;
        let client = match self.client_for(thread_id).await {
            Ok(client) => client,
            Err(error) => {
                if !already_owned {
                    self.release_checkout(&row.cwd, thread_id);
                }
                return Err(error);
            }
        };
        let command = match mode {
            "steer" => "steer",
            "follow_up" => "follow_up",
            _ => "prompt",
        };
        let args = Map::from_iter([("message".into(), json!(message))]);
        if let Err(error) = client.call(command, args).await {
            if !already_owned {
                self.release_checkout(&row.cwd, thread_id);
                self.set_status(thread_id, "failed");
            }
            return Err(error);
        }
        let busy = self
            .live
            .lock()
            .get(thread_id)
            .is_some_and(|live| live.is_busy());
        self.set_status(thread_id, "active");
        self.touch_activity(thread_id);
        if !busy && !already_owned {
            // Pi can acknowledge before agent_start. Hold the checkout for a
            // bounded window; terminal event handlers release it earlier.
            let store = self.store.clone();
            let owners = self.checkout_owners.clone();
            let live_map = self.live.clone();
            let tid = thread_id.to_string();
            let cwd = row.cwd.clone();
            let key = key.clone();
            tauri::async_runtime::spawn(async move {
                let deadline = Instant::now() + Duration::from_secs(PI_PROMPT_SETTLE_SECS);
                while Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let state = live_map.lock().get(&tid).map(|live| {
                        (
                            live.streaming.load(Ordering::SeqCst),
                            live.failed.load(Ordering::SeqCst),
                            live.is_busy(),
                        )
                    });
                    let Some((streaming, failed, now_busy)) = state else {
                        return;
                    };
                    if streaming || failed || now_busy {
                        return;
                    }
                    if !owners.lock().get(&key).is_some_and(|owner| owner == &tid) {
                        return;
                    }
                }
                let locks = shared_locks(&tid);
                let _prompt = locks.prompt.lock().await;
                let Some(live) = live_map.lock().get(&tid).cloned() else {
                    return;
                };
                if live.streaming.load(Ordering::SeqCst)
                    || live.failed.load(Ordering::SeqCst)
                    || live.is_busy()
                    || !owners.lock().get(&key).is_some_and(|owner| owner == &tid)
                {
                    return;
                }
                if store
                    .get_thread(&tid)
                    .ok()
                    .is_some_and(|row| row.status == "active")
                {
                    live.note_activity();
                    let _ = store.update_thread_status(&tid, "completed");
                    release_owner(&owners, &cwd, &tid);
                }
            });
        }
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
        // Capture one incarnation and keep its request tracked until the frame
        // is accepted by that exact client. A replacement can never inherit
        // this response.
        let live = self.live.lock().get(thread_id).cloned().ok_or_else(|| {
            AppError::new("The session is no longer running; the request has expired.")
        })?;
        if live.ui_fire_and_forget.lock().remove(request_id) {
            return Ok(());
        }
        let method = live
            .pending_ui_requests
            .lock()
            .get(request_id)
            .cloned()
            .ok_or_else(|| AppError::new("The UI request has already expired."))?;
        let cancelled = response.cancelled.unwrap_or(false);
        if !cancelled {
            let usable = match method.as_str() {
                "select" | "input" | "editor" => response
                    .value
                    .as_ref()
                    .is_some_and(|value| !value.is_empty()),
                "confirm" => response.confirmed.is_some(),
                _ => false,
            };
            if !usable {
                return Err(AppError::new(format!(
                    "This answer is missing the {method} request's required field; the request is still waiting."
                )));
            }
            if response.value.is_some() && response.confirmed.is_some() {
                return Err(AppError::new(
                    "This answer mixes a value with a confirmation; the request is still waiting.",
                ));
            }
        }
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
        live.client.send(Value::Object(frame)).await?;
        live.pending_ui_requests.lock().remove(request_id);
        live.note_activity();
        if self
            .live
            .lock()
            .get(thread_id)
            .is_some_and(|current| Arc::ptr_eq(current, &live))
        {
            self.set_status(thread_id, "active");
        }
        Ok(())
    }

    /// Memory and activity for every running Pi. Reads only kernel counters and
    /// metadata; it never spawns processes or touches a thread's lifecycle.
    pub fn runtime_stats(&self) -> crate::dto::RuntimeStats {
        let live: Vec<(String, Arc<LiveThread>)> = self
            .live
            .lock()
            .iter()
            .filter(|(_, process)| !process.client.is_exited())
            .map(|(id, process)| (id.clone(), process.clone()))
            .collect();
        let mut threads: Vec<crate::dto::ThreadRuntime> = live
            .into_iter()
            .filter_map(|(thread_id, process)| {
                let row = self.store.get_thread(&thread_id).ok()?;
                let pid = process.client.process_group_id();
                let (memory_bytes, process_count) = pid
                    .and_then(util::process_group_footprint)
                    .map_or((None, 0), |(bytes, count)| (Some(bytes), count));
                Some(crate::dto::ThreadRuntime {
                    thread_id,
                    project_id: row.project_id,
                    title: row.title,
                    pid,
                    memory_bytes,
                    process_count,
                    busy: process.is_busy(),
                    idle_seconds: process.last_activity.lock().elapsed().as_secs(),
                })
            })
            .collect();
        threads.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes));
        crate::dto::RuntimeStats {
            app_bytes: util::process_footprint(std::process::id() as i32),
            threads,
        }
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
        owners: Arc<Mutex<HashMap<String, String>>>,
        cwd: String,
        idle_for: Duration,
    ) {
        let tid = thread_id.to_string();
        let watched = live.clone();
        let task = tauri::async_runtime::spawn(async move {
            loop {
                if watched.is_busy() {
                    // Streaming or a pending dialog: wait for the next state
                    // change instead of a timer.
                    watched.activity.notified().await;
                    continue;
                }
                let elapsed = watched.last_activity.lock().elapsed();
                let Some(remaining) = idle_for.checked_sub(elapsed) else {
                    // Serialize with in-flight prompts/stops through the same
                    // per-thread locks `stop()` takes (prompt, then lifecycle,
                    // same order). Holding both across the re-check and the
                    // shutdown closes the window where a prompt sits between
                    // its RPC return and its busy flag. The guards are scoped
                    // so the locks are released before the task waits again.
                    let locks = shared_locks(&tid);
                    {
                        let _prompt = locks.prompt.lock().await;
                        let _lifecycle = locks.lifecycle.lock().await;
                        // Suspend only if this process is still the live one; a
                        // stale watch must never kill a replacement.
                        let current = live_map
                            .lock()
                            .get(&tid)
                            .is_some_and(|l| Arc::ptr_eq(l, &watched));
                        if !current || watched.client.is_exited() {
                            reap_shared_locks(&tid, &locks);
                            return;
                        }
                        if watched.is_busy() || watched.last_activity.lock().elapsed() < idle_for {
                            continue;
                        }
                        watched.client.expect_exit();
                        watched.client.shutdown().await;
                        // A prompt that raced the shutdown wins if it made the
                        // process busy or replaced the live pointer; never write
                        // `idle` or release a reservation that no longer belongs
                        // to this generation.
                        if !live_map
                            .lock()
                            .get(&tid)
                            .is_some_and(|l| Arc::ptr_eq(l, &watched))
                            || watched.is_busy()
                        {
                            reap_shared_locks(&tid, &locks);
                            return;
                        }
                        let _ = store.update_thread_status(&tid, "idle");
                        release_owner(&owners, &cwd, &tid);
                        reap_shared_locks(&tid, &locks);
                        return;
                    }
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
        self.shutting_down.store(true, Ordering::SeqCst);
        let entries: Vec<(String, Arc<ThreadLocks>)> = shared_locks_map()
            .lock()
            .iter()
            .map(|(id, locks)| (id.clone(), locks.clone()))
            .collect();
        for (id, locks) in entries {
            let _lifecycle = locks.lifecycle.lock().await;
            if let Some(live) = self.live.lock().remove(&id) {
                live.cancel_idle_watch();
                live.client.expect_exit();
                self.watcher.unwatch(&id);
                live.client.shutdown().await;
                if let Ok(row) = self.store.get_thread(&id) {
                    self.release_checkout(&row.cwd, &id);
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_mapped_session_fails_closed() {
        let missing = std::env::temp_dir().join(format!("pidesk-missing-{}", uuid::Uuid::new_v4()));
        assert!(resume_arguments("session-1", &missing.to_string_lossy())
            .unwrap_err()
            .to_string()
            .contains("missing or has moved"));
        assert!(resume_arguments("", "").unwrap().is_empty());
    }

    #[test]
    fn spawn_tells_pi_that_mermaid_renders() {
        let args = spawn_arguments(Path::new("/tmp/sessions"));
        let flag = args
            .iter()
            .position(|arg| arg == "--append-system-prompt")
            .expect("rendering guide flag");
        assert!(args[flag + 1].contains("```mermaid"));
        // Pi reads an existing path's contents instead; the guide must stay literal text.
        assert!(!Path::new(&args[flag + 1]).exists());
        assert_eq!(
            args[..5],
            [
                "--mode",
                "rpc",
                "--no-approve",
                "--session-dir",
                "/tmp/sessions"
            ]
        );
    }

    #[test]
    fn session_ids_are_validated_before_reaching_the_cli() {
        assert!(is_valid_session_id("01a0db99-77b6-7537-954a-8a394b96b5bc"));
        assert!(is_valid_session_id("session_1.2"));
        assert!(!is_valid_session_id(""));
        assert!(!is_valid_session_id("--mode"));
        assert!(!is_valid_session_id("-x"));
        assert!(!is_valid_session_id("a b"));
        assert!(!is_valid_session_id("../escape"));
        assert!(!is_valid_session_id(&"a".repeat(129)));
    }

    #[test]
    fn checkout_reservation_race_has_one_owner() {
        let owners = Arc::new(Mutex::new(HashMap::new()));
        let cwd = std::env::temp_dir().join(format!("pidesk-checkout-{}", uuid::Uuid::new_v4()));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|id| {
                let owners = owners.clone();
                let barrier = barrier.clone();
                let cwd = cwd.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    acquire_owner(&owners, &cwd.to_string_lossy(), id)
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|owned| **owned).count(), 1);
    }

    #[tokio::test]
    async fn expected_exit_is_never_reused() {
        let old = live_cat();
        let map = live_map_with("restart", &old);
        old.client.expect_exit();
        old.client.shutdown().await;
        assert!(wait_exited(&old.client).await);
        assert!(ThreadManager::take_reusable_live(&map, "restart").is_none());
    }

    #[tokio::test]
    async fn stale_generation_cannot_remove_replacement() {
        let old = live_cat();
        let mut new = live_cat();
        Arc::get_mut(&mut new).expect("unique live").generation = 2;
        let map: Arc<Mutex<HashMap<String, Arc<LiveThread>>>> =
            Arc::new(Mutex::new(HashMap::from([("thread".into(), new.clone())])));
        assert!(ThreadManager::remove_generation(&map, "thread", old.generation).is_none());
        assert!(Arc::ptr_eq(
            map.lock().get("thread").expect("replacement remains"),
            &new
        ));
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
            },
        ))
    }

    fn live_cat() -> Arc<LiveThread> {
        Arc::new(LiveThread {
            client: spawn_cat(),
            generation: 1,
            last_activity: Mutex::new(Instant::now()),
            streaming: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            pending_ui_requests: Mutex::new(HashMap::new()),
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
        ThreadManager::start_idle_watch(
            &live,
            "t1",
            map,
            test_store(),
            Arc::new(Mutex::new(HashMap::new())),
            "/tmp".into(),
            Duration::from_millis(50),
        );
        assert!(wait_exited(&live.client).await);
        assert!(live.client.expected_exit());
    }

    #[tokio::test]
    async fn idle_watch_waits_out_streaming_then_suspends() {
        let live = live_cat();
        live.streaming.store(true, Ordering::SeqCst);
        let map = live_map_with("t2", &live);
        ThreadManager::start_idle_watch(
            &live,
            "t2",
            map,
            test_store(),
            Arc::new(Mutex::new(HashMap::new())),
            "/tmp".into(),
            Duration::from_millis(50),
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!live.client.is_exited());
        live.streaming.store(false, Ordering::SeqCst);
        live.note_activity();
        assert!(wait_exited(&live.client).await);
    }

    #[tokio::test]
    async fn idle_watch_waits_out_pending_ui_request() {
        let live = live_cat();
        live.pending_ui_requests
            .lock()
            .insert("req-1".to_string(), "input".to_string());
        let map = live_map_with("t3", &live);
        ThreadManager::start_idle_watch(
            &live,
            "t3",
            map,
            test_store(),
            Arc::new(Mutex::new(HashMap::new())),
            "/tmp".into(),
            Duration::from_millis(50),
        );
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
        ThreadManager::start_idle_watch(
            &old,
            "t4",
            map,
            test_store(),
            Arc::new(Mutex::new(HashMap::new())),
            "/tmp".into(),
            Duration::from_millis(50),
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!old.client.is_exited());
        assert!(!new.client.is_exited());
    }

    #[tokio::test]
    async fn cancelled_idle_watch_never_suspends() {
        let live = live_cat();
        let map = live_map_with("t5", &live);
        ThreadManager::start_idle_watch(
            &live,
            "t5",
            map,
            test_store(),
            Arc::new(Mutex::new(HashMap::new())),
            "/tmp".into(),
            Duration::from_millis(50),
        );
        live.cancel_idle_watch();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!live.client.is_exited());
    }
}
