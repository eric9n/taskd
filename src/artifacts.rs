//! Artifact workflow config, schemas, and `artifactctl` runtime.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::thread;

use anyhow::{Context, Result, anyhow, bail, ensure};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::artifact_cli::{ArtifactCli, ArtifactCommand};

const ARTIFACT_SCHEMA_VERSION: u32 = 1;
const ARTIFACT_STATUS_VALUES: &[&str] = &["ok", "warn", "error"];
const ALLOWED_CONTENT_TYPES: &[&str] = &["application/json", "text/plain"];
const ALLOWED_TEMPLATE_VARS: &[&str] = &[
    "artifact_id",
    "artifact_date",
    "run_id",
    "run_dir",
    "collect_file",
    "render_input_file",
    "render_file",
    "collector_id",
    "collector_output",
    "host",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactsConfig {
    pub version: u32,
    #[serde(default)]
    pub artifacts: Vec<ArtifactConfig>,
}

impl ArtifactsConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read artifacts config '{}'", path.display()))?;
        let config = serde_yaml::from_str::<Self>(&raw)
            .with_context(|| format!("failed to parse yaml '{}'", path.display()))?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            "unsupported artifacts config version {}",
            self.version
        );

        let mut seen = HashSetLike::default();
        for (index, artifact) in self.artifacts.iter().enumerate() {
            let path = format!("artifacts[{index}]");
            artifact.validate(&path)?;
            if !seen.insert(&artifact.id) {
                bail!("{path}.id duplicates artifact id '{}'", artifact.id);
            }
        }
        Ok(())
    }

    pub fn artifact(&self, artifact_id: &str) -> Option<&ArtifactConfig> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.id == artifact_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactConfig {
    pub id: String,
    pub timezone: String,
    pub workdir: PathBuf,
    #[serde(default)]
    pub collectors: Vec<ArtifactCollectorConfig>,
    pub renderer: ArtifactCommandConfig,
    #[serde(default)]
    pub sinks: Vec<ArtifactSinkConfig>,
}

