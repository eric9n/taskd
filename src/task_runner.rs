//! External command execution, timeout handling, and guarded task outcomes.

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::time::{Duration, timeout};
use tracing::{error, info, warn};

use crate::config::{CommandConfig, TaskConfig};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskRunStatus {
    Success,
    Failed,
    Error,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskStepResult {
    pub step_id: String,
    pub status: TaskRunStatus,
    pub summary: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub exit_code: i32,
}

#[derive(Debug, Clone)]
pub struct TaskOutcome {
    status: TaskRunStatus,
    summary: String,
    exit_status: Option<ExitStatus>,
    explicit_exit_code: Option<i32>,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    steps: Vec<TaskStepResult>,
    stdout: Option<String>,
    stderr: Option<String>,
}

impl TaskOutcome {
    pub fn synthetic(
        status: TaskRunStatus,
        summary: String,
        _exit_code: i32,
        started_at: DateTime<Utc>,
        finished_at: DateTime<Utc>,
    ) -> Self {
        Self {
            status,
            summary,
            exit_status: None,
            explicit_exit_code: Some(_exit_code),
            started_at,
            finished_at,
            steps: Vec::new(),
            stdout: None,
            stderr: None,
        }
    }

    pub fn panic(task_id: &str, error: &str) -> Self {
        let now = Utc::now();
        Self {
            status: TaskRunStatus::Error,
            summary: format!("task '{task_id}' panicked: {error}"),
            exit_status: None,
            explicit_exit_code: Some(1),
            started_at: now,
            finished_at: now,
            steps: Vec::new(),
            stdout: None,
            stderr: None,
        }
    }

    pub fn success(&self) -> bool {
        self.status == TaskRunStatus::Success
    }

    pub fn status(&self) -> TaskRunStatus {
        self.status
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }

    pub fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }

    pub fn finished_at(&self) -> DateTime<Utc> {
        self.finished_at
    }

    pub fn exit_code(&self) -> i32 {
        if let Some(code) = self.explicit_exit_code {
            return code;
        }
        if let Some(status) = self.exit_status {
            if let Some(code) = status.code() {
                return code;
            }
            #[cfg(unix)]
            if let Some(signal) = status.signal() {
                return 128 + signal;
            }
        }
        1
    }

    pub fn steps(&self) -> &[TaskStepResult] {
        &self.steps
    }

    pub fn stdout(&self) -> Option<&str> {
        self.stdout.as_deref()
    }

    pub fn stderr(&self) -> Option<&str> {
        self.stderr.as_deref()
    }

    fn with_retry_summary(mut self, attempts_run: u8) -> Self {
        if attempts_run > 1 {
            self.summary = if self.success() {
                format!(
                    "{} (succeeded after {} attempts)",
                    self.summary, attempts_run
                )
            } else {
                format!("{} (failed after {} attempts)", self.summary, attempts_run)
            };
        }
        self
    }
}

pub async fn run_task(task: &TaskConfig) -> TaskOutcome {
    run_task_with_optional_cancel(task, None).await
}

async fn run_task_with_optional_cancel(
    task: &TaskConfig,
    cancel: Option<&mut oneshot::Receiver<()>>,
) -> TaskOutcome {
    run_single_command_task(task, &task.command, cancel).await
}

pub async fn run_task_or_error(task: &TaskConfig) -> Result<()> {
    let outcome = run_task(task).await;
    if outcome.success() {
        Ok(())
    } else {
        Err(anyhow!(outcome.summary().to_string()))
    }
}

pub async fn run_task_with_retry(task: &TaskConfig) -> TaskOutcome {
    run_task_with_retry_and_optional_cancel(task, None).await
}

