//! Shared library entrypoints for the `taskd` daemon and `taskctl` control plane.

pub mod cli;
pub mod config;
pub mod config_path;
pub mod daemon_cli;
pub mod history;
pub mod runtime_paths;
pub mod scheduler;
pub mod state;
pub mod task_runner;

use std::process;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use serde::Serialize;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command, ConcurrencyPolicyArg};
use crate::config::{
    AppConfig, CommandConfig, ConcurrencyConfig, ConcurrencyPolicy, PipelineConfig, RetryConfig,
    ScheduleConfig, TaskConfig,
};
use crate::daemon_cli::{TaskdCli, TaskdCommand};
use crate::history::{HistoryRecord, HistoryStore};
use crate::state::{
    RuntimeStateStore, TaskRuntimeState, load_runtime_state, state_path_for_config,
};
use crate::task_runner::{TaskOutcome, TaskRunStatus, TaskStepResult};

pub async fn run_taskd() -> i32 {
    if let Err(error) = init_tracing() {
        eprintln!("failed to initialize logging: {error:#}");
        return 1;
    }

    match try_run_taskd().await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            1
        }
    }
}

pub async fn run_taskctl() -> i32 {
    if let Err(error) = init_tracing() {
        eprintln!("failed to initialize logging: {error:#}");
        return 1;
    }

    match try_run_taskctl().await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            1
        }
    }
}

async fn try_run_taskd() -> Result<i32> {
    let cli = TaskdCli::parse();

    match cli.command {
        TaskdCommand::Daemon => {
            let app = AppConfig::load(&cli.config)?;
            app.validate()?;
            let state_store =
                std::sync::Arc::new(RuntimeStateStore::from_config_path(&cli.config)?);
            let history_store = std::sync::Arc::new(HistoryStore::from_config_path(&cli.config)?);
            scheduler::run_daemon(cli.config, app, state_store, history_store).await?;
            Ok(0)
        }
    }
}

