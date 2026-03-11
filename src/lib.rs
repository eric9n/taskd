pub mod cli;
pub mod config;
pub mod scheduler;
pub mod task_runner;

use std::process;

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};
use crate::config::{AppConfig, CommandConfig, ScheduleConfig, TaskConfig};

pub async fn run() -> i32 {
    if let Err(error) = init_tracing() {
        eprintln!("failed to initialize logging: {error:#}");
        return 1;
    }

    match try_run().await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            1
        }
    }
}

async fn try_run() -> Result<i32> {
    let cli = Cli::parse();

    match cli.command {
        Command::Daemon => {
            let app = AppConfig::load(&cli.config)?;
            app.validate()?;
            scheduler::run_daemon(app).await?;
            Ok(0)
        }
        Command::List => {
            let app = AppConfig::load(&cli.config)?;
            app.validate()?;
            print_tasks(&app);
            Ok(0)
        }
        Command::Validate => {
            let app = AppConfig::load(&cli.config)?;
            app.validate()?;
            println!("config is valid");
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
            workdir,
            env,
        } => {
            let mut app = AppConfig::load_or_default(&cli.config)?;
            app.add_task(TaskConfig {
                id,
                name,
                enabled,
                schedule: ScheduleConfig::Cron { expr, timezone },
                command: CommandConfig {
                    program,
                    args,
                    workdir,
                    env: env.into_iter().collect(),
                },
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
            workdir,
            env,
        } => {
            let mut app = AppConfig::load_or_default(&cli.config)?;
            app.add_task(TaskConfig {
                id,
                name,
                enabled,
                schedule: ScheduleConfig::Interval { seconds },
                command: CommandConfig {
                    program,
                    args,
                    workdir,
                    env: env.into_iter().collect(),
                },
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
            println!("removed task {removed}");
            Ok(0)
        }
        Command::Enable { id } => {
            let mut app = AppConfig::load(&cli.config)?;
            app.set_enabled(&id, true)?;
            app.validate()?;
            app.write(&cli.config)?;
            println!("enabled task {id}");
            Ok(0)
        }
        Command::Disable { id } => {
            let mut app = AppConfig::load(&cli.config)?;
            app.set_enabled(&id, false)?;
            app.validate()?;
            app.write(&cli.config)?;
            println!("disabled task {id}");
            Ok(0)
        }
        Command::RunNow { id } => {
            let app = AppConfig::load(&cli.config)?;
            app.validate()?;
            let task = app
                .task(&id)
                .with_context(|| format!("task '{id}' not found"))?;
            let outcome = task_runner::run_task(task).await?;
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

fn print_tasks(app: &AppConfig) {
    if app.tasks.is_empty() {
        println!("no tasks configured");
        return;
    }

    println!("{:<20} {:<8} {:<24} schedule", "id", "status", "name");
    for task in &app.tasks {
        println!(
            "{:<20} {:<8} {:<24} {}",
            task.id,
            if task.enabled { "enabled" } else { "disabled" },
            task.name,
            task.schedule.summary()
        );
    }
}

pub fn exit(code: i32) -> ! {
    process::exit(code)
}