async fn run_task_with_retry_and_optional_cancel(
    task: &TaskConfig,
    mut cancel: Option<&mut oneshot::Receiver<()>>,
) -> TaskOutcome {
    let total_attempts = task.retry.max_attempts.saturating_add(1);

    for attempt in 1..=total_attempts {
        let outcome = run_task_with_optional_cancel(task, cancel.as_deref_mut()).await;
        if !should_retry(task, &outcome, attempt) {
            return outcome.with_retry_summary(attempt);
        }

        warn!(
            task_id = %task.id,
            attempt,
            total_attempts,
            retry_delay_seconds = task.retry.delay_seconds,
            result = %outcome.summary(),
            "task failed and will be retried"
        );

        if wait_for_retry_delay(task, cancel.as_deref_mut()).await {
            return TaskOutcome {
                status: TaskRunStatus::Cancelled,
                summary: format!(
                    "task '{}' cancelled by replace policy during retry backoff",
                    task.id
                ),
                exit_status: None,
                explicit_exit_code: Some(1),
                started_at: Utc::now(),
                finished_at: Utc::now(),
                steps: Vec::new(),
                stdout: None,
                stderr: None,
            };
        }
    }

    unreachable!("retry loop must return an outcome")
}

pub async fn run_task_guarded(task: Arc<TaskConfig>) -> Result<TaskOutcome> {
    let task_id = task.id.clone();
    tokio::spawn(async move { run_task(task.as_ref()).await })
        .await
        .map_err(|error| anyhow!("task '{}' panicked: {}", task_id, error))
}

pub async fn run_task_guarded_with_cancel(
    task: Arc<TaskConfig>,
    cancel: oneshot::Receiver<()>,
) -> Result<TaskOutcome> {
    let task_id = task.id.clone();
    tokio::spawn(async move {
        let mut cancel = cancel;
        run_task_with_optional_cancel(task.as_ref(), Some(&mut cancel)).await
    })
    .await
    .map_err(|error| anyhow!("task '{}' panicked: {}", task_id, error))
}

pub async fn run_task_with_retry_guarded(task: Arc<TaskConfig>) -> Result<TaskOutcome> {
    let task_id = task.id.clone();
    tokio::spawn(async move { run_task_with_retry(task.as_ref()).await })
        .await
        .map_err(|error| anyhow!("task '{}' panicked: {}", task_id, error))
}

pub async fn run_task_with_retry_guarded_with_cancel(
    task: Arc<TaskConfig>,
    cancel: oneshot::Receiver<()>,
) -> Result<TaskOutcome> {
    let task_id = task.id.clone();
    tokio::spawn(async move {
        let mut cancel = cancel;
        run_task_with_retry_and_optional_cancel(task.as_ref(), Some(&mut cancel)).await
    })
    .await
    .map_err(|error| anyhow!("task '{}' panicked: {}", task_id, error))
}

pub async fn run_task_or_error_guarded(task: Arc<TaskConfig>) -> Result<()> {
    let outcome = run_task_guarded(task.clone()).await?;
    if outcome.success() {
        Ok(())
    } else {
        Err(anyhow!(outcome.summary().to_string()))
    }
}

pub async fn run_task_or_error_guarded_with_cancel(
    task: Arc<TaskConfig>,
    cancel: oneshot::Receiver<()>,
) -> Result<()> {
    let outcome = run_task_guarded_with_cancel(task.clone(), cancel).await?;
    if outcome.success() {
        Ok(())
    } else {
        Err(anyhow!(outcome.summary().to_string()))
    }
}

fn outcome_from_exit_status(
    started_at: DateTime<Utc>,
    status: ExitStatus,
    stdout: String,
    stderr: String,
) -> TaskOutcome {
    let status_kind = if status.success() {
        TaskRunStatus::Success
    } else {
        TaskRunStatus::Failed
    };
    let summary = if let Some(code) = status.code() {
        format!("exit code {code}")
    } else {
        #[cfg(unix)]
        if let Some(signal) = status.signal() {
            return TaskOutcome {
                status: TaskRunStatus::Failed,
                summary: format!("terminated by signal {signal}"),
                exit_status: Some(status),
                explicit_exit_code: None,
                started_at,
                finished_at: Utc::now(),
                steps: Vec::new(),
                stdout: Some(stdout),
                stderr: Some(stderr),
            };
        }
        "terminated without an exit code".to_string()
    };

    TaskOutcome {
        status: status_kind,
        summary,
        exit_status: Some(status),
        explicit_exit_code: None,
        started_at,
        finished_at: Utc::now(),
        steps: Vec::new(),
        stdout: Some(stdout),
        stderr: Some(stderr),
    }
}

