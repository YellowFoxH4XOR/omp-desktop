use crate::dto::BackendEvent;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

/// Watches thread working directories and emits `git_changed` when files or
/// git metadata change. Event-driven only — no polling.
///
/// `notify::RecommendedWatcher` is `Send` but not `Sync`, so all watchers live
/// on one dedicated thread driven by a command channel; the manager itself is
/// a cheap `Send + Sync` handle.
pub struct WatcherManager {
    cmd: SyncSender<Cmd>,
    /// thread_id → (cwd, generation); the generation rejects stale commands.
    threads: Arc<Mutex<HashMap<String, (PathBuf, u64)>>>,
    next_generation: std::sync::atomic::AtomicU64,
}

enum Cmd {
    Watch {
        thread_id: String,
        cwd: PathBuf,
        generation: u64,
    },
    Unwatch {
        thread_id: String,
        generation: u64,
    },
}

const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);
/// Continuous churn (builds, installs) would otherwise reset the debounce
/// forever and the Changes panel would never refresh while an agent works.
const MAX_DEBOUNCE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
const COMMAND_CAPACITY: usize = 256;
const EVENT_CAPACITY: usize = 8;
const MAX_WATCHED_DIRS: usize = 32;

impl WatcherManager {
    pub fn new(app: AppHandle) -> Self {
        let (cmd_tx, cmd_rx) = sync_channel::<Cmd>(COMMAND_CAPACITY);
        let threads = Arc::new(Mutex::new(HashMap::<String, (PathBuf, u64)>::new()));
        let threads2 = threads.clone();
        std::thread::spawn(move || watcher_thread(cmd_rx, threads2, app));
        Self {
            cmd: cmd_tx,
            threads,
            next_generation: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// Start watching `cwd` for `thread_id` (idempotent).
    pub fn watch(&self, thread_id: &str, cwd: &Path) {
        let generation = self
            .next_generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Publish membership only when the request was accepted. A failed send
        // must not leave the manager claiming a directory is watched; the next
        // `ensure_running` retries.
        if self
            .cmd
            .send(Cmd::Watch {
                thread_id: thread_id.to_string(),
                cwd: cwd.to_path_buf(),
                generation,
            })
            .is_err()
        {
            eprintln!("pidesk: watcher command channel is closed; {thread_id} is not watched");
            return;
        }
        self.threads
            .lock()
            .insert(thread_id.to_string(), (cwd.to_path_buf(), generation));
    }

    pub fn unwatch(&self, thread_id: &str) {
        let generation = self
            .next_generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.threads.lock().remove(thread_id);
        let _ = self.cmd.send(Cmd::Unwatch {
            thread_id: thread_id.to_string(),
            generation,
        });
    }

    #[cfg(test)]
    fn is_watching(&self, thread_id: &str) -> bool {
        self.threads.lock().contains_key(thread_id)
    }
}

struct WatchedDir {
    _watcher: notify::RecommendedWatcher,
    _git_watcher: Option<notify::RecommendedWatcher>,
    threads: Vec<String>,
}

/// A registration that could not be installed must not stay in the membership
/// map: the manager would claim a directory is watched while no change event
/// can ever arrive. Clearing it also lets the next `watch()` (a newer
/// generation) retry the install.
fn forget_failed_watch(
    threads: &Mutex<HashMap<String, (PathBuf, u64)>>,
    thread_id: &str,
    cwd: &Path,
    generation: u64,
) {
    threads.lock().retain(|id, (current, latest)| {
        !(id == thread_id && current == cwd && *latest == generation)
    });
    eprintln!(
        "pidesk: could not watch {}; change notifications are unavailable for {thread_id}",
        cwd.display()
    );
}

fn watcher_thread(
    rx: std::sync::mpsc::Receiver<Cmd>,
    threads: Arc<Mutex<HashMap<String, (PathBuf, u64)>>>,
    app: AppHandle,
) {
    let mut watchers: HashMap<PathBuf, WatchedDir> = HashMap::new();
    let mut applied: HashMap<String, u64> = HashMap::new();
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Watch {
                thread_id,
                cwd,
                generation,
            } => {
                if applied
                    .get(&thread_id)
                    .is_some_and(|current| *current >= generation)
                {
                    continue;
                }
                applied.insert(thread_id.clone(), generation);
                if !threads
                    .lock()
                    .get(&thread_id)
                    .is_some_and(|(current, latest)| *current == cwd && *latest == generation)
                {
                    continue;
                }
                for (_, watched) in watchers.iter_mut() {
                    watched.threads.retain(|t| t != &thread_id);
                }
                watchers.retain(|_, watched| !watched.threads.is_empty());
                if let Some(watched) = watchers.get_mut(&cwd) {
                    watched.threads.push(thread_id);
                    continue;
                }
                if watchers.len() >= MAX_WATCHED_DIRS {
                    forget_failed_watch(&threads, &thread_id, &cwd, generation);
                    continue;
                }
                let (tx, event_rx) = sync_channel::<()>(EVENT_CAPACITY);
                let watched = cwd.clone();
                let event_tx = tx.clone();
                let watcher =
                    notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
                        let Ok(event) = res else { return };
                        if !matches!(
                            event.kind,
                            EventKind::Create(_)
                                | EventKind::Modify(_)
                                | EventKind::Remove(_)
                                | EventKind::Any
                        ) {
                            return;
                        }
                        if event_paths_relevant(&event.paths, &watched) {
                            let _ = event_tx.try_send(());
                        }
                    });
                let mut watcher = match watcher {
                    Ok(watcher) => watcher,
                    Err(_) => {
                        forget_failed_watch(&threads, &thread_id, &cwd, generation);
                        continue;
                    }
                };
                if watcher.watch(&cwd, RecursiveMode::Recursive).is_err() {
                    forget_failed_watch(&threads, &thread_id, &cwd, generation);
                    continue;
                }
                let git_watcher = worktree_git_watcher(&cwd, tx.clone());
                watchers.insert(
                    cwd.clone(),
                    WatchedDir {
                        _watcher: watcher,
                        _git_watcher: git_watcher,
                        threads: vec![thread_id],
                    },
                );
                let app = app.clone();
                let threads = threads.clone();
                let dir = cwd.clone();
                std::thread::spawn(move || {
                    while event_rx.recv().is_ok() {
                        let deadline = std::time::Instant::now() + MAX_DEBOUNCE_WAIT;
                        loop {
                            let wait = DEBOUNCE
                                .min(deadline.saturating_duration_since(std::time::Instant::now()));
                            if wait.is_zero() {
                                break;
                            }
                            match event_rx.recv_timeout(wait) {
                                Ok(()) => continue,
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                            }
                        }
                        let interested: Vec<String> = {
                            let map = threads.lock();
                            map.iter()
                                .filter(|(_, (cwd, _))| *cwd == dir)
                                .map(|(thread_id, _)| thread_id.clone())
                                .collect()
                        };
                        for thread_id in interested {
                            let _ =
                                app.emit("desktop-event", BackendEvent::GitChanged { thread_id });
                        }
                    }
                });
            }
            Cmd::Unwatch {
                thread_id,
                generation,
            } => {
                if applied
                    .get(&thread_id)
                    .is_some_and(|current| *current >= generation)
                {
                    continue;
                }
                applied.insert(thread_id.clone(), generation);
                for (_, watched) in watchers.iter_mut() {
                    watched.threads.retain(|t| t != &thread_id);
                }
                watchers.retain(|_, watched| !watched.threads.is_empty());
            }
        }
    }
}

