//! Shared library entrypoints for the `taskd` daemon and `taskctl` control plane.

pub mod cli;
pub mod config;
pub mod config_path;
pub mod daemon_cli;
pub mod history;
pub mod notifications;
pub mod runtime_paths;
pub mod scheduler;
pub mod state;
pub mod task_runner;

use std::path::Path;
use std::process::{self, Command as ProcessCommand, Stdio};
use std::sync::Arc;
use std::{collections::BTreeMap, fs::File};

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use clap::Parser;
use serde::Serialize;
use tracing::warn;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command, ConcurrencyPolicyArg, ReportCommand};
use crate::config::{
    AppConfig, CommandConfig, ConcurrencyConfig, ConcurrencyPolicy, RetryConfig, ScheduleConfig,
    TaskConfig,
};
use crate::daemon_cli::{TaskdCli, TaskdCommand};
use crate::history::{HistoryRecord, HistoryStore};
use crate::notifications::maybe_send_task_notification;
use crate::runtime_paths::last_good_config_path_for_config;
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
            let app = load_valid_daemon_config(&cli.config)?;
            let state_store =
                std::sync::Arc::new(RuntimeStateStore::from_config_path(&cli.config)?);
            let history_store = std::sync::Arc::new(HistoryStore::from_config_path(&cli.config)?);
            scheduler::run_daemon(cli.config, app, state_store, history_store).await?;
            Ok(0)
        }
    }
}

fn load_valid_daemon_config(config_path: &Path) -> Result<AppConfig> {
    match load_and_validate_config(config_path) {
        Ok(config) => {
            persist_last_good_config(config_path, &config)?;
            Ok(config)
        }
        Err(primary_error) => {
            let fallback_path = last_good_config_path_for_config(config_path);
            let fallback_config = load_and_validate_config(&fallback_path).with_context(|| {
                format!(
                    "primary config '{}' is invalid and no valid last-known-good config was found at '{}'",
                    config_path.display(),
                    fallback_path.display()
                )
            })?;
            warn!(
                config = %config_path.display(),
                fallback = %fallback_path.display(),
                error = %primary_error,
                "primary config invalid, continuing with last-known-good config"
            );
            Ok(fallback_config)
        }
    }
}

fn load_and_validate_config(path: &Path) -> Result<AppConfig> {
    let config = AppConfig::load(path)?;
    config.validate()?;
    Ok(config)
}