enum TaskWaitError {
    TimedOut(u64),
    Cancelled,
    WaitFailed(anyhow::Error),
}

async fn wait_for_child(
    task_id: &str,
    timeout_seconds: Option<u64>,
    child: &mut tokio::process::Child,
    cancel: Option<&mut oneshot::Receiver<()>>,
) -> std::result::Result<ExitStatus, TaskWaitError> {
    match (timeout_seconds, cancel) {
        (Some(timeout_seconds), Some(cancel)) => {
            tokio::select! {
                status = child.wait() => {
                    status.with_context(|| format!("failed to run task '{}'", task_id))
                        .map_err(TaskWaitError::WaitFailed)
                }
                _ = tokio::time::sleep(Duration::from_secs(timeout_seconds)) => {
                    let _ = terminate_child(child).await;
                    Err(TaskWaitError::TimedOut(timeout_seconds))
                }
                _ = cancel => {
                    let _ = terminate_child(child).await;
                    Err(TaskWaitError::Cancelled)
                }
            }
        }
        (Some(timeout_seconds), None) => {
            match timeout(Duration::from_secs(timeout_seconds), child.wait()).await {
                Ok(status) => status
                    .with_context(|| format!("failed to run task '{}'", task_id))
                    .map_err(TaskWaitError::WaitFailed),
                Err(_) => {
                    let _ = terminate_child(child).await;
                    Err(TaskWaitError::TimedOut(timeout_seconds))
                }
            }
        }
        (None, Some(cancel)) => {
            tokio::select! {
                status = child.wait() => {
                    status.with_context(|| format!("failed to run task '{}'", task_id))
                        .map_err(TaskWaitError::WaitFailed)
                }
                _ = cancel => {
                    let _ = terminate_child(child).await;
                    Err(TaskWaitError::Cancelled)
                }
            }
        }
        (None, None) => child
            .wait()
            .await
            .with_context(|| format!("failed to run task '{}'", task_id))
            .map_err(TaskWaitError::WaitFailed),
    }
}

async fn run_single_command_task(
    task: &TaskConfig,
    command: &CommandConfig,
    cancel: Option<&mut oneshot::Receiver<()>>,
) -> TaskOutcome {
    let outcome = run_command(task, None, command, cancel).await;
    log_task_result(&task.id, &outcome);
    outcome
}

async fn run_command(
    task: &TaskConfig,
    step_id: Option<&str>,
    command: &CommandConfig,
    cancel: Option<&mut oneshot::Receiver<()>>,
) -> TaskOutcome {
    let started_at = Utc::now();
    match step_id {
        Some(step_id) => info!(
            task_id = %task.id,
            step_id = %step_id,
            command = %command.program,
            "starting task step"
        ),
        None => info!(task_id = %task.id, command = %command.program, "starting task"),
    }

    let mut child_command = Command::new(&command.program);
    child_command.args(&command.args);
    if let Some(workdir) = &command.workdir {
        child_command.current_dir(workdir);
    }
    for (key, value) in &command.env {
        child_command.env(key, value);
    }
    child_command.stdout(std::process::Stdio::piped());
    child_command.stderr(std::process::Stdio::piped());

    let mut child = match child_command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return TaskOutcome {
                status: TaskRunStatus::Error,
                summary: format_command_error(&task.id, step_id, &error.to_string()),
                exit_status: None,
                explicit_exit_code: Some(1),
                started_at,
                finished_at: Utc::now(),
                steps: Vec::new(),
                stdout: None,
                stderr: None,
            };
        }
    };

    let stdout_reader = tokio::spawn(read_optional_stream(child.stdout.take()));
    let stderr_reader = tokio::spawn(read_optional_stream(child.stderr.take()));

    let wait_result = wait_for_child(&task.id, command.timeout_seconds, &mut child, cancel).await;
    let stdout = stdout_reader.await.unwrap_or_default();
    let stderr = stderr_reader.await.unwrap_or_default();

    match wait_result {
        Ok(status) => outcome_from_exit_status(started_at, status, stdout, stderr),
        Err(TaskWaitError::TimedOut(timeout_seconds)) => TaskOutcome {
            status: TaskRunStatus::TimedOut,
            summary: format_timeout_error(&task.id, step_id, timeout_seconds),
            exit_status: None,
            explicit_exit_code: Some(1),
            started_at,
            finished_at: Utc::now(),
            steps: Vec::new(),
            stdout: Some(stdout),
            stderr: Some(stderr),
        },
        Err(TaskWaitError::Cancelled) => TaskOutcome {
            status: TaskRunStatus::Cancelled,
            summary: format_cancel_error(&task.id, step_id),
            exit_status: None,
            explicit_exit_code: Some(1),
            started_at,
            finished_at: Utc::now(),
            steps: Vec::new(),
            stdout: Some(stdout),
            stderr: Some(stderr),
        },
        Err(TaskWaitError::WaitFailed(error)) => TaskOutcome {
            status: TaskRunStatus::Error,
            summary: format_command_error(&task.id, step_id, &error.to_string()),
            exit_status: None,
            explicit_exit_code: Some(1),
            started_at,
            finished_at: Utc::now(),
            steps: Vec::new(),
            stdout: Some(stdout),
            stderr: Some(stderr),
        },
    }
}

