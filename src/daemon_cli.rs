//! `taskd` command-line definitions for daemon-only operations.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "taskd", version, about = "taskd daemon")]
pub struct TaskdCli {
    #[arg(long, global = true, default_value = "config/tasks.yaml")]
    pub config: PathBuf,
    #[command(subcommand)]
    pub command: TaskdCommand,
}

#[derive(Debug, Subcommand)]
pub enum TaskdCommand {
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
}
