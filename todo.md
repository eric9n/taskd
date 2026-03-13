# TODO

## P0 - 第一版必须完成

### 项目初始化
- [x] 初始化 Cargo 项目
- [x] 配置 `Cargo.toml`
- [x] 添加基础依赖：
  - [x] `tokio`
  - [x] `tokio-cron-scheduler`
  - [x] `clap`
  - [x] `serde`
  - [x] `serde_yaml`
  - [x] `anyhow`
  - [x] `tracing`
  - [x] `tracing-subscriber`

### CLI 框架
- [x] 定义 `taskd` 主命令
- [x] 支持全局 `--config` 参数
- [x] 实现子命令：
  - [x] `daemon`
  - [x] `list`
  - [x] `validate`
  - [x] `add-cron`
  - [x] `add-interval`
  - [x] `remove`
  - [x] `enable`
  - [x] `disable`
  - [x] `run-now`

### 配置系统
- [x] 定义 `AppConfig`
- [x] 定义 `TaskConfig`
- [x] 定义 `ScheduleConfig`
- [x] 定义 `CommandConfig`
- [x] 实现 YAML 读取
- [x] 实现 YAML 写回
- [x] 检查配置文件不存在时的错误提示
- [x] 检查 YAML 解析失败时的错误提示
- [x] 检查重复 `id`
- [x] 检查非法 schedule

### 任务执行
- [x] 使用 `tokio::process::Command` 执行外部命令
- [x] 支持 `args`
- [x] 支持 `workdir`
- [x] 支持 `env`
- [x] 正确返回命令退出状态
- [x] 输出任务开始日志
- [x] 输出任务完成日志
- [x] 输出失败日志

### 调度器
- [x] 创建 `JobScheduler`
- [x] 注册所有 `enabled = true` 的任务
- [x] 支持 cron 调度
- [x] 支持 interval 调度
- [x] 启动 scheduler
- [x] 监听退出信号
- [x] 优雅退出

### CLI 管理功能
- [x] `list` 输出任务清单
- [x] `validate` 校验配置
- [x] `add-cron` 写入 YAML
- [x] `add-interval` 写入 YAML
- [x] `remove` 删除指定任务
- [x] `enable` 启用指定任务
- [x] `disable` 禁用指定任务
- [x] `run-now` 立即执行指定任务

### 日志
- [x] 初始化 `tracing`
- [x] 支持 `RUST_LOG`
- [x] 输出关键生命周期日志
- [x] 保证 systemd/journald 可直接接收日志

### 部署
- [x] 编写 `taskd.service`
- [x] 在 Ubuntu VPS 测试 `systemctl start`
- [x] 在 Ubuntu VPS 测试 `systemctl restart`
- [x] 在 Ubuntu VPS 测试 `systemctl status`
- [x] 在 Ubuntu VPS 测试 `journalctl -u taskd -f`

---

## P1 - 建议尽快补齐

### 配置校验增强
- [x] 校验 `program` 非空
- [x] 校验 interval `seconds > 0`
- [x] 校验 cron 表达式合法
- [x] 校验 `workdir` 是否存在
- [x] 校验任务 ID 字符格式

### CLI 体验优化
- [x] `list` 输出更整齐
- [x] 拆分为 `taskd` 和 `taskctl` 两个 CLI 命令
- [x] `taskd` 仅保留 daemon 相关职责
- [x] `taskctl` 承担配置管理、执行、历史查询等控制面命令
- [x] 增加 `show <id>`
- [x] 增加 `--json` 输出模式
- [x] `validate` 输出更明确的错误定位

### 代码质量
- [x] 增加单元测试
- [x] 增加配置解析测试
- [x] 增加 CLI 参数测试
- [x] 增加 scheduler 注册测试
- [x] 增加命令执行测试
- [x] 补充模块文档注释

### 示例与文档
- [x] 编写 `README.md`
- [x] 编写 `task.md`
- [x] 提供示例 `tasks.yaml`
- [x] 提供安装说明
- [x] 提供 systemd 部署说明

---

## P2 - 第二阶段增强

### 性能优化
- [x] 支持多个任务并发执行，彼此不影响

### 并发控制
- [x] 为任务增加 `concurrency` 字段
- [x] 支持 `allow`
- [x] 支持 `forbid`
- [x] 支持 `replace`

### 运行状态记录
- [x] 设计任务运行结果结构
- [x] 增加最近一次运行状态展示
- [x] 增加最近一次运行时间展示

### 历史持久化
- [x] 引入 SQLite
- [x] 保存执行历史
- [x] 查询任务历史
- [x] 查询最近失败记录

### 失败重试
- [x] 增加 `retry` 配置
- [x] 支持固定次数重试
- [x] 支持重试间隔配置

### 配置热重载
- [x] 监听 YAML 文件变化
- [x] diff 新旧任务
- [x] 增量更新 scheduler
- [x] 避免重复注册
- [x] 正确移除旧任务

---

## 可能的后续增强

### 简单任务编排
- [x] 支持简单线性 pipeline
- [x] 单个 pipeline 最多 3 步
- [x] 仅支持串行执行，不支持复杂 DAG
- [x] 失败即中断后续步骤
- [x] 展示每一步执行结果

---

## 验收清单

- [x] 能从 YAML 正确读取任务
- [x] 能校验配置合法性
- [x] 能列出任务
- [x] 能新增任务
- [x] 能删除任务
- [x] 能启用/禁用任务
- [x] 能手动执行任务
- [x] 能启动 daemon
- [x] 能按 cron 执行任务
- [x] 能按 interval 执行任务
- [x] 能通过 systemd 管理
- [x] 能通过 journald 查看日志
- [x] 修改配置后重启服务能生效

---

## 当前推荐实现顺序

1. CLI 骨架
2. YAML 读写
3. `run-now`
4. scheduler + daemon
5. list / add / remove / enable / disable
6. systemd 部署
7. 校验与测试
8. 第二阶段增强