async fn try_run_taskctl() -> Result<i32> {
    let cli = Cli::parse();

    match cli.command {
        Command::List => {
            let app = AppConfig::load(&cli.config)?;
            app.validate()?;
            let state = load_runtime_state(&state_path_for_config(&cli.config))?;
            if cli.json {
                emit_json(&ListOutput {
                    tasks: app
                        .tasks
                        .iter()
                        .map(|task| TaskListItem::from_task(task, state.tasks.get(&task.id)))
                        .collect(),
                })?;
            } else {
                print_tasks(&app, &state);
            }
            Ok(0)
        }
        Command::Show { id } => {
            let app = AppConfig::load(&cli.config)?;
            app.validate()?;
            let task = app
                .task(&id)
                .with_context(|| format!("task '{id}' not found"))?;
            let state = load_runtime_state(&state_path_for_config(&cli.config))?;
            let runtime = state.tasks.get(&id);
            let latest_history = HistoryStore::for_read_only(&cli.config)
                .list_task_history(&id, 1)?
                .into_iter()
                .next();
            if cli.json {
                emit_json(&ShowOutput {
                    task: task.clone(),
                    runtime_state: runtime.cloned(),
                    latest_history,
                })?;
            } else {
                print_task_detail(task, runtime, latest_history.as_ref());
            }
            Ok(0)
        }
        Command::Validate => {
            let app = AppConfig::load(&cli.config)?;
            app.validate()?;
            if cli.json {
                emit_json(&ValidateOutput {
                    ok: true,
                    message: "config is valid",
                    task_count: app.tasks.len(),
                })?;
            } else {
                println!("config is valid");
            }
            Ok(0)
        }
        Command::AddCron {
            id,
            name,
            expr,
            program,
            args,
            timezone,
            enabled,
            max_running,
            concurrency_policy,
            workdir,
            timeout_seconds,
            retry_max_attempts,
            retry_delay_seconds,
            env,
        } => {
            let mut app = AppConfig::load_or_default(&cli.config)?;
            app.add_task(TaskConfig {
                id,
                name,
                enabled,
                concurrency: ConcurrencyConfig {
                    policy: concurrency_policy.into(),
                    max_running,
                },
                retry: RetryConfig {
                    max_attempts: retry_max_attempts,
                    delay_seconds: retry_delay_seconds,
                },
                schedule: ScheduleConfig::Cron { expr, timezone },
                command: Some(CommandConfig {
                    program,
                    args,
                    workdir,
                    timeout_seconds,
                    env: env.into_iter().collect(),
                }),
                pipeline: None,
            })?;
            app.validate()?;
            app.write(&cli.config)?;
            Ok(0)
        }
        Command::AddInterval {
            id,
            name,
            seconds,
            program,
            args,
            enabled,
            max_running,
            concurrency_policy,
            workdir,
            timeout_seconds,
            retry_max_attempts,
            retry_delay_seconds,
            env,
        } => {
            let mut app = AppConfig::load_or_default(&cli.config)?;
            app.add_task(TaskConfig {
                id,
                name,
                enabled,
                concurrency: ConcurrencyConfig {
                    policy: concurrency_policy.into(),
                    max_running,
                },
                retry: RetryConfig {
                    max_attempts: retry_max_attempts,
                    delay_seconds: retry_delay_seconds,
                },
                schedule: ScheduleConfig::Interval { seconds },
                command: Some(CommandConfig {
                    program,
                    args,
                    workdir,
                    timeout_seconds,
                    env: env.into_iter().collect(),
                }),
                pipeline: None,
            })?;
            app.validate()?;
            app.write(&cli.config)?;
            Ok(0)
        }
        Command::Remove { id } => {
            let mut app = AppConfig::load(&cli.config)?;
            let removed = app.remove_task(&id)?;
            app.validate()?;
            app.write(&cli.config)?;
            let _ = RuntimeStateStore::from_config_path(&cli.config)?.remove_task(&removed);
            if cli.json {
                emit_json(&MessageOutput {
                    ok: true,
                    message: format!("removed task {removed}"),
                    task_id: Some(removed),
                })?;
            } else {
                println!("removed task {removed}");
            }
            Ok(0)
        }
        Command::Enable { id } => {
            let mut app = AppConfig::load(&cli.config)?;
            app.set_enabled(&id, true)?;
            app.validate()?;
            app.write(&cli.config)?;
            if cli.json {
                emit_json(&MessageOutput {
                    ok: true,
                    message: format!("enabled task {id}"),
                    task_id: Some(id),
                })?;
            } else {
                println!("enabled task {id}");
            }
            Ok(0)
        }
        Command::Disable { id } => {
            let mut app = AppConfig::load(&cli.config)?;
            app.set_enabled(&id, false)?;
            app.validate()?;
            app.write(&cli.config)?;
            if cli.json {
                emit_json(&MessageOutput {
                    ok: true,
                    message: format!("disabled task {id}"),
                    task_id: Some(id),
                })?;
            } else {
                println!("disabled task {id}");
            }
            Ok(0)
        }
        Command::History { id, limit } => {
            let rows = HistoryStore::for_read_only(&cli.config).list_task_history(&id, limit)?;
            if cli.json {
                emit_json(&HistoryOutput { records: rows })?;
            } else {
                print_history(&rows);
            }
            Ok(0)
        }
        Command::RecentFailures { limit } => {
            let rows = HistoryStore::for_read_only(&cli.config).list_recent_failures(limit)?;
            if cli.json {
                emit_json(&HistoryOutput { records: rows })?;
            } else {
                print_history(&rows);
            }
            Ok(0)
        }
        Command::RunNow { id } => {
            let app = AppConfig::load(&cli.config)?;
            app.validate()?;
            let task = app
                .task(&id)
                .with_context(|| format!("task '{id}' not found"))?
                .clone();
            let outcome = match task_runner::run_task_guarded(Arc::new(task.clone())).await {
                Ok(outcome) => outcome,
                Err(error) => TaskOutcome::panic(&task.id, &error.to_string()),
            };
            RuntimeStateStore::from_config_path(&cli.config)?.record(&task.id, &outcome)?;
            HistoryStore::from_config_path(&cli.config)?.record(&task.id, &outcome)?;
            if cli.json {
                emit_json(&RunNowOutput {
                    task_id: task.id.clone(),
                    outcome: TaskOutcomeOutput::from_outcome(&outcome),
                })?;
            }
            Ok(outcome.exit_code())
        }
    }
}

