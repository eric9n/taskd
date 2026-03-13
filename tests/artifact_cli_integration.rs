use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::tempdir;

use chrono::{TimeZone, Utc};
use taskd::history::HistoryStore;
use taskd::task_runner::{TaskOutcome, TaskRunStatus};

#[test]
fn artifactctl_run_collects_renders_and_sinks() {
    let dir = tempdir().expect("tempdir");
    let workdir = dir.path().join("artifacts");
    let sink_output = dir.path().join("sink-output.json");
    let collector_ok = write_script(
        dir.path(),
        "collector-ok.sh",
        r#"#!/bin/sh
set -eu
cat > "$1" <<'EOF'
{"schema_version":1,"status":"ok","summary":"taskd summary","content_type":"application/json","payload":{"runs":3}}
EOF
"#,
    );
    let collector_fail = write_script(
        dir.path(),
        "collector-fail.sh",
        r#"#!/bin/sh
set -eu
exit 7
"#,
    );
    let renderer = write_script(
        dir.path(),
        "renderer.sh",
        r#"#!/bin/sh
set -eu
cat > "$2" <<'EOF'
{"schema_version":1,"status":"ok","title":"Daily Ops","content_type":"text/markdown","body":"rendered artifact","meta":{"highlights":["one collector failed"]}}
EOF
"#,
    );
    let sink = write_script(
        dir.path(),
        "sink.sh",
        &format!(
            r#"#!/bin/sh
set -eu
cp "$1" "{}"
"#,
            sink_output.display()
        ),
    );

    let config = dir.path().join("artifacts.yaml");
    fs::write(
        &config,
        format!(
            r#"
version: 1
artifacts:
  - id: daily_ops
    timezone: UTC
    workdir: {}
    collectors:
      - id: taskd
        command:
          program: /bin/sh
          args:
            - {}
            - "{{{{collector_output}}}}"
      - id: failing
        command:
          program: /bin/sh
          args:
            - {}
            - "{{{{collector_output}}}}"
    renderer:
      program: /bin/sh
      args:
        - {}
        - "{{{{render_input_file}}}}"
        - "{{{{render_file}}}}"
    sinks:
      - id: discord
        command:
          program: /bin/sh
          args:
            - {}
            - "{{{{render_file}}}}"
"#,
            yaml_string(&workdir),
            yaml_string(&collector_ok),
            yaml_string(&collector_fail),
            yaml_string(&renderer),
            yaml_string(&sink),
        ),
    )
    .expect("write config");

    let mut cmd = Command::cargo_bin("artifactctl").expect("binary");
    cmd.arg("--config")
        .arg(&config)
        .arg("run")
        .arg("daily_ops")
        .arg("--date")
        .arg("2026-03-12");

    cmd.assert().success();

    let run_dir = workdir.join("2026-03-12");
    let records = fs::read_to_string(run_dir.join("records.jsonl")).expect("records");
    assert_eq!(records.lines().count(), 2);
    assert!(records.contains(r#""collector_id":"taskd""#));
    assert!(records.contains(r#""collector_id":"failing""#));
    assert!(records.contains(r#""status":"error""#));

    let rendered = fs::read_to_string(run_dir.join("rendered.json")).expect("rendered");
    assert!(rendered.contains("Daily Ops"));
    let sink_body = fs::read_to_string(sink_output).expect("sink output");
    assert_eq!(sink_body, rendered);

    let run_file: Value =
        serde_json::from_str(&fs::read_to_string(run_dir.join("run.json")).expect("run file"))
            .expect("run json");
    assert_eq!(run_file["status"], "partial_success");
    assert_eq!(run_file["renderer"]["status"], "ok");
    assert_eq!(run_file["sinks"][0]["status"], "ok");
}

#[test]
fn artifactctl_validate_rejects_unknown_template_variables() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("artifacts.yaml");
    fs::write(
        &config,
        r#"
version: 1
artifacts:
  - id: daily_ops
    timezone: UTC
    workdir: /tmp/daily_ops
    collectors:
      - id: taskd
        command:
          program: /bin/echo
          args: ["{{unknown_var}}"]
    renderer:
      program: /bin/echo
      args: ["{{render_file}}"]
"#,
    )
    .expect("write config");

    let mut cmd = Command::cargo_bin("artifactctl").expect("binary");
    cmd.arg("--config").arg(&config).arg("validate");

    cmd.assert()
        .failure()
        .stderr(predicates::str::contains("unknown template variable"));
}

#[test]
fn taskctl_report_daily_emits_collector_compatible_json() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");
    fs::write(&config, "version: 1\ntasks: []\n").expect("write config");

    let history = HistoryStore::from_config_path(&config).expect("history store");
    history
        .record(
            "job-success",
            &TaskOutcome::synthetic(
                TaskRunStatus::Success,
                "exit code 0".to_string(),
                0,
                Utc.with_ymd_and_hms(2026, 3, 12, 0, 0, 0)
                    .single()
                    .expect("start"),
                Utc.with_ymd_and_hms(2026, 3, 12, 0, 1, 0)
                    .single()
                    .expect("finish"),
            ),
        )
        .expect("record success");
    history
        .record(
            "job-fail",
            &TaskOutcome::synthetic(
                TaskRunStatus::Failed,
                "exit code 7".to_string(),
                7,
                Utc.with_ymd_and_hms(2026, 3, 12, 1, 0, 0)
                    .single()
                    .expect("start"),
                Utc.with_ymd_and_hms(2026, 3, 12, 1, 1, 0)
                    .single()
                    .expect("finish"),
            ),
        )
        .expect("record failure");

    let output = dir.path().join("report.json");
    let mut cmd = Command::cargo_bin("taskctl").expect("binary");
    cmd.arg("--config")
        .arg(&config)
        .arg("--json")
        .arg("report")
        .arg("daily")
        .arg("--date")
        .arg("2026-03-12")
        .arg("--timezone")
        .arg("UTC")
        .arg("--output")
        .arg(&output);

    cmd.assert().success();

    let json: Value =
        serde_json::from_str(&fs::read_to_string(output).expect("report output")).expect("json");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["status"], "warn");
    assert_eq!(json["content_type"], "application/json");
    assert_eq!(json["payload"]["total_runs"], 2);
    assert_eq!(json["payload"]["totals"]["success"], 1);
    assert_eq!(json["payload"]["totals"]["failed"], 1);
    assert_eq!(json["payload"]["tasks"].as_array().expect("tasks").len(), 2);
    assert_eq!(
        json["payload"]["failures"]
            .as_array()
            .expect("failures")
            .len(),
        1
    );
}

fn yaml_string(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).expect("write script");
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod");
    }
    path
}
