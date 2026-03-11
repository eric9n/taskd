可以，下面这份 task.md 可以直接作为项目任务说明文档使用。

# taskd

基于 `tokio-cron-scheduler` 的 Rust CLI 定时任务管理器。

它通过读取 YAML 配置文件来生成和管理定时任务，并以守护进程模式运行，由 `systemd` 负责托管、重启、日志管理和开机自启。

---

## 1. 项目目标

实现一个轻量、稳定、可维护的 Rust 定时任务系统，满足以下需求：

- 使用 YAML 配置文件定义任务
- 使用 `tokio-cron-scheduler` 调度任务
- 提供 CLI 管理命令
- 支持增删改查任务配置
- 支持手动立即执行任务
- 支持启用/禁用任务
- 使用 `systemd` 管理守护进程
- 将配置文件作为唯一事实来源（single source of truth）

---

## 2. 设计原则

### 2.1 配置驱动
所有任务定义都存放在 YAML 文件中，CLI 的本质是操作配置文件。

### 2.2 守护进程与管理命令分离
程序分为两种运行方式：

- `daemon` 模式：常驻运行，加载并调度任务
- `cli` 模式：管理 YAML 配置，必要时配合 `systemctl restart` 重载服务

### 2.3 systemd 托管
不自行实现后台 daemonize、日志轮转、进程守护等功能，而是交给 `systemd`。

### 2.4 第一版优先稳定
第一版不做复杂热重载，不做数据库，不做 RPC，不做 socket 控制。
推荐工作流：

1. CLI 修改 YAML
2. `systemctl restart taskd`
3. 守护进程重新读取配置并注册任务

---

## 3. 功能范围

### 3.1 必做功能

- 读取 YAML 配置文件
- 校验配置是否合法
- 启动调度器并注册所有启用状态的任务
- 支持 cron 表达式任务
- 支持固定间隔任务
- 执行外部命令
- 通过 CLI 列出任务
- 通过 CLI 新增任务
- 通过 CLI 删除任务
- 通过 CLI 启用/禁用任务
- 通过 CLI 立即执行某个任务
- 通过 `systemd` 管理整个守护进程

### 3.2 暂不实现

- 配置文件热重载
- Web UI
- 数据库存储
- 任务执行历史持久化
- 分布式调度
- 多节点选主
- HTTP API
- 动态在线增删 scheduler 内存任务而不重启进程

---

## 4. 项目结构

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

5. 配置文件设计

配置文件采用 YAML 格式。

示例：

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

6. 配置字段说明

顶层字段
	•	version: 配置版本号
	•	tasks: 任务列表

每个任务字段
	•	id: 任务唯一标识，必须唯一
	•	name: 任务名称，便于展示
	•	enabled: 是否启用
	•	schedule: 调度配置
	•	command: 执行命令配置

schedule

支持两种类型：

cron 任务

schedule:
  kind: "cron"
  expr: "0 0 2 * * *"
  timezone: "Asia/Singapore"

字段说明：
	•	kind: 固定为 cron
	•	expr: cron 表达式
	•	timezone: 可选，时区

interval 任务

schedule:
  kind: "interval"
  seconds: 300

字段说明：
	•	kind: 固定为 interval
	•	seconds: 每隔多少秒执行一次

command

command:
  program: "/usr/local/bin/backup.sh"
  args: ["--full"]
  workdir: "/opt/app"
  env:
    RUST_LOG: "info"

字段说明：
	•	program: 可执行文件路径
	•	args: 命令参数
	•	workdir: 可选，工作目录
	•	env: 可选，环境变量

⸻

7. CLI 设计

程序名暂定为：

taskd

7.1 启动守护进程

taskd daemon --config /etc/taskd/tasks.yaml

功能：
	•	读取配置
	•	注册所有已启用任务
	•	启动 scheduler
	•	常驻运行，等待系统信号退出

7.2 查看任务列表

taskd list --config /etc/taskd/tasks.yaml

输出任务的基本信息：
	•	id
	•	enabled/disabled
	•	name
	•	schedule

7.3 校验配置

taskd validate --config /etc/taskd/tasks.yaml

功能：
	•	检查 YAML 是否可解析
	•	检查字段是否合法
	•	检查任务 id 是否重复
	•	检查 schedule 是否有效

7.4 新增 cron 任务

taskd add-cron <id> <name> <expr> <program> [args...]

示例：

taskd add-cron backup-db "backup database" "0 0 2 * * *" /usr/local/bin/backup.sh -- --full

7.5 新增 interval 任务

taskd add-interval <id> <name> <seconds> <program> [args...]

示例：

taskd add-interval health-check "health check" 300 /usr/local/bin/health_check.sh

7.6 删除任务

taskd remove <id> --config /etc/taskd/tasks.yaml

7.7 启用任务

taskd enable <id> --config /etc/taskd/tasks.yaml

7.8 禁用任务

taskd disable <id> --config /etc/taskd/tasks.yaml

7.9 立即执行任务

taskd run-now <id> --config /etc/taskd/tasks.yaml

功能：
	•	不经过 scheduler
	•	直接读取配置并执行一次命令
	•	用于调试或临时手动运行

⸻

8. 守护进程行为设计

启动流程
	1.	启动 taskd daemon
	2.	读取 YAML 配置
	3.	过滤出 enabled = true 的任务
	4.	将任务注册到 tokio-cron-scheduler
	5.	启动 scheduler
	6.	进入常驻状态
	7.	等待退出信号

退出流程

