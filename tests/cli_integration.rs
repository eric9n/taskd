use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn validate_reports_success_for_valid_config() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");
    fs::write(
        &config,
        r#"
version: 1
tasks:
  - id: health-check
    name: health check
    enabled: true
    schedule:
      kind: interval
      seconds: 10
    command:
      program: /bin/echo
"#,
    )
    .expect("write config");

    let mut cmd = Command::cargo_bin("taskctl").expect("binary");
    cmd.arg("--config").arg(&config).arg("validate");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("config is valid"));
}

#[test]
fn validate_supports_json_output() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");
    fs::write(
        &config,
        r#"
version: 1
tasks:
  - id: health-check
    name: health check
    enabled: true
    schedule:
      kind: interval
      seconds: 10
    command:
      program: /bin/echo
"#,
    )
    .expect("write config");

    let mut cmd = Command::cargo_bin("taskctl").expect("binary");
    cmd.arg("--config")
        .arg(&config)
        .arg("--json")
        .arg("validate");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("\"ok\": true"))
        .stdout(predicate::str::contains("\"task_count\": 1"));
}

#[test]
fn add_interval_writes_task_to_yaml() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");

    let mut cmd = Command::cargo_bin("taskctl").expect("binary");
    cmd.arg("--config")
        .arg(&config)
        .arg("add-interval")
        .arg("health-check")
        .arg("health check")
        .arg("60")
        .arg("/bin/echo")
        .arg("--timeout-seconds")
        .arg("30")
        .arg("--")
        .arg("ok");

    cmd.assert().success();

    let body = fs::read_to_string(config).expect("config file");
    assert!(body.contains("health-check"));
    assert!(body.contains("seconds: 60"));
    assert!(body.contains("timeout_seconds: 30"));
}

#[test]
fn run_now_returns_child_exit_code() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");
    fs::write(
        &config,
        r#"
version: 1
tasks:
  - id: failing
    name: failing
    enabled: true
    schedule:
      kind: interval
      seconds: 10
    command:
      program: /bin/sh
      args:
        - -c
        - exit 7
"#,
    )
    .expect("write config");

    let mut cmd = Command::cargo_bin("taskctl").expect("binary");
    cmd.arg("--config")
        .arg(&config)
        .arg("run-now")
        .arg("failing");

    cmd.assert().code(7);
}

#[test]
fn run_now_missing_program_returns_error_instead_of_crashing() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");
    fs::write(
        &config,
        r#"
version: 1
tasks:
  - id: missing
    name: missing
    enabled: true
    schedule:
      kind: interval
      seconds: 10
    command:
      program: /definitely/missing/taskd-bin
"#,
    )
    .expect("write config");

    let mut cmd = Command::cargo_bin("taskctl").expect("binary");
    cmd.arg("--config")
        .arg(&config)
        .arg("run-now")
        .arg("missing");

    cmd.assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("failed to run task 'missing'"));
}

#[test]
fn validate_rejects_zero_timeout() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");
    fs::write(
        &config,
        r#"
version: 1
tasks:
  - id: invalid-timeout
    name: invalid-timeout
    enabled: true
    schedule:
      kind: interval
      seconds: 10
    command:
      program: /bin/echo
      timeout_seconds: 0
"#,
    )
    .expect("write config");

    let mut cmd = Command::cargo_bin("taskctl").expect("binary");
    cmd.arg("--config").arg(&config).arg("validate");

    cmd.assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "tasks[0].command.timeout_seconds must be > 0",
        ));
}

#[test]
fn list_shows_last_run_status_from_state_file() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");
    let state = dir.path().join("tasks.state.yaml");
    fs::write(
        &config,
        r#"
version: 1
tasks:
  - id: health-check
    name: health check
    enabled: true
    concurrency:
      policy: forbid
      max_running: 1
    schedule:
      kind: interval
      seconds: 10
    command:
      program: /bin/echo
"#,
    )
    .expect("write config");
    fs::write(
        &state,
        r#"
version: 1
tasks:
  health-check:
    last_status: success
    last_summary: exit code 0
    last_started_at: 2026-03-11T10:00:00Z
    last_finished_at: 2026-03-11T10:00:01Z
"#,
    )
    .expect("write state");

    let mut cmd = Command::cargo_bin("taskctl").expect("binary");
    cmd.arg("--config").arg(&config).arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("success"))
        .stdout(predicate::str::contains("2026-03-11 10:00:01"));
}

