//! YAML-backed configuration types, validation, and persistence helpers.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail, ensure};
use chrono_tz::Tz;
use cron::Schedule;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppConfig {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<NotificationsConfig>,
    #[serde(default)]
    pub tasks: Vec<TaskConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: 1,
            notifications: None,
            tasks: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config '{}'", path.display()))?;
        let config = serde_yaml::from_str::<Self>(&raw)
            .with_context(|| format!("failed to parse yaml '{}'", path.display()))?;
        Ok(config)
    }

    pub fn load_or_default(path: &Path) -> Result<Self> {
        match Self::load(path) {
            Ok(config) => Ok(config),
            Err(error) if is_missing_file_error(&error) => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory '{}'", parent.display()))?;
        let yaml = serde_yaml::to_string(self).context("failed to serialize config")?;
        fs::write(path, yaml)
            .with_context(|| format!("failed to write config '{}'", path.display()))?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            "unsupported config version {}",
            self.version
        );
        if let Some(notifications) = &self.notifications {
            notifications.validate("notifications")?;
        }

        let mut seen = HashMap::new();
        for (index, task) in self.tasks.iter().enumerate() {
            let task_path = format!("tasks[{index}]");
            task.validate(&task_path, self.notifications.as_ref())?;
            if let Some(previous_index) = seen.insert(task.id.clone(), index) {
                bail!(
                    "{}.id duplicates task id '{}' already used by tasks[{}].id",
                    task_path,
                    task.id,
                    previous_index
                );
            }
        }

        Ok(())
    }

    pub fn task(&self, id: &str) -> Option<&TaskConfig> {
        self.tasks.iter().find(|task| task.id == id)
    }

    pub fn add_task(&mut self, task: TaskConfig) -> Result<()> {
        if self.task(&task.id).is_some() {
            bail!("task '{}' already exists", task.id);
        }
        self.tasks.push(task);
        Ok(())
    }

    pub fn remove_task(&mut self, id: &str) -> Result<String> {
        let index = self
            .tasks
            .iter()
            .position(|task| task.id == id)
            .ok_or_else(|| anyhow!("task '{id}' not found"))?;
        Ok(self.tasks.remove(index).id)
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<()> {
        let task = self
            .tasks
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or_else(|| anyhow!("task '{id}' not found"))?;
        task.enabled = enabled;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskConfig {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub concurrency: ConcurrencyConfig,
    #[serde(default, skip_serializing_if = "RetryConfig::is_disabled")]
    pub retry: RetryConfig,
    pub schedule: ScheduleConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<TaskNotifyConfig>,
    pub command: CommandConfig,
}

impl TaskConfig {
    pub fn validate(
        &self,
        task_path: &str,
        notifications: Option<&NotificationsConfig>,
    ) -> Result<()> {
        ensure!(
            is_valid_task_id(&self.id),
            "{}.id must use only letters, digits, '-', '_' or '.'",
            task_path
        );
        ensure!(
            !self.name.trim().is_empty(),
            "{}.name must not be empty",
            task_path
        );
        self.concurrency.validate(task_path)?;
        self.retry.validate(task_path)?;
        self.schedule.validate(task_path)?;
        if let Some(notify) = &self.notify {
            let notifications = notifications.ok_or_else(|| {
                anyhow!(
                    "{}.notify requires top-level notifications to be configured",
                    task_path
                )
            })?;
            notifications.validate(&format!("{task_path}.notifications_ref"))?;
            ensure!(
                notifications.enabled,
                "{}.notify requires top-level notifications.enabled to be true",
                task_path
            );
            notify.validate(task_path)?;
        }
        self.command.validate(&format!("{task_path}.command"))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer: Option<PiRendererConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook: Option<WebhookConfig>,
}

impl NotificationsConfig {
    pub fn validate(&self, path: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        self.renderer
            .as_ref()
            .ok_or_else(|| {
                anyhow!(
                    "{}.renderer must be set when notifications.enabled is true",
                    path
                )
            })?
            .validate(&format!("{path}.renderer"))?;
        self.webhook
            .as_ref()
            .ok_or_else(|| {
                anyhow!(
                    "{}.webhook must be set when notifications.enabled is true",
                    path
                )
            })?
            .validate(&format!("{path}.webhook"))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PiRendererConfig {
    #[serde(default = "default_pi_program")]
    pub program: String,
    pub workdir: PathBuf,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl PiRendererConfig {
    pub fn validate(&self, path: &str) -> Result<()> {
        ensure!(
            !self.program.trim().is_empty(),
            "{}.program must not be empty",
            path
        );
        ensure!(
            !self.prompt.trim().is_empty(),
            "{}.prompt must not be empty",
            path
        );
        ensure!(
            self.workdir.exists(),
            "{}.workdir '{}' does not exist",
            path,
            self.workdir.display()
        );
        ensure!(
            self.workdir.is_dir(),
            "{}.workdir '{}' is not a directory",
            path,
            self.workdir.display()
        );
        if let Some(timeout_seconds) = self.timeout_seconds {
            ensure!(timeout_seconds > 0, "{}.timeout_seconds must be > 0", path);
        }
        validate_optional_dir(&self.session_dir, &format!("{path}.session_dir"))?;
        validate_optional_dir(&self.agent_dir, &format!("{path}.agent_dir"))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebhookConfig {
    pub url_env: String,
}

impl WebhookConfig {
    pub fn validate(&self, path: &str) -> Result<()> {
        ensure!(
            !self.url_env.trim().is_empty(),
            "{}.url_env must not be empty",
            path
        );
        Ok(())
    }
}

fn default_pi_program() -> String {
    "/usr/bin/pi".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskNotifyConfig {
    pub result_source: NotifyResultSourceConfig,
}

impl TaskNotifyConfig {
    pub fn validate(&self, task_path: &str) -> Result<()> {
        self.result_source
            .validate(&format!("{task_path}.notify.result_source"))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum NotifyResultSourceConfig {
    Stdout,
    File { path: PathBuf },
}

impl NotifyResultSourceConfig {
    pub fn validate(&self, path: &str) -> Result<()> {
        match self {
            Self::Stdout => Ok(()),
            Self::File { path: file_path } => {
                ensure!(
                    !file_path.as_os_str().is_empty(),
                    "{}.path must not be empty",
                    path
                );
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetryConfig {
    #[serde(default)]
    pub max_attempts: u8,
    #[serde(default = "default_retry_delay_seconds")]
    pub delay_seconds: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 0,
            delay_seconds: default_retry_delay_seconds(),
        }
    }
}

impl RetryConfig {
    pub fn is_disabled(&self) -> bool {
        self.max_attempts == 0
    }

    pub fn validate(&self, task_path: &str) -> Result<()> {
        if self.max_attempts > 0 {
            ensure!(
                self.delay_seconds > 0,
                "{}.retry.delay_seconds must be > 0 when retry.max_attempts > 0",
                task_path
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConcurrencyConfig {
    #[serde(default)]
    pub policy: ConcurrencyPolicy,
    #[serde(default = "default_max_running")]
    pub max_running: u8,
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            policy: ConcurrencyPolicy::default(),
            max_running: default_max_running(),
        }
    }
}

impl ConcurrencyConfig {
    pub fn validate(&self, task_path: &str) -> Result<()> {
        ensure!(
            (1..=3).contains(&self.max_running),
            "{}.concurrency.max_running must be between 1 and 3",
            task_path
        );
        if self.policy == ConcurrencyPolicy::Forbid {
            ensure!(
                self.max_running == 1,
                "{}.concurrency.max_running must be 1 when policy is forbid",
                task_path
            );
        }
        if self.policy == ConcurrencyPolicy::Replace {
            ensure!(
                self.max_running == 1,
                "{}.concurrency.max_running must be 1 when policy is replace",
                task_path
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConcurrencyPolicy {
    Allow,
    #[default]
    Forbid,
    Replace,
}

fn default_max_running() -> u8 {
    1
}

fn default_retry_delay_seconds() -> u64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ScheduleConfig {
    Cron {
        expr: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
    },
    Interval {
        seconds: u64,
    },
}

impl ScheduleConfig {
    pub fn validate(&self, task_path: &str) -> Result<()> {
        match self {
            Self::Cron { expr, timezone } => {
                ensure!(
                    !expr.trim().is_empty(),
                    "{}.schedule.expr must not be empty",
                    task_path
                );
                Schedule::from_str(expr).with_context(|| {
                    format!(
                        "{}.schedule.expr has invalid cron expression '{expr}'",
                        task_path
                    )
                })?;
                if let Some(timezone) = timezone {
                    Tz::from_str(timezone).with_context(|| {
                        format!(
                            "{}.schedule.timezone has invalid timezone '{timezone}'",
                            task_path
                        )
                    })?;
                }
            }
            Self::Interval { seconds } => {
                ensure!(*seconds > 0, "{}.schedule.seconds must be > 0", task_path);
            }
        }
        Ok(())
    }

    pub fn summary(&self) -> String {
        match self {
            Self::Cron { expr, timezone } => match timezone {
                Some(timezone) => format!("cron({expr}, tz={timezone})"),
                None => format!("cron({expr})"),
            },
            Self::Interval { seconds } => format!("interval({seconds}s)"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandConfig {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl CommandConfig {
    pub fn validate(&self, command_path: &str) -> Result<()> {
        ensure!(
            !self.program.trim().is_empty(),
            "{}.program must not be empty",
            command_path
        );
        if let Some(timeout_seconds) = self.timeout_seconds {
            ensure!(
                timeout_seconds > 0,
                "{}.timeout_seconds must be > 0",
                command_path
            );
        }
        if let Some(workdir) = &self.workdir {
            ensure!(
                workdir.exists(),
                "{}.workdir '{}' does not exist",
                command_path,
                workdir.display()
            );
            ensure!(
                workdir.is_dir(),
                "{}.workdir '{}' is not a directory",
                command_path,
                workdir.display()
            );
        }
        Ok(())
    }
}

fn validate_optional_dir(path: &Option<PathBuf>, field: &str) -> Result<()> {
    if let Some(path) = path {
        ensure!(!path.as_os_str().is_empty(), "{} must not be empty", field);
        if path.exists() {
            ensure!(
                path.is_dir(),
                "{} '{}' is not a directory",
                field,
                path.display()
            );
        }
    }
    Ok(())
}

fn is_valid_task_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn is_missing_file_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|io_error| io_error.kind() == std::io::ErrorKind::NotFound)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{
        AppConfig, CommandConfig, ConcurrencyConfig, ConcurrencyPolicy, NotificationsConfig,
        NotifyResultSourceConfig, PiRendererConfig, RetryConfig, ScheduleConfig, TaskConfig,
        TaskNotifyConfig, WebhookConfig,
    };

    #[test]
    fn parses_and_validates_config() {
        let yaml = r#"
version: 1
tasks:
  - id: backup-db
    name: backup database
    enabled: true
    concurrency:
      max_running: 1
    schedule:
      kind: cron
      expr: "0 0 2 * * *"
      timezone: Asia/Shanghai
    command:
      program: /bin/echo
      args:
        - ok
  - id: health-check
    name: health check
    enabled: false
    concurrency:
      policy: allow
      max_running: 2
    schedule:
      kind: interval
      seconds: 300
    command:
      program: /bin/echo
"#;

        let config: AppConfig = serde_yaml::from_str(yaml).expect("valid yaml");
        config.validate().expect("config should validate");
        assert_eq!(config.tasks.len(), 2);
    }

    #[test]
    fn rejects_duplicate_ids() {
        let config = AppConfig {
            version: 1,
            notifications: None,
            tasks: vec![sample_task("job"), sample_task("job")],
        };

        let err = config.validate().expect_err("duplicate ids should fail");
        assert!(
            err.to_string()
                .contains("tasks[1].id duplicates task id 'job' already used by tasks[0].id")
        );
    }

    #[test]
    fn rejects_invalid_interval() {
        let config = AppConfig {
            version: 1,
            notifications: None,
            tasks: vec![TaskConfig {
                schedule: ScheduleConfig::Interval { seconds: 0 },
                ..sample_task("job")
            }],
        };

        let err = config.validate().expect_err("invalid interval should fail");
        assert!(
            err.to_string()
                .contains("tasks[0].schedule.seconds must be > 0")
        );
    }

    #[test]
    fn rejects_invalid_cron() {
        let config = AppConfig {
            version: 1,
            notifications: None,
            tasks: vec![TaskConfig {
                schedule: ScheduleConfig::Cron {
                    expr: "not-a-cron".to_string(),
                    timezone: None,
                },
                ..sample_task("job")
            }],
        };

        let err = config.validate().expect_err("invalid cron should fail");
        assert!(err.to_string().contains("invalid cron expression"));
    }

    #[test]
    fn rejects_missing_workdir() {
        let config = AppConfig {
            version: 1,
            notifications: None,
            tasks: vec![TaskConfig {
                command: CommandConfig {
                    workdir: Some(PathBuf::from("/definitely/missing")),
                    ..sample_task("job").command
                },
                ..sample_task("job")
            }],
        };

        let err = config.validate().expect_err("missing workdir should fail");
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn rejects_invalid_timeout() {
        let config = AppConfig {
            version: 1,
            notifications: None,
            tasks: vec![TaskConfig {
                command: CommandConfig {
                    timeout_seconds: Some(0),
                    ..sample_task("job").command
                },
                ..sample_task("job")
            }],
        };

        let err = config.validate().expect_err("invalid timeout should fail");
        assert!(
            err.to_string()
                .contains("tasks[0].command.timeout_seconds must be > 0")
        );
    }

    #[test]
    fn rejects_invalid_concurrency_limit() {
        let config = AppConfig {
            version: 1,
            notifications: None,
            tasks: vec![TaskConfig {
                concurrency: ConcurrencyConfig {
                    policy: ConcurrencyPolicy::Allow,
                    max_running: 4,
                },
                ..sample_task("job")
            }],
        };

        let err = config
            .validate()
            .expect_err("invalid concurrency should fail");
        assert!(
            err.to_string()
                .contains("concurrency.max_running must be between 1 and 3")
        );
    }

    #[test]
    fn rejects_retry_without_delay() {
        let config = AppConfig {
            version: 1,
            notifications: None,
            tasks: vec![TaskConfig {
                retry: RetryConfig {
                    max_attempts: 2,
                    delay_seconds: 0,
                },
                ..sample_task("job")
            }],
        };

        let err = config.validate().expect_err("retry delay should fail");
        assert!(
            err.to_string()
                .contains("tasks[0].retry.delay_seconds must be > 0 when retry.max_attempts > 0")
        );
    }

    #[test]
    fn rejects_forbid_with_multiple_slots() {
        let config = AppConfig {
            version: 1,
            notifications: None,
            tasks: vec![TaskConfig {
                concurrency: ConcurrencyConfig {
                    policy: ConcurrencyPolicy::Forbid,
                    max_running: 2,
                },
                ..sample_task("job")
            }],
        };

        let err = config
            .validate()
            .expect_err("forbid should require one slot");

        assert!(
            err.to_string()
                .contains("concurrency.max_running must be 1 when policy is forbid")
        );
    }

    #[test]
    fn rejects_replace_with_multiple_slots() {
        let config = AppConfig {
            version: 1,
            notifications: None,
            tasks: vec![TaskConfig {
                concurrency: ConcurrencyConfig {
                    policy: ConcurrencyPolicy::Replace,
                    max_running: 2,
                },
                ..sample_task("job")
            }],
        };

        let err = config
            .validate()
            .expect_err("replace should require one slot");

        assert!(
            err.to_string()
                .contains("concurrency.max_running must be 1 when policy is replace")
        );
    }

    #[test]
    fn writes_and_reads_yaml() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("tasks.yaml");
        let config = AppConfig {
            version: 1,
            notifications: None,
            tasks: vec![sample_task("job")],
        };

        config.write(&path).expect("write config");
        let reloaded = AppConfig::load(&path).expect("read config");

        assert_eq!(reloaded, config);
    }

    #[test]
    fn load_or_default_uses_empty_config_when_missing() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("missing.yaml");
        let config = AppConfig::load_or_default(&path).expect("load default");

        assert_eq!(config, AppConfig::default());
    }

    #[test]
    fn accepts_existing_workdir() {
        let dir = tempdir().expect("tempdir");
        let workdir = dir.path().join("work");
        fs::create_dir(&workdir).expect("create workdir");

        let config = AppConfig {
            version: 1,
            notifications: None,
            tasks: vec![TaskConfig {
                command: CommandConfig {
                    workdir: Some(workdir),
                    ..sample_task("job").command
                },
                ..sample_task("job")
            }],
        };

        config.validate().expect("config should validate");
    }

    #[test]
    fn rejects_notify_without_top_level_notifications() {
        let config = AppConfig {
            version: 1,
            notifications: None,
            tasks: vec![TaskConfig {
                notify: Some(TaskNotifyConfig {
                    result_source: NotifyResultSourceConfig::Stdout,
                }),
                ..sample_task("job")
            }],
        };

        let err = config
            .validate()
            .expect_err("notify should require global config");
        assert!(
            err.to_string()
                .contains("tasks[0].notify requires top-level notifications to be configured")
        );
    }

    #[test]
    fn rejects_notification_renderer_without_workdir() {
        let config = AppConfig {
            version: 1,
            notifications: Some(NotificationsConfig {
                enabled: true,
                renderer: Some(PiRendererConfig {
                    program: "/usr/bin/pi".to_string(),
                    workdir: PathBuf::from("/definitely/missing"),
                    prompt: "summarize".to_string(),
                    timeout_seconds: None,
                    session_dir: None,
                    agent_dir: None,
                    provider: None,
                    model: None,
                    env: Default::default(),
                }),
                webhook: Some(WebhookConfig {
                    url_env: "TASKD_WEBHOOK_URL".to_string(),
                }),
            }),
            tasks: vec![sample_task("job")],
        };

        let err = config
            .validate()
            .expect_err("missing renderer workdir should fail");
        assert!(
            err.to_string()
                .contains("notifications.renderer.workdir '/definitely/missing' does not exist")
        );
    }

    #[test]
    fn disabled_notifications_allow_missing_renderer_and_webhook() {
        let config = AppConfig {
            version: 1,
            notifications: Some(NotificationsConfig {
                enabled: false,
                renderer: None,
                webhook: None,
            }),
            tasks: vec![sample_task("job")],
        };

        config
            .validate()
            .expect("disabled notifications should validate");
    }

    #[test]
    fn rejects_notify_when_notifications_are_disabled() {
        let config = AppConfig {
            version: 1,
            notifications: Some(NotificationsConfig {
                enabled: false,
                renderer: None,
                webhook: None,
            }),
            tasks: vec![TaskConfig {
                notify: Some(TaskNotifyConfig {
                    result_source: NotifyResultSourceConfig::Stdout,
                }),
                ..sample_task("job")
            }],
        };

        let err = config
            .validate()
            .expect_err("notify should require enabled notifications");
        assert!(
            err.to_string()
                .contains("tasks[0].notify requires top-level notifications.enabled to be true")
        );
    }

    fn sample_task(id: &str) -> TaskConfig {
        TaskConfig {
            id: id.to_string(),
            name: "sample".to_string(),
            enabled: true,
            concurrency: ConcurrencyConfig::default(),
            retry: RetryConfig::default(),
            schedule: ScheduleConfig::Interval { seconds: 60 },
            notify: None,
            command: CommandConfig {
                program: "/bin/echo".to_string(),
                args: vec!["ok".to_string()],
                workdir: None,
                timeout_seconds: None,
                env: Default::default(),
            },
        }
    }
}
