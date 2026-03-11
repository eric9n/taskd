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

    let mut cmd = Command::cargo_bin("taskd").expect("binary");
    cmd.arg("--config").arg(&config).arg("validate");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("config is valid"));
}

#[test]
fn add_interval_writes_task_to_yaml() {
    let dir = tempdir().expect("tempdir");
    let config = dir.path().join("tasks.yaml");

    let mut cmd = Command::cargo_bin("taskd").expect("binary");
    cmd.arg("--config")
        .arg(&config)
        .arg("add-interval")
        .arg("health-check")
        .arg("health check")
        .arg("60")
        .arg("/bin/echo")
        .arg("--")
        .arg("ok");

    cmd.assert().success();

    let body = fs::read_to_string(config).expect("config file");
    assert!(body.contains("health-check"));
    assert!(body.contains("seconds: 60"));
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

    let mut cmd = Command::cargo_bin("taskd").expect("binary");
    cmd.arg("--config")
        .arg(&config)
        .arg("run-now")
        .arg("failing");

    cmd.assert().code(7);
}