fn worktree_git_watcher(cwd: &Path, tx: SyncSender<()>) -> Option<notify::RecommendedWatcher> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let git_dir = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
    if !git_dir.is_dir() || git_dir.starts_with(cwd) {
        return None;
    }
    let watched = git_dir.clone();
    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        let Ok(event) = res else { return };
        if !matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
        ) {
            return;
        }
        if git_metadata_relevant(&event.paths, &watched) {
            let _ = tx.try_send(());
        }
    })
    .ok()?;
    watcher.watch(&git_dir, RecursiveMode::Recursive).ok()?;
    Some(watcher)
}

fn git_metadata_relevant(paths: &[PathBuf], git_dir: &Path) -> bool {
    paths.iter().any(|path| {
        let Ok(rel) = path.strip_prefix(git_dir) else {
            return false;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        matches!(
            rel.as_str(),
            "index" | "HEAD" | "packed-refs" | "COMMIT_EDITMSG" | "MERGE_HEAD"
        ) || rel.starts_with("refs/")
            || rel.starts_with("logs/")
    })
}

/// Filter out noise: everything under `.git/` except the files that signal
/// real state changes (index, HEAD, refs, packed-refs, COMMIT_EDITMSG).
fn event_paths_relevant(paths: &[PathBuf], cwd: &Path) -> bool {
    paths.iter().any(|p| {
        let Ok(rel) = p.strip_prefix(cwd) else {
            return true;
        };
        let mut comps = rel.components();
        if let Some(first) = comps.next() {
            if first.as_os_str() == ".git" {
                let rest = comps.as_path().to_string_lossy().replace('\\', "/");
                return rest == "index"
                    || rest == "HEAD"
                    || rest == "packed-refs"
                    || rest == "COMMIT_EDITMSG"
                    || rest == "MERGE_HEAD"
                    || rest.starts_with("refs/")
                    || rest.starts_with("logs/");
            }
        }
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git;

    #[test]
    fn unwatch_removes_failed_start_membership() {
        let (tx, _rx) = sync_channel(COMMAND_CAPACITY);
        let threads = Arc::new(Mutex::new(HashMap::new()));
        let manager = WatcherManager {
            cmd: tx,
            threads: threads.clone(),
            next_generation: std::sync::atomic::AtomicU64::new(1),
        };
        manager.watch("failed", Path::new("/tmp"));
        assert!(manager.is_watching("failed"));
        manager.unwatch("failed");
        assert!(!manager.is_watching("failed"));
    }

    /// A watch request that never reached the watcher thread must not be
    /// published as an installed watch.
    #[test]
    fn failed_command_send_does_not_publish_membership() {
        let (tx, rx) = sync_channel(COMMAND_CAPACITY);
        drop(rx);
        let manager = WatcherManager {
            cmd: tx,
            threads: Arc::new(Mutex::new(HashMap::new())),
            next_generation: std::sync::atomic::AtomicU64::new(1),
        };
        manager.watch("closed", Path::new("/tmp"));
        assert!(!manager.is_watching("closed"));
    }

    #[test]
    fn replacement_generation_wins_over_stale_unwatch() {
        let (tx, rx) = sync_channel(COMMAND_CAPACITY);
        let threads = Arc::new(Mutex::new(HashMap::new()));
        let manager = WatcherManager {
            cmd: tx,
            threads,
            next_generation: std::sync::atomic::AtomicU64::new(10),
        };
        manager.watch("thread", Path::new("/tmp/new"));
        manager.unwatch("thread");
        manager.watch("thread", Path::new("/tmp/replacement"));
        let commands: Vec<Cmd> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let mut applied = HashMap::new();
        for command in commands {
            match command {
                Cmd::Watch {
                    thread_id,
                    generation,
                    ..
                }
                | Cmd::Unwatch {
                    thread_id,
                    generation,
                } => {
                    if applied
                        .get(&thread_id)
                        .is_some_and(|current| *current > generation)
                    {
                        continue;
                    }
                    applied.insert(thread_id, generation);
                }
            }
        }
        assert_eq!(applied.get("thread"), Some(&12));
        assert_eq!(
            manager.threads.lock().get("thread").unwrap().0,
            Path::new("/tmp/replacement")
        );
    }
    use std::process::Command;

    #[test]
    fn staging_in_linked_worktree_emits_metadata_change() {
        let root = std::env::temp_dir().join(format!("pidesk-watch-{}", uuid::Uuid::new_v4()));
        let source = root.join("source");
        let worktree = root.join("worktree");
        std::fs::create_dir_all(&source).unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success());
        std::fs::write(source.join("tracked.txt"), "base\n").unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["add", "."])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .arg("-C")
            .arg(&source)
            .args([
                "-c",
                "user.name=QA",
                "-c",
                "user.email=qa@localhost",
                "commit",
                "-qm",
                "Base"
            ])
            .status()
            .unwrap()
            .success());
        git::create_worktree(&source, &worktree).unwrap();
        let (tx, rx) = sync_channel(EVENT_CAPACITY);
        let watcher = worktree_git_watcher(&worktree, tx)
            .expect("linked worktree Git directory is watchable");
        std::fs::write(worktree.join("tracked.txt"), "staged\n").unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&worktree)
            .args(["add", "tracked.txt"])
            .status()
            .unwrap()
            .success());
        let result = rx.recv_timeout(std::time::Duration::from_secs(6));
        drop(watcher);
        assert!(Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["worktree", "remove", "--force"])
            .arg(&worktree)
            .status()
            .unwrap()
            .success());
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            result.is_ok(),
            "staging a worktree file must invalidate Git review"
        );
    }
}
