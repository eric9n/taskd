# taskd

`taskd` / `taskctl` 是一组基于 Rust 和 `tokio-cron-scheduler` 的轻量级定时任务管理工具。

它的核心思路很简单：

- 用 YAML 配置文件定义任务
- 用 `taskctl` 增删改查任务、手动执行任务、查询历史
- 用 `taskd daemon` 加载并调度任务
- 用 `systemd` 托管进程、查看日志、开机自启

第一版重点是稳定和清晰，不追求复杂动态控制。

## Features

- YAML 配置驱动
- 支持 cron 任务
- 支持 interval 固定间隔任务
- 支持启用 / 禁用任务
- 支持手动立即执行任务
- 支持配置校验
- 支持通过 `systemd` 进行守护和日志管理

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
│  ├─ daemon_cli.rs
│  ├─ cli.rs
│  ├─ config.rs
│  ├─ history.rs
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

## Configuration

配置文件为 YAML 格式，例如：

```yaml
version: 1
tasks:
  - id: "backup-db"
    name: "backup database"
    enabled: true
    concurrency:
      policy: "forbid"
      max_running: 1
    schedule:
      kind: "cron"
      expr: "0 0 2 * * *"
      timezone: "Asia/Singapore"
    command:
      program: "/usr/local/bin/backup.sh"
      args: ["--full"]
      workdir: "/opt/app"
      timeout_seconds: 600
      env:
        RUST_LOG: "info"

  - id: "health-check"
    name: "health check"
    enabled: true
    concurrency:
      policy: "allow"
      max_running: 2
    schedule:
      kind: "interval"
      seconds: 300
    command:
      program: "/usr/local/bin/health_check.sh"
      args: []

  - id: "daily-pipeline"
    name: "daily pipeline"
    enabled: false
    schedule:
      kind: "cron"
      expr: "0 0 3 * * *"
    pipeline:
      steps:
        - id: "extract"
          command:
            program: "/bin/sh"
            args: ["-c", "echo extract"]
        - id: "transform"
          command:
            program: "/bin/sh"
            args: ["-c", "echo transform"]
        - id: "publish"
          command:
            program: "/bin/sh"
            args: ["-c", "echo publish"]
```

## Configuration Fields

### Top-level

- `version`: 配置版本号
- `tasks`: 任务数组

### Task fields

- `id`: 唯一任务 ID
- `name`: 任务名称
- `enabled`: 是否启用
- `concurrency`: 同一任务的并发配置
- `schedule`: 调度策略
- `retry`: 失败重试配置
- `command`: 单步执行命令配置
- `pipeline`: 简单线性 pipeline 配置

### Concurrency

```yaml
concurrency:
  policy: "forbid"
  max_running: 1
```

- `policy`: `forbid` 表示禁止重入，`allow` 表示允许有限重入
- `max_running`: 同一任务最大并发数，范围 `1..=3`
- `forbid`: 必须配合 `max_running: 1`
- `allow`: 允许 `1..=3`，超过上限的触发会被跳过
- `replace`: 必须配合 `max_running: 1`，新触发会终止旧实例并启动新的

### Schedule kinds

#### Cron

```yaml
schedule:
  kind: "cron"
  expr: "0 0 2 * * *"
  timezone: "Asia/Singapore"
```

#### Interval

```yaml
schedule:
  kind: "interval"
  seconds: 300
```

### Retry

```yaml
retry:
  max_attempts: 2
  delay_seconds: 5
```

- `max_attempts`: 首次失败后的最大重试次数，`0` 表示不重试
- `delay_seconds`: 每次重试前的等待秒数；当 `max_attempts > 0` 时必须大于 `0`
- 当前重试只在 `taskd daemon` 的调度执行路径生效

### Pipeline

```yaml
pipeline:
  steps:
    - id: "extract"
      command:
        program: "/bin/sh"
        args: ["-c", "echo extract"]
    - id: "transform"
      command:
        program: "/bin/sh"
        args: ["-c", "echo transform"]
```

- `command` 和 `pipeline` 二选一
- pipeline 仅支持线性串行执行
- 单个 pipeline 必须有 `2..=3` 步
- 任一步失败、超时或取消，后续步骤不会继续执行
- 每一步结果会进入最终 summary，并记录到状态文件与历史库

### Command

```yaml
command:
  program: "/usr/local/bin/backup.sh"
  args: ["--full"]
  workdir: "/opt/app"
  timeout_seconds: 600
  env:
    RUST_LOG: "info"
```