#[test]
fn list_supports_json_output() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");
    let state = dir.path().join("tasks.state.yaml");
    fs::write(
        &config,
        r#"
version: 1
tasks:
  - id: health-check
    name: health check
    enabled: true
    concurrency:
      policy: allow
      max_running: 2
    schedule:
      kind: interval
      seconds: 10
    command:
      program: /bin/echo
"#,
    )
    .expect("write config");
    fs::write(
        &state,
        r#"
version: 1
tasks:
  health-check:
    last_status: success
    last_summary: exit code 0
    last_started_at: 2026-03-11T10:00:00Z
    last_finished_at: 2026-03-11T10:00:01Z
"#,
    )
    .expect("write state");

    let mut cmd = Command::cargo_bin("taskctl").expect("binary");
    cmd.arg("--config").arg(&config).arg("--json").arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("\"tasks\""))
        .stdout(predicate::str::contains("\"id\": \"health-check\""))
        .stdout(predicate::str::contains(
            "\"concurrency_policy\": \"allow\"",
        ))
        .stdout(predicate::str::contains("\"last_status\": \"success\""));
}

#[test]
fn show_displays_task_details_and_runtime_state() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");
    let state = dir.path().join("tasks.state.yaml");
    fs::write(
        &config,
        r#"
version: 1
tasks:
  - id: health-check
    name: health check
    enabled: true
    concurrency:
      policy: allow
      max_running: 2
    schedule:
      kind: interval
      seconds: 10
    command:
      program: /bin/echo
      args:
        - ok
      timeout_seconds: 30
      env:
        FOO: bar
"#,
    )
    .expect("write config");
    fs::write(
        &state,
        r#"
version: 1
tasks:
  health-check:
    last_status: success
    last_summary: exit code 0
    last_started_at: 2026-03-11T10:00:00Z
    last_finished_at: 2026-03-11T10:00:01Z
"#,
    )
    .expect("write state");

    let mut cmd = Command::cargo_bin("taskctl").expect("binary");
    cmd.arg("--config")
        .arg(&config)
        .arg("show")
        .arg("health-check");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("id: health-check"))
        .stdout(predicate::str::contains(
            "concurrency: allow (max_running=2)",
        ))
        .stdout(predicate::str::contains("timeout_seconds: 30"))
        .stdout(predicate::str::contains("env: FOO=bar"))
        .stdout(predicate::str::contains("last_status: success"))
        .stdout(predicate::str::contains("latest_history_status: -"));
}

#[test]
fn show_supports_json_output() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");
    fs::write(
        &config,
        r#"
version: 1
tasks:
  - id: health-check
    name: health check
    enabled: true
    concurrency:
      policy: allow
      max_running: 2
    schedule:
      kind: interval
      seconds: 10
    command:
      program: /bin/echo
"#,
    )
    .expect("write config");

    let mut cmd = Command::cargo_bin("taskctl").expect("binary");
    cmd.arg("--config")
        .arg(&config)
        .arg("--json")
        .arg("show")
        .arg("health-check");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("\"task\""))
        .stdout(predicate::str::contains("\"id\": \"health-check\""))
        .stdout(predicate::str::contains("\"runtime_state\": null"));
}

