# taskd 项目说明

`taskd` 是一个单机定时任务调度器。

当前实现已经收敛到很简单的模型：

- 一个 task 就是一个 command
- `taskd daemon` 负责调度
- `taskctl` 负责控制面操作
- 可选通知链路支持 `pi` 总结 + Discord webhook 发送

## 核心能力

- cron 调度
- interval 调度
- 手动立即执行任务
- 启用 / 禁用任务
- 配置校验
- 最近一次运行状态
- SQLite 历史记录
- 最近失败任务查询
- 配置文件热重载

## 配置模型

顶层结构：

```yaml
version: 1

notifications:
  enabled: false

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
```

说明：

- `notifications.enabled: false` 表示默认关闭通知
- `tasks[*].command` 是唯一的任务执行方式
- 当前不支持 pipeline

## 通知模型

通知是可选能力，不属于默认执行路径。

要启用通知，需要两层配置：

1. 顶层 `notifications.enabled: true`
2. 对某个任务增加 `notify`

示例：

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
  - id: backup-db
    name: backup database
    enabled: true
    schedule:
      kind: cron
      expr: "0 0 2 * * *"
    command:
      program: /usr/local/bin/backup.sh
    notify:
      result_source:
        kind: stdout
```

`notify.result_source.kind` 支持：

- `stdout`
- `file`

## CLI

守护进程：

```bash
taskd daemon --config /etc/taskd/tasks.yaml
```

控制面：

```bash
taskctl validate
taskctl list
taskctl show <id>
taskctl run-now <id>
taskctl history <id> --limit 20
taskctl recent-failures --limit 20
taskctl logs --lines 100
```

## 设计边界

- 单机运行
- YAML 是配置事实来源
- 不支持 pipeline
- 不支持 artifact 子系统
- 不做分布式调度
- 不做 Web UI / HTTP API