async fn read_optional_stream<T>(stream: Option<T>) -> String
where
    T: tokio::io::AsyncRead + Unpin,
{
    let Some(mut stream) = stream else {
        return String::new();
    };
    let mut bytes = Vec::new();
    if stream.read_to_end(&mut bytes).await.is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn format_command_error(task_id: &str, step_id: Option<&str>, error: &str) -> String {
    match step_id {
        Some(step_id) => format!(
            "failed to run task '{}' step '{}': {error}",
            task_id, step_id
        ),
        None => format!("failed to run task '{}': {error}", task_id),
    }
}

fn format_timeout_error(task_id: &str, step_id: Option<&str>, timeout_seconds: u64) -> String {
    match step_id {
        Some(step_id) => format!(
            "task '{}' step '{}' timed out after {}s",
            task_id, step_id, timeout_seconds
        ),
        None => format!("task '{}' timed out after {}s", task_id, timeout_seconds),
    }
}

fn format_cancel_error(task_id: &str, step_id: Option<&str>) -> String {
    match step_id {
        Some(step_id) => format!(
            "task '{}' step '{}' cancelled by replace policy",
            task_id, step_id
        ),
        None => format!("task '{}' cancelled by replace policy", task_id),
    }
}

fn log_task_result(task_id: &str, outcome: &TaskOutcome) {
    if outcome.success() {
        info!(task_id = %task_id, result = %outcome.summary(), "task completed");
    } else {
        error!(task_id = %task_id, result = %outcome.summary(), "task failed");
    }
}

fn should_retry(task: &TaskConfig, outcome: &TaskOutcome, attempt: u8) -> bool {
    attempt < task.retry.max_attempts.saturating_add(1)
        && matches!(
            outcome.status(),
            TaskRunStatus::Failed | TaskRunStatus::Error | TaskRunStatus::TimedOut
        )
}

async fn wait_for_retry_delay(
    task: &TaskConfig,
    cancel: Option<&mut oneshot::Receiver<()>>,
) -> bool {
    match cancel {
        Some(cancel) => {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(task.retry.delay_seconds)) => false,
                _ = cancel => true,
            }
        }
        None => {
            tokio::time::sleep(Duration::from_secs(task.retry.delay_seconds)).await;
            false
        }
    }
}