#[test]
fn history_lists_task_history() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");
    std::fs::write(&config, "version: 1\ntasks: []\n").expect("write config");

    let mut run_now = Command::cargo_bin("taskctl").expect("binary");
    run_now
        .arg("--config")
        .arg(&config)
        .arg("add-interval")
        .arg("history-job")
        .arg("history job")
        .arg("10")
        .arg("/bin/echo")
        .arg("--")
        .arg("ok");
    run_now.assert().success();

    let mut exec = Command::cargo_bin("taskctl").expect("binary");
    exec.arg("--config")
        .arg(&config)
        .arg("run-now")
        .arg("history-job");
    exec.assert().success();

    let mut history = Command::cargo_bin("taskctl").expect("binary");
    history
        .arg("--config")
        .arg(&config)
        .arg("history")
        .arg("history-job")
        .arg("--limit")
        .arg("5");

    history
        .assert()
        .success()
        .stdout(predicate::str::contains("history-job"))
        .stdout(predicate::str::contains("success"));
}

#[test]
fn history_returns_empty_when_db_is_missing() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");
    std::fs::write(
        &config,
        r#"
version: 1
tasks:
  - id: health-check
    name: health check
    enabled: true
    schedule:
      kind: interval
      seconds: 10
    command:
      program: /bin/echo
"#,
    )
    .expect("write config");

    let mut history = Command::cargo_bin("taskctl").expect("binary");
    history
        .arg("--config")
        .arg(&config)
        .arg("history")
        .arg("health-check")
        .arg("--limit")
        .arg("5");

    history
        .assert()
        .success()
        .stdout(predicate::str::contains("no history records"));
    assert!(!dir.path().join("tasks.history.db").exists());
}

#[test]
fn recent_failures_lists_failed_runs() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");
    fs::write(
        &config,
        r#"
version: 1
tasks:
  - id: failing
    name: failing
    enabled: true
    concurrency:
      policy: forbid
      max_running: 1
    schedule:
      kind: interval
      seconds: 10
    command:
      program: /bin/sh
      args:
        - -c
        - exit 7
"#,
    )
    .expect("write config");

    let mut exec = Command::cargo_bin("taskctl").expect("binary");
    exec.arg("--config")
        .arg(&config)
        .arg("run-now")
        .arg("failing");
    exec.assert().code(7);

    let mut failures = Command::cargo_bin("taskctl").expect("binary");
    failures
        .arg("--config")
        .arg(&config)
        .arg("recent-failures")
        .arg("--limit")
        .arg("5");

    failures
        .assert()
        .success()
        .stdout(predicate::str::contains("failing"))
        .stdout(predicate::str::contains("failed"));
}

#[test]
fn pipeline_runs_serially_and_exposes_step_results() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");
    let output = dir.path().join("pipeline.txt");
    fs::write(
        &config,
        format!(
            r#"
version: 1
tasks:
  - id: pipeline-job
    name: pipeline job
    enabled: true
    schedule:
      kind: interval
      seconds: 10
    pipeline:
      steps:
        - id: step-1
          command:
            program: /bin/sh
            args:
              - -c
              - printf 'one\n' >> {}
        - id: step-2
          command:
            program: /bin/sh
            args:
              - -c
              - printf 'two\n' >> {}
"#,
            output.display(),
            output.display()
        ),
    )
    .expect("write config");

    let mut run_now = Command::cargo_bin("taskctl").expect("binary");
    run_now
        .arg("--config")
        .arg(&config)
        .arg("--json")
        .arg("run-now")
        .arg("pipeline-job");
    run_now
        .assert()
        .success()
        .stdout(predicate::str::contains("\"steps\""))
        .stdout(predicate::str::contains("\"step_id\": \"step-1\""))
        .stdout(predicate::str::contains("\"step_id\": \"step-2\""));

    let body = fs::read_to_string(output).expect("output");
    assert_eq!(body, "one\ntwo\n");

    let mut show = Command::cargo_bin("taskctl").expect("binary");
    show.arg("--config")
        .arg(&config)
        .arg("show")
        .arg("pipeline-job");
    show.assert()
        .success()
        .stdout(predicate::str::contains("pipeline.steps: 2"))
        .stdout(predicate::str::contains("last_steps: step-1=success"))
        .stdout(predicate::str::contains(
            "latest_history_steps: step-1=success",
        ));
}
