use crate::dto::{HarnessKind, Project, Thread};
use crate::error::{AppError, AppResult};
use crate::util::now_iso;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

/// Persistent app-owned metadata: projects, threads, settings.
/// Harness transcripts stay in the harness's own session files; we only keep
/// UI metadata (titles, pins, archive flags, harness/session mapping).
pub struct Store {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone)]
pub struct ThreadRow {
    pub id: String,
    pub project_id: String,
    pub harness: HarnessKind,
    pub session_id: String,
    pub session_file: String,
    pub cwd: String,
    pub title: String,
    pub pinned: bool,
    pub archived: bool,
    pub status: String,
    pub created_at: String,
    pub last_viewed_at: String,
    pub worktree_path: Option<String>,
}

impl ThreadRow {
    pub fn into_dto(self) -> Thread {
        Thread {
            id: self.id,
            project_id: self.project_id,
            harness: self.harness,
            session_id: self.session_id,
            session_file: self.session_file,
            cwd: self.cwd,
            title: self.title,
            pinned: self.pinned,
            archived: self.archived,
            status: self.status,
            created_at: self.created_at,
            last_viewed_at: self.last_viewed_at,
            worktree_path: self.worktree_path,
        }
    }
}

impl Store {
    pub fn open(path: &Path) -> AppResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AppError::new(format!("Cannot create data directory: {e}")))?;
        }
        let conn = Connection::open(path)
            .map_err(|e| AppError::new(format!("Cannot open local store: {e}")))?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> AppResult<()> {
        let conn = self.conn.lock();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS projects (
               id TEXT PRIMARY KEY,
               path TEXT NOT NULL UNIQUE,
               display_name TEXT NOT NULL,
               preferred_harness TEXT NOT NULL,
               is_git INTEGER NOT NULL DEFAULT 0,
               created_at TEXT NOT NULL,
               last_opened_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS threads (
               id TEXT PRIMARY KEY,
               project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
               harness TEXT NOT NULL,
               session_id TEXT NOT NULL DEFAULT '',
               session_file TEXT NOT NULL DEFAULT '',
               cwd TEXT NOT NULL,
               title TEXT NOT NULL DEFAULT '',
               pinned INTEGER NOT NULL DEFAULT 0,
               archived INTEGER NOT NULL DEFAULT 0,
               status TEXT NOT NULL DEFAULT 'idle',
               created_at TEXT NOT NULL,
               last_viewed_at TEXT NOT NULL,
               worktree_path TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_threads_project ON threads(project_id);
             CREATE UNIQUE INDEX IF NOT EXISTS idx_threads_session_file ON threads(session_file) WHERE session_file != '';
             CREATE TABLE IF NOT EXISTS settings (
               key TEXT PRIMARY KEY,
               value TEXT NOT NULL
             );",
        )?;
        Ok(())
    }

    // ---- settings ----

    /// Checked read: only "no such row" maps to `Ok(None)`; lock/IO and
    /// corruption errors propagate to the caller. Callers where a missing
    /// value changes what runs or what the user sees as configured MUST use
    /// this rather than swallowing errors.
    pub fn get_setting_checked(&self, key: &str) -> AppResult<Option<String>> {
        let conn = self.conn.lock();
        match conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        ) {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(other) => Err(other.into()),
        }
    }

    pub fn set_setting(&self, key: &str, value: &str) -> AppResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ---- projects ----

    pub fn list_projects(&self) -> AppResult<Vec<Project>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, path, display_name, preferred_harness, is_git, created_at, last_opened_at
             FROM projects ORDER BY last_opened_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Project {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    display_name: r.get(2)?,
                    preferred_harness: r.get::<_, String>(3)?.parse().unwrap_or(HarnessKind::Omp),
                    is_git: r.get::<_, i64>(4)? != 0,
                    created_at: r.get(5)?,
                    last_opened_at: r.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn add_project(
        &self,
        path: &str,
        display_name: &str,
        harness: HarnessKind,
        is_git: bool,
    ) -> AppResult<Project> {
        let conn = self.conn.lock();
        let now = now_iso();
        // Re-adding an existing path refreshes metadata instead of duplicating.
        if let Ok(existing) = conn.query_row(
            "SELECT id, created_at FROM projects WHERE path = ?1",
            params![path],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        ) {
            conn.execute(
                "UPDATE projects SET display_name = ?2, preferred_harness = ?3, is_git = ?4, last_opened_at = ?5 WHERE id = ?1",
                params![existing.0, display_name, harness.as_str(), is_git as i64, now],
            )?;
            return Ok(Project {
                id: existing.0,
                path: path.to_string(),
                display_name: display_name.to_string(),
                preferred_harness: harness,
                is_git,
                created_at: existing.1,
                last_opened_at: now,
            });
        }
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects(id, path, display_name, preferred_harness, is_git, created_at, last_opened_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, path, display_name, harness.as_str(), is_git as i64, now, now],
        )?;
        Ok(Project {
            id,
            path: path.to_string(),
            display_name: display_name.to_string(),
            preferred_harness: harness,
            is_git,
            created_at: now.clone(),
            last_opened_at: now,
        })
    }

    pub fn get_project(&self, id: &str) -> AppResult<Project> {
        let conn = self.conn.lock();
        let p = conn.query_row(
            "SELECT id, path, display_name, preferred_harness, is_git, created_at, last_opened_at
             FROM projects WHERE id = ?1",
            params![id],
            |r| {
                Ok(Project {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    display_name: r.get(2)?,
                    preferred_harness: r.get::<_, String>(3)?.parse().unwrap_or(HarnessKind::Omp),
                    is_git: r.get::<_, i64>(4)? != 0,
                    created_at: r.get(5)?,
                    last_opened_at: r.get(6)?,
                })
            },
        );
        match p {
            Ok(p) => Ok(p),
            Err(rusqlite::Error::QueryReturnedNoRows) => Err(AppError::new(
                "Project not found. It may have been removed.",
            )),
            Err(e) => Err(e.into()),
        }
    }

    pub fn touch_project(&self, id: &str) -> AppResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE projects SET last_opened_at = ?2 WHERE id = ?1",
            params![id, now_iso()],
        )?;
        Ok(())
    }

    pub fn remove_project(&self, id: &str) -> AppResult<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ---- threads ----

    fn row_to_thread(r: &rusqlite::Row<'_>) -> rusqlite::Result<ThreadRow> {
        Ok(ThreadRow {
            id: r.get(0)?,
            project_id: r.get(1)?,
            harness: r.get::<_, String>(2)?.parse().unwrap_or(HarnessKind::Omp),
            session_id: r.get(3)?,
            session_file: r.get(4)?,
            cwd: r.get(5)?,
            title: r.get(6)?,
            pinned: r.get::<_, i64>(7)? != 0,
            archived: r.get::<_, i64>(8)? != 0,
            status: r.get(9)?,
            created_at: r.get(10)?,
            last_viewed_at: r.get(11)?,
            worktree_path: r.get(12)?,
        })
    }

    const THREAD_COLS: &'static str =
        "id, project_id, harness, session_id, session_file, cwd, title, pinned, archived, status, created_at, last_viewed_at, worktree_path";

    pub fn list_threads(&self, project_id: &str) -> AppResult<Vec<ThreadRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM threads WHERE project_id = ?1
             ORDER BY pinned DESC, archived ASC, last_viewed_at DESC",
            Self::THREAD_COLS
        ))?;
        let rows = stmt
            .query_map(params![project_id], Self::row_to_thread)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_thread(&self, id: &str) -> AppResult<ThreadRow> {
        let conn = self.conn.lock();
        let res = conn.query_row(
            &format!("SELECT {} FROM threads WHERE id = ?1", Self::THREAD_COLS),
            params![id],
            Self::row_to_thread,
        );
        match res {
            Ok(t) => Ok(t),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(AppError::new("Thread not found. It may have been removed."))
            }
            Err(e) => Err(e.into()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_thread(
        &self,
        id: &str,
        project_id: &str,
        harness: HarnessKind,
        session_id: &str,
        session_file: &str,
        cwd: &str,
        title: &str,
        status: &str,
        worktree_path: Option<&str>,
        created_at: Option<&str>,
    ) -> AppResult<ThreadRow> {
        let conn = self.conn.lock();
        let now = now_iso();
        let created = created_at.filter(|c| !c.is_empty()).unwrap_or(&now);
        conn.execute(
            "INSERT INTO threads(id, project_id, harness, session_id, session_file, cwd, title, status, created_at, last_viewed_at, worktree_path)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
               session_id = CASE WHEN excluded.session_id != '' THEN excluded.session_id ELSE threads.session_id END,
               session_file = CASE WHEN excluded.session_file != '' THEN excluded.session_file ELSE threads.session_file END,
               cwd = excluded.cwd,
               title = CASE WHEN excluded.title != '' THEN excluded.title ELSE threads.title END,
               status = excluded.status,
               worktree_path = COALESCE(excluded.worktree_path, threads.worktree_path)",
            params![id, project_id, harness.as_str(), session_id, session_file, cwd, title, status, created, now, worktree_path],
        )?;
        drop(conn);
        self.get_thread(id)
    }

    pub fn update_thread_status(&self, id: &str, status: &str) -> AppResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE threads SET status = ?2 WHERE id = ?1",
            params![id, status],
        )?;
        Ok(())
    }

    pub fn update_thread_session(
        &self,
        id: &str,
        session_id: &str,
        session_file: &str,
    ) -> AppResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE threads SET
               session_id = CASE WHEN ?2 != '' THEN ?2 ELSE threads.session_id END,
               session_file = CASE WHEN ?3 != '' THEN ?3 ELSE threads.session_file END
             WHERE id = ?1",
            params![id, session_id, session_file],
        )?;
        Ok(())
    }

    pub fn rename_thread(&self, id: &str, title: &str) -> AppResult<ThreadRow> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE threads SET title = ?2 WHERE id = ?1",
            params![id, title],
        )?;
        drop(conn);
        self.get_thread(id)
    }

    pub fn set_thread_flags(
        &self,
        id: &str,
        pinned: Option<bool>,
        archived: Option<bool>,
    ) -> AppResult<ThreadRow> {
        let conn = self.conn.lock();
        if let Some(p) = pinned {
            conn.execute(
                "UPDATE threads SET pinned = ?2 WHERE id = ?1",
                params![id, p as i64],
            )?;
        }
        if let Some(a) = archived {
            conn.execute(
                "UPDATE threads SET archived = ?2 WHERE id = ?1",
                params![id, a as i64],
            )?;
        }
        drop(conn);
        self.get_thread(id)
    }

    pub fn touch_thread(&self, id: &str) -> AppResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE threads SET last_viewed_at = ?2 WHERE id = ?1",
            params![id, now_iso()],
        )?;
        Ok(())
    }

    /// Mark interrupted runs disconnected. Idle and completed threads have no
    /// live process by design and must not look like crashes.
    pub fn mark_all_threads_disconnected(&self) -> AppResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE threads SET status = 'disconnected' WHERE status IN ('active', 'waiting')",
            [],
        )?;
        Ok(())
    }
}

