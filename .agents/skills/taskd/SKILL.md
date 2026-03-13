---
name: taskd
description: taskd is a single-host scheduler daemon for managing cron and interval tasks from a YAML config, with taskctl as its control-plane CLI.
---

# taskd

`taskd` is a single-host scheduler daemon. `taskctl` is its control-plane CLI.

## Remote host

- For this project, when operating the installed scheduler, run commands on `vps(host:srv1313960)`, not on the local machine
- On the VPS, prefer the absolute CLI path to avoid `PATH` drift: `ssh srv1313960 '/opt/taskd/taskctl ...'`
- Use `ssh srv1313960 'taskctl ...'` only if you have already confirmed `taskctl` is in `PATH`
- Use local `taskctl` only when you are explicitly working with this repository's local `config/tasks.yaml`

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

- `--config <PATH>`: config file path; same lookup rule as `taskd`
- `--json`: return machine-readable JSON instead of table/text output

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
```

Machine-readable output:

```bash
taskctl --json list
taskctl --json show <id>
taskctl --json history <id>
```

Meaning:

- `list`: show all configured tasks with enabled status, schedule, and latest runtime status
- `show <id>`: show one task, including config, latest runtime state, and most recent history row
- `validate`: check YAML structure and task semantics without changing anything
- `run-now <id>`: execute a task immediately outside its normal schedule
- `history <id>`: show recent execution history for one task
- `recent-failures`: show failed runs across all tasks
- `logs --lines <N>`: show recent `taskd` service logs via `journalctl`
- Prefer `--json` when another agent needs to parse results reliably

## Common taskctl workflows

Add or update tasks through the CLI:

```bash
taskctl add-cron <id> <name> "<cron>" <program> -- --arg1 --arg2
taskctl add-interval <id> <name> <seconds> <program> -- --arg1 --arg2
taskctl enable <id>
taskctl disable <id>
taskctl remove <id>
```

Important options for `add-cron` and `add-interval`:

- positional `id`: unique task ID
- positional `name`: human-readable task name
- positional `program`: executable or script path
- trailing `-- ...`: arguments passed to the target program
- `--enabled`: whether the task starts enabled
- `--timezone <TZ>`: cron timezone; only for `add-cron`
- `--max-running <1..3>`: max concurrent runs for the same task
- `--concurrency-policy <allow|forbid|replace>`:
  - `allow`: permit overlap up to `--max-running`
  - `forbid`: skip new triggers while one run is active
  - `replace`: cancel the old run and start the new one
- `--workdir <PATH>`: working directory for the spawned process
- `--timeout-seconds <N>`: kill the process if it exceeds the timeout
- `--retry-max-attempts <N>`: retry count after the initial failed run
- `--retry-delay-seconds <N>`: seconds between retries
- `--env KEY=VALUE`: environment variable; may be repeated

Validate and execute:

```bash
taskctl validate
taskctl run-now <id>
```

Task state management:

- `enable <id>`: mark a task enabled in YAML
- `disable <id>`: mark a task disabled in YAML
- `remove <id>`: delete a task from YAML
- After config edits, `taskd` will reload automatically when watching the config file

## Task model

Each task is one of:

- `command`: single external command
- `pipeline`: linear pipeline with 2 to 3 steps

Important rules:

- `command` and `pipeline` are mutually exclusive
- concurrency policies: `forbid`, `allow`, `replace`
- `max_running` is limited to `1..=3`
- retry applies to daemon-triggered runs
- runtime state stores only the latest outcome
- history persists every finished run in SQLite

Practical implications:

- `run-now` is useful for smoke tests and manual execution
- `pipeline` is YAML-defined; CLI add commands create single-command tasks
- use `forbid` for non-reentrant jobs
- use `replace` for jobs where only the latest run matters
- use `allow` only when overlap is safe

## Verification checklist

On an installed host, verify:

```bash
systemctl status taskd --no-pager
ssh srv1313960 '/opt/taskd/taskctl list'
journalctl -u taskd -n 100 --no-pager
```

Expect:

- `taskd.service` is active
- `/opt/taskd/taskctl list` can read `/etc/taskd/tasks.yaml` without `--config`
- logs show scheduler startup without config validation failures

Quick debugging commands:

```bash
ssh srv1313960 '/opt/taskd/taskctl validate'
ssh srv1313960 '/opt/taskd/taskctl show <id>'
ssh srv1313960 '/opt/taskd/taskctl history <id> --limit 20'
ssh srv1313960 '/opt/taskd/taskctl recent-failures --limit 20'
ssh srv1313960 '/opt/taskd/taskctl logs --lines 100'
```
