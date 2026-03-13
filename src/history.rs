//! SQLite-backed execution history storage and query helpers.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;

use crate::runtime_paths::runtime_data_path_for_config;
use crate::task_runner::{TaskOutcome, TaskStepResult};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HistoryRecord {
    pub id: i64,
    pub task_id: String,
    pub status: String,
    pub summary: String,
    pub exit_code: i32,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub step_details: Vec<TaskStepResult>,
}

pub struct HistoryStore {
    path: PathBuf,
    write_lock: std::sync::Mutex<()>,
}

impl HistoryStore {
    pub fn from_config_path(config_path: &Path) -> Result<Self> {
        let store = Self {
            path: history_path_for_config(config_path),
            write_lock: std::sync::Mutex::new(()),
        };
        store.init()?;
        Ok(store)
    }

    pub fn for_read_only(config_path: &Path) -> Self {
        Self {
            path: history_path_for_config(config_path),
            write_lock: std::sync::Mutex::new(()),
        }
    }

    pub fn init(&self) -> Result<()> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).with_context(|| {
            format!("failed to create history directory '{}'", parent.display())
        })?;
        let conn = self.open_rw_connection()?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS task_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                status TEXT NOT NULL,
                summary TEXT NOT NULL,
                exit_code INTEGER NOT NULL,
                started_at TEXT NOT NULL,
                finished_at TEXT NOT NULL,
                step_details TEXT NOT NULL DEFAULT '[]'
            );
            CREATE INDEX IF NOT EXISTS idx_task_history_task_id_finished_at
                ON task_history(task_id, finished_at DESC);
            CREATE INDEX IF NOT EXISTS idx_task_history_status_finished_at
                ON task_history(status, finished_at DESC);
            "#,
        )
        .context("failed to initialize history schema")?;
        ensure_step_details_column(&conn)?;
        Ok(())
    }

    pub fn record(&self, task_id: &str, outcome: &TaskOutcome) -> Result<()> {
        let _guard = self
            .write_lock
            .lock()
            .expect("history store mutex poisoned");
        self.init()?;
        let conn = self.open_rw_connection()?;
        conn.execute(
            r#"
            INSERT INTO task_history (
                task_id,
                status,
                summary,
                exit_code,
                started_at,
                finished_at,
                step_details
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                task_id,
                format!("{:?}", outcome.status()).to_lowercase(),
                outcome.summary(),
                outcome.exit_code(),
                outcome.started_at().to_rfc3339(),
                outcome.finished_at().to_rfc3339(),
                serde_json::to_string(outcome.steps()).context("failed to encode step details")?,
            ],
        )
        .context("failed to insert history record")?;
        Ok(())
    }

    pub fn list_task_history(&self, task_id: &str, limit: usize) -> Result<Vec<HistoryRecord>> {
        let Some(conn) = self.open_ro_connection_if_present()? else {
            return Ok(Vec::new());
        };
        let mut stmt = conn.prepare(
            r#"
            SELECT id, task_id, status, summary, exit_code, started_at, finished_at, step_details
            FROM task_history
            WHERE task_id = ?1
            ORDER BY finished_at DESC, id DESC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![task_id, limit as i64], map_history_row)?;
        collect_rows(rows)
    }

    pub fn list_recent_failures(&self, limit: usize) -> Result<Vec<HistoryRecord>> {
        let Some(conn) = self.open_ro_connection_if_present()? else {
            return Ok(Vec::new());
        };
        let mut stmt = conn.prepare(
            r#"
            SELECT id, task_id, status, summary, exit_code, started_at, finished_at, step_details
            FROM task_history
            WHERE status != 'success'
            ORDER BY finished_at DESC, id DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![limit as i64], map_history_row)?;
        collect_rows(rows)
    }

    pub fn list_history_between(
        &self,
        started_at: DateTime<Utc>,
        finished_before: DateTime<Utc>,
    ) -> Result<Vec<HistoryRecord>> {
        let Some(conn) = self.open_ro_connection_if_present()? else {
            return Ok(Vec::new());
        };
        let mut stmt = conn.prepare(
            r#"
            SELECT id, task_id, status, summary, exit_code, started_at, finished_at, step_details
            FROM task_history
            WHERE finished_at >= ?1 AND finished_at < ?2
            ORDER BY finished_at DESC, id DESC
            "#,
        )?;
        let rows = stmt.query_map(
            params![started_at.to_rfc3339(), finished_before.to_rfc3339()],
            map_history_row,
        )?;
        collect_rows(rows)
    }

    fn open_rw_connection(&self) -> Result<Connection> {
        open_connection(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
    }

    fn open_ro_connection_if_present(&self) -> Result<Option<Connection>> {
        if !self.path.exists() {
            return Ok(None);
        }
        Ok(Some(open_connection(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?))
    }
}

pub fn history_path_for_config(config_path: &Path) -> PathBuf {
    runtime_data_path_for_config(config_path, "history.db")
}

fn map_history_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryRecord> {
    let started_at: String = row.get(5)?;
    let finished_at: String = row.get(6)?;
    let step_details: String = row.get(7)?;
    Ok(HistoryRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        status: row.get(2)?,
        summary: row.get(3)?,
        exit_code: row.get(4)?,
        started_at: DateTime::parse_from_rfc3339(&started_at)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?,
        finished_at: DateTime::parse_from_rfc3339(&finished_at)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    6,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?,
        step_details: serde_json::from_str(&step_details).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(err))
        })?,
    })
}

