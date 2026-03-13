---
name: taskd
description: taskd is a single-host scheduler daemon for managing cron and interval tasks from a YAML config, with taskctl as its control-plane CLI.
---

# taskd

`taskd` is a single-host scheduler daemon. `taskctl` is its control-plane CLI.

## Execution context

- For this project, operate `taskd` and `taskctl` on the local machine by default
- Prefer the installed scheduler config when it exists at `/etc/taskd/tasks.yaml`
- Use a repo-local config such as `./config/tasks.yaml` only when the task is explicitly about a local checkout or test setup
- Do not assume any remote VPS, SSH hop, or host alias unless the user explicitly asks for remote operations

## Command paths

- Prefer `taskd` and `taskctl` from `PATH`
- If they are not in `PATH`, use `/opt/taskd/taskd` and `/opt/taskd/taskctl`
- Use absolute paths in automation when you need deterministic behavior

## Default config behavior

- Prefer `/etc/taskd/tasks.yaml` when it exists
- Otherwise fall back to `./config/tasks.yaml`
- Both `taskd` and `taskctl` accept `--config` to override
- For `/etc/taskd/tasks.yaml`, runtime state is `/var/lib/taskd/tasks.state.yaml`
- For `/etc/taskd/tasks.yaml`, execution history is `/var/lib/taskd/tasks.history.db`
- For non-system config paths, state and history live next to the config file

## Global options

`taskd`:

- `--config <PATH>`: config file path; default lookup is `/etc/taskd/tasks.yaml`, then `./config/tasks.yaml`

`taskctl`:

- `--config <PATH>`: global option for all subcommands; same lookup rule as `taskd`
- `--json`: global option; meaningful for `list`, `show`, `validate`, `history`, `recent-failures`, `logs`, `run-now`, `report daily`, and also `remove` / `enable` / `disable`
- `taskctl logs --json` cannot be combined with `--follow`
- `add-cron` and `add-interval` currently accept `--json` because it is global, but they do not emit structured JSON output; do not rely on it

## Daemon commands

```bash
taskd daemon
taskd daemon --config /etc/taskd/tasks.yaml
```

Meaning:

- `taskd daemon`: start the background scheduler and config watcher
- Use this under `systemd` on a server, not as an interactive foreground tool unless debugging

## Control-plane commands

```bash
taskctl list
taskctl show <id>
taskctl validate
taskctl run-now <id>
taskctl history <id> --limit 20
taskctl recent-failures --limit 20
taskctl logs --lines 100
taskctl logs --lines 100 --follow
taskctl report daily --date 2026-03-12 --timezone Asia/Shanghai
```

Machine-readable output:

```bash
taskctl --json list
taskctl --json show <id>
taskctl --json validate
taskctl --json history <id>
taskctl --json recent-failures --limit 20
taskctl --json run-now <id>
taskctl --json report daily --date 2026-03-12 --timezone Asia/Shanghai
```

Meaning:

- `list`: show all configured tasks with enabled status, schedule, and latest runtime status
- `show <id>`: show one task, including config, latest runtime state, and most recent history row
- `validate`: check YAML structure and task semantics without changing anything
- `run-now <id>`: execute a task immediately, record runtime state and history, optionally trigger notifications, and exit with the task's exit code
- `history <id>`: show recent execution history for one task; default limit is `20`
- `recent-failures`: show failed runs across all tasks; default limit is `20`
- `logs --lines <N>`: show recent `taskd` service logs via `journalctl`; `--follow` streams continuously
- `report daily`: build a daily summary from history for the requested `--date` and `--timezone`
- Prefer `--json` when another agent needs to parse results reliably

## Common taskctl workflows

Add or update tasks through the CLI:

```bash
taskctl add-cron [OPTIONS] <id> <name> "<expr>" <program> -- [args...]
taskctl add-interval [OPTIONS] <id> <name> <seconds> <program> -- [args...]
taskctl enable <id>
taskctl disable <id>
taskctl remove <id>
```

Important options for `add-cron` and `add-interval`:

- positional `id`: unique task ID
- positional `name`: human-readable task name
- positional `program`: executable or script path
- trailing `-- [args...]`: arguments passed to the target program
- new tasks are enabled by default; disable them afterward if needed
- `--timezone <TZ>`: cron timezone; only for `add-cron`
- `--max-running <1..3>`: max concurrent runs for the same task; default is `1`
- `--concurrency-policy <allow|forbid|replace>`: overlap policy; default is `forbid`
- `--workdir <PATH>`: working directory for the spawned process
- `--timeout-seconds <N>`: kill the process if it exceeds the timeout
- `--retry-max-attempts <N>`: retry count after the initial failed run; default is `0`
- `--retry-delay-seconds <N>`: seconds between retries; default is `1`
- `--env KEY=VALUE`: environment variable; may be repeated

Cron expression details for `add-cron`:

- The parser is `cron::Schedule::from_str` from the Rust `cron` crate
- In this project, use 6-field cron by default: `second minute hour day-of-month month day-of-week`
- A 7th `year` field is also accepted
- Standard 5-field cron like `0 2 * * *` is invalid here
- Valid examples:
  - `0 0 2 * * *`
  - `0/30 * * * * *`

Task state management:

- `enable <id>`: mark a task enabled in YAML
- `disable <id>`: mark a task disabled in YAML
- `remove <id>`: delete a task from YAML and remove its runtime-state entry
- After config edits, `taskd` will reload automatically when watching the config file

## Task model

Each task is:

- `command`: single external command

Important rules:

- concurrency policies: `forbid`, `allow`, `replace`
- `max_running` is limited to `1..=3`
- retry applies to daemon-triggered runs
- runtime state stores only the latest outcome
- history persists every finished run in SQLite

Practical implications:

- `run-now` is useful for smoke tests and manual execution
- use `forbid` for non-reentrant jobs
- use `replace` for jobs where only the latest run matters
- use `allow` only when overlap is safe

## Notifications

Notifications are optional and disabled by default.

- The default sample config keeps:
  - `notifications.enabled: false`
- A task only sends notifications when both are true:
  - top-level `notifications.enabled: true`
  - the task defines `notify.result_source`
- Supported `notify.result_source.kind` values are:
  - `stdout`
  - `file`
- The global renderer is `pi`
- `pi` is executed in `notifications.renderer.workdir`, so that directory's `AGENTS.md` and repo context apply
- Discord delivery is sent as webhook JSON `content`
- Messages longer than 2000 characters are truncated before sending

## Verification checklist

On an installed host, verify:

```bash
systemctl status taskd --no-pager
/opt/taskd/taskctl list
journalctl -u taskd -n 100 --no-pager
```

Expect:

- `taskd.service` is active
- `/opt/taskd/taskctl list` can read `/etc/taskd/tasks.yaml` without `--config`
- logs show scheduler startup without config validation failures

Quick debugging commands:

```bash
/opt/taskd/taskctl validate
/opt/taskd/taskctl show <id>
/opt/taskd/taskctl history <id> --limit 20
/opt/taskd/taskctl recent-failures --limit 20
/opt/taskd/taskctl logs --lines 100
```
