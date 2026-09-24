use crate::dto::BackendEvent;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

/// Watches thread working directories and emits `git_changed` when files or
/// git metadata change. Event-driven only — no polling.
///
/// `notify::RecommendedWatcher` is `Send` but not `Sync`, so all watchers live
/// on one dedicated thread driven by a command channel; the manager itself is
/// a cheap `Send + Sync` handle.
pub struct WatcherManager {
    cmd: Sender<Cmd>,
    /// thread_id → cwd (kept here for fast membership checks)
    threads: Arc<Mutex<HashMap<String, PathBuf>>>,
}

enum Cmd {
    Watch { thread_id: String, cwd: PathBuf },
    Unwatch { thread_id: String },
}

const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);

impl WatcherManager {
    pub fn new(app: AppHandle) -> Self {
        let (cmd_tx, cmd_rx) = channel::<Cmd>();
        let threads = Arc::new(Mutex::new(HashMap::<String, PathBuf>::new()));
        let threads2 = threads.clone();
        std::thread::spawn(move || watcher_thread(cmd_rx, threads2, app));
        Self {
            cmd: cmd_tx,
            threads,
        }
    }

    /// Start watching `cwd` for `thread_id` (idempotent).
    pub fn watch(&self, thread_id: &str, cwd: &Path) {
        let cwd = cwd.to_path_buf();
        {
            let mut threads = self.threads.lock();
            if threads.get(thread_id) == Some(&cwd) {
                return;
            }
            threads.insert(thread_id.to_string(), cwd.clone());
        }
        let _ = self.cmd.send(Cmd::Watch {
            thread_id: thread_id.to_string(),
            cwd,
        });
    }

    pub fn unwatch(&self, thread_id: &str) {
        self.threads.lock().remove(thread_id);
        let _ = self.cmd.send(Cmd::Unwatch {
            thread_id: thread_id.to_string(),
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

fn watcher_thread(
    rx: std::sync::mpsc::Receiver<Cmd>,
    threads: Arc<Mutex<HashMap<String, PathBuf>>>,
    app: AppHandle,
) {
    let mut watchers: HashMap<PathBuf, WatchedDir> = HashMap::new();
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Watch { thread_id, cwd } => {
                // Remove from any previous directory first.
                for (_, wd) in watchers.iter_mut() {
                    wd.threads.retain(|t| t != &thread_id);
                }
                watchers.retain(|_, wd| !wd.threads.is_empty());
                if watchers.contains_key(&cwd) {
                    watchers
                        .get_mut(&cwd)
                        .expect("checked")
                        .threads
                        .push(thread_id);
                    continue;
                }
                let (tx, event_rx) = channel::<()>();
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
                        if !event_paths_relevant(&event.paths, &watched) {
                            return;
                        }
                        let _ = event_tx.send(());
                    });
                let mut watcher = match watcher {
                    Ok(w) => w,
                    Err(_) => continue,
                };
                if watcher.watch(&cwd, RecursiveMode::Recursive).is_err() {
                    continue;
                }
                // A linked worktree has a `.git` pointer file; its index and
                // HEAD live outside `cwd`. Watch that Git directory separately.
                let git_watcher = worktree_git_watcher(&cwd, tx.clone());
                watchers.insert(
                    cwd.clone(),
                    WatchedDir {
                        _watcher: watcher,
                        _git_watcher: git_watcher,
                        threads: vec![thread_id],
                    },
                );
                // Fan-out thread: debounce bursts, emit per interested thread.
                let app = app.clone();
                let threads = threads.clone();
                let dir = cwd.clone();
                std::thread::spawn(move || {
                    while event_rx.recv().is_ok() {
                        loop {
                            match event_rx.recv_timeout(DEBOUNCE) {
                                Ok(()) => continue,
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                            }
                        }
                        let interested: Vec<String> = {
                            let map = threads.lock();
                            map.iter()
                                .filter(|(_, c)| **c == dir)
                                .map(|(t, _)| t.clone())
                                .collect()
                        };
                        if interested.is_empty() {
                            return;
                        }
                        for tid in interested {
                            let _ = app
                                .emit("desktop-event", BackendEvent::GitChanged { thread_id: tid });
                        }
                    }
                });
            }
            Cmd::Unwatch { thread_id } => {
                for (_, wd) in watchers.iter_mut() {
                    wd.threads.retain(|t| t != &thread_id);
                }
                watchers.retain(|_, wd| !wd.threads.is_empty());
            }
        }
    }
}

fn worktree_git_watcher(cwd: &Path, tx: Sender<()>) -> Option<notify::RecommendedWatcher> {
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
            let _ = tx.send(());
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
        let (tx, _rx) = channel();
        let threads = Arc::new(Mutex::new(HashMap::new()));
        let manager = WatcherManager {
            cmd: tx,
            threads: threads.clone(),
        };
        manager.watch("failed", Path::new("/tmp"));
        assert!(manager.is_watching("failed"));
        manager.unwatch("failed");
        assert!(!manager.is_watching("failed"));
    }
    use std::process::Command;

    #[test]
    fn staging_in_linked_worktree_emits_metadata_change() {
        let root = std::env::temp_dir().join(format!("omp-watch-{}", uuid::Uuid::new_v4()));
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
        let (tx, rx) = channel();
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
