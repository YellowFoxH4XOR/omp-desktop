use crate::harness::HarnessRegistry;
use crate::store::Store;
use crate::terminal::TerminalManager;
use crate::threads::ThreadManager;
use crate::watcher::WatcherManager;
use std::sync::Arc;
use tauri::AppHandle;

/// Shared application state managed by Tauri.
pub struct AppState {
    pub store: Arc<Store>,
    pub registry: Arc<HarnessRegistry>,
    #[allow(dead_code)] // held for lifetime; ThreadManager drives it
    pub watcher: Arc<WatcherManager>,
    pub threads: Arc<ThreadManager>,
    pub terminals: Arc<TerminalManager>,
    pub intern: Arc<crate::intern::InternManager>,
    pub app: AppHandle,
}

impl AppState {
    pub fn new(app: AppHandle, store: Arc<Store>) -> Self {
        let registry = Arc::new(HarnessRegistry::new());
        let watcher = Arc::new(WatcherManager::new(app.clone()));
        let threads = ThreadManager::new(
            store.clone(),
            registry.clone(),
            watcher.clone(),
            app.clone(),
        );
        Self {
            store,
            registry,
            watcher,
            threads,
            terminals: TerminalManager::new(),
            intern: crate::intern::InternManager::new(),
            app,
        }
    }
}
