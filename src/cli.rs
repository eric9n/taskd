//! `taskctl` command-line definitions for control-plane operations.

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};

use crate::config_path::default_config_path;

const TASKCTL_AFTER_HELP: &str = "\
Examples:
  taskctl list
  taskctl show backup-db
  taskctl validate
  taskctl logs --lines 200
  taskctl run-now backup-db
  taskctl add-cron backup-db \"backup database\" \"0 0 2 * * *\" /usr/local/bin/backup.sh -- --full

Default config lookup:
  1. /etc/taskd/tasks.yaml
  2. ./config/tasks.yaml

Use --json when another agent or script needs structured output.";

#[derive(Debug, Parser)]
#[command(
    name = "taskctl",
    version,
    about = "Inspect, validate, edit, and run tasks managed by taskd",
    long_about = "taskctl is the control-plane CLI for taskd. It reads a YAML config, lets you inspect and modify tasks, validates scheduler settings, runs tasks immediately, and queries runtime state and history.",
    after_help = TASKCTL_AFTER_HELP
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        default_value_os_t = default_config_path(),
        hide_default_value = true,
        help = "Path to the task config file. Defaults to /etc/taskd/tasks.yaml if present, otherwise ./config/tasks.yaml"
    )]
    pub config: PathBuf,
    #[arg(
        long,
        global = true,
        help = "Emit machine-readable JSON instead of human-friendly text"
    )]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(about = "List configured tasks with current runtime status")]
    List,
    #[command(about = "Show one task in detail, including latest runtime state and history")]
    Show {
        #[arg(help = "Task ID to inspect")]
        id: String,
    },
    #[command(about = "Validate the config file and all task definitions")]
    Validate,
    #[command(about = "Add a new cron-scheduled command task to the config file")]
    AddCron {
        #[arg(help = "Unique task ID")]
        id: String,
        #[arg(help = "Human-readable task name")]
        name: String,
        #[arg(help = "Cron expression in the scheduler's expected format")]
        expr: String,
        #[arg(help = "Program or script to execute")]
        program: String,
        #[arg(long, help = "Optional cron timezone, for example Asia/Shanghai")]
        timezone: Option<String>,
        #[arg(long, default_value_t = true, help = "Whether the task starts enabled")]
        enabled: bool,
        #[arg(
            long,
            default_value_t = 1,
            value_parser = clap::value_parser!(u8).range(1..=3),
            help = "Maximum concurrent runs for this task (1-3)"
        )]
        max_running: u8,
        #[arg(
            long,
            value_enum,
            default_value_t = ConcurrencyPolicyArg::Forbid,
            help = "Concurrency policy when the task triggers again while already running"
        )]
        concurrency_policy: ConcurrencyPolicyArg,
        #[arg(long, help = "Optional working directory for the spawned process")]
        workdir: Option<PathBuf>,
        #[arg(
            long,
            help = "Optional timeout in seconds before the process is terminated"
        )]
        timeout_seconds: Option<u64>,
        #[arg(
            long,
            default_value_t = 0,
            help = "How many retry attempts to make after the first failed run"
        )]
        retry_max_attempts: u8,
        #[arg(
            long,
            default_value_t = 1,
            help = "Delay in seconds between retry attempts"
        )]
        retry_delay_seconds: u64,
        #[arg(long = "env", value_parser = parse_env, help = "Environment variable in KEY=VALUE format; can be repeated")]
        env: Vec<(String, String)>,
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            help = "Arguments passed to the target program; place them after --"
        )]
        args: Vec<String>,
    },
    #[command(about = "Add a new interval-scheduled command task to the config file")]
    AddInterval {
        #[arg(help = "Unique task ID")]
        id: String,
        #[arg(help = "Human-readable task name")]
        name: String,
        #[arg(help = "Run interval in seconds")]
        seconds: u64,
        #[arg(help = "Program or script to execute")]
        program: String,
        #[arg(long, default_value_t = true, help = "Whether the task starts enabled")]
        enabled: bool,
        #[arg(
            long,
            default_value_t = 1,
            value_parser = clap::value_parser!(u8).range(1..=3),
            help = "Maximum concurrent runs for this task (1-3)"
        )]
        max_running: u8,
        #[arg(
            long,
            value_enum,
            default_value_t = ConcurrencyPolicyArg::Forbid,
            help = "Concurrency policy when the task triggers again while already running"
        )]
        concurrency_policy: ConcurrencyPolicyArg,
        #[arg(long, help = "Optional working directory for the spawned process")]
        workdir: Option<PathBuf>,
        #[arg(
            long,
            help = "Optional timeout in seconds before the process is terminated"
        )]
        timeout_seconds: Option<u64>,
        #[arg(
            long,
            default_value_t = 0,
            help = "How many retry attempts to make after the first failed run"
        )]
        retry_max_attempts: u8,
        #[arg(
            long,
            default_value_t = 1,
            help = "Delay in seconds between retry attempts"
        )]
        retry_delay_seconds: u64,
        #[arg(long = "env", value_parser = parse_env, help = "Environment variable in KEY=VALUE format; can be repeated")]
        env: Vec<(String, String)>,
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            help = "Arguments passed to the target program; place them after --"
        )]
        args: Vec<String>,
    },
    #[command(about = "Remove a task from the config file")]
    Remove {
        #[arg(help = "Task ID to remove")]
        id: String,
    },
    #[command(about = "Enable a configured task")]
    Enable {
        #[arg(help = "Task ID to enable")]
        id: String,
    },
    #[command(about = "Disable a configured task")]
    Disable {
        #[arg(help = "Task ID to disable")]
        id: String,
    },
    #[command(about = "Show recent execution history for one task")]
    History {
        #[arg(help = "Task ID to query")]
        id: String,
        #[arg(
            long,
            default_value_t = 20,
            help = "Maximum number of history rows to return"
        )]
        limit: usize,
    },
    #[command(about = "Show the most recent failed task runs across all tasks")]
    RecentFailures {
        #[arg(
            long,
            default_value_t = 20,
            help = "Maximum number of failed rows to return"
        )]
        limit: usize,
    },
    #[command(about = "Show logs for the taskd systemd service via journalctl")]
    Logs {
        #[arg(
            long,
            default_value_t = 100,
            help = "Maximum number of journal lines to show"
        )]
        lines: usize,
        #[arg(long, help = "Follow the journal output continuously")]
        follow: bool,
    },
    #[command(about = "Run one task immediately outside its normal schedule")]
    RunNow {
        #[arg(help = "Task ID to execute now")]
        id: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum ConcurrencyPolicyArg {
    #[value(help = "Allow overlapping runs up to --max-running")]
    Allow,
    #[value(help = "Forbid overlap; skip triggers while a run is active")]
    Forbid,
    #[value(help = "Cancel the previous run and start the new one")]
    Replace,
}

fn parse_env(value: &str) -> Result<(String, String)> {
    let (key, val) = value
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("invalid env '{value}', expected KEY=VALUE"))?;
    if key.is_empty() {
        bail!("invalid env '{value}', key must not be empty");
    }
    Ok((key.to_string(), val.to_string()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use super::{Cli, Command, ConcurrencyPolicyArg};

    #[test]
    fn parses_add_cron_with_trailing_args() {
        let cli = Cli::parse_from([
            "taskctl",
            "--config",
            "/tmp/tasks.yaml",
            "add-cron",
            "backup-db",
            "backup database",
            "0 0 2 * * *",
            "/bin/echo",
            "--timezone",
            "Asia/Shanghai",
            "--max-running",
            "3",
            "--concurrency-policy",
            "allow",
            "--timeout-seconds",
            "30",
            "--",
            "--full",
            "nightly",
        ]);

        assert_eq!(cli.config, PathBuf::from("/tmp/tasks.yaml"));
        match cli.command {
            Command::AddCron {
                id,
                timezone,
                max_running,
                concurrency_policy,
                timeout_seconds,
                retry_max_attempts,
                retry_delay_seconds,
                args,
                ..
            } => {
                assert_eq!(id, "backup-db");
                assert_eq!(timezone.as_deref(), Some("Asia/Shanghai"));
                assert_eq!(max_running, 3);
                assert_eq!(concurrency_policy, ConcurrencyPolicyArg::Allow);
                assert_eq!(timeout_seconds, Some(30));
                assert_eq!(retry_max_attempts, 0);
                assert_eq!(retry_delay_seconds, 1);
                assert_eq!(args, vec!["--full", "nightly"]);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_run_now() {
        let cli = Cli::parse_from(["taskctl", "run-now", "job-1"]);

        match cli.command {
            Command::RunNow { id } => assert_eq!(id, "job-1"),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_default_config_path() {
        let cli = Cli::parse_from(["taskctl", "list"]);

        assert!(
            cli.config == PathBuf::from("/etc/taskd/tasks.yaml")
                || cli.config == PathBuf::from("config/tasks.yaml")
        );
    }

    #[test]
    fn parses_show() {
        let cli = Cli::parse_from(["taskctl", "show", "job-1"]);

        match cli.command {
            Command::Show { id } => assert_eq!(id, "job-1"),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_history() {
        let cli = Cli::parse_from(["taskctl", "--json", "history", "job-1", "--limit", "5"]);

        assert!(cli.json);
        match cli.command {
            Command::History { id, limit } => {
                assert_eq!(id, "job-1");
                assert_eq!(limit, 5);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_logs() {
        let cli = Cli::parse_from(["taskctl", "logs", "--lines", "50", "--follow"]);

        match cli.command {
            Command::Logs { lines, follow } => {
                assert_eq!(lines, 50);
                assert!(follow);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