fn init_tracing() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .without_time()
        .try_init()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(())
}

fn print_tasks(app: &AppConfig, state: &crate::state::RuntimeStateFile) {
    if app.tasks.is_empty() {
        println!("no tasks configured");
        return;
    }

    println!(
        "{:<20} {:<8} {:<24} {:<14} {:<20} schedule",
        "id", "status", "name", "last_status", "last_run"
    );
    for task in &app.tasks {
        let runtime = state.tasks.get(&task.id);
        println!(
            "{:<20} {:<8} {:<24} {:<14} {:<20} {}",
            task.id,
            if task.enabled { "enabled" } else { "disabled" },
            task.name,
            runtime
                .map(|value| format!("{:?}", value.last_status).to_lowercase())
                .unwrap_or_else(|| "-".to_string()),
            runtime
                .map(|value| value
                    .last_finished_at
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string())
                .unwrap_or_else(|| "-".to_string()),
            task.schedule.summary()
        );
    }
}

fn print_history(rows: &[HistoryRecord]) {
    if rows.is_empty() {
        println!("no history records");
        return;
    }

    println!(
        "{:<6} {:<20} {:<12} {:<20} summary",
        "id", "task_id", "status", "finished_at"
    );
    for row in rows {
        println!(
            "{:<6} {:<20} {:<12} {:<20} {}",
            row.id,
            row.task_id,
            row.status,
            row.finished_at.format("%Y-%m-%d %H:%M:%S"),
            if row.step_details.is_empty() {
                row.summary.clone()
            } else {
                format!(
                    "{} | steps: {}",
                    row.summary,
                    format_step_results(&row.step_details)
                )
            }
        );
    }
}

fn print_task_detail(
    task: &TaskConfig,
    runtime: Option<&TaskRuntimeState>,
    latest_history: Option<&HistoryRecord>,
) {
    println!("id: {}", task.id);
    println!("name: {}", task.name);
    println!("enabled: {}", task.enabled);
    println!(
        "concurrency: {} (max_running={})",
        format_concurrency_policy(task.concurrency.policy),
        task.concurrency.max_running
    );
    println!(
        "retry: max_attempts={}, delay_seconds={}",
        task.retry.max_attempts, task.retry.delay_seconds
    );
    println!("schedule: {}", task.schedule.summary());
    match (&task.command, &task.pipeline) {
        (Some(command), None) => print_command_detail("command", command),
        (None, Some(pipeline)) => print_pipeline_detail(pipeline),
        _ => println!("execution: invalid"),
    }

    match runtime {
        Some(runtime) => {
            println!(
                "last_status: {}",
                format!("{:?}", runtime.last_status).to_lowercase()
            );
            println!("last_summary: {}", runtime.last_summary);
            println!(
                "last_started_at: {}",
                runtime.last_started_at.format("%Y-%m-%d %H:%M:%S")
            );
            println!(
                "last_finished_at: {}",
                runtime.last_finished_at.format("%Y-%m-%d %H:%M:%S")
            );
            if runtime.last_steps.is_empty() {
                println!("last_steps: -");
            } else {
                println!("last_steps: {}", format_step_results(&runtime.last_steps));
            }
        }
        None => {
            println!("last_status: -");
            println!("last_summary: -");
            println!("last_started_at: -");
            println!("last_finished_at: -");
            println!("last_steps: -");
        }
    }

    match latest_history {
        Some(history) => {
            println!("latest_history_status: {}", history.status);
            println!("latest_history_summary: {}", history.summary);
            println!(
                "latest_history_finished_at: {}",
                history.finished_at.format("%Y-%m-%d %H:%M:%S")
            );
            println!("latest_history_exit_code: {}", history.exit_code);
            if history.step_details.is_empty() {
                println!("latest_history_steps: -");
            } else {
                println!(
                    "latest_history_steps: {}",
                    format_step_results(&history.step_details)
                );
            }
        }
        None => {
            println!("latest_history_status: -");
            println!("latest_history_summary: -");
            println!("latest_history_finished_at: -");
            println!("latest_history_exit_code: -");
            println!("latest_history_steps: -");
        }
    }
}