impl ArtifactConfig {
    fn validate(&self, artifact_path: &str) -> Result<()> {
        ensure!(
            is_valid_identifier(&self.id),
            "{}.id must use only letters, digits, '-', '_' or '.'",
            artifact_path
        );
        ensure!(
            !self.timezone.trim().is_empty(),
            "{}.timezone must not be empty",
            artifact_path
        );
        self.timezone.parse::<chrono_tz::Tz>().with_context(|| {
            format!(
                "{}.timezone has invalid timezone '{}'",
                artifact_path, self.timezone
            )
        })?;
        ensure!(
            !self.collectors.is_empty(),
            "{}.collectors must contain at least one collector",
            artifact_path
        );

        let mut collector_ids = HashSetLike::default();
        for (index, collector) in self.collectors.iter().enumerate() {
            let path = format!("{artifact_path}.collectors[{index}]");
            collector.validate(&path)?;
            if !collector_ids.insert(&collector.id) {
                bail!("{path}.id duplicates collector id '{}'", collector.id);
            }
        }

        self.renderer
            .validate(&format!("{artifact_path}.renderer.command"))?;

        let mut sink_ids = HashSetLike::default();
        for (index, sink) in self.sinks.iter().enumerate() {
            let path = format!("{artifact_path}.sinks[{index}]");
            sink.validate(&path)?;
            if !sink_ids.insert(&sink.id) {
                bail!("{path}.id duplicates sink id '{}'", sink.id);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactCollectorConfig {
    pub id: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub command: ArtifactCommandConfig,
}

impl ArtifactCollectorConfig {
    fn validate(&self, collector_path: &str) -> Result<()> {
        ensure!(
            is_valid_identifier(&self.id),
            "{}.id must use only letters, digits, '-', '_' or '.'",
            collector_path
        );
        self.command
            .validate(&format!("{collector_path}.command"))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactSinkConfig {
    pub id: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub command: ArtifactCommandConfig,
}

impl ArtifactSinkConfig {
    fn validate(&self, sink_path: &str) -> Result<()> {
        ensure!(
            is_valid_identifier(&self.id),
            "{}.id must use only letters, digits, '-', '_' or '.'",
            sink_path
        );
        self.command.validate(&format!("{sink_path}.command"))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactCommandConfig {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl ArtifactCommandConfig {
    fn validate(&self, command_path: &str) -> Result<()> {
        ensure!(
            !self.program.trim().is_empty(),
            "{}.program must not be empty",
            command_path
        );
        validate_template_string(&self.program, &format!("{command_path}.program"))?;
        for (index, arg) in self.args.iter().enumerate() {
            validate_template_string(arg, &format!("{command_path}.args[{index}]"))?;
        }
        if let Some(workdir) = &self.workdir {
            validate_template_string(
                &workdir.to_string_lossy(),
                &format!("{command_path}.workdir"),
            )?;
        }
        for (key, value) in &self.env {
            ensure!(
                !key.trim().is_empty(),
                "{}.env contains an empty key",
                command_path
            );
            validate_template_string(value, &format!("{command_path}.env.{key}"))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorRecordInput {
    pub schema_version: u32,
    pub status: String,
    pub summary: String,
    pub content_type: String,
    pub payload: Value,
}

impl CollectorRecordInput {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == ARTIFACT_SCHEMA_VERSION,
            "collector schema_version must be {}",
            ARTIFACT_SCHEMA_VERSION
        );
        validate_status(&self.status)?;
        ensure!(
            !self.summary.trim().is_empty(),
            "collector summary must not be empty"
        );
        ensure!(
            ALLOWED_CONTENT_TYPES.contains(&self.content_type.as_str()),
            "collector content_type '{}' is not supported",
            self.content_type
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub schema_version: u32,
    pub artifact_id: String,
    pub artifact_date: String,
    pub run_id: String,
    pub collector_id: String,
    pub host: String,
    pub collected_at: DateTime<Utc>,
    pub status: String,
    pub summary: String,
    pub content_type: String,
    pub payload: Value,
}

impl ArtifactRecord {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == ARTIFACT_SCHEMA_VERSION,
            "record schema_version must be {}",
            ARTIFACT_SCHEMA_VERSION
        );
        validate_status(&self.status)?;
        ensure!(
            !self.summary.trim().is_empty(),
            "record summary must not be empty"
        );
        ensure!(
            ALLOWED_CONTENT_TYPES.contains(&self.content_type.as_str()),
            "record content_type '{}' is not supported",
            self.content_type
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderInputDocument {
    pub schema_version: u32,
    pub artifact_id: String,
    pub artifact_date: String,
    pub run_id: String,
    pub host: String,
    pub timezone: String,
    pub generated_at: DateTime<Utc>,
    pub records: Vec<ArtifactRecord>,
    pub meta: RenderInputMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderInputMeta {
    pub record_count: usize,
    pub status_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedArtifact {
    pub schema_version: u32,
    pub status: String,
    pub title: String,
    pub content_type: String,
    pub body: String,
    #[serde(default)]
    pub meta: Value,
}

impl RenderedArtifact {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == ARTIFACT_SCHEMA_VERSION,
            "rendered schema_version must be {}",
            ARTIFACT_SCHEMA_VERSION
        );
        validate_status(&self.status)?;
        ensure!(
            !self.title.trim().is_empty(),
            "rendered title must not be empty"
        );
        ensure!(
            !self.body.trim().is_empty(),
            "rendered body must not be empty"
        );
        ensure!(
            matches!(
                self.content_type.as_str(),
                "text/markdown" | "application/json" | "text/plain"
            ),
            "rendered content_type '{}' is not supported",
            self.content_type
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRunFile {
    pub schema_version: u32,
    pub artifact_id: String,
    pub artifact_date: String,
    pub run_id: String,
    pub host: String,
    pub timezone: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: String,
    pub paths: ArtifactRunPaths,
    pub collectors: Vec<ArtifactRunStepStatus>,
    pub renderer: ArtifactRunStepStatus,
    pub sinks: Vec<ArtifactRunStepStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRunPaths {
    pub records: PathBuf,
    pub render_input: PathBuf,
    pub rendered: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRunStepStatus {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Clone)]
struct ArtifactPaths {
    run_dir: PathBuf,
    collectors_dir: PathBuf,
    collect_file: PathBuf,
    collect_partial_file: PathBuf,
    render_input_file: PathBuf,
    render_file: PathBuf,
    run_file: PathBuf,
}

#[derive(Debug, Clone)]
struct CollectorExecutionOutcome {
    collector_id: String,
    record: ArtifactRecord,
}

#[derive(Debug, Clone)]
struct RuntimeTemplateVars {
    artifact_id: String,
    artifact_date: String,
    run_id: String,
    run_dir: PathBuf,
    collect_file: PathBuf,
    render_input_file: PathBuf,
    render_file: PathBuf,
    collector_id: Option<String>,
    collector_output: Option<PathBuf>,
    host: String,
}

impl RuntimeTemplateVars {
    fn to_map(&self) -> BTreeMap<&'static str, String> {
        let mut values = BTreeMap::new();
        values.insert("artifact_id", self.artifact_id.clone());
        values.insert("artifact_date", self.artifact_date.clone());
        values.insert("run_id", self.run_id.clone());
        values.insert("run_dir", self.run_dir.display().to_string());
        values.insert("collect_file", self.collect_file.display().to_string());
        values.insert(
            "render_input_file",
            self.render_input_file.display().to_string(),
        );
        values.insert("render_file", self.render_file.display().to_string());
        values.insert("host", self.host.clone());
        if let Some(collector_id) = &self.collector_id {
            values.insert("collector_id", collector_id.clone());
        }
        if let Some(collector_output) = &self.collector_output {
            values.insert("collector_output", collector_output.display().to_string());
        }
        values
    }
}

#[derive(Debug, Clone)]
struct ResolvedCommand {
    program: String,
    args: Vec<String>,
    workdir: Option<PathBuf>,
    env: BTreeMap<String, String>,
}

pub async fn run(cli: ArtifactCli) -> Result<i32> {
    let config = ArtifactsConfig::load(&cli.config)?;
    config.validate()?;

    match cli.command {
        ArtifactCommand::Validate => {
            println!("config is valid");
            Ok(0)
        }
        ArtifactCommand::List => {
            for artifact in &config.artifacts {
                println!("{}", artifact.id);
            }
            Ok(0)
        }
        ArtifactCommand::Collect { artifact_id, date } => {
            let artifact = config
                .artifact(&artifact_id)
                .with_context(|| format!("artifact '{artifact_id}' not found"))?;
            collect_artifact(artifact, &date)?;
            Ok(0)
        }
        ArtifactCommand::Render { artifact_id, date } => {
            let artifact = config
                .artifact(&artifact_id)
                .with_context(|| format!("artifact '{artifact_id}' not found"))?;
            render_artifact(artifact, &date)?;
            Ok(0)
        }
        ArtifactCommand::Sink { artifact_id, date } => {
            let artifact = config
                .artifact(&artifact_id)
                .with_context(|| format!("artifact '{artifact_id}' not found"))?;
            sink_artifact(artifact, &date)?;
            Ok(0)
        }
        ArtifactCommand::Run { artifact_id, date } => {
            let artifact = config
                .artifact(&artifact_id)
                .with_context(|| format!("artifact '{artifact_id}' not found"))?;
            collect_artifact(artifact, &date)?;
            render_artifact(artifact, &date)?;
            sink_artifact(artifact, &date)?;
            Ok(0)
        }
    }
}

fn collect_artifact(artifact: &ArtifactConfig, date: &str) -> Result<()> {
    let date = parse_artifact_date(date)?;
    let host = current_host_name();
    let paths = artifact_paths(artifact, date);
    fs::create_dir_all(&paths.collectors_dir).with_context(|| {
        format!(
            "failed to create collector output directory '{}'",
            paths.collectors_dir.display()
        )
    })?;
    let run_id = generate_run_id();
    let mut run_file = ArtifactRunFile::new(artifact, &date, &run_id, &host, &paths);
    write_json_pretty(&paths.run_file, &run_file)?;

    let enabled_collectors = artifact
        .collectors
        .iter()
        .filter(|collector| collector.enabled)
        .cloned()
        .collect::<Vec<_>>();

    let mut handles = Vec::with_capacity(enabled_collectors.len());
    for collector in enabled_collectors {
        let artifact_id = artifact.id.clone();
        let artifact_date = date.to_string();
        let run_id = run_id.clone();
        let host = host.clone();
        let collect_file = paths.collect_file.clone();
        let render_input_file = paths.render_input_file.clone();
        let render_file = paths.render_file.clone();
        let run_dir = paths.run_dir.clone();
        let collector_output = paths.collectors_dir.join(format!("{}.json", collector.id));
        handles.push(thread::spawn(move || {
            let vars = RuntimeTemplateVars {
                artifact_id,
                artifact_date,
                run_id,
                run_dir,
                collect_file,
                render_input_file,
                render_file,
                collector_id: Some(collector.id.clone()),
                collector_output: Some(collector_output.clone()),
                host,
            };
            run_collector_command(&collector, &vars, collector_output)
        }));
    }

    let mut outcomes = Vec::new();
    for handle in handles {
        outcomes.push(
            handle
                .join()
                .map_err(|_| anyhow!("collector thread panicked"))??,
        );
    }

    outcomes.sort_by(|left, right| left.collector_id.cmp(&right.collector_id));

    write_records_file(&paths.collect_partial_file, &outcomes)?;
    fs::rename(&paths.collect_partial_file, &paths.collect_file).with_context(|| {
        format!(
            "failed to finalize records file '{}'",
            paths.collect_file.display()
        )
    })?;

    let ok_count = outcomes
        .iter()
        .filter(|outcome| outcome.record.status == "ok")
        .count();
    ensure!(
        !outcomes.is_empty(),
        "artifact '{}' produced no collector records",
        artifact.id
    );
    ensure!(
        ok_count > 0,
        "artifact '{}' did not produce any successful collector records",
        artifact.id
    );

    for collector_status in &mut run_file.collectors {
        let outcome = outcomes
            .iter()
            .find(|outcome| outcome.collector_id == collector_status.id)
            .ok_or_else(|| anyhow!("missing collector outcome for '{}'", collector_status.id))?;
        collector_status.status = outcome.record.status.clone();
    }
    run_file.status = if outcomes.iter().any(|outcome| outcome.record.status != "ok") {
        "partial_success".to_string()
    } else {
        "success".to_string()
    };
    run_file.finished_at = Some(Utc::now());
    write_json_pretty(&paths.run_file, &run_file)?;
    Ok(())
}

fn render_artifact(artifact: &ArtifactConfig, date: &str) -> Result<()> {
    let date = parse_artifact_date(date)?;
    let paths = artifact_paths(artifact, date);
    let mut run_file = load_json::<ArtifactRunFile>(&paths.run_file)?;
    let mut records = read_records(&paths.collect_file)?;
    records.sort_by(|left, right| left.collector_id.cmp(&right.collector_id));
    let render_input = build_render_input(artifact, &run_file, records);
    write_json_pretty(&paths.render_input_file, &render_input)?;

    run_file.renderer.status = "running".to_string();
    write_json_pretty(&paths.run_file, &run_file)?;

    let vars = RuntimeTemplateVars {
        artifact_id: artifact.id.clone(),
        artifact_date: date.to_string(),
        run_id: run_file.run_id.clone(),
        run_dir: paths.run_dir.clone(),
        collect_file: paths.collect_file.clone(),
        render_input_file: paths.render_input_file.clone(),
        render_file: paths.render_file.clone(),
        collector_id: None,
        collector_output: None,
        host: run_file.host.clone(),
    };
    execute_command(&artifact.renderer, &vars).context("renderer command failed")?;
    let rendered = load_json::<RenderedArtifact>(&paths.render_file)?;
    rendered.validate()?;
    run_file.renderer.status = rendered.status.clone();
    if run_file.status == "success" && rendered.status != "ok" {
        run_file.status = "partial_success".to_string();
    } else if rendered.status == "error" {
        run_file.status = "failed".to_string();
    }
    write_json_pretty(&paths.run_file, &run_file)?;
    Ok(())
}

fn sink_artifact(artifact: &ArtifactConfig, date: &str) -> Result<()> {
    let date = parse_artifact_date(date)?;
    let paths = artifact_paths(artifact, date);
    let mut run_file = load_json::<ArtifactRunFile>(&paths.run_file)?;
    let rendered = load_json::<RenderedArtifact>(&paths.render_file)?;
    rendered.validate()?;

    let vars = RuntimeTemplateVars {
        artifact_id: artifact.id.clone(),
        artifact_date: date.to_string(),
        run_id: run_file.run_id.clone(),
        run_dir: paths.run_dir.clone(),
        collect_file: paths.collect_file.clone(),
        render_input_file: paths.render_input_file.clone(),
        render_file: paths.render_file.clone(),
        collector_id: None,
        collector_output: None,
        host: run_file.host.clone(),
    };

    for index in 0..run_file.sinks.len() {
        let sink_id = run_file.sinks[index].id.clone();
        let sink = artifact
            .sinks
            .iter()
            .find(|sink| sink.id == sink_id)
            .ok_or_else(|| anyhow!("missing sink config for '{sink_id}'"))?;
        if !sink.enabled {
            run_file.sinks[index].status = "disabled".to_string();
            continue;
        }
        run_file.sinks[index].status = "running".to_string();
        write_json_pretty(&paths.run_file, &run_file)?;
        match execute_command(&sink.command, &vars) {
            Ok(()) => run_file.sinks[index].status = "ok".to_string(),
            Err(error) => {
                run_file.sinks[index].status = "error".to_string();
                run_file.status = "failed".to_string();
                write_json_pretty(&paths.run_file, &run_file)?;
                return Err(error).with_context(|| format!("sink '{}' failed", sink.id));
            }
        }
    }

    if run_file.status == "running" {
        run_file.status = "success".to_string();
    }
    run_file.finished_at = Some(Utc::now());
    write_json_pretty(&paths.run_file, &run_file)?;
    Ok(())
}

fn run_collector_command(
    collector: &ArtifactCollectorConfig,
    vars: &RuntimeTemplateVars,
    collector_output: PathBuf,
) -> Result<CollectorExecutionOutcome> {
    let resolved = resolve_command(&collector.command, vars)?;
    let output = run_resolved_command(&resolved)?;
    let collected_at = Utc::now();
    let record = match output.status.code() {
        Some(0) => match load_json::<CollectorRecordInput>(&collector_output) {
            Ok(record) => match record.validate() {
                Ok(()) => ArtifactRecord {
                    schema_version: ARTIFACT_SCHEMA_VERSION,
                    artifact_id: vars.artifact_id.clone(),
                    artifact_date: vars.artifact_date.clone(),
                    run_id: vars.run_id.clone(),
                    collector_id: collector.id.clone(),
                    host: vars.host.clone(),
                    collected_at,
                    status: record.status,
                    summary: record.summary,
                    content_type: record.content_type,
                    payload: record.payload,
                },
                Err(error) => synthetic_error_record(
                    vars,
                    &collector.id,
                    format!("collector output schema invalid: {error:#}"),
                    json!({"error": {"kind": "schema_invalid", "message": error.to_string()}}),
                    collected_at,
                ),
            },
            Err(error) => synthetic_error_record(
                vars,
                &collector.id,
                format!("collector output missing or unreadable: {error:#}"),
                json!({"error": {"kind": "output_missing", "message": error.to_string()}}),
                collected_at,
            ),
        },
        code => synthetic_error_record(
            vars,
            &collector.id,
            format!(
                "collector exited with {}",
                code.map_or_else(|| "signal".to_string(), |code| format!("code {code}"))
            ),
            json!({
                "error": {
                    "kind": "command_failed",
                    "exit_code": code,
                    "stdout": String::from_utf8_lossy(&output.stdout),
                    "stderr": String::from_utf8_lossy(&output.stderr),
                }
            }),
            collected_at,
        ),
    };
    record.validate()?;
    Ok(CollectorExecutionOutcome {
        collector_id: collector.id.clone(),
        record,
    })
}

fn build_render_input(
    artifact: &ArtifactConfig,
    run_file: &ArtifactRunFile,
    records: Vec<ArtifactRecord>,
) -> RenderInputDocument {
    let mut status_counts = BTreeMap::new();
    for record in &records {
        *status_counts.entry(record.status.clone()).or_insert(0) += 1;
    }

    RenderInputDocument {
        schema_version: ARTIFACT_SCHEMA_VERSION,
        artifact_id: artifact.id.clone(),
        artifact_date: run_file.artifact_date.clone(),
        run_id: run_file.run_id.clone(),
        host: run_file.host.clone(),
        timezone: artifact.timezone.clone(),
        generated_at: Utc::now(),
        records,
        meta: RenderInputMeta {
            record_count: status_counts.values().sum(),
            status_counts,
        },
    }
}

fn resolve_command(
    config: &ArtifactCommandConfig,
    vars: &RuntimeTemplateVars,
) -> Result<ResolvedCommand> {
    let values = vars.to_map();
    let program = resolve_template(&config.program, &values)?;
    let args = config
        .args
        .iter()
        .map(|arg| resolve_template(arg, &values))
        .collect::<Result<Vec<_>>>()?;
    let workdir = config
        .workdir
        .as_ref()
        .map(|workdir| resolve_template(&workdir.to_string_lossy(), &values).map(PathBuf::from))
        .transpose()?;
    let mut env = BTreeMap::new();
    for (key, value) in &config.env {
        env.insert(key.clone(), resolve_template(value, &values)?);
    }
    env.insert("ARTIFACT_ID".to_string(), vars.artifact_id.clone());
    env.insert("ARTIFACT_DATE".to_string(), vars.artifact_date.clone());
    env.insert("ARTIFACT_RUN_ID".to_string(), vars.run_id.clone());
    env.insert(
        "ARTIFACT_RUN_DIR".to_string(),
        vars.run_dir.display().to_string(),
    );
    env.insert(
        "ARTIFACT_COLLECT_FILE".to_string(),
        vars.collect_file.display().to_string(),
    );
    env.insert(
        "ARTIFACT_RENDER_INPUT_FILE".to_string(),
        vars.render_input_file.display().to_string(),
    );
    env.insert(
        "ARTIFACT_RENDER_FILE".to_string(),
        vars.render_file.display().to_string(),
    );
    env.insert("ARTIFACT_HOST".to_string(), vars.host.clone());
    if let Some(collector_id) = &vars.collector_id {
        env.insert("ARTIFACT_COLLECTOR_ID".to_string(), collector_id.clone());
    }
    if let Some(collector_output) = &vars.collector_output {
        env.insert(
            "ARTIFACT_COLLECTOR_OUTPUT".to_string(),
            collector_output.display().to_string(),
        );
    }

    Ok(ResolvedCommand {
        program,
        args,
        workdir,
        env,
    })
}

fn execute_command(config: &ArtifactCommandConfig, vars: &RuntimeTemplateVars) -> Result<()> {
    let resolved = resolve_command(config, vars)?;
    let output = run_resolved_command(&resolved)?;
    ensure!(
        output.status.success(),
        "command exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn run_resolved_command(resolved: &ResolvedCommand) -> Result<std::process::Output> {
    let mut command = StdCommand::new(&resolved.program);
    command.args(&resolved.args);
    if let Some(workdir) = &resolved.workdir {
        command.current_dir(workdir);
    }
    for (key, value) in &resolved.env {
        command.env(key, value);
    }
    command
        .output()
        .with_context(|| format!("failed to execute command '{}'", resolved.program))
}

fn write_records_file(path: &Path, outcomes: &[CollectorExecutionOutcome]) -> Result<()> {
    let file = File::create(path)
        .with_context(|| format!("failed to create records file '{}'", path.display()))?;
    let mut writer = BufWriter::new(file);
    for outcome in outcomes {
        serde_json::to_writer(&mut writer, &outcome.record)
            .with_context(|| format!("failed to encode record for '{}'", outcome.collector_id))?;
        writer.write_all(b"\n").context("failed to write newline")?;
    }
    writer.flush().context("failed to flush records file")?;
    Ok(())
}

fn read_records(path: &Path) -> Result<Vec<ArtifactRecord>> {
    let file = File::open(path)
        .with_context(|| format!("failed to open records file '{}'", path.display()))?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line.with_context(|| {
            format!(
                "failed to read line {} from records file '{}'",
                index + 1,
                path.display()
            )
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let record = serde_json::from_str::<ArtifactRecord>(&line).with_context(|| {
            format!(
                "failed to parse record line {} from '{}'",
                index + 1,
                path.display()
            )
        })?;
        record.validate()?;
        records.push(record);
    }
    Ok(records)
}

fn write_json_pretty<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory '{}'", parent.display()))?;
    }
    let file = File::create(path)
        .with_context(|| format!("failed to create json file '{}'", path.display()))?;
    serde_json::to_writer_pretty(file, value)
        .with_context(|| format!("failed to write json file '{}'", path.display()))?;
    Ok(())
}

fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let file = File::open(path)
        .with_context(|| format!("failed to open json file '{}'", path.display()))?;
    serde_json::from_reader(file)
        .with_context(|| format!("failed to parse json file '{}'", path.display()))
}

fn artifact_paths(artifact: &ArtifactConfig, date: NaiveDate) -> ArtifactPaths {
    let run_dir = artifact.workdir.join(date.to_string());
    let collectors_dir = run_dir.join("collectors");
    ArtifactPaths {
        collect_file: run_dir.join("records.jsonl"),
        collect_partial_file: run_dir.join("records.jsonl.partial"),
        render_input_file: run_dir.join("render-input.json"),
        render_file: run_dir.join("rendered.json"),
        run_file: run_dir.join("run.json"),
        run_dir,
        collectors_dir,
    }
}

fn parse_artifact_date(input: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(input, "%Y-%m-%d")
        .with_context(|| format!("invalid artifact date '{input}', expected YYYY-MM-DD"))
}

fn generate_run_id() -> String {
    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let short_uuid = uuid::Uuid::new_v4().simple().to_string();
    format!("{timestamp}-{}", &short_uuid[..8])
}

fn current_host_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn resolve_template(template: &str, values: &BTreeMap<&'static str, String>) -> Result<String> {
    let placeholders = extract_placeholders(template)?;
    let mut result = template.to_string();
    for placeholder in placeholders {
        let value = values
            .get(placeholder.as_str())
            .ok_or_else(|| anyhow!("unknown template variable '{{{{{placeholder}}}}}'"))?;
        result = result.replace(&format!("{{{{{placeholder}}}}}"), value);
    }
    Ok(result)
}

fn validate_template_string(value: &str, field_path: &str) -> Result<()> {
    for placeholder in extract_placeholders(value)? {
        ensure!(
            ALLOWED_TEMPLATE_VARS.contains(&placeholder.as_str()),
            "{} uses unknown template variable '{{{{{}}}}}'",
            field_path,
            placeholder
        );
    }
    Ok(())
}

fn extract_placeholders(input: &str) -> Result<Vec<String>> {
    let mut placeholders = BTreeSet::new();
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            bail!("unclosed template variable in '{input}'");
        };
        let placeholder = after_start[..end].trim();
        ensure!(
            !placeholder.is_empty(),
            "empty template variable in '{input}'"
        );
        placeholders.insert(placeholder.to_string());
        rest = &after_start[end + 2..];
    }
    Ok(placeholders.into_iter().collect())
}

fn validate_status(status: &str) -> Result<()> {
    ensure!(
        ARTIFACT_STATUS_VALUES.contains(&status),
        "status '{}' must be one of {}",
        status,
        ARTIFACT_STATUS_VALUES.join(", ")
    );
    Ok(())
}

fn synthetic_error_record(
    vars: &RuntimeTemplateVars,
    collector_id: &str,
    summary: String,
    payload: Value,
    collected_at: DateTime<Utc>,
) -> ArtifactRecord {
    ArtifactRecord {
        schema_version: ARTIFACT_SCHEMA_VERSION,
        artifact_id: vars.artifact_id.clone(),
        artifact_date: vars.artifact_date.clone(),
        run_id: vars.run_id.clone(),
        collector_id: collector_id.to_string(),
        host: vars.host.clone(),
        collected_at,
        status: "error".to_string(),
        summary,
        content_type: "application/json".to_string(),
        payload,
    }
}

fn default_enabled() -> bool {
    true
}

fn is_valid_identifier(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

#[derive(Default)]
struct HashSetLike {
    values: HashMap<String, ()>,
}

impl HashSetLike {
    fn insert(&mut self, value: &str) -> bool {
        self.values.insert(value.to_string(), ()).is_none()
    }
}

impl ArtifactRunFile {
    fn new(
        artifact: &ArtifactConfig,
        date: &NaiveDate,
        run_id: &str,
        host: &str,
        paths: &ArtifactPaths,
    ) -> Self {
        Self {
            schema_version: ARTIFACT_SCHEMA_VERSION,
            artifact_id: artifact.id.clone(),
            artifact_date: date.to_string(),
            run_id: run_id.to_string(),
            host: host.to_string(),
            timezone: artifact.timezone.clone(),
            started_at: Utc::now(),
            finished_at: None,
            status: "running".to_string(),
            paths: ArtifactRunPaths {
                records: paths.collect_file.clone(),
                render_input: paths.render_input_file.clone(),
                rendered: paths.render_file.clone(),
            },
            collectors: artifact
                .collectors
                .iter()
                .filter(|collector| collector.enabled)
                .map(|collector| ArtifactRunStepStatus {
                    id: collector.id.clone(),
                    status: "pending".to_string(),
                })
                .collect(),
            renderer: ArtifactRunStepStatus {
                id: "renderer".to_string(),
                status: "pending".to_string(),
            },
            sinks: artifact
                .sinks
                .iter()
                .filter(|sink| sink.enabled)
                .map(|sink| ArtifactRunStepStatus {
                    id: sink.id.clone(),
                    status: "pending".to_string(),
                })
                .collect(),
        }
    }
}