收到系统信号后：
	•	输出日志
	•	停止 scheduler
	•	优雅退出

⸻

9. 任务执行模型

任务本质上是执行外部命令。

推荐使用：
	•	tokio::process::Command

执行时支持：
	•	参数传递
	•	工作目录设置
	•	环境变量注入

执行结果

需要记录：
	•	任务开始执行
	•	任务结束状态
	•	退出码
	•	错误信息

日志交由 tracing 输出，由 journald 接管。

⸻

10. systemd 集成

使用 systemd 托管守护进程。

service 文件示例

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

systemd 职责
	•	进程守护
	•	异常重启
	•	开机自启
	•	日志收集
	•	状态查看

常用命令

sudo systemctl daemon-reload
sudo systemctl enable --now taskd
sudo systemctl restart taskd
sudo systemctl status taskd
journalctl -u taskd -f


⸻

11. 推荐工作流

修改任务配置后的标准流程

taskd add-cron ...
taskd validate --config /etc/taskd/tasks.yaml
sudo systemctl restart taskd
sudo systemctl status taskd

禁用某个任务后的流程

taskd disable backup-db --config /etc/taskd/tasks.yaml
sudo systemctl restart taskd

手动测试任务

taskd run-now backup-db --config /etc/taskd/tasks.yaml


⸻

12. 模块划分

main.rs

负责：
	•	程序入口
	•	初始化日志
	•	解析 CLI 参数
	•	调度子命令逻辑

cli.rs

负责：
	•	定义命令行参数结构
	•	定义子命令

config.rs

负责：
	•	YAML 配置结构体
	•	配置文件读写
	•	配置校验逻辑

scheduler.rs

负责：
	•	创建 JobScheduler
	•	注册任务
	•	启动和停止 scheduler

task_runner.rs

负责：
	•	执行外部命令
	•	处理退出码
	•	输出执行日志

⸻

13. 错误处理要求

统一使用 anyhow 处理应用层错误。

需要覆盖的错误场景包括：
	•	配置文件不存在
	•	YAML 解析失败
	•	任务 id 冲突
	•	无效 cron 表达式
	•	找不到指定任务
	•	外部命令无法启动
	•	外部命令执行失败
	•	工作目录不存在

CLI 输出要尽量明确，方便排查问题。

⸻

14. 日志要求

使用：
	•	tracing
	•	tracing-subscriber

至少输出以下内容：
	•	程序启动
	•	配置加载成功/失败
	•	任务注册成功/失败
	•	任务触发开始
	•	任务执行成功/失败
	•	程序收到退出信号
	•	程序退出

日志最终通过 journald 查看。

⸻

15. 第一版限制与约束

第一版明确采用“配置文件驱动 + 重启服务生效”的方式。

即：
	•	CLI 不直接操作 daemon 内存中的任务表
	•	daemon 不监听配置文件变化
	•	修改配置后通过 systemctl restart taskd 生效

这样做的好处：
	•	结构简单
	•	状态单一
	•	便于调试
	•	更适合第一版快速落地

⸻

16. 第二阶段可扩展方向

后续可以考虑逐步加入以下功能：

16.1 配置热重载

监听 YAML 文件变化，自动重建任务表。

16.2 任务运行历史

将任务执行记录写入 sqlite。

可记录：
	•	task_id
	•	start_time
	•	end_time
	•	status
	•	exit_code
	•	stderr/stdout 摘要

16.3 防重入控制

为任务增加并发策略，例如：

concurrency: "forbid"

可选策略：
	•	allow: 允许重入
	•	forbid: 上一轮没结束则跳过
	•	replace: 新任务替换旧任务

16.4 失败重试

为任务增加失败后自动重试机制。

16.5 HTTP API / 管理界面

在需要时再考虑，不作为当前版本重点。

⸻

17. 开发任务拆分

阶段 1：基础骨架
	•	初始化 Cargo 项目
	•	添加依赖
	•	完成 CLI 基本框架
	•	完成日志初始化

阶段 2：配置系统
	•	定义 YAML 结构体
	•	实现配置读取
	•	实现配置保存
	•	实现配置校验

阶段 3：任务执行
	•	实现外部命令执行器
	•	支持 args/workdir/env
	•	补充执行日志

阶段 4：调度器
	•	集成 tokio-cron-scheduler
	•	实现 cron 任务注册
	•	实现 interval 任务注册
	•	实现 daemon 模式

阶段 5：CLI 管理
	•	list
	•	validate
	•	add-cron
	•	add-interval
	•	remove
	•	enable
	•	disable
	•	run-now

阶段 6：部署与 systemd
	•	编写 service 文件
	•	本地测试启动/停止
	•	测试日志查看
	•	测试开机自启

⸻

18. 验收标准

满足以下条件即视为第一版完成：
	•	可以从 YAML 正常读取任务
	•	可以通过 CLI 查看和管理任务
	•	可以启动 daemon 并调度已启用任务
	•	可以执行外部命令
	•	可以通过 systemd 启动和管理服务
	•	修改配置后重启服务能正确生效
	•	错误日志和运行日志可通过 journalctl 查看

⸻

19. 总结

本项目第一版的核心思路是：
	•	用 YAML 管任务定义
	•	用 Rust CLI 管配置
	•	用 tokio-cron-scheduler 做调度
	•	用 systemd 做守护

优先保证：
	•	结构清晰
	•	易于实现
	•	易于部署
	•	易于排障

而不是一开始就引入热重载、数据库、Web 管理界面等复杂能力。
