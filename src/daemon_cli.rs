//! `taskd` command-line definitions for daemon-only operations.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::config_path::default_config_path;

const TASKD_AFTER_HELP: &str = "\
Examples:
  taskd daemon
  taskd daemon --config /etc/taskd/tasks.yaml

Default config lookup:
  1. /etc/taskd/tasks.yaml
  2. ./config/tasks.yaml";

#[derive(Debug, Parser)]
#[command(
    name = "taskd",
    version,
    about = "Run the taskd scheduler daemon on a single host",
    long_about = "taskd is the daemon process that watches the task config, schedules cron and interval jobs, records runtime state and history, and executes tasks in the background.",
    after_help = TASKD_AFTER_HELP
)]
pub struct TaskdCli {
    #[arg(
        long,
        global = true,
        default_value_os_t = default_config_path(),
        hide_default_value = true,
        help = "Path to the task config file. Defaults to /etc/taskd/tasks.yaml if present, otherwise ./config/tasks.yaml"
    )]
    pub config: PathBuf,
    #[command(subcommand)]
    pub command: TaskdCommand,
}

#[derive(Debug, Subcommand)]
pub enum TaskdCommand {
    #[command(about = "Start the background scheduler and config watcher")]
    Daemon,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use super::{TaskdCli, TaskdCommand};

    #[test]
    fn parses_daemon_command() {
        let cli = TaskdCli::parse_from(["taskd", "--config", "/tmp/tasks.yaml", "daemon"]);

        assert_eq!(cli.config, PathBuf::from("/tmp/tasks.yaml"));
        match cli.command {
            TaskdCommand::Daemon => {}
        }
    }

    #[test]
    fn parses_default_config_path() {
        let cli = TaskdCli::parse_from(["taskd", "daemon"]);

        assert!(
            cli.config == PathBuf::from("/etc/taskd/tasks.yaml")
                || cli.config == PathBuf::from("config/tasks.yaml")
        );
    }
}