- `program`: 可执行文件路径
- `args`: 命令参数
- `workdir`: 可选工作目录
- `timeout_seconds`: 可选超时秒数，超过后会终止子进程
- `env`: 可选环境变量

## Hot Reload

- `taskd daemon` 使用文件系统事件监听配置文件变化
- 监听目标是配置文件所在目录，并只对目标配置文件事件做过滤
- 保存时会做一个很短的 debounce，避免编辑器一次保存触发多次 reload
- 同时保留一个 `300s` 的低频 fallback polling，用于特殊挂载场景兜底
- 配置合法时会按任务 `id` 增量更新 scheduler
- 未变化的任务不会重复注册
- 被删除或被禁用的任务会从 scheduler 中移除
- 配置非法时会保留旧调度继续运行，并记录告警日志
- 每次成功加载配置后，都会保存一份 last-known-good 快照
- 如果 daemon 启动时发现当前配置非法，但存在 last-known-good 快照，会记录告警并继续使用该快照启动

## CLI Usage

默认配置路径规则：

- 如果 `/etc/taskd/tasks.yaml` 存在，`taskd` 和 `taskctl` 默认读取它
- 否则回退到仓库内的 `config/tasks.yaml`
- 你仍然可以用 `--config` 显式指定别的配置文件

### Run daemon

```bash
taskd daemon
```

### List tasks

```bash
taskctl list
```

`list` 会同时展示最近一次运行状态和时间。如果状态文件不存在，会显示 `-`。

### Show task details

```bash
taskctl show backup-db --config /etc/taskd/tasks.yaml
```

`show` 会展示任务配置、最近一次运行状态，以及最近一条历史记录。

### JSON output

```bash
taskctl --json list --config /etc/taskd/tasks.yaml
taskctl --json show backup-db --config /etc/taskd/tasks.yaml
taskctl --json history backup-db --config /etc/taskd/tasks.yaml --limit 20
```

`--json` 是 `taskctl` 的全局参数，适合脚本调用。当前已覆盖 `list`、`show`、`validate`、`history`、`recent-failures`、`run-now`，以及常见的增删启停命令成功响应。

### Validate config

```bash
taskctl validate --config /etc/taskd/tasks.yaml
```

`validate` 的错误会尽量指到具体字段路径，例如 `tasks[0].command.timeout_seconds`。

### Add cron task

```bash
taskctl add-cron backup-db "backup database" "0 0 2 * * *" /usr/local/bin/backup.sh --concurrency-policy replace --retry-max-attempts 2 --retry-delay-seconds 10 -- --full
```

### Add interval task

```bash
taskctl add-interval health-check "health check" 300 /usr/local/bin/health_check.sh --concurrency-policy allow --max-running 2 --retry-max-attempts 2 --retry-delay-seconds 5
```

### Remove task

```bash
taskctl remove backup-db --config /etc/taskd/tasks.yaml
```

### Enable task

```bash
taskctl enable backup-db --config /etc/taskd/tasks.yaml
```

### Disable task

```bash
taskctl disable backup-db --config /etc/taskd/tasks.yaml
```

### Run task immediately

```bash
taskctl run-now backup-db --config /etc/taskd/tasks.yaml
```

### Query task history

```bash
taskctl history backup-db --config /etc/taskd/tasks.yaml --limit 20
```

### Query recent failures

```bash
taskctl recent-failures --config /etc/taskd/tasks.yaml --limit 20
```

### Show service logs

```bash
taskctl logs
taskctl logs --lines 200
taskctl logs --follow
```

`taskctl logs` 是对 `journalctl -u taskd` 的便捷封装，适合直接在 systemd 主机上排障。

## Recommended Workflow

第一版推荐工作流：

1. 用 `taskctl` 修改 YAML
2. 校验配置
3. 重启 `systemd` 服务
4. 查看服务状态和日志

例如：

```bash
taskctl add-cron backup-db "backup database" "0 0 2 * * *" /usr/local/bin/backup.sh -- --full
taskctl validate --config /etc/taskd/tasks.yaml
sudo systemctl restart taskd
sudo systemctl status taskd
```

## systemd Integration

示例 service 文件见 `deploy/taskd.service`。

安装示例：

```bash
sudo mkdir -p /opt/taskd /etc/taskd
sudo cp target/release/taskd /opt/taskd/taskd
sudo cp target/release/taskctl /opt/taskd/taskctl
sudo cp config/tasks.yaml /etc/taskd/tasks.yaml
sudo cp deploy/taskd.service /etc/systemd/system/taskd.service

sudo systemctl daemon-reload
sudo systemctl enable --now taskd
sudo systemctl status taskd
```

