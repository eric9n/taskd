# taskd

`taskd` 是一个单机定时任务调度器，配套 `taskctl` 作为控制面 CLI。

当前模型很简单：

- 任务就是一个外部命令
- 配置文件是 YAML
- `taskd daemon` 负责调度
- `taskctl` 负责校验、查看、手动执行、查历史
- 通知链路是可选能力，默认不开启

## Features

- cron 和 interval 调度
- 启用 / 禁用任务
- 手动立即执行
- YAML 配置校验
- SQLite 历史记录
- 最近失败任务查询
- 任务结果通知
  - 全局配置 `pi` renderer
  - 全局配置 Discord webhook
  - 任务级可选启用 `notify`

## Project Layout

```text
taskd/
├─ Cargo.toml
├─ config/
│  └─ tasks.yaml
├─ deploy/
│  ├─ install.sh
│  └─ taskd.service
├─ src/
│  ├─ bin/
│  │  └─ taskctl.rs
│  ├─ main.rs
│  ├─ cli.rs
│  ├─ config.rs
│  ├─ config_path.rs
│  ├─ daemon_cli.rs
│  ├─ history.rs
│  ├─ notifications.rs
│  ├─ runtime_paths.rs
│  ├─ scheduler.rs
│  ├─ state.rs
│  └─ task_runner.rs
└─ tests/
   └─ cli_integration.rs
```

## Quick Start

1. Build

```bash
cargo build --release
```

2. Validate config

```bash
./target/release/taskctl validate --config ./config/tasks.yaml
```

3. Start daemon locally

```bash
./target/release/taskd daemon --config ./config/tasks.yaml
```

4. List tasks

```bash
./target/release/taskctl list --config ./config/tasks.yaml
```

5. Run a task immediately

```bash
./target/release/taskctl run-now --config ./config/tasks.yaml health-check
```

## Install

安装脚本会：

- 下载 release 包
- 安装 `taskd` / `taskctl` 到 `/opt/taskd`
- 安装默认配置到 `/etc/taskd/tasks.yaml`
- 安装并启动 `systemd` 服务

默认安装不会启用通知，只会写入：

```yaml
notifications:
  enabled: false
```

安装命令：

```bash
sudo ./deploy/install.sh
```

## Configuration

默认配置示例：

```yaml
version: 1

notifications:
  enabled: false

tasks:
  - id: health-check
    name: health check
    enabled: true
    concurrency:
      policy: forbid
      max_running: 1
    retry:
      max_attempts: 2
      delay_seconds: 5
    schedule:
      kind: interval
      seconds: 300
    command:
      program: /bin/echo
      args:
        - ok
      timeout_seconds: 5

  - id: backup-db
    name: backup database
    enabled: true
    schedule:
      kind: cron
      expr: "0 0 2 * * *"
      timezone: Asia/Shanghai
    command:
      program: /usr/local/bin/backup.sh
      args:
        - --full
      workdir: /opt/app
      timeout_seconds: 600
      env:
        RUST_LOG: info
```

## Notify Model

默认安装和默认示例配置都会带一个 `notifications` block，但默认是 `enabled: false`。只有你显式改成 `enabled: true`，并补齐 renderer/webhook 配置后，通知链路才会生效。

通知配置分两层：

- 顶层 `notifications`
  - 配一次 `pi` renderer
  - 配一次 Discord webhook
- 任务级 `notify`
  - 不写表示不开启通知
  - 写了就表示该任务完成后需要发送通知

`notify.result_source.kind` 目前支持：

- `stdout`
- `file`

`file.path` 可以是绝对路径，也可以是相对路径。相对路径会相对于任务 `command.workdir` 解析。

如果 `notify.result_source` 读到的内容是一个 JSON object，并且顶层有 `notify: false`，则本次运行会跳过发送通知。没有这个字段时，保持现有行为，默认发送。

通知配置示例：

```yaml
version: 1

notifications:
  enabled: true
  renderer:
    program: /usr/bin/pi
    workdir: /opt/taskd
    prompt: |
      请把任务执行结果整理成简洁的 markdown 通知，
      包含任务名、状态、关键结果、是否需要关注。
    provider: google
    model: gemini-2.5-pro
    session_dir: /var/lib/taskd/pi-session
    agent_dir: /var/lib/taskd/pi-agent
    env:
      GEMINI_API_KEY: ${GEMINI_API_KEY}
  webhook:
    url_env: TASKD_WEBHOOK_URL

tasks:
  - id: backup-db
    name: backup database
    enabled: true
    schedule:
      kind: cron
      expr: "0 0 2 * * *"
    command:
      program: /usr/local/bin/backup.sh
      args:
        - --full
      workdir: /opt/app
    notify:
      result_source:
        kind: file
        path: backup-report.txt
```

可选地，你也可以让任务结果自己决定本次是否通知，例如：

```json
{
  "notify": false,
  "summary": "routine success"
}
```

## Renderer Behavior

`taskd` 不让你手写 shell wrapper，而是直接调用 `pi`：

- 在 `notifications.renderer.workdir` 下执行
- 自动把任务结果写到一个输入 JSON 文件
- 用 `@<input-file>` 的方式传给 `pi`
- 再附加全局 `prompt`
- 读取 `pi` 的 stdout 作为最终通知正文

因为 `pi` 会在配置的工作目录里执行，所以该目录下的 `AGENTS.md` 和项目上下文会生效。

## Discord Webhook Behavior

当前 webhook 发送逻辑按 Discord webhook 的 `content` 负载工作：

- 发送 JSON：`{"content":"..."}`
- 超过 2000 字符会自动截断
- 截断后会追加 `[truncated]`

这版不做 Discord embed，先保持简单。

## Enable Notifications

如果你要真正开启通知，最小步骤是：

1. 把顶层 `notifications.enabled` 改成 `true`
2. 配置 `notifications.renderer`
3. 配置 `notifications.webhook`
4. 给目标任务增加 `notify.result_source`

最小示例：

```yaml
notifications:
  enabled: true
  renderer:
    program: /usr/bin/pi
    workdir: /opt/taskd
    prompt: |
      请把任务执行结果整理成简洁的 markdown 通知。
  webhook:
    url_env: TASKD_WEBHOOK_URL

tasks:
  - id: health-check
    name: health check
    enabled: true
    schedule:
      kind: interval
      seconds: 300
    command:
      program: /bin/echo
      args:
        - ok
    notify:
      result_source:
        kind: stdout
```

## Commands

```bash
taskctl list
taskctl show <id>
taskctl validate
taskctl run-now <id>
taskctl history <id> --limit 20
taskctl recent-failures --limit 20
taskctl logs --lines 100
```

JSON 输出：

```bash
taskctl --json list
taskctl --json show <id>
taskctl --json history <id>
```

## Notes

- `taskd` 是单机调度器，不做分布式协调
- 历史记录保存在 SQLite
- 运行状态保存在 YAML state 文件
- 目前只支持 command task，不支持 pipeline
