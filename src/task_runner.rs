use std::process::ExitStatus;

use anyhow::{Context, Result, anyhow};
use tokio::process::Command;
use tracing::{error, info};

use crate::config::TaskConfig;

#[derive(Debug)]
pub struct TaskOutcome {
    status: ExitStatus,
}

impl TaskOutcome {
    pub fn status(&self) -> ExitStatus {
        self.status
    }

    pub fn exit_code(&self) -> i32 {
        self.status.code().unwrap_or(1)
    }
}

pub async fn run_task(task: &TaskConfig) -> Result<TaskOutcome> {
    info!(task_id = %task.id, command = %task.command.program, "starting task");

    let mut command = Command::new(&task.command.program);
    command.args(&task.command.args);
    if let Some(workdir) = &task.command.workdir {
        command.current_dir(workdir);
    }
    for (key, value) in &task.command.env {
        command.env(key, value);
    }

    let status = command
        .status()
        .await
        .with_context(|| format!("failed to run task '{}'", task.id))?;

    if status.success() {
        info!(task_id = %task.id, status = ?status, "task completed");
    } else {
        error!(task_id = %task.id, status = ?status, "task failed");
    }

    Ok(TaskOutcome { status })
}

pub async fn run_task_or_error(task: &TaskConfig) -> Result<()> {
    let outcome = run_task(task).await?;
    if outcome.status().success() {
        Ok(())
    } else {
        Err(anyhow!(
            "task '{}' exited with status {}",
            task.id,
            outcome.exit_code()
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{run_task, run_task_or_error};
    use crate::config::{CommandConfig, ScheduleConfig, TaskConfig};

    #[tokio::test]
    async fn runs_command_successfully() {
        let task = sample_task(vec!["-c".into(), "exit 0".into()]);

        let outcome = run_task(&task).await.expect("task should run");

        assert!(outcome.status().success());
        assert_eq!(outcome.exit_code(), 0);
    }

    #[tokio::test]
    async fn returns_non_zero_exit_status() {
        let task = sample_task(vec!["-c".into(), "exit 7".into()]);

        let outcome = run_task(&task).await.expect("task should run");

        assert_eq!(outcome.exit_code(), 7);
        assert!(!outcome.status().success());
    }

    #[tokio::test]
    async fn supports_workdir_and_env() {
        let dir = tempdir().expect("tempdir");
        let output = dir.path().join("output.txt");
        let task = TaskConfig {
            id: "job".to_string(),
            name: "job".to_string(),
            enabled: true,
            schedule: ScheduleConfig::Interval { seconds: 60 },
            command: CommandConfig {
                program: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    format!("printf '%s' \"$TASKD_VALUE\" > {}", output.display()),
                ],
                workdir: Some(PathBuf::from(dir.path())),
                env: BTreeMap::from([("TASKD_VALUE".to_string(), "written".to_string())]),
            },
        };

        run_task_or_error(&task).await.expect("task should succeed");

        let body = std::fs::read_to_string(output).expect("output file");
        assert_eq!(body, "written");
    }

    fn sample_task(args: Vec<String>) -> TaskConfig {
        TaskConfig {
            id: "job".to_string(),
            name: "job".to_string(),
            enabled: true,
            schedule: ScheduleConfig::Interval { seconds: 60 },
            command: CommandConfig {
                program: "/bin/sh".to_string(),
                args,
                workdir: None,
                env: BTreeMap::new(),
            },
        }
    }
}
