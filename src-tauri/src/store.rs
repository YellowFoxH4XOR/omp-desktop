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
               runtime TEXT NOT NULL DEFAULT 'external',
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
             );
             CREATE TABLE IF NOT EXISTS hidden_projects (
               project_id TEXT PRIMARY KEY
             );
             UPDATE projects SET preferred_harness = 'pi' WHERE preferred_harness != 'pi';",
        )?;
        let columns = conn
            .prepare("PRAGMA table_info(threads)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|column| column == "runtime") {
            // Old Pi threads belong to the external installation, even if a
            // session path happens to resemble the new private directory.
            conn.execute(
                "ALTER TABLE threads ADD COLUMN runtime TEXT NOT NULL DEFAULT 'external'",
                [],
            )?;
        }
        Ok(())
    }

    // ---- projects ----

    pub fn list_projects(&self) -> AppResult<Vec<Project>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, path, display_name, preferred_harness, is_git, created_at, last_opened_at
             FROM projects
             WHERE id NOT IN (SELECT project_id FROM hidden_projects)
             ORDER BY last_opened_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Project {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    display_name: r.get(2)?,
                    preferred_harness: r.get::<_, String>(3)?.parse().unwrap_or(HarnessKind::Pi),
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
            conn.execute(
                "DELETE FROM hidden_projects WHERE project_id = ?1",
                params![existing.0],
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
             FROM projects
             WHERE id = ?1 AND id NOT IN (SELECT project_id FROM hidden_projects)",
            params![id],
            |r| {
                Ok(Project {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    display_name: r.get(2)?,
                    preferred_harness: r.get::<_, String>(3)?.parse().unwrap_or(HarnessKind::Pi),
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
        let mut conn = self.conn.lock();
        let transaction = conn.transaction()?;
        transaction.execute(
            "DELETE FROM threads WHERE project_id = ?1 AND harness = 'pi' AND runtime = 'managed'",
            params![id],
        )?;
        let has_legacy_threads: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM threads WHERE project_id = ?1)",
            params![id],
            |row| row.get(0),
        )?;
        if has_legacy_threads {
            transaction.execute(
                "INSERT OR IGNORE INTO hidden_projects(project_id) VALUES(?1)",
                params![id],
            )?;
        } else {
            transaction.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
            transaction.execute(
                "DELETE FROM hidden_projects WHERE project_id = ?1",
                params![id],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    // ---- threads ----

    fn row_to_thread(r: &rusqlite::Row<'_>) -> rusqlite::Result<ThreadRow> {
        Ok(ThreadRow {
            id: r.get(0)?,
            project_id: r.get(1)?,
            harness: HarnessKind::Pi,
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

    /// Include unsupported legacy mappings so discovery never imports their
    /// schema-compatible session headers as new Pi threads.
    pub fn registered_session_files(&self, project_id: &str) -> AppResult<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT session_file FROM threads WHERE project_id = ?1 AND session_file != ''",
        )?;
        let files = stmt
            .query_map(params![project_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(files)
    }

    pub fn list_threads(&self, project_id: &str) -> AppResult<Vec<ThreadRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM threads WHERE project_id = ?1 AND harness = 'pi' AND runtime = 'managed'
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
            &format!(
                "SELECT {} FROM threads WHERE id = ?1 AND harness = 'pi' AND runtime = 'managed'",
                Self::THREAD_COLS
            ),
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
            "INSERT INTO threads(id, project_id, harness, session_id, session_file, cwd, title, status, created_at, last_viewed_at, worktree_path, runtime)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'managed')
             ON CONFLICT(id) DO UPDATE SET
               session_id = CASE WHEN excluded.session_id != '' THEN excluded.session_id ELSE threads.session_id END,
               session_file = CASE WHEN excluded.session_file != '' THEN excluded.session_file ELSE threads.session_file END,
               cwd = excluded.cwd,
               title = CASE WHEN excluded.title != '' THEN excluded.title ELSE threads.title END,
               status = excluded.status,
               worktree_path = COALESCE(excluded.worktree_path, threads.worktree_path)
             WHERE threads.harness = 'pi' AND threads.runtime = 'managed'", 
            params![id, project_id, harness.as_str(), session_id, session_file, cwd, title, status, created, now, worktree_path],
        )?;
        drop(conn);
        self.get_thread(id)
    }

    pub fn update_thread_status(&self, id: &str, status: &str) -> AppResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE threads SET status = ?2 WHERE id = ?1 AND harness = 'pi' AND runtime = 'managed'",
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
             WHERE id = ?1 AND harness = 'pi' AND runtime = 'managed'",
            params![id, session_id, session_file],
        )?;
        Ok(())
    }

    pub fn rename_thread(&self, id: &str, title: &str) -> AppResult<ThreadRow> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE threads SET title = ?2 WHERE id = ?1 AND harness = 'pi' AND runtime = 'managed'",
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
                "UPDATE threads SET pinned = ?2 WHERE id = ?1 AND harness = 'pi' AND runtime = 'managed'",
                params![id, p as i64],
            )?;
        }
        if let Some(a) = archived {
            conn.execute(
                "UPDATE threads SET archived = ?2 WHERE id = ?1 AND harness = 'pi' AND runtime = 'managed'",
                params![id, a as i64],
            )?;
        }
        drop(conn);
        self.get_thread(id)
    }

    pub fn touch_thread(&self, id: &str) -> AppResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE threads SET last_viewed_at = ?2 WHERE id = ?1 AND harness = 'pi' AND runtime = 'managed'",
            params![id, now_iso()],
        )?;
        Ok(())
    }

    /// Mark interrupted runs disconnected. Idle and completed threads have no
    /// live process by design and must not look like crashes.
    pub fn mark_all_threads_disconnected(&self) -> AppResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE threads SET status = 'disconnected'
             WHERE harness = 'pi' AND runtime = 'managed' AND status IN ('active', 'waiting')",
            [],
        )?;
        Ok(())
    }
}