pub(crate) fn persist_last_good_config(config_path: &Path, config: &AppConfig) -> Result<()> {
    let snapshot_path = last_good_config_path_for_config(config_path);
    config.write(&snapshot_path).with_context(|| {
        format!(
            "failed to persist last-known-good config '{}'",
            snapshot_path.display()
        )
    })
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
                command: CommandConfig {
                    program,
                    args,
                    workdir,
                    timeout_seconds,
                    env: env.into_iter().collect(),
                },
                notify: None,
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
                command: CommandConfig {
                    program,
                    args,
                    workdir,
                    timeout_seconds,
                    env: env.into_iter().collect(),
                },
                notify: None,
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
        Command::Logs { lines, follow } => {
            if cli.json && follow {
                anyhow::bail!("taskctl logs --json does not support --follow");
            }
            if cli.json {
                let output = run_journalctl_capture(lines)?;
                emit_json(&LogsOutput {
                    service: "taskd",
                    lines,
                    output: String::from_utf8_lossy(&output.stdout).into_owned(),
                })?;
                Ok(output.status.code().unwrap_or(1))
            } else {
                run_journalctl_stream(lines, follow)
            }
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
            if let Err(error) =
                maybe_send_task_notification(app.notifications.as_ref(), &task, &outcome).await
            {
                tracing::error!(
                    task_id = %task.id,
                    error = %error,
                    "failed to send task notification"
                );
            }
            if cli.json {
                emit_json(&RunNowOutput {
                    task_id: task.id.clone(),
                    outcome: TaskOutcomeOutput::from_outcome(&outcome),
                })?;
            }
            Ok(outcome.exit_code())
        }
        Command::Report { command } => match command {
            ReportCommand::Daily {
                date,
                timezone,
                output,
            } => {
                let report = build_daily_report_output(&cli.config, &date, &timezone)?;
                if let Some(output) = output {
                    let file = File::create(&output).with_context(|| {
                        format!("failed to create report output '{}'", output.display())
                    })?;
                    serde_json::to_writer_pretty(file, &report).with_context(|| {
                        format!("failed to write report output '{}'", output.display())
                    })?;
                } else if cli.json {
                    emit_json(&report)?;
                } else {
                    print_daily_report(&report);
                }
                Ok(0)
            }
        },
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
    print_command_detail("command", &task.command);

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

fn run_journalctl_capture(lines: usize) -> Result<std::process::Output> {
    let output = ProcessCommand::new("journalctl")
        .args(journalctl_args(lines, false))
        .output()
        .context("failed to execute journalctl for taskd logs")?;
    if !output.status.success() && !output.stderr.is_empty() {
        anyhow::bail!(
            "journalctl returned non-zero status: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output)
}

fn run_journalctl_stream(lines: usize, follow: bool) -> Result<i32> {
    let status = ProcessCommand::new("journalctl")
        .args(journalctl_args(lines, follow))
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to execute journalctl for taskd logs")?;
    Ok(status.code().unwrap_or(1))
}

fn journalctl_args(lines: usize, follow: bool) -> Vec<String> {
    let mut args = vec![
        "-u".to_string(),
        "taskd".to_string(),
        "--no-pager".to_string(),
        "-n".to_string(),
        lines.to_string(),
    ];
    if follow {
        args.push("-f".to_string());
    }
    args
}

fn build_daily_report_output(
    config_path: &Path,
    date: &str,
    timezone: &str,
) -> Result<DailyReportRecordOutput> {
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .with_context(|| format!("invalid report date '{date}', expected YYYY-MM-DD"))?;
    let timezone = timezone
        .parse::<chrono_tz::Tz>()
        .with_context(|| format!("invalid timezone '{timezone}'"))?;
    let start_local = timezone
        .with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0)
        .single()
        .ok_or_else(|| {
            anyhow::anyhow!("failed to resolve start of day for {date} in {timezone}")
        })?;
    let end_local = start_local + chrono::Duration::days(1);
    let start_utc = start_local.with_timezone(&Utc);
    let end_utc = end_local.with_timezone(&Utc);

    let rows = HistoryStore::for_read_only(config_path).list_history_between(start_utc, end_utc)?;

    let mut totals = BTreeMap::new();
    let mut task_summaries = BTreeMap::<String, DailyTaskSummary>::new();
    let mut failures = Vec::new();
    for row in rows {
        *totals.entry(row.status.clone()).or_insert(0) += 1;
        let entry = task_summaries
            .entry(row.task_id.clone())
            .or_insert_with(|| DailyTaskSummary {
                task_id: row.task_id.clone(),
                run_count: 0,
                failure_count: 0,
                last_status: row.status.clone(),
                last_summary: row.summary.clone(),
                last_finished_at: row.finished_at,
            });
        entry.run_count += 1;
        if row.status != "success" {
            entry.failure_count += 1;
            failures.push(DailyFailureSummary::from(&row));
        }
        if row.finished_at > entry.last_finished_at {
            entry.last_status = row.status.clone();
            entry.last_summary = row.summary.clone();
            entry.last_finished_at = row.finished_at;
        }
    }

    let total_runs = totals.values().sum();
    let status = if totals.get("error").copied().unwrap_or(0) > 0 {
        "error"
    } else if total_runs == 0
        || totals
            .iter()
            .any(|(status, count)| status != "success" && *count > 0)
    {
        "warn"
    } else {
        "ok"
    };
    let failure_count = failures.len();
    let summary = if total_runs == 0 {
        format!("no task runs recorded for {date}")
    } else if failure_count == 0 {
        format!("{total_runs} task runs recorded for {date}, all successful")
    } else {
        format!("{total_runs} task runs recorded for {date}, {failure_count} failures")
    };

    let tasks = task_summaries.into_values().collect::<Vec<_>>();
    Ok(DailyReportRecordOutput {
        schema_version: 1,
        status: status.to_string(),
        summary,
        content_type: "application/json".to_string(),
        payload: DailyReportPayload {
            date: date.to_string(),
            timezone: timezone.name().to_string(),
            window_start: start_utc,
            window_end: end_utc,
            totals,
            total_runs,
            tasks,
            failures,
        },
    })
}

fn print_daily_report(report: &DailyReportRecordOutput) {
    println!("status: {}", report.status);
    println!("summary: {}", report.summary);
    println!("date: {}", report.payload.date);
    println!("timezone: {}", report.payload.timezone);
    println!("window_start: {}", report.payload.window_start.to_rfc3339());
    println!("window_end: {}", report.payload.window_end.to_rfc3339());
    println!("total_runs: {}", report.payload.total_runs);
    if report.payload.totals.is_empty() {
        println!("totals: -");
    } else {
        let totals = report
            .payload
            .totals
            .iter()
            .map(|(status, count)| format!("{status}={count}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("totals: {totals}");
    }
    println!("tasks: {}", report.payload.tasks.len());
    println!("failures: {}", report.payload.failures.len());
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
struct LogsOutput {
    service: &'static str,
    lines: usize,
    output: String,
}

#[derive(Serialize)]
struct RunNowOutput {
    task_id: String,
    outcome: TaskOutcomeOutput,
}

#[derive(Serialize)]
struct DailyReportRecordOutput {
    schema_version: u32,
    status: String,
    summary: String,
    content_type: String,
    payload: DailyReportPayload,
}

#[derive(Serialize)]
struct DailyReportPayload {
    date: String,
    timezone: String,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
    totals: BTreeMap<String, usize>,
    total_runs: usize,
    tasks: Vec<DailyTaskSummary>,
    failures: Vec<DailyFailureSummary>,
}

#[derive(Serialize)]
struct DailyTaskSummary {
    task_id: String,
    run_count: usize,
    failure_count: usize,
    last_status: String,
    last_summary: String,
    last_finished_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct DailyFailureSummary {
    task_id: String,
    status: String,
    summary: String,
    exit_code: i32,
    finished_at: DateTime<Utc>,
    step_details: Vec<TaskStepResult>,
}

impl From<&HistoryRecord> for DailyFailureSummary {
    fn from(value: &HistoryRecord) -> Self {
        Self {
            task_id: value.task_id.clone(),
            status: value.status.clone(),
            summary: value.summary.clone(),
            exit_code: value.exit_code,
            finished_at: value.finished_at,
            step_details: value.step_details.clone(),
        }
    }
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

#[cfg(test)]
mod logs_tests {
    use super::journalctl_args;

    #[test]
    fn builds_journalctl_args_without_follow() {
        assert_eq!(
            journalctl_args(50, false),
            vec![
                "-u".to_string(),
                "taskd".to_string(),
                "--no-pager".to_string(),
                "-n".to_string(),
                "50".to_string()
            ]
        );
    }

    #[test]
    fn builds_journalctl_args_with_follow() {
        assert_eq!(
            journalctl_args(20, true),
            vec![
                "-u".to_string(),
                "taskd".to_string(),
                "--no-pager".to_string(),
                "-n".to_string(),
                "20".to_string(),
                "-f".to_string()
            ]
        );
    }
}

#[cfg(test)]
mod daemon_config_tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{load_valid_daemon_config, persist_last_good_config};
    use crate::config::{AppConfig, CommandConfig, ScheduleConfig, TaskConfig};

    fn valid_config() -> AppConfig {
        AppConfig {
            version: 1,
            notifications: None,
            tasks: vec![TaskConfig {
                id: "health-check".to_string(),
                name: "health check".to_string(),
                enabled: true,
                concurrency: Default::default(),
                retry: Default::default(),
                schedule: ScheduleConfig::Interval { seconds: 60 },
                notify: None,
                command: CommandConfig {
                    program: "/bin/echo".to_string(),
                    args: vec!["ok".to_string()],
                    workdir: None,
                    timeout_seconds: None,
                    env: Default::default(),
                },
            }],
        }
    }

    #[test]
    fn loads_primary_config_and_persists_last_good_snapshot() {
        let dir = tempdir().expect("tempdir");
        let config_path = dir.path().join("tasks.yaml");
        let config = valid_config();
        config.write(&config_path).expect("write config");

        let loaded = load_valid_daemon_config(&config_path).expect("load valid config");

        assert_eq!(loaded, config);
        assert!(
            dir.path().join("tasks.last-good.yaml").exists(),
            "expected snapshot to be persisted"
        );
    }

    #[test]
    fn falls_back_to_last_good_snapshot_when_primary_config_is_invalid() {
        let dir = tempdir().expect("tempdir");
        let config_path = dir.path().join("tasks.yaml");
        let snapshot_config = valid_config();
        persist_last_good_config(&config_path, &snapshot_config).expect("persist snapshot");
        fs::write(
            &config_path,
            "version: 1\ntasks:\n  - id: broken\n    name: broken\n",
        )
        .expect("write invalid config");

        let loaded = load_valid_daemon_config(&config_path).expect("fallback config");

        assert_eq!(loaded, snapshot_config);
    }

    #[test]
    fn returns_error_when_primary_config_is_invalid_and_no_snapshot_exists() {
        let dir = tempdir().expect("tempdir");
        let config_path = dir.path().join("tasks.yaml");
        fs::write(&config_path, "version: nope").expect("write invalid config");

        let error = load_valid_daemon_config(&config_path).expect_err("should fail");
        let message = format!("{error:#}");

        assert!(message.contains("primary config"));
        assert!(message.contains("last-known-good config"));
    }
}
