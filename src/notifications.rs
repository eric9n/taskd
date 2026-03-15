//! Task completion notifications via a `pi` renderer command and webhook delivery.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::Value;
use tokio::process::Command;
use tokio::time::{Duration, timeout};
use uuid::Uuid;

use crate::config::{NotificationsConfig, NotifyResultSourceConfig, PiRendererConfig, TaskConfig};
use crate::task_runner::{TaskOutcome, TaskRunStatus, TaskStepResult};

const DISCORD_CONTENT_LIMIT: usize = 2000;
const DISCORD_TRUNCATED_SUFFIX: &str = "\n\n[truncated]";

pub async fn maybe_send_task_notification(
    notifications: Option<&NotificationsConfig>,
    task: &TaskConfig,
    outcome: &TaskOutcome,
    inherited_env: Option<&BTreeMap<String, String>>,
) -> Result<()> {
    let Some(task_notify) = &task.notify else {
        return Ok(());
    };
    let notifications = notifications.ok_or_else(|| {
        anyhow!(
            "task '{}' requested notify but notifications are not configured",
            task.id
        )
    })?;
    if !notifications.enabled {
        return Ok(());
    }
    let task_result = load_task_result(task, outcome, &task_notify.result_source)?;
    if !should_send_notification(&task_result)? {
        return Ok(());
    }
    let renderer = notifications.renderer.as_ref().ok_or_else(|| {
        anyhow!(
            "task '{}' requested notify but notifications.renderer is not configured",
            task.id
        )
    })?;
    let webhook = notifications.webhook.as_ref().ok_or_else(|| {
        anyhow!(
            "task '{}' requested notify but notifications.webhook is not configured",
            task.id
        )
    })?;
    let temp_dir = notification_temp_dir(task)?;
    fs::create_dir_all(&temp_dir).with_context(|| {
        format!(
            "failed to create notification temp dir '{}'",
            temp_dir.display()
        )
    })?;
    let input_path = temp_dir.join("notify-input.json");
    let input = NotificationRendererInput::from_task(task, outcome, task_result);
    let input_body =
        serde_json::to_vec_pretty(&input).context("failed to encode notification input")?;
    fs::write(&input_path, input_body).with_context(|| {
        format!(
            "failed to write notification input '{}'",
            input_path.display()
        )
    })?;

    let rendered = run_pi_renderer(renderer, task, &input_path, inherited_env).await?;
    send_webhook(&webhook.url_env, &rendered, inherited_env).await?;
    let _ = fs::remove_dir_all(&temp_dir);
    Ok(())
}

fn load_task_result(
    task: &TaskConfig,
    outcome: &TaskOutcome,
    result_source: &NotifyResultSourceConfig,
) -> Result<String> {
    match result_source {
        NotifyResultSourceConfig::Stdout => Ok(outcome.stdout().unwrap_or("").to_string()),
        NotifyResultSourceConfig::File { path } => {
            let resolved = resolve_result_file(task, path);
            fs::read_to_string(&resolved).with_context(|| {
                format!(
                    "failed to read notify result file for task '{}' from '{}'",
                    task.id,
                    resolved.display()
                )
            })
        }
    }
}

fn should_send_notification(task_result: &str) -> Result<bool> {
    let trimmed = task_result.trim();
    if trimmed.is_empty() {
        return Ok(true);
    }

    let parsed = match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => value,
        Err(_) => return Ok(true),
    };
    let Some(object) = parsed.as_object() else {
        return Ok(true);
    };
    match object.get("notify") {
        None => Ok(true),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => bail!("task notify result field 'notify' must be a boolean when present"),
    }
}

fn resolve_result_file(task: &TaskConfig, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if let Some(workdir) = &task.command.workdir {
        return workdir.join(path);
    }
    path.to_path_buf()
}

fn notification_temp_dir(task: &TaskConfig) -> Result<PathBuf> {
    let task_dir = std::env::temp_dir().join("taskd-notify");
    let run_id = Uuid::new_v4().simple().to_string();
    Ok(task_dir.join(format!("{}-{}", task.id, &run_id[..8])))
}

