# taskd

`taskd` 是一个基于 Rust 和 `tokio-cron-scheduler` 的轻量级定时任务管理工具。

它的核心思路很简单：

- 用 YAML 配置文件定义任务
- 用 CLI 增删改查任务
- 用守护进程模式加载并调度任务
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
│  ├─ main.rs
│  ├─ cli.rs
│  ├─ config.rs
│  ├─ scheduler.rs
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
./target/release/taskd validate --config ./config/tasks.yaml
```

3. Start daemon locally

```bash
./target/release/taskd daemon --config ./config/tasks.yaml
```

4. List tasks

```bash
./target/release/taskd list --config ./config/tasks.yaml
```

## Configuration

配置文件为 YAML 格式，例如：

```yaml
version: 1
tasks:
  - id: "backup-db"
    name: "backup database"
    enabled: true
    schedule:
      kind: "cron"
      expr: "0 0 2 * * *"
      timezone: "Asia/Singapore"
    command:
      program: "/usr/local/bin/backup.sh"
      args: ["--full"]
      workdir: "/opt/app"
      env:
        RUST_LOG: "info"

  - id: "health-check"
    name: "health check"
    enabled: true
    schedule:
      kind: "interval"
      seconds: 300
    command:
      program: "/usr/local/bin/health_check.sh"
      args: []
```

## Configuration Fields

### Top-level

- `version`: 配置版本号
- `tasks`: 任务数组

### Task fields

- `id`: 唯一任务 ID
- `name`: 任务名称
- `enabled`: 是否启用
- `schedule`: 调度策略
- `command`: 执行命令配置

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

### Command

```yaml
command:
  program: "/usr/local/bin/backup.sh"
  args: ["--full"]
  workdir: "/opt/app"
  env:
    RUST_LOG: "info"
```

## CLI Usage

### Run daemon

```bash
taskd daemon --config /etc/taskd/tasks.yaml
```

### List tasks

```bash
taskd list --config /etc/taskd/tasks.yaml
```

### Validate config

```bash
taskd validate --config /etc/taskd/tasks.yaml
```

### Add cron task

```bash
taskd add-cron backup-db "backup database" "0 0 2 * * *" /usr/local/bin/backup.sh -- --full
```

### Add interval task

```bash
taskd add-interval health-check "health check" 300 /usr/local/bin/health_check.sh
```

### Remove task

```bash
taskd remove backup-db --config /etc/taskd/tasks.yaml
```

### Enable task

```bash
taskd enable backup-db --config /etc/taskd/tasks.yaml
```

### Disable task

```bash
taskd disable backup-db --config /etc/taskd/tasks.yaml
```

### Run task immediately

```bash
taskd run-now backup-db --config /etc/taskd/tasks.yaml
```

## Recommended Workflow

第一版推荐工作流：

1. 用 CLI 修改 YAML
2. 校验配置
3. 重启 `systemd` 服务
4. 查看服务状态和日志

例如：

```bash
taskd add-cron backup-db "backup database" "0 0 2 * * *" /usr/local/bin/backup.sh -- --full
taskd validate --config /etc/taskd/tasks.yaml
sudo systemctl restart taskd
sudo systemctl status taskd
```

## systemd Integration

示例 service 文件见 `deploy/taskd.service`。

安装示例：

```bash
sudo mkdir -p /opt/taskd /etc/taskd
sudo cp target/release/taskd /opt/taskd/taskd
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

## Ubuntu VPS One-Liner

如果要在 Ubuntu VPS 上一键部署，可以直接执行：

```bash
curl -fsSL https://raw.githubusercontent.com/eric9n/taskd/main/deploy/install.sh | sudo bash
```

脚本会完成：

- 安装系统依赖
- 安装 Rust toolchain（如果不存在）
- 拉取 `eric9n/taskd` 仓库
- 编译 release 二进制
- 安装到 `/opt/taskd/taskd`
- 安装配置到 `/etc/taskd/tasks.yaml`
- 安装并启动 `systemd` 服务

可选环境变量：

```bash
curl -fsSL https://raw.githubusercontent.com/eric9n/taskd/main/deploy/install.sh | sudo TASKD_REPO_REF=main TASKD_INSTALL_DIR=/opt/taskd bash
```

- `TASKD_REPO_URL`
- `TASKD_REPO_REF`
- `TASKD_INSTALL_DIR`
- `TASKD_CONFIG_DIR`
- `TASKD_SYSTEMD_UNIT_PATH`
- `TASKD_BUILD_ROOT`
- `TASKD_RUST_TOOLCHAIN`
- `TASKD_RUST_LOG`

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

后续可考虑：

- 配置热重载
- SQLite 记录任务运行历史
- 任务防重入控制
- 失败重试
- HTTP API
- Web 管理界面

但这些都不属于第一版必须内容。

## Development Goal

当前版本目标是先把单机、配置驱动、可由 `systemd` 托管的任务调度器打稳，再考虑第二阶段增强能力。
