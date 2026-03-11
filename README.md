# taskd

`taskd` 是一个基于 Rust + `tokio-cron-scheduler` 的轻量级定时任务管理工具。

它的核心思路很简单：

- 用 YAML 配置文件定义任务
- 用 CLI 增删改查任务
- 用守护进程模式加载并调度任务
- 用 `systemd` 托管进程、查看日志、开机自启

第一版重点是稳定和清晰，不追求复杂动态控制。

---

## Features

- YAML 配置驱动
- 支持 cron 任务
- 支持 interval 固定间隔任务
- 支持启用 / 禁用任务
- 支持手动立即执行任务
- 支持配置校验
- 支持通过 `systemd` 进行守护和日志管理

---

## Project Layout

```text
taskd/
├─ Cargo.toml
├─ config/
│  └─ tasks.yaml
├─ src/
│  ├─ main.rs
│  ├─ cli.rs
│  ├─ config.rs
│  ├─ scheduler.rs
│  └─ task_runner.rs
└─ deploy/
   └─ taskd.service


⸻

Quick Start

1. Build

cargo build --release

2. Validate config

./target/release/taskd validate --config ./config/tasks.yaml

3. Start daemon locally

./target/release/taskd daemon --config ./config/tasks.yaml

4. List tasks

./target/release/taskd list --config ./config/tasks.yaml


⸻

Configuration

配置文件为 YAML 格式，例如：

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


⸻

Configuration Fields

Top-level
	•	version: 配置版本号
	•	tasks: 任务数组

Task fields
	•	id: 唯一任务 ID
	•	name: 任务名称
	•	enabled: 是否启用
	•	schedule: 调度策略
	•	command: 执行命令配置

Schedule kinds

Cron

schedule:
  kind: "cron"
  expr: "0 0 2 * * *"
  timezone: "Asia/Singapore"

Interval

schedule:
  kind: "interval"
  seconds: 300

Command

command:
  program: "/usr/local/bin/backup.sh"
  args: ["--full"]
  workdir: "/opt/app"
  env:
    RUST_LOG: "info"


⸻

CLI Usage

Run daemon

taskd daemon --config /etc/taskd/tasks.yaml

List tasks

taskd list --config /etc/taskd/tasks.yaml

Validate config

taskd validate --config /etc/taskd/tasks.yaml

Add cron task

taskd add-cron backup-db "backup database" "0 0 2 * * *" /usr/local/bin/backup.sh -- --full

Add interval task

taskd add-interval health-check "health check" 300 /usr/local/bin/health_check.sh

Remove task

taskd remove backup-db --config /etc/taskd/tasks.yaml

Enable task

taskd enable backup-db --config /etc/taskd/tasks.yaml

Disable task

taskd disable backup-db --config /etc/taskd/tasks.yaml

Run task immediately

taskd run-now backup-db --config /etc/taskd/tasks.yaml


⸻

Recommended Workflow

第一版推荐工作流：
	1.	用 CLI 修改 YAML
	2.	校验配置
	3.	重启 systemd 服务
	4.	查看服务状态和日志

例如：

taskd add-cron backup-db "backup database" "0 0 2 * * *" /usr/local/bin/backup.sh -- --full
taskd validate --config /etc/taskd/tasks.yaml
sudo systemctl restart taskd
sudo systemctl status taskd


⸻

systemd Integration

示例 service 文件：

[Unit]
Description=taskd scheduler daemon
After=network.target

[Service]
Type=simple
User=root
Group=root
WorkingDirectory=/opt/taskd
ExecStart=/opt/taskd/taskd daemon --config /etc/taskd/tasks.yaml
Restart=on-failure
RestartSec=3
Environment=RUST_LOG=info
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target

安装示例：

sudo mkdir -p /opt/taskd /etc/taskd
sudo cp target/release/taskd /opt/taskd/taskd
sudo cp config/tasks.yaml /etc/taskd/tasks.yaml
sudo cp deploy/taskd.service /etc/systemd/system/taskd.service

sudo systemctl daemon-reload
sudo systemctl enable --now taskd
sudo systemctl status taskd

日志查看：

journalctl -u taskd -f


⸻

Logging

建议使用：
	•	tracing
	•	tracing-subscriber

日志输出内容包括：
	•	服务启动/退出
	•	配置加载成功/失败
	•	任务注册成功/失败
	•	任务开始执行
	•	任务执行结果
	•	命令退出状态

⸻

Design Notes

第一版明确采用：
	•	配置文件为唯一真相源
	•	CLI 只改配置，不直接操作 daemon 内存
	•	修改配置后通过 systemctl restart 生效

这样做的优点：
	•	实现简单
	•	易于排错
	•	不容易出现状态不一致

⸻

Future Improvements

后续可考虑：
	•	配置热重载
	•	SQLite 记录任务运行历史
	•	任务防重入控制
	•	失败重试
	•	HTTP API
	•	Web 管理界面

但这些都不属于第一版必须内容。

⸻

Development Goal

第一版的目标不是做一个复杂的调度平台，而是做一个：
	•	能跑
	•	稳定
	•	易部署
	•	易维护
	•	便于后续扩展

的 Rust 定时任务工具。


## 当前推荐实现顺序

1. CLI 骨架
2. YAML 读写
3. `run-now`
4. scheduler + daemon
5. list / add / remove / enable / disable
6. systemd 部署
7. 校验与测试
8. 第二阶段增强

