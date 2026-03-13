//! `artifactctl` command-line definitions for artifact collect/render/sink workflows.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::config_path::default_artifacts_config_path;

const ARTIFACTCTL_AFTER_HELP: &str = "\
Examples:
  artifactctl validate
  artifactctl collect daily_ops --date 2026-03-12
  artifactctl render daily_ops --date 2026-03-12
  artifactctl sink daily_ops --date 2026-03-12
  artifactctl run daily_ops --date 2026-03-12

Default config lookup:
  1. /etc/taskd/artifacts.yaml
  2. ./config/artifacts.yaml";

#[derive(Debug, Parser)]
#[command(
    name = "artifactctl",
    version,
    about = "Collect, render, and deliver structured artifacts",
    long_about = "artifactctl orchestrates generic artifact workflows defined in a separate YAML config. It runs collector commands, validates JSON records, prepares renderer input, and dispatches rendered output to configured sinks.",
    after_help = ARTIFACTCTL_AFTER_HELP
)]
pub struct ArtifactCli {
    #[arg(
        long,
        global = true,
        default_value_os_t = default_artifacts_config_path(),
        hide_default_value = true,
        help = "Path to the artifacts config file. Defaults to /etc/taskd/artifacts.yaml if present, otherwise ./config/artifacts.yaml"
    )]
    pub config: PathBuf,
    #[command(subcommand)]
    pub command: ArtifactCommand,
}

#[derive(Debug, Subcommand)]
pub enum ArtifactCommand {
    #[command(about = "Validate the artifacts config file")]
    Validate,
    #[command(about = "List configured artifact ids")]
    List,
    #[command(about = "Run all configured collectors for one artifact date")]
    Collect {
        #[arg(help = "Artifact id to collect")]
        artifact_id: String,
        #[arg(long, help = "Artifact business date in YYYY-MM-DD format")]
        date: String,
    },
    #[command(about = "Render one collected artifact date")]
    Render {
        #[arg(help = "Artifact id to render")]
        artifact_id: String,
        #[arg(long, help = "Artifact business date in YYYY-MM-DD format")]
        date: String,
    },
    #[command(about = "Send one rendered artifact date to all enabled sinks")]
    Sink {
        #[arg(help = "Artifact id to send")]
        artifact_id: String,
        #[arg(long, help = "Artifact business date in YYYY-MM-DD format")]
        date: String,
    },
    #[command(about = "Run collect, render, and sink for one artifact date")]
    Run {
        #[arg(help = "Artifact id to execute")]
        artifact_id: String,
        #[arg(long, help = "Artifact business date in YYYY-MM-DD format")]
        date: String,
    },
}
