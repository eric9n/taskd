use std::time::Duration;

use anyhow::{Result, anyhow};
use chrono_tz::Tz;
use tokio::signal;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::info;

use crate::config::{AppConfig, ScheduleConfig, TaskConfig};
use crate::task_runner;

pub async fn run_daemon(config: AppConfig) -> Result<()> {
    let mut scheduler = JobScheduler::new().await?;
    let registered = register_tasks(&scheduler, &config).await?;

    info!(registered, "starting scheduler");
    scheduler.start().await?;
    info!("scheduler started, waiting for shutdown signal");
    signal::ctrl_c().await?;
    info!("shutdown signal received");
    let _ = scheduler.shutdown().await;
    Ok(())
}

pub async fn register_tasks(scheduler: &JobScheduler, config: &AppConfig) -> Result<usize> {
    let mut registered = 0;
    for task in &config.tasks {
        if !task.enabled {
            continue;
        }
        let job = job_for_task(task.clone())?;
        scheduler.add(job).await?;
        registered += 1;
    }
    Ok(registered)
}

fn job_for_task(task: TaskConfig) -> Result<Job> {
    match task.schedule.clone() {
        ScheduleConfig::Cron { expr, timezone } => {
            let task = std::sync::Arc::new(task);
            match timezone {
                Some(timezone) => Ok(Job::new_async_tz(
                    expr.as_str(),
                    timezone.parse::<Tz>()?,
                    move |_id, _lock| Box::pin(execute_scheduled_task(task.clone())),
                )?),
                None => Ok(Job::new_async(expr.as_str(), move |_id, _lock| {
                    Box::pin(execute_scheduled_task(task.clone()))
                })?),
            }
        }
        ScheduleConfig::Interval { seconds } => {
            let task = std::sync::Arc::new(task);
            Ok(Job::new_repeated_async(
                Duration::from_secs(seconds),
                move |_id, _lock| Box::pin(execute_scheduled_task(task.clone())),
            )?)
        }
    }
}

async fn execute_scheduled_task(task: std::sync::Arc<TaskConfig>) {
    if let Err(error) = task_runner::run_task_or_error(task.as_ref()).await {
        tracing::error!(task_id = %task.id, error = %error, "scheduled task failed");
    }
}

pub fn enabled_task_count(config: &AppConfig) -> usize {
    config.tasks.iter().filter(|task| task.enabled).count()
}

pub fn ensure_enabled_tasks(config: &AppConfig) -> Result<()> {
    let count = enabled_task_count(config);
    if count == 0 {
        return Err(anyhow!("no enabled tasks configured"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tokio_cron_scheduler::JobScheduler;

    use super::{enabled_task_count, register_tasks};
    use crate::config::{AppConfig, CommandConfig, ScheduleConfig, TaskConfig};

    #[test]
    fn counts_only_enabled_tasks() {
        let config = AppConfig {
            version: 1,
            tasks: vec![
                sample_task("job-1", true, ScheduleConfig::Interval { seconds: 30 }),
                sample_task("job-2", false, ScheduleConfig::Interval { seconds: 30 }),
            ],
        };

        assert_eq!(enabled_task_count(&config), 1);
    }

    #[tokio::test]
    async fn registers_only_enabled_tasks() {
        let scheduler = JobScheduler::new().await.expect("scheduler");
        let config = AppConfig {
            version: 1,
            tasks: vec![
                sample_task("job-1", true, ScheduleConfig::Interval { seconds: 30 }),
                sample_task("job-2", false, ScheduleConfig::Interval { seconds: 30 }),
            ],
        };

        let count = register_tasks(&scheduler, &config)
            .await
            .expect("register tasks");

        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn registers_cron_tasks() {
        let scheduler = JobScheduler::new().await.expect("scheduler");
        let config = AppConfig {
            version: 1,
            tasks: vec![sample_task(
                "job-1",
                true,
                ScheduleConfig::Cron {
                    expr: "0/30 * * * * *".to_string(),
                    timezone: None,
                },
            )],
        };

        let count = register_tasks(&scheduler, &config)
            .await
            .expect("register tasks");

        assert_eq!(count, 1);
    }

    fn sample_task(id: &str, enabled: bool, schedule: ScheduleConfig) -> TaskConfig {
        TaskConfig {
            id: id.to_string(),
            name: id.to_string(),
            enabled,
            schedule,
            command: CommandConfig {
                program: "/bin/echo".to_string(),
                args: vec!["ok".to_string()],
                workdir: None,
                env: BTreeMap::new(),
            },
        }
    }
}