fn ensure_step_details_column(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(task_history)")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut has_step_details = false;
    for row in rows {
        if row? == "step_details" {
            has_step_details = true;
            break;
        }
    }
    if !has_step_details {
        conn.execute(
            "ALTER TABLE task_history ADD COLUMN step_details TEXT NOT NULL DEFAULT '[]'",
            [],
        )
        .context("failed to add step_details column to history schema")?;
    }
    Ok(())
}

fn open_connection(path: &Path, flags: OpenFlags) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, flags)
        .with_context(|| format!("failed to open history db '{}'", path.display()))?;
    conn.busy_timeout(Duration::from_secs(5))
        .context("failed to configure sqlite busy timeout")?;
    Ok(conn)
}

fn collect_rows(
    rows: rusqlite::MappedRows<
        '_,
        impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<HistoryRecord>,
    >,
) -> Result<Vec<HistoryRecord>> {
    let mut values = Vec::new();
    for row in rows {
        values.push(row?);
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::Utc;
    use tempfile::tempdir;

    use super::{HistoryStore, history_path_for_config};
    use crate::task_runner::{TaskOutcome, TaskRunStatus};

    #[test]
    fn derives_history_path_from_config_path() {
        let path = history_path_for_config(std::path::Path::new("/tmp/taskd/tasks.yaml"));
        assert_eq!(path, PathBuf::from("/tmp/taskd/tasks.history.db"));
    }

    #[test]
    fn derives_history_path_from_system_config_path() {
        let path = history_path_for_config(std::path::Path::new("/etc/taskd/tasks.yaml"));
        assert_eq!(path, PathBuf::from("/var/lib/taskd/tasks.history.db"));
    }

    #[test]
    fn records_and_queries_history() {
        let dir = tempdir().expect("tempdir");
        let store = HistoryStore {
            path: dir.path().join("tasks.history.db"),
            write_lock: std::sync::Mutex::new(()),
        };
        store.init().expect("init");
        let now = Utc::now();
        let outcome = TaskOutcome::synthetic(
            TaskRunStatus::Success,
            "exit code 0".to_string(),
            0,
            now,
            now,
        );

        store.record("job-1", &outcome).expect("record");
        let rows = store.list_task_history("job-1", 10).expect("query");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].task_id, "job-1");
        assert_eq!(rows[0].status, "success");
    }

    #[test]
    fn read_only_queries_return_empty_when_history_is_missing() {
        let dir = tempdir().expect("tempdir");
        let store = HistoryStore::for_read_only(&dir.path().join("tasks.yaml"));

        let rows = store
            .list_task_history("job-1", 10)
            .expect("query missing history");
        let failures = store
            .list_recent_failures(10)
            .expect("query missing failures");

        assert!(rows.is_empty());
        assert!(failures.is_empty());
        assert!(!dir.path().join("tasks.history.db").exists());
    }
}