async fn terminate_child(child: &mut tokio::process::Child) -> Result<()> {
    let _ = child.start_kill();
    let _ = child.wait().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use tempfile::tempdir;

    use super::{TaskRunStatus, run_task, run_task_guarded, run_task_or_error};
    use crate::config::{
        CommandConfig, ConcurrencyConfig, RetryConfig, ScheduleConfig, TaskConfig,
    };

    #[tokio::test]
    async fn runs_command_successfully() {
        let task = sample_task(vec!["-c".into(), "exit 0".into()]);

        let outcome = run_task(&task).await;

        assert!(outcome.success());
        assert_eq!(outcome.exit_code(), 0);
        assert_eq!(outcome.status(), TaskRunStatus::Success);
    }

    #[tokio::test]
    async fn returns_non_zero_exit_status() {
        let task = sample_task(vec!["-c".into(), "exit 7".into()]);

        let outcome = run_task(&task).await;

        assert_eq!(outcome.exit_code(), 7);
        assert_eq!(outcome.status(), TaskRunStatus::Failed);
        assert_eq!(outcome.summary(), "exit code 7");
    }

    #[tokio::test]
    async fn captures_stdout_and_stderr_for_command_tasks() {
        let task = sample_task(vec![
            "-c".into(),
            "printf 'hello'; printf 'warn' >&2".into(),
        ]);

        let outcome = run_task(&task).await;

        assert_eq!(outcome.stdout(), Some("hello"));
        assert_eq!(outcome.stderr(), Some("warn"));
    }

    #[tokio::test]
    async fn supports_workdir_and_env() {
        let dir = tempdir().expect("tempdir");
        let output = dir.path().join("output.txt");
        let task = TaskConfig {
            id: "job".to_string(),
            name: "job".to_string(),
            enabled: true,
            concurrency: ConcurrencyConfig::default(),
            retry: RetryConfig::default(),
            schedule: ScheduleConfig::Interval { seconds: 60 },
            notify: None,
            command: CommandConfig {
                program: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    format!("printf '%s' \"$TASKD_VALUE\" > {}", output.display()),
                ],
                workdir: Some(PathBuf::from(dir.path())),
                timeout_seconds: None,
                env: BTreeMap::from([("TASKD_VALUE".to_string(), "written".to_string())]),
            },
        };

        run_task_or_error(&task).await.expect("task should succeed");

        let body = std::fs::read_to_string(output).expect("output file");
        assert_eq!(body, "written");
    }

    #[tokio::test]
    async fn missing_executable_returns_error_without_panicking() {
        let task = TaskConfig {
            command: CommandConfig {
                program: "/definitely/missing/taskd-bin".to_string(),
                ..sample_task(vec![]).command
            },
            ..sample_task(vec![])
        };

        let outcome = run_task_guarded(Arc::new(task))
            .await
            .expect("task should not panic");

        assert_eq!(outcome.status(), TaskRunStatus::Error);
        assert!(outcome.summary().contains("failed to run task 'job'"));
    }

    #[tokio::test]
    async fn timeout_kills_long_running_command() {
        let task = TaskConfig {
            command: CommandConfig {
                timeout_seconds: Some(1),
                ..sample_task(vec!["-c".into(), "sleep 5".into()]).command
            },
            ..sample_task(vec!["-c".into(), "sleep 5".into()])
        };

        let outcome = run_task(&task).await;

        assert_eq!(outcome.status(), TaskRunStatus::TimedOut);
        assert!(outcome.summary().contains("timed out after 1s"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn signal_terminated_process_returns_shell_convention_exit_code() {
        let task = sample_task(vec!["-c".into(), "kill -9 $$".into()]);

        let outcome = run_task(&task).await;

        assert_eq!(outcome.exit_code(), 137);
        assert_eq!(outcome.summary(), "terminated by signal 9");
    }

    fn sample_task(args: Vec<String>) -> TaskConfig {
        TaskConfig {
            id: "job".to_string(),
            name: "job".to_string(),
            enabled: true,
            concurrency: ConcurrencyConfig::default(),
            retry: RetryConfig::default(),
            schedule: ScheduleConfig::Interval { seconds: 60 },
            notify: None,
            command: CommandConfig {
                program: "/bin/sh".to_string(),
                args,
                workdir: None,
                timeout_seconds: None,
                env: BTreeMap::new(),
            },
        }
    }
}