/// Path of the SQLite database inside the app data dir.
pub fn db_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("pidesk.sqlite3")
}

/// Prefer πDesk's database. Before it exists, open the prior app's database
/// in place so its SQLite WAL remains authoritative; no live database files
/// are copied. Once πDesk has created its own database, it wins.
pub fn database_path(app_data_dir: &Path) -> PathBuf {
    let current = db_path(app_data_dir);
    if current.exists() {
        return current;
    }
    let legacy = app_data_dir
        .parent()
        .unwrap_or(app_data_dir)
        .join("dev.ompui.desktop")
        .join("omp-desktop.sqlite3");
    if legacy.exists() {
        legacy
    } else {
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_and_threads_survive_restart_without_owning_files() {
        let dir = std::env::temp_dir().join(format!("pidesk-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.txt");
        std::fs::write(&source, "keep me").unwrap();
        let db = db_path(&dir);
        let thread_id = uuid::Uuid::new_v4().to_string();
        let project_id;
        {
            let store = Store::open(&db).unwrap();
            let project = store
                .add_project(&dir.to_string_lossy(), "Fixture", HarnessKind::Pi, true)
                .unwrap();
            project_id = project.id;
            store
                .upsert_thread(
                    &thread_id,
                    &project_id,
                    HarnessKind::Pi,
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
        let dir = std::env::temp_dir().join(format!("pidesk-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let project = store
            .add_project(&dir.to_string_lossy(), "Fixture", HarnessKind::Pi, false)
            .unwrap();
        for (index, status) in ["idle", "completed", "active", "waiting"]
            .iter()
            .enumerate()
        {
            store
                .upsert_thread(
                    &format!("thread-{index}"),
                    &project.id,
                    HarnessKind::Pi,
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
    fn legacy_mixed_database_exposes_only_pi_threads() {
        let dir = std::env::temp_dir().join(format!("pidesk-mixed-db-{}", uuid::Uuid::new_v4()));
        let db = dir.join("metadata.sqlite3");
        let project_id;
        {
            let store = Store::open(&db).unwrap();
            let project = store
                .add_project("/tmp/project", "Fixture", HarnessKind::Pi, false)
                .unwrap();
            project_id = project.id;
            store
                .upsert_thread(
                    "pi-thread",
                    &project_id,
                    HarnessKind::Pi,
                    "pi-session",
                    "/tmp/pi.jsonl",
                    "/tmp/project",
                    "Pi",
                    "active",
                    None,
                    None,
                )
                .unwrap();
            let conn = store.conn.lock();
            conn.execute(
                "INSERT INTO threads(id, project_id, harness, session_file, cwd, title, status, created_at, last_viewed_at)
                 VALUES('omp-thread', ?1, 'omp', '/tmp/omp.jsonl', '/tmp/project', 'OMP', 'active', 'now', 'now')",
                params![project_id],
            )
            .unwrap();
            conn.execute(
                "UPDATE projects SET preferred_harness = 'omp' WHERE id = ?1",
                params![project_id],
            )
            .unwrap();
        }

        // Reopening runs the Pi-only migration against a realistic mixed
        // legacy database.
        let store = Store::open(&db).unwrap();
        assert_eq!(store.list_threads(&project_id).unwrap().len(), 1);
        assert!(store.get_thread("omp-thread").is_err());
        let registered = store.registered_session_files(&project_id).unwrap();
        assert!(registered.contains(&"/tmp/omp.jsonl".to_string()));
        assert!(registered.contains(&"/tmp/pi.jsonl".to_string()));
        // The global uniqueness constraint also prevents a legacy file from
        // being remapped under a new Pi thread id.
        assert!(store
            .upsert_thread(
                "reimported-legacy",
                &project_id,
                HarnessKind::Pi,
                "legacy",
                "/tmp/omp.jsonl",
                "/tmp/project",
                "Wrong",
                "idle",
                None,
                None,
            )
            .is_err());
        assert!(store
            .upsert_thread(
                "omp-thread",
                &project_id,
                HarnessKind::Pi,
                "wrong",
                "/tmp/wrong.jsonl",
                "/tmp/project",
                "Wrong",
                "idle",
                None,
                None,
            )
            .is_err());
        store.mark_all_threads_disconnected().unwrap();
        let conn = store.conn.lock();
        let (harness, status): (String, String) = conn
            .query_row(
                "SELECT harness, status FROM threads WHERE id = 'omp-thread'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((harness.as_str(), status.as_str()), ("omp", "active"));
        drop(conn);
        assert_eq!(
            store.get_project(&project_id).unwrap().preferred_harness,
            HarnessKind::Pi
        );

        store.remove_project(&project_id).unwrap();
        assert!(store.get_project(&project_id).is_err());
        assert!(store.list_projects().unwrap().is_empty());
        let conn = store.conn.lock();
        let remaining: Vec<String> = conn
            .prepare("SELECT id FROM threads WHERE project_id = ?1 ORDER BY id")
            .unwrap()
            .query_map(params![project_id], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(remaining, vec!["omp-thread"]);
        drop(conn);

        let restored = store
            .add_project("/tmp/project", "Fixture", HarnessKind::Pi, false)
            .unwrap();
        assert_eq!(restored.id, project_id);
        assert_eq!(store.list_projects().unwrap().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_hides_external_pi_without_deleting_projects_or_threads() {
        let dir =
            std::env::temp_dir().join(format!("pidesk-runtime-migration-{}", uuid::Uuid::new_v4()));
        let db = db_path(&dir);
        let project_id;
        {
            let store = Store::open(&db).unwrap();
            let project = store
                .add_project("/tmp/project", "Preserved", HarnessKind::Pi, false)
                .unwrap();
            project_id = project.id;
            store
                .upsert_thread(
                    "external",
                    &project_id,
                    HarnessKind::Pi,
                    "old",
                    "/tmp/external.jsonl",
                    "/tmp/project",
                    "Old thread",
                    "active",
                    None,
                    None,
                )
                .unwrap();
            store
                .set_thread_flags("external", Some(true), None)
                .unwrap();
            // Reproduce the schema from before private runtimes existed.
            store
                .conn
                .lock()
                .execute("ALTER TABLE threads DROP COLUMN runtime", [])
                .unwrap();
        }
        let store = Store::open(&db).unwrap();
        assert_eq!(store.list_projects().unwrap()[0].display_name, "Preserved");
        assert!(store.list_threads(&project_id).unwrap().is_empty());
        assert!(store.get_thread("external").is_err());
        assert!(store.rename_thread("external", "Changed").is_err());
        assert!(store
            .upsert_thread(
                "external",
                &project_id,
                HarnessKind::Pi,
                "new",
                "",
                "/tmp/project",
                "Wrong",
                "idle",
                None,
                None
            )
            .is_err());
        store.update_thread_status("external", "completed").unwrap();
        store.mark_all_threads_disconnected().unwrap();
        let before: (String, String, i64) = store
            .conn
            .lock()
            .query_row(
                "SELECT title, status, pinned FROM threads WHERE id='external'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(before, ("Old thread".into(), "active".into(), 1));
        store
            .upsert_thread(
                "private",
                &project_id,
                HarnessKind::Pi,
                "",
                "",
                "/tmp/project",
                "New thread",
                "idle",
                None,
                None,
            )
            .unwrap();
        assert_eq!(store.list_threads(&project_id).unwrap()[0].id, "private");
        store.remove_project(&project_id).unwrap();
        let remaining: i64 = store
            .conn
            .lock()
            .query_row(
                "SELECT count(*) FROM threads WHERE id='external'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 1);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn database_path_uses_legacy_database_only_until_current_exists() {
        let root = std::env::temp_dir().join(format!("pidesk-db-path-{}", uuid::Uuid::new_v4()));
        let current_dir = root.join("dev.pidesk.desktop");
        let legacy = root.join("dev.ompui.desktop").join("omp-desktop.sqlite3");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, b"legacy").unwrap();
        assert_eq!(database_path(&current_dir), legacy);
        std::fs::create_dir_all(&current_dir).unwrap();
        let current = db_path(&current_dir);
        std::fs::write(&current, b"current").unwrap();
        assert_eq!(database_path(&current_dir), current);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn omitted_state_fields_do_not_erase_session_mapping() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let dir = std::env::temp_dir().join(format!("pidesk-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let project = store
            .add_project(&dir.to_string_lossy(), "Fixture", HarnessKind::Pi, false)
            .unwrap();
        store
            .upsert_thread(
                "thread",
                &project.id,
                HarnessKind::Pi,
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