fn print_command_detail(label: &str, command: &CommandConfig) {
    println!("{label}.program: {}", command.program);
    println!("{label}.args: {:?}", command.args);
    println!(
        "{label}.workdir: {}",
        command
            .workdir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!(
        "{label}.timeout_seconds: {}",
        command
            .timeout_seconds
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    if command.env.is_empty() {
        println!("{label}.env: -");
    } else {
        let env = command
            .env
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("{label}.env: {env}");
    }
}

fn print_pipeline_detail(pipeline: &PipelineConfig) {
    println!("pipeline.steps: {}", pipeline.steps.len());
    for step in &pipeline.steps {
        println!("pipeline.step.id: {}", step.id);
        print_command_detail(&format!("pipeline.step.{}", step.id), &step.command);
    }
}

fn format_step_results(steps: &[crate::task_runner::TaskStepResult]) -> String {
    steps
        .iter()
        .map(|step| {
            format!(
                "{}={} ({})",
                step.step_id,
                format!("{:?}", step.status).to_lowercase(),
                step.summary
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn format_concurrency_policy(policy: ConcurrencyPolicy) -> &'static str {
    match policy {
        ConcurrencyPolicy::Allow => "allow",
        ConcurrencyPolicy::Forbid => "forbid",
        ConcurrencyPolicy::Replace => "replace",
    }
}

fn emit_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[derive(Serialize)]
struct ValidateOutput {
    ok: bool,
    message: &'static str,
    task_count: usize,
}

#[derive(Serialize)]
struct MessageOutput {
    ok: bool,
    message: String,
    task_id: Option<String>,
}

#[derive(Serialize)]
struct ListOutput {
    tasks: Vec<TaskListItem>,
}

#[derive(Serialize)]
struct TaskListItem {
    id: String,
    name: String,
    enabled: bool,
    concurrency_policy: &'static str,
    max_running: u8,
    schedule: String,
    last_status: Option<TaskRunStatus>,
    last_summary: Option<String>,
    last_started_at: Option<DateTime<Utc>>,
    last_finished_at: Option<DateTime<Utc>>,
}

impl TaskListItem {
    fn from_task(task: &TaskConfig, runtime: Option<&TaskRuntimeState>) -> Self {
        Self {
            id: task.id.clone(),
            name: task.name.clone(),
            enabled: task.enabled,
            concurrency_policy: format_concurrency_policy(task.concurrency.policy),
            max_running: task.concurrency.max_running,
            schedule: task.schedule.summary(),
            last_status: runtime.map(|value| value.last_status),
            last_summary: runtime.map(|value| value.last_summary.clone()),
            last_started_at: runtime.map(|value| value.last_started_at),
            last_finished_at: runtime.map(|value| value.last_finished_at),
        }
    }
}

#[derive(Serialize)]
struct ShowOutput {
    task: TaskConfig,
    runtime_state: Option<TaskRuntimeState>,
    latest_history: Option<HistoryRecord>,
}

#[derive(Serialize)]
struct HistoryOutput {
    records: Vec<HistoryRecord>,
}

#[derive(Serialize)]
struct RunNowOutput {
    task_id: String,
    outcome: TaskOutcomeOutput,
}

#[derive(Serialize)]
struct TaskOutcomeOutput {
    status: TaskRunStatus,
    summary: String,
    exit_code: i32,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    steps: Vec<TaskStepResult>,
}

impl TaskOutcomeOutput {
    fn from_outcome(outcome: &TaskOutcome) -> Self {
        Self {
            status: outcome.status(),
            summary: outcome.summary().to_string(),
            exit_code: outcome.exit_code(),
            started_at: outcome.started_at(),
            finished_at: outcome.finished_at(),
            steps: outcome.steps().to_vec(),
        }
    }
}

pub fn exit(code: i32) -> ! {
    process::exit(code)
}

impl From<ConcurrencyPolicyArg> for ConcurrencyPolicy {
    fn from(value: ConcurrencyPolicyArg) -> Self {
        match value {
            ConcurrencyPolicyArg::Allow => ConcurrencyPolicy::Allow,
            ConcurrencyPolicyArg::Forbid => ConcurrencyPolicy::Forbid,
            ConcurrencyPolicyArg::Replace => ConcurrencyPolicy::Replace,
        }
    }
}
