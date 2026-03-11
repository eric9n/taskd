# TODO

## P0 - 第一版必须完成

### 项目初始化
- [ ] 初始化 Cargo 项目
- [ ] 配置 `Cargo.toml`
- [ ] 添加基础依赖：
  - [ ] `tokio`
  - [ ] `tokio-cron-scheduler`
  - [ ] `clap`
  - [ ] `serde`
  - [ ] `serde_yaml`
  - [ ] `anyhow`
  - [ ] `tracing`
  - [ ] `tracing-subscriber`

### CLI 框架
- [ ] 定义 `taskd` 主命令
- [ ] 支持全局 `--config` 参数
- [ ] 实现子命令：
  - [ ] `daemon`
  - [ ] `list`
  - [ ] `validate`
  - [ ] `add-cron`
  - [ ] `add-interval`
  - [ ] `remove`
  - [ ] `enable`
  - [ ] `disable`
  - [ ] `run-now`

### 配置系统
- [ ] 定义 `AppConfig`
- [ ] 定义 `TaskConfig`
- [ ] 定义 `ScheduleConfig`
- [ ] 定义 `CommandConfig`
- [ ] 实现 YAML 读取
- [ ] 实现 YAML 写回
- [ ] 检查配置文件不存在时的错误提示
- [ ] 检查 YAML 解析失败时的错误提示
- [ ] 检查重复 `id`
- [ ] 检查非法 schedule

### 任务执行
- [ ] 使用 `tokio::process::Command` 执行外部命令
- [ ] 支持 `args`
- [ ] 支持 `workdir`
- [ ] 支持 `env`
- [ ] 正确返回命令退出状态
- [ ] 输出任务开始日志
- [ ] 输出任务完成日志
- [ ] 输出失败日志

### 调度器
- [ ] 创建 `JobScheduler`
- [ ] 注册所有 `enabled = true` 的任务
- [ ] 支持 cron 调度
- [ ] 支持 interval 调度
- [ ] 启动 scheduler
- [ ] 监听退出信号
- [ ] 优雅退出

### CLI 管理功能
- [ ] `list` 输出任务清单
- [ ] `validate` 校验配置
- [ ] `add-cron` 写入 YAML
- [ ] `add-interval` 写入 YAML
- [ ] `remove` 删除指定任务
- [ ] `enable` 启用指定任务
- [ ] `disable` 禁用指定任务
- [ ] `run-now` 立即执行指定任务

### 日志
- [ ] 初始化 `tracing`
- [ ] 支持 `RUST_LOG`
- [ ] 输出关键生命周期日志
- [ ] 保证 systemd/journald 可直接接收日志

### 部署
- [ ] 编写 `taskd.service`
- [ ] 本地测试 `systemctl start`
- [ ] 本地测试 `systemctl restart`
- [ ] 本地测试 `systemctl status`
- [ ] 本地测试 `journalctl -u taskd -f`
- [ ] 测试开机自启

---

## P1 - 建议尽快补齐

### 配置校验增强
- [ ] 校验 `program` 非空
- [ ] 校验 interval `seconds > 0`
- [ ] 校验 cron 表达式合法
- [ ] 校验 `workdir` 是否存在
- [ ] 校验任务 ID 字符格式

### CLI 体验优化
- [ ] `list` 输出更整齐
- [ ] 增加 `show <id>`
- [ ] 增加 `--json` 输出模式
- [ ] `validate` 输出更明确的错误定位

### 代码质量
- [ ] 增加单元测试
- [ ] 增加配置解析测试
- [ ] 增加 CLI 参数测试
- [ ] 增加 scheduler 注册测试
- [ ] 增加命令执行测试
- [ ] 补充模块文档注释

### 示例与文档
- [ ] 编写 `README.md`
- [ ] 编写 `task.md`
- [ ] 提供示例 `tasks.yaml`
- [ ] 提供安装说明
- [ ] 提供 systemd 部署说明

---

## P2 - 第二阶段增强

### 并发控制
- [ ] 为任务增加 `concurrency` 字段
- [ ] 支持 `allow`
- [ ] 支持 `forbid`
- [ ] 支持 `replace`

### 运行状态记录
- [ ] 设计任务运行结果结构
- [ ] 增加最近一次运行状态展示
- [ ] 增加最近一次运行时间展示

### 历史持久化
- [ ] 引入 SQLite
- [ ] 保存执行历史
- [ ] 查询任务历史
- [ ] 查询最近失败记录

### 失败重试
- [ ] 增加 `retry` 配置
- [ ] 支持固定次数重试
- [ ] 支持重试间隔配置

### 配置热重载
- [ ] 监听 YAML 文件变化
- [ ] diff 新旧任务
- [ ] 增量更新 scheduler
- [ ] 避免重复注册
- [ ] 正确移除旧任务

---

## P3 - 以后再说

- [ ] HTTP API
- [ ] Unix socket 控制
- [ ] Web UI
- [ ] 多用户支持
- [ ] RBAC
- [ ] 分布式调度
- [ ] 集群高可用
- [ ] 远程节点执行
- [ ] 任务依赖编排
- [ ] DAG 模式

---

## 验收清单

- [ ] 能从 YAML 正确读取任务
- [ ] 能校验配置合法性
- [ ] 能列出任务
- [ ] 能新增任务
- [ ] 能删除任务
- [ ] 能启用/禁用任务
- [ ] 能手动执行任务
- [ ] 能启动 daemon
- [ ] 能按 cron 执行任务
- [ ] 能按 interval 执行任务
- [ ] 能通过 systemd 管理
- [ ] 能通过 journald 查看日志
- [ ] 修改配置后重启服务能生效

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