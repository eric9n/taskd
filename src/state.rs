//! Lightweight persisted runtime state for the most recent task outcome.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::runtime_paths::runtime_data_path_for_config;
use crate::task_runner::{TaskOutcome, TaskRunStatus, TaskStepResult};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeStateFile {
    pub version: u32,
    #[serde(default)]
    pub tasks: BTreeMap<String, TaskRuntimeState>,
}

impl Default for RuntimeStateFile {
    fn default() -> Self {
        Self {
            version: 1,
            tasks: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskRuntimeState {
    pub last_status: TaskRunStatus,
    pub last_summary: String,
    pub last_started_at: DateTime<Utc>,
    pub last_finished_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub last_steps: Vec<TaskStepResult>,
}

impl TaskRuntimeState {
    pub fn from_outcome(outcome: &TaskOutcome) -> Self {
        Self {
            last_status: outcome.status(),
            last_summary: outcome.summary().to_string(),
            last_started_at: outcome.started_at(),
            last_finished_at: outcome.finished_at(),
            last_steps: outcome.steps().to_vec(),
        }
    }
}

pub struct RuntimeStateStore {
    path: PathBuf,
    inner: std::sync::Mutex<RuntimeStateFile>,
}

impl RuntimeStateStore {
    pub fn load(path: PathBuf) -> Result<Self> {
        let state = load_runtime_state(&path)?;
        Ok(Self {
            path,
            inner: std::sync::Mutex::new(state),
        })
    }

    pub fn from_config_path(config_path: &Path) -> Result<Self> {
        Self::load(state_path_for_config(config_path))
    }

    pub fn record(&self, task_id: &str, outcome: &TaskOutcome) -> Result<()> {
        let mut state = self.inner.lock().expect("runtime state mutex poisoned");
        state
            .tasks
            .insert(task_id.to_string(), TaskRuntimeState::from_outcome(outcome));
        persist_runtime_state(&self.path, &state)
    }

    pub fn remove_task(&self, task_id: &str) -> Result<()> {
        let mut state = self.inner.lock().expect("runtime state mutex poisoned");
        state.tasks.remove(task_id);
        persist_runtime_state(&self.path, &state)
    }
}

pub fn state_path_for_config(config_path: &Path) -> PathBuf {
    runtime_data_path_for_config(config_path, "state.yaml")
}

pub fn load_runtime_state(path: &Path) -> Result<RuntimeStateFile> {
    match fs::read_to_string(path) {
        Ok(raw) => serde_yaml::from_str::<RuntimeStateFile>(&raw)
            .with_context(|| format!("failed to parse runtime state '{}'", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(RuntimeStateFile::default())
        }
        Err(error) => {
            Err(error).with_context(|| format!("failed to read runtime state '{}'", path.display()))
        }
    }
}

fn persist_runtime_state(path: &Path, state: &RuntimeStateFile) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create state directory '{}'", parent.display()))?;
    let yaml = serde_yaml::to_string(state).context("failed to serialize runtime state")?;
    fs::write(path, yaml)
        .with_context(|| format!("failed to write runtime state '{}'", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use chrono::Utc;
    use tempfile::tempdir;

    use super::{RuntimeStateStore, load_runtime_state, state_path_for_config};
    use crate::task_runner::{TaskOutcome, TaskRunStatus};

    #[test]
    fn derives_state_path_from_config_path() {
        let path = state_path_for_config(Path::new("/tmp/taskd/tasks.yaml"));
        assert_eq!(path, PathBuf::from("/tmp/taskd/tasks.state.yaml"));
    }

    #[test]
    fn derives_state_path_from_system_config_path() {
        let path = state_path_for_config(Path::new("/etc/taskd/tasks.yaml"));
        assert_eq!(path, PathBuf::from("/var/lib/taskd/tasks.state.yaml"));
    }

    #[test]
    fn records_and_reads_runtime_state() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("tasks.state.yaml");
        let store = RuntimeStateStore::load(path.clone()).expect("load store");
        let now = Utc::now();
        let outcome = TaskOutcome::synthetic(
            TaskRunStatus::Success,
            "exit code 0".to_string(),
            0,
            now,
            now,
        );

        store.record("job-1", &outcome).expect("record state");
        let state = load_runtime_state(&path).expect("load state");

        assert_eq!(state.tasks["job-1"].last_status, TaskRunStatus::Success);
    }
}