async fn run_pi_renderer(
    renderer: &PiRendererConfig,
    task: &TaskConfig,
    input_path: &Path,
    inherited_env: Option<&BTreeMap<String, String>>,
) -> Result<String> {
    let mut command = Command::new(&renderer.program);
    command.current_dir(&renderer.workdir);
    command.arg("--print");
    command.arg("--no-session");
    if let Some(session_dir) = &renderer.session_dir {
        command.arg("--session-dir");
        command.arg(session_dir);
    }
    if let Some(provider) = &renderer.provider {
        command.arg("--provider");
        command.arg(provider);
    }
    if let Some(model) = &renderer.model {
        command.arg("--model");
        command.arg(model);
    }
    command.arg(format!("@{}", input_path.display()));
    command.arg(&renderer.prompt);
    if let Some(inherited_env) = inherited_env {
        command.envs(inherited_env);
    }
    for (key, value) in &renderer.env {
        command.env(key, value);
    }
    if let Some(agent_dir) = &renderer.agent_dir {
        command.env("PI_CODING_AGENT_DIR", agent_dir);
    }
    command.env("TASKD_NOTIFY_INPUT_FILE", input_path);
    command.env("TASKD_TASK_ID", &task.id);
    command.env("TASKD_TASK_NAME", &task.name);
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let output = if let Some(timeout_seconds) = renderer.timeout_seconds {
        timeout(Duration::from_secs(timeout_seconds), command.output())
            .await
            .with_context(|| {
                format!(
                    "pi renderer command '{}' timed out after {}s",
                    renderer.program, timeout_seconds
                )
            })??
    } else {
        command.output().await?
    };

    if !output.status.success() {
        bail!(
            "pi renderer command '{}' exited with {}: {}",
            renderer.program,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !stdout.trim().is_empty() {
        return Ok(stdout);
    }

    bail!(
        "pi renderer command '{}' produced no stdout content",
        renderer.program
    )
}

async fn send_webhook(
    url_env: &str,
    body: &str,
    inherited_env: Option<&BTreeMap<String, String>>,
) -> Result<()> {
    let url = inherited_env
        .and_then(|env| env.get(url_env).cloned())
        .or_else(|| std::env::var(url_env).ok())
        .with_context(|| format!("webhook env '{}' is not set", url_env))?;
    let payload = DiscordWebhookPayload {
        content: discord_content(body),
    };
    let payload_body =
        serde_json::to_string(&payload).context("failed to encode discord webhook payload")?;
    let response = reqwest::Client::new()
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(payload_body)
        .send()
        .await
        .context("failed to send webhook request")?;
    if !response.status().is_success() {
        let status = response.status();
        let response_body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unreadable webhook response>".to_string());
        bail!("webhook returned {}: {}", status, response_body.trim());
    }
    Ok(())
}

fn discord_content(body: &str) -> String {
    if body.chars().count() <= DISCORD_CONTENT_LIMIT {
        return body.to_string();
    }

    let truncated_len = DISCORD_CONTENT_LIMIT - DISCORD_TRUNCATED_SUFFIX.chars().count();
    let prefix = body.chars().take(truncated_len).collect::<String>();
    format!("{prefix}{DISCORD_TRUNCATED_SUFFIX}")
}

#[derive(Debug, Serialize)]
struct DiscordWebhookPayload {
    content: String,
}

#[derive(Debug, Serialize)]
struct NotificationRendererInput {
    task_id: String,
    task_name: String,
    status: TaskRunStatus,
    summary: String,
    exit_code: i32,
    started_at: chrono::DateTime<chrono::Utc>,
    finished_at: chrono::DateTime<chrono::Utc>,
    result: String,
    stdout: Option<String>,
    stderr: Option<String>,
    steps: Vec<TaskStepResult>,
    meta: BTreeMap<String, String>,
}

impl NotificationRendererInput {
    fn from_task(task: &TaskConfig, outcome: &TaskOutcome, result: String) -> Self {
        Self {
            task_id: task.id.clone(),
            task_name: task.name.clone(),
            status: outcome.status(),
            summary: outcome.summary().to_string(),
            exit_code: outcome.exit_code(),
            started_at: outcome.started_at(),
            finished_at: outcome.finished_at(),
            result,
            stdout: outcome.stdout().map(ToString::to_string),
            stderr: outcome.stderr().map(ToString::to_string),
            steps: outcome.steps().to_vec(),
            meta: BTreeMap::from([
                ("schedule".to_string(), task.schedule.summary()),
                (
                    "notify_result_source".to_string(),
                    match &task.notify {
                        Some(notify) => match &notify.result_source {
                            NotifyResultSourceConfig::Stdout => "stdout".to_string(),
                            NotifyResultSourceConfig::File { path } => {
                                format!("file:{}", path.display())
                            }
                        },
                        None => "disabled".to_string(),
                    },
                ),
            ]),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{
        DISCORD_CONTENT_LIMIT, DISCORD_TRUNCATED_SUFFIX, discord_content,
        maybe_send_task_notification,
    };
    use crate::config::{
        CommandConfig, ConcurrencyConfig, NotificationsConfig, NotifyResultSourceConfig,
        PiRendererConfig, RetryConfig, ScheduleConfig, TaskConfig, TaskNotifyConfig, WebhookConfig,
    };
    use crate::task_runner::{TaskOutcome, TaskRunStatus};

    #[tokio::test]
    async fn notification_runs_renderer_and_then_requires_webhook_env() {
        let dir = tempdir().expect("tempdir");
        let result_file = dir.path().join("result.txt");
        std::fs::write(&result_file, "raw result").expect("write result");
        let renderer = dir.path().join("renderer.sh");
        std::fs::write(&renderer, "#!/bin/sh\nset -eu\nprintf 'rendered summary'\n")
            .expect("write renderer");
        #[cfg(unix)]
        {
            let mut perms = std::fs::metadata(&renderer)
                .expect("metadata")
                .permissions();
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
            std::fs::set_permissions(&renderer, perms).expect("chmod");
        }

        let task = TaskConfig {
            id: "job".to_string(),
            name: "job".to_string(),
            enabled: true,
            concurrency: ConcurrencyConfig::default(),
            retry: RetryConfig::default(),
            schedule: ScheduleConfig::Interval { seconds: 60 },
            notify: Some(TaskNotifyConfig {
                result_source: NotifyResultSourceConfig::File {
                    path: PathBuf::from(&result_file),
                },
            }),
            command: CommandConfig {
                program: "/bin/echo".to_string(),
                args: vec!["ok".to_string()],
                workdir: None,
                timeout_seconds: None,
                env: BTreeMap::new(),
            },
        };
        let notifications = NotificationsConfig {
            enabled: true,
            renderer: Some(PiRendererConfig {
                program: renderer.display().to_string(),
                workdir: dir.path().to_path_buf(),
                prompt: "summarize".to_string(),
                timeout_seconds: None,
                session_dir: None,
                agent_dir: None,
                provider: None,
                model: None,
                env: BTreeMap::new(),
            }),
            webhook: Some(WebhookConfig {
                url_env: "MISSING_TASKD_WEBHOOK_URL".to_string(),
            }),
        };
        let outcome = TaskOutcome::synthetic(
            TaskRunStatus::Success,
            "exit code 0".to_string(),
            0,
            chrono::Utc::now(),
            chrono::Utc::now(),
        );

        let error = maybe_send_task_notification(Some(&notifications), &task, &outcome, None)
            .await
            .expect_err("missing webhook env should fail");
        assert!(error.to_string().contains("webhook env"));
    }

    #[tokio::test]
    async fn notification_skips_send_when_result_schema_sets_notify_false() {
        let dir = tempdir().expect("tempdir");
        let result_file = dir.path().join("result.json");
        std::fs::write(
            &result_file,
            r#"{"notify":false,"summary":"routine success"}"#,
        )
        .expect("write result");

        let task = TaskConfig {
            id: "job".to_string(),
            name: "job".to_string(),
            enabled: true,
            concurrency: ConcurrencyConfig::default(),
            retry: RetryConfig::default(),
            schedule: ScheduleConfig::Interval { seconds: 60 },
            notify: Some(TaskNotifyConfig {
                result_source: NotifyResultSourceConfig::File {
                    path: PathBuf::from(&result_file),
                },
            }),
            command: CommandConfig {
                program: "/bin/echo".to_string(),
                args: vec!["ok".to_string()],
                workdir: None,
                timeout_seconds: None,
                env: BTreeMap::new(),
            },
        };
        let notifications = NotificationsConfig {
            enabled: true,
            renderer: None,
            webhook: None,
        };
        let outcome = TaskOutcome::synthetic(
            TaskRunStatus::Success,
            "exit code 0".to_string(),
            0,
            chrono::Utc::now(),
            chrono::Utc::now(),
        );

        maybe_send_task_notification(Some(&notifications), &task, &outcome, None)
            .await
            .expect("notify=false should skip sending");
    }

    #[tokio::test]
    async fn notification_rejects_non_boolean_notify_field() {
        let dir = tempdir().expect("tempdir");
        let result_file = dir.path().join("result.json");
        std::fs::write(&result_file, r#"{"notify":"no"}"#).expect("write result");

        let task = TaskConfig {
            id: "job".to_string(),
            name: "job".to_string(),
            enabled: true,
            concurrency: ConcurrencyConfig::default(),
            retry: RetryConfig::default(),
            schedule: ScheduleConfig::Interval { seconds: 60 },
            notify: Some(TaskNotifyConfig {
                result_source: NotifyResultSourceConfig::File {
                    path: PathBuf::from(&result_file),
                },
            }),
            command: CommandConfig {
                program: "/bin/echo".to_string(),
                args: vec!["ok".to_string()],
                workdir: None,
                timeout_seconds: None,
                env: BTreeMap::new(),
            },
        };
        let notifications = NotificationsConfig {
            enabled: true,
            renderer: None,
            webhook: None,
        };
        let outcome = TaskOutcome::synthetic(
            TaskRunStatus::Success,
            "exit code 0".to_string(),
            0,
            chrono::Utc::now(),
            chrono::Utc::now(),
        );

        let error = maybe_send_task_notification(Some(&notifications), &task, &outcome, None)
            .await
            .expect_err("non-boolean notify should fail");
        assert!(
            error
                .to_string()
                .contains("field 'notify' must be a boolean")
        );
    }

    #[tokio::test]
    async fn notification_uses_loaded_env_before_process_env() {
        let dir = tempdir().expect("tempdir");
        let result_file = dir.path().join("result.txt");
        std::fs::write(&result_file, "raw result").expect("write result");
        let renderer = dir.path().join("renderer.sh");
        std::fs::write(&renderer, "#!/bin/sh\nset -eu\nprintf 'rendered summary'\n")
            .expect("write renderer");
        #[cfg(unix)]
        {
            let mut perms = std::fs::metadata(&renderer)
                .expect("metadata")
                .permissions();
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
            std::fs::set_permissions(&renderer, perms).expect("chmod");
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let read = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
                    .await
                    .expect("read request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            tokio::io::AsyncWriteExt::write_all(
                &mut stream,
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n",
            )
            .await
            .expect("write response");
        });

        let task = TaskConfig {
            id: "job".to_string(),
            name: "job".to_string(),
            enabled: true,
            concurrency: ConcurrencyConfig::default(),
            retry: RetryConfig::default(),
            schedule: ScheduleConfig::Interval { seconds: 60 },
            notify: Some(TaskNotifyConfig {
                result_source: NotifyResultSourceConfig::File {
                    path: PathBuf::from(&result_file),
                },
            }),
            command: CommandConfig {
                program: "/bin/echo".to_string(),
                args: vec!["ok".to_string()],
                workdir: None,
                timeout_seconds: None,
                env: BTreeMap::new(),
            },
        };
        let notifications = NotificationsConfig {
            enabled: true,
            renderer: Some(PiRendererConfig {
                program: renderer.display().to_string(),
                workdir: dir.path().to_path_buf(),
                prompt: "summarize".to_string(),
                timeout_seconds: None,
                session_dir: None,
                agent_dir: None,
                provider: None,
                model: None,
                env: BTreeMap::new(),
            }),
            webhook: Some(WebhookConfig {
                url_env: "TASKD_WEBHOOK_URL".to_string(),
            }),
        };
        let inherited_env = BTreeMap::from([(
            "TASKD_WEBHOOK_URL".to_string(),
            format!("http://{addr}/webhook"),
        )]);
        let outcome = TaskOutcome::synthetic(
            TaskRunStatus::Success,
            "exit code 0".to_string(),
            0,
            chrono::Utc::now(),
            chrono::Utc::now(),
        );

        maybe_send_task_notification(Some(&notifications), &task, &outcome, Some(&inherited_env))
            .await
            .expect("notification should use loaded env");
        server.await.expect("server join");
    }

    #[test]
    fn discord_content_keeps_short_body() {
        let body = "short body";
        assert_eq!(discord_content(body), body);
    }

    #[test]
    fn discord_content_truncates_long_body() {
        let body = "a".repeat(2500);
        let content = discord_content(&body);

        assert_eq!(content.chars().count(), DISCORD_CONTENT_LIMIT);
        assert!(content.ends_with(DISCORD_TRUNCATED_SUFFIX));
    }
}
