use crate::model::{Job, PendingUpdate};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

pub type DbResult<T> = Result<T, String>;

fn err(e: rusqlite::Error) -> String {
    format!("database error: {e}")
}

impl Db {
    pub fn open(path: &Path) -> DbResult<Self> {
        let conn = Connection::open(path).map_err(err)?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(err)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS jobs (
                 id INTEGER PRIMARY KEY,
                 task TEXT NOT NULL,
                 workstream TEXT NOT NULL,
                 start TEXT NOT NULL,
                 end TEXT NOT NULL,
                 status TEXT NOT NULL,
                 depends_on TEXT,
                 owner TEXT,
                 plan_section TEXT
             );
             CREATE TABLE IF NOT EXISTS snapshot_meta (
                 key TEXT PRIMARY KEY,
                 value TEXT
             );
             CREATE TABLE IF NOT EXISTS updates (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 job_id INTEGER NOT NULL,
                 status TEXT,
                 note TEXT,
                 requested_by TEXT NOT NULL,
                 created_at TEXT NOT NULL,
                 applied_at TEXT,
                 result TEXT
             );",
        )
        .map_err(err)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn replace_jobs(&self, jobs: &[Job], notes: Option<&str>, taken_at: &str) -> DbResult<usize> {
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(err)?;
        tx.execute("DELETE FROM jobs", []).map_err(err)?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO jobs (id, task, workstream, start, end, status, depends_on, owner, plan_section)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )
                .map_err(err)?;
            for j in jobs {
                stmt.execute(params![
                    j.id,
                    j.task,
                    j.workstream,
                    j.start.to_string(),
                    j.end.to_string(),
                    j.status,
                    j.depends_on,
                    j.owner,
                    j.plan_section
                ])
                .map_err(err)?;
            }
        }
        tx.execute(
            "INSERT INTO snapshot_meta (key, value) VALUES ('taken_at', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![taken_at],
        )
        .map_err(err)?;
        tx.execute(
            "INSERT INTO snapshot_meta (key, value) VALUES ('notes', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![notes],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(jobs.len())
    }

    pub fn jobs(&self) -> DbResult<Vec<Job>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, task, workstream, start, end, status, depends_on, owner, plan_section
                 FROM jobs ORDER BY start, id",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| {
                let start: String = r.get(3)?;
                let end: String = r.get(4)?;
                Ok(Job {
                    id: r.get(0)?,
                    task: r.get(1)?,
                    workstream: r.get(2)?,
                    start: start.parse().unwrap_or_default(),
                    end: end.parse().unwrap_or_default(),
                    status: r.get(5)?,
                    depends_on: r.get(6)?,
                    owner: r.get(7)?,
                    plan_section: r.get(8)?,
                })
            })
            .map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }

    pub fn job_exists(&self, id: i64) -> DbResult<bool> {
        let conn = self.lock();
        conn.query_row("SELECT 1 FROM jobs WHERE id = ?1", params![id], |_| Ok(()))
            .optional()
            .map(|o| o.is_some())
            .map_err(err)
    }

    pub fn meta(&self, key: &str) -> DbResult<Option<String>> {
        let conn = self.lock();
        conn.query_row("SELECT value FROM snapshot_meta WHERE key = ?1", params![key], |r| r.get(0))
            .optional()
            .map(|o| o.flatten())
            .map_err(err)
    }

    pub fn queue_update(
        &self,
        job_id: i64,
        status: Option<&str>,
        note: Option<&str>,
        requested_by: &str,
        created_at: &str,
    ) -> DbResult<i64> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO updates (job_id, status, note, requested_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![job_id, status, note, requested_by, created_at],
        )
        .map_err(err)?;
        Ok(conn.last_insert_rowid())
    }

    pub fn pending_updates(&self) -> DbResult<Vec<PendingUpdate>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, job_id, status, note, requested_by, created_at
                 FROM updates WHERE applied_at IS NULL ORDER BY id",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(PendingUpdate {
                    id: r.get(0)?,
                    job_id: r.get(1)?,
                    status: r.get(2)?,
                    note: r.get(3)?,
                    requested_by: r.get(4)?,
                    created_at: r.get(5)?,
                })
            })
            .map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }

    pub fn mark_applied(&self, ids: &[i64], result: Option<&str>, applied_at: &str) -> DbResult<usize> {
        let conn = self.lock();
        let mut n = 0;
        for id in ids {
            n += conn
                .execute(
                    "UPDATE updates SET applied_at = ?1, result = ?2 WHERE id = ?3 AND applied_at IS NULL",
                    params![applied_at, result, id],
                )
                .map_err(err)?;
        }
        Ok(n)
    }
}
