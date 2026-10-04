use super::*;

#[test]
fn slash_commands_are_bounded_and_hide_host_controls() {
    let reply = json!({"commands": [
        {"name": "mcp-auth", "description": "Sign in", "source": "extension"},
        {"name": "skill:mcp-scripting", "source": "skill"},
        {"name": "fix-tests", "description": "Fix failing tests", "source": "prompt"},
        {"name": "pidesk-mode", "source": "extension"},
        {"name": "bad name", "source": "extension"},
        {"name": "odd", "source": "weird"}
    ]});
    let commands = commands_from(&reply);
    let names: Vec<_> = commands
        .iter()
        .map(|c| (c.name.as_str(), c.source.as_str()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("mcp-auth", "extension"),
            ("skill:mcp-scripting", "skill"),
            ("fix-tests", "prompt"),
            ("odd", "extension")
        ]
    );
    assert_eq!(commands[0].description.as_deref(), Some("Sign in"));
    assert!(commands_from(&json!({})).is_empty());
}

#[test]
fn deletion_only_removes_private_journals() {
    let root = std::env::temp_dir().join(format!("pidesk-delete-{}", uuid::Uuid::new_v4()));
    let sessions = root.join("agent/sessions/thread");
    std::fs::create_dir_all(&sessions).unwrap();
    let journal = sessions.join("session.jsonl");
    let outside = root.join("outside.jsonl");
    std::fs::write(&journal, "history").unwrap();
    std::fs::write(&outside, "keep").unwrap();
    let store = Store::open(Path::new(":memory:")).unwrap();
    let project = store
        .add_project("/tmp/project", "Fixture", HarnessKind::Pi, false)
        .unwrap();
    let row = store
        .upsert_thread(
            "one",
            &project.id,
            HarnessKind::Pi,
            "session",
            &journal.to_string_lossy(),
            "/tmp/project",
            "",
            "idle",
            None,
            None,
        )
        .unwrap();
    delete_session_journal_at(&row, &root).unwrap();
    assert!(!journal.exists());
    // Missing journals are safe to retry, but an outside mapping is not.
    delete_session_journal_at(&row, &root).unwrap();
    let mut forged = row;
    forged.session_file = outside.to_string_lossy().into_owned();
    assert!(delete_session_journal_at(&forged, &root).is_err());
    assert!(outside.exists());
    std::fs::remove_dir_all(root).unwrap();
}

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
        directory: None,
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
