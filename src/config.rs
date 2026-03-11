use std::collections::{BTreeMap, HashSet};
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

        let mut seen = HashSet::new();
        for task in &self.tasks {
            task.validate()?;
            if !seen.insert(task.id.as_str()) {
                bail!("duplicate task id '{}'", task.id);
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
    pub schedule: ScheduleConfig,
    pub command: CommandConfig,
}

impl TaskConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            is_valid_task_id(&self.id),
            "invalid task id '{}': expected letters, digits, '-', '_' or '.'",
            self.id
        );
        ensure!(
            !self.name.trim().is_empty(),
            "task '{}' name must not be empty",
            self.id
        );
        self.schedule.validate(&self.id)?;
        self.command.validate(&self.id)?;
        Ok(())
    }
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
    pub fn validate(&self, task_id: &str) -> Result<()> {
        match self {
            Self::Cron { expr, timezone } => {
                ensure!(
                    !expr.trim().is_empty(),
                    "task '{}' cron expr must not be empty",
                    task_id
                );
                Schedule::from_str(expr).with_context(|| {
                    format!("task '{task_id}' has invalid cron expression '{expr}'")
                })?;
                if let Some(timezone) = timezone {
                    Tz::from_str(timezone).with_context(|| {
                        format!("task '{task_id}' has invalid timezone '{timezone}'")
                    })?;
                }
            }
            Self::Interval { seconds } => {
                ensure!(
                    *seconds > 0,
                    "task '{}' interval seconds must be > 0",
                    task_id
                );
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl CommandConfig {
    pub fn validate(&self, task_id: &str) -> Result<()> {
        ensure!(
            !self.program.trim().is_empty(),
            "task '{}' program must not be empty",
            task_id
        );
        if let Some(workdir) = &self.workdir {
            ensure!(
                workdir.exists(),
                "task '{}' workdir '{}' does not exist",
                task_id,
                workdir.display()
            );
            ensure!(
                workdir.is_dir(),
                "task '{}' workdir '{}' is not a directory",
                task_id,
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

    use super::{AppConfig, CommandConfig, ScheduleConfig, TaskConfig};

    #[test]
    fn parses_and_validates_config() {
        let yaml = r#"
version: 1
tasks:
  - id: backup-db
    name: backup database
    enabled: true
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
        assert!(err.to_string().contains("duplicate task id"));
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
        assert!(err.to_string().contains("interval seconds must be > 0"));
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
                command: CommandConfig {
                    workdir: Some(workdir),
                    ..sample_task("job").command
                },
                ..sample_task("job")
            }],
        };

        config.validate().expect("config should validate");
    }

    fn sample_task(id: &str) -> TaskConfig {
        TaskConfig {
            id: id.to_string(),
            name: "sample".to_string(),
            enabled: true,
            schedule: ScheduleConfig::Interval { seconds: 60 },
            command: CommandConfig {
                program: "/bin/echo".to_string(),
                args: vec!["ok".to_string()],
                workdir: None,
                env: Default::default(),
            },
        }
    }
}
