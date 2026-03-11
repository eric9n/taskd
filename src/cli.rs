//! `taskctl` command-line definitions for control-plane operations.

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "taskctl", version, about = "taskd control plane")]
pub struct Cli {
    #[arg(long, global = true, default_value = "config/tasks.yaml")]
    pub config: PathBuf,
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    List,
    Show {
        id: String,
    },
    Validate,
    AddCron {
        id: String,
        name: String,
        expr: String,
        program: String,
        #[arg(long)]
        timezone: Option<String>,
        #[arg(long, default_value_t = true)]
        enabled: bool,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=3))]
        max_running: u8,
        #[arg(long, value_enum, default_value_t = ConcurrencyPolicyArg::Forbid)]
        concurrency_policy: ConcurrencyPolicyArg,
        #[arg(long)]
        workdir: Option<PathBuf>,
        #[arg(long)]
        timeout_seconds: Option<u64>,
        #[arg(long, default_value_t = 0)]
        retry_max_attempts: u8,
        #[arg(long, default_value_t = 1)]
        retry_delay_seconds: u64,
        #[arg(long = "env", value_parser = parse_env)]
        env: Vec<(String, String)>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    AddInterval {
        id: String,
        name: String,
        seconds: u64,
        program: String,
        #[arg(long, default_value_t = true)]
        enabled: bool,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=3))]
        max_running: u8,
        #[arg(long, value_enum, default_value_t = ConcurrencyPolicyArg::Forbid)]
        concurrency_policy: ConcurrencyPolicyArg,
        #[arg(long)]
        workdir: Option<PathBuf>,
        #[arg(long)]
        timeout_seconds: Option<u64>,
        #[arg(long, default_value_t = 0)]
        retry_max_attempts: u8,
        #[arg(long, default_value_t = 1)]
        retry_delay_seconds: u64,
        #[arg(long = "env", value_parser = parse_env)]
        env: Vec<(String, String)>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Remove {
        id: String,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    History {
        id: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    RecentFailures {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    RunNow {
        id: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum ConcurrencyPolicyArg {
    Allow,
    Forbid,
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
}
