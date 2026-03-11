use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "taskd", version, about = "YAML-driven task scheduler")]
pub struct Cli {
    #[arg(long, global = true, default_value = "config/tasks.yaml")]
    pub config: PathBuf,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Daemon,
    List,
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
        #[arg(long)]
        workdir: Option<PathBuf>,
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
        #[arg(long)]
        workdir: Option<PathBuf>,
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
    RunNow {
        id: String,
    },
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

    use super::{Cli, Command};

    #[test]
    fn parses_add_cron_with_trailing_args() {
        let cli = Cli::parse_from([
            "taskd",
            "--config",
            "/tmp/tasks.yaml",
            "add-cron",
            "backup-db",
            "backup database",
            "0 0 2 * * *",
            "/bin/echo",
            "--timezone",
            "Asia/Shanghai",
            "--",
            "--full",
            "nightly",
        ]);

        assert_eq!(cli.config, PathBuf::from("/tmp/tasks.yaml"));
        match cli.command {
            Command::AddCron {
                id, timezone, args, ..
            } => {
                assert_eq!(id, "backup-db");
                assert_eq!(timezone.as_deref(), Some("Asia/Shanghai"));
                assert_eq!(args, vec!["--full", "nightly"]);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_run_now() {
        let cli = Cli::parse_from(["taskd", "run-now", "job-1"]);

        match cli.command {
            Command::RunNow { id } => assert_eq!(id, "job-1"),
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
