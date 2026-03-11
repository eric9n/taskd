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
    #[serde(default)]
    pub tasks: Vec<TaskConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: 1,
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

        let mut seen = HashMap::new();
        for (index, task) in self.tasks.iter().enumerate() {
            let task_path = format!("tasks[{index}]");
            task.validate(&task_path)?;
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
    pub command: Option<CommandConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<PipelineConfig>,
}

impl TaskConfig {
    pub fn validate(&self, task_path: &str) -> Result<()> {
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
        match (&self.command, &self.pipeline) {
            (Some(command), None) => command.validate(&format!("{task_path}.command"))?,
            (None, Some(pipeline)) => pipeline.validate(task_path)?,
            (Some(_), Some(_)) => bail!(
                "{}.command and {}.pipeline are mutually exclusive",
                task_path,
                task_path
            ),
            (None, None) => bail!(
                "{}.command or {}.pipeline must be set",
                task_path,
                task_path
            ),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipelineConfig {
    pub steps: Vec<PipelineStepConfig>,
}

impl PipelineConfig {
    pub fn validate(&self, task_path: &str) -> Result<()> {
        ensure!(
            (2..=3).contains(&self.steps.len()),
            "{}.pipeline.steps must contain between 2 and 3 steps",
            task_path
        );

        let mut seen = HashMap::new();
        for (index, step) in self.steps.iter().enumerate() {
            let step_path = format!("{task_path}.pipeline.steps[{index}]");
            step.validate(&step_path)?;
            if let Some(previous_index) = seen.insert(step.id.clone(), index) {
                bail!(
                    "{}.id duplicates step id '{}' already used by {}.pipeline.steps[{}].id",
                    step_path,
                    step.id,
                    task_path,
                    previous_index
                );
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipelineStepConfig {
    pub id: String,
    pub command: CommandConfig,
}

impl PipelineStepConfig {
    pub fn validate(&self, step_path: &str) -> Result<()> {
        ensure!(
            is_valid_task_id(&self.id),
            "{}.id must use only letters, digits, '-', '_' or '.'",
            step_path
        );
        self.command.validate(&format!("{step_path}.command"))?;
        Ok(())
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
        AppConfig, CommandConfig, ConcurrencyConfig, ConcurrencyPolicy, PipelineConfig,
        PipelineStepConfig, RetryConfig, ScheduleConfig, TaskConfig,
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
            tasks: vec![TaskConfig {
                command: Some(CommandConfig {
                    workdir: Some(PathBuf::from("/definitely/missing")),
                    ..sample_task("job").command.expect("command")
                }),
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
            tasks: vec![TaskConfig {
                command: Some(CommandConfig {
                    timeout_seconds: Some(0),
                    ..sample_task("job").command.expect("command")
                }),
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
            tasks: vec![TaskConfig {
                command: Some(CommandConfig {
                    workdir: Some(workdir),
                    ..sample_task("job").command.expect("command")
                }),
                ..sample_task("job")
            }],
        };

        config.validate().expect("config should validate");
    }

    #[test]
    fn validates_pipeline_with_three_steps() {
        let config = AppConfig {
            version: 1,
            tasks: vec![TaskConfig {
                command: None,
                pipeline: Some(PipelineConfig {
                    steps: vec![
                        sample_step("step-1"),
                        sample_step("step-2"),
                        sample_step("step-3"),
                    ],
                }),
                ..sample_task("pipeline-job")
            }],
        };

        config.validate().expect("pipeline should validate");
    }

    #[test]
    fn rejects_pipeline_with_more_than_three_steps() {
        let config = AppConfig {
            version: 1,
            tasks: vec![TaskConfig {
                command: None,
                pipeline: Some(PipelineConfig {
                    steps: vec![
                        sample_step("step-1"),
                        sample_step("step-2"),
                        sample_step("step-3"),
                        sample_step("step-4"),
                    ],
                }),
                ..sample_task("pipeline-job")
            }],
        };

        let err = config.validate().expect_err("pipeline size should fail");
        assert!(
            err.to_string()
                .contains("tasks[0].pipeline.steps must contain between 2 and 3 steps")
        );
    }

    #[test]
    fn rejects_task_with_both_command_and_pipeline() {
        let config = AppConfig {
            version: 1,
            tasks: vec![TaskConfig {
                pipeline: Some(PipelineConfig {
                    steps: vec![sample_step("step-1"), sample_step("step-2")],
                }),
                ..sample_task("job")
            }],
        };

        let err = config.validate().expect_err("execution kind should fail");
        assert!(
            err.to_string()
                .contains("tasks[0].command and tasks[0].pipeline are mutually exclusive")
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
            command: Some(CommandConfig {
                program: "/bin/echo".to_string(),
                args: vec!["ok".to_string()],
                workdir: None,
                timeout_seconds: None,
                env: Default::default(),
            }),
            pipeline: None,
        }
    }

    fn sample_step(id: &str) -> PipelineStepConfig {
        PipelineStepConfig {
            id: id.to_string(),
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