/// Path of the SQLite database inside the app data dir.
pub fn db_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("omp-desktop.sqlite3")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_and_threads_survive_restart_without_owning_files() {
        let dir = std::env::temp_dir().join(format!("omp-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.txt");
        std::fs::write(&source, "keep me").unwrap();
        let db = db_path(&dir);
        let thread_id = uuid::Uuid::new_v4().to_string();
        let project_id;
        {
            let store = Store::open(&db).unwrap();
            let project = store
                .add_project(&dir.to_string_lossy(), "Fixture", HarnessKind::Omp, true)
                .unwrap();
            project_id = project.id;
            store
                .upsert_thread(
                    &thread_id,
                    &project_id,
                    HarnessKind::Omp,
                    "session-1",
                    "/tmp/session.jsonl",
                    &dir.to_string_lossy(),
                    "First task",
                    "idle",
                    None,
                    None,
                )
                .unwrap();
            store
                .set_thread_flags(&thread_id, Some(true), Some(false))
                .unwrap();
        }
        {
            let store = Store::open(&db).unwrap();
            assert_eq!(store.list_projects().unwrap()[0].id, project_id);
            let thread = store.get_thread(&thread_id).unwrap();
            assert_eq!(thread.title, "First task");
            assert!(thread.pinned);
            store.remove_project(&project_id).unwrap();
            assert!(store.get_thread(&thread_id).is_err());
        }
        assert_eq!(std::fs::read_to_string(source).unwrap(), "keep me");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn startup_marks_interrupted_threads_disconnected() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let dir = std::env::temp_dir().join(format!("omp-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let project = store
            .add_project(&dir.to_string_lossy(), "Fixture", HarnessKind::Omp, false)
            .unwrap();
        for (index, status) in ["idle", "completed", "active", "waiting"]
            .iter()
            .enumerate()
        {
            store
                .upsert_thread(
                    &format!("thread-{index}"),
                    &project.id,
                    HarnessKind::Omp,
                    "",
                    "",
                    &dir.to_string_lossy(),
                    "",
                    status,
                    None,
                    None,
                )
                .unwrap();
        }
        store.mark_all_threads_disconnected().unwrap();
        let statuses: Vec<_> = store
            .list_threads(&project.id)
            .unwrap()
            .into_iter()
            .map(|thread| thread.status)
            .collect();
        assert!(statuses.contains(&"idle".to_string()));
        assert!(statuses.contains(&"completed".to_string()));
        assert_eq!(
            statuses
                .iter()
                .filter(|status| *status == "disconnected")
                .count(),
            2
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn omitted_state_fields_do_not_erase_session_mapping() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let dir = std::env::temp_dir().join(format!("omp-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let project = store
            .add_project(&dir.to_string_lossy(), "Fixture", HarnessKind::Omp, false)
            .unwrap();
        store
            .upsert_thread(
                "thread",
                &project.id,
                HarnessKind::Omp,
                "session",
                "/tmp/session.jsonl",
                &dir.to_string_lossy(),
                "",
                "idle",
                None,
                None,
            )
            .unwrap();
        store
            .update_thread_session("thread", "session", "")
            .unwrap();
        let thread = store.get_thread("thread").unwrap();
        assert_eq!(thread.session_id, "session");
        assert_eq!(thread.session_file, "/tmp/session.jsonl");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