日志查看：

```bash
journalctl -u taskd -f
```

## Runtime State

`taskd` 会维护一个轻量状态快照文件：

- `config/tasks.yaml` -> `config/tasks.state.yaml`
- `/etc/taskd/tasks.yaml` -> `/var/lib/taskd/tasks.state.yaml`

另外还会维护最近一次成功加载的配置快照：

- `config/tasks.yaml` -> `config/tasks.last-good.yaml`
- `/etc/taskd/tasks.yaml` -> `/var/lib/taskd/tasks.last-good.yaml`

这份文件会记录每个任务最近一次运行的：

- 状态
- 开始时间
- 结束时间
- 结果摘要

daemon 调度执行和 `taskctl run-now` 都会更新这份状态文件，`taskctl list` 和 `taskctl show <id>` 会读取它来展示最近运行信息。

## History Persistence

`taskd` 还会维护 SQLite 历史库：

- `config/tasks.yaml` -> `config/tasks.history.db`
- `/etc/taskd/tasks.yaml` -> `/var/lib/taskd/tasks.history.db`

这份数据库会记录每次执行的：

- `task_id`
- `status`
- `summary`
- `exit_code`
- `started_at`
- `finished_at`

可通过 `taskctl history <id>` 查询某任务历史，通过 `taskctl recent-failures` 查询最近失败记录。

## Ubuntu VPS One-Liner

如果要在 Ubuntu VPS 上一键部署，可以直接执行：

```bash
curl -fsSL https://raw.githubusercontent.com/eric9n/taskd/main/deploy/install.sh | sudo bash
```

脚本会完成：

- 安装系统依赖
- 从 GitHub Release 下载预编译二进制包
- 安装到 `/opt/taskd/taskd` 和 `/opt/taskd/taskctl`
- 安装配置到 `/etc/taskd/tasks.yaml`
- 安装运行时数据目录到 `/var/lib/taskd`
- 安装并启动 `systemd` 服务

可选环境变量：

```bash
curl -fsSL https://raw.githubusercontent.com/eric9n/taskd/main/deploy/install.sh | sudo TASKD_RELEASE=v0.1.0 TASKD_INSTALL_DIR=/opt/taskd bash
```

- `TASKD_GITHUB_REPOSITORY`
- `TASKD_RELEASE`
- `TASKD_ASSET_NAME`
- `TASKD_INSTALL_DIR`
- `TASKD_CONFIG_DIR`
- `TASKD_DATA_DIR`
- `TASKD_SYSTEMD_UNIT_PATH`
- `TASKD_DOWNLOAD_ROOT`
- `TASKD_RUST_LOG`

默认会下载 `latest` release，对应资产名是 `taskd-x86_64-unknown-linux-gnu.tar.gz`。

## Release

仓库已配置 tag 驱动的 GitHub Actions release workflow：

- workflow 文件：`.github/workflows/release.yml`
- 触发条件：push `v*` tag
- 产物：
  - `taskd-x86_64-unknown-linux-gnu.tar.gz`
  - `taskd-x86_64-unknown-linux-gnu.tar.gz.sha256`

发布方式：

```bash
git tag v0.1.0
git push origin v0.1.0
```

workflow 会自动：

- 执行 `cargo test --locked`
- 构建 release 二进制
- 打包 `taskd`、`taskctl`、示例配置和 systemd service
- 创建或更新对应 GitHub Release，并上传资产

## Logging

建议使用：

- `tracing`
- `tracing-subscriber`

日志输出内容包括：

- 服务启动 / 退出
- 配置加载成功 / 失败
- 任务注册成功 / 失败
- 任务开始执行
- 任务执行结果
- 命令退出状态

## Design Notes

第一版明确采用：

- 配置文件为唯一真相源
- CLI 只改配置，不直接操作 daemon 内存
- 修改配置后通过 `systemctl restart` 生效

这样做的优点：

- 实现简单
- 易于排错
- 不容易出现状态不一致

## Future Improvements

后续只考虑仍然符合“单机 CLI 管理 cron / interval”的增强：

- 配置热重载
- 失败重试
- 简单线性 pipeline

## Development Goal

当前版本目标是先把单机、配置驱动、可由 `systemd` 托管的任务调度器打稳，再考虑第二阶段增强能力。
