//! Scheduler wiring, task registration, and in-process concurrency control.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::SystemTime;

use anyhow::{Result, anyhow};
use chrono_tz::Tz;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::signal;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::{
    AppConfig, ConcurrencyPolicy, LoadedConfig, NotificationsConfig, ScheduleConfig, TaskConfig,
};
use crate::history::HistoryStore;
use crate::notifications::maybe_send_task_notification;
use crate::persist_last_good_config;
use crate::state::RuntimeStateStore;
use crate::task_runner;

struct TaskExecutionControl {
    semaphore: std::sync::Arc<Semaphore>,
    replace_state: Mutex<ReplaceState>,
}

#[derive(Default)]
struct ReplaceState {
    generation: u64,
    cancel: Option<oneshot::Sender<()>>,
}

pub async fn run_daemon(
    config_path: PathBuf,
    config: LoadedConfig,
    state_store: std::sync::Arc<RuntimeStateStore>,
    history_store: std::sync::Arc<HistoryStore>,
) -> Result<()> {
    let mut scheduler = JobScheduler::new().await?;
    let mut current_config = config;
    let mut registered_tasks = register_tasks(
        &scheduler,
        &current_config.app,
        current_config.app.notifications.clone(),
        current_config.env.clone(),
        state_store.clone(),
        history_store.clone(),
    )
    .await?;
    let config_path = normalize_watch_path(&config_path)?;
    let mut last_seen_config_stamp =
        config_inputs_stamp(&config_path, &current_config.resolved_env_files)?;
    let (watch_tx, mut watch_rx) = mpsc::unbounded_channel();
    let watched_paths = watched_paths(&config_path, &current_config.resolved_env_files);
    let mut watcher = match create_config_watcher(&watched_paths, watch_tx) {
        Ok(watcher) => Some(watcher),
        Err(error) => {
            warn!(error = %error, "failed to start config watcher, falling back to polling only");
            None
        }
    };
    let mut watcher_active = watcher.is_some();
    let mut current_watched_paths = watched_paths;
    let mut poll_tick = tokio::time::interval(Duration::from_secs(300));
    poll_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    info!(registered = registered_tasks.len(), "starting scheduler");
    scheduler.start().await?;
    info!("scheduler started, watching config for changes");

    loop {
        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("shutdown signal received");
                break;
            }
            maybe_event = watch_rx.recv(), if watcher_active => {
                let Some(event) = maybe_event else {
                    warn!("config watcher channel closed, falling back to polling only");
                    watcher_active = false;
                    continue;
                };
                if !event_targets_config_path(&event.paths, &current_watched_paths) {
                    continue;
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
                drain_config_events(&mut watch_rx, &current_watched_paths).await;
                reload_if_needed(
                    &scheduler,
                    &config_path,
                    &mut current_config,
                    &mut registered_tasks,
                    &mut last_seen_config_stamp,
                    watcher.as_mut(),
                    &mut current_watched_paths,
                    state_store.clone(),
                    history_store.clone(),
                )
                .await?;
            }
            _ = poll_tick.tick() => {
                reload_if_needed(
                    &scheduler,
                    &config_path,
                    &mut current_config,
                    &mut registered_tasks,
                    &mut last_seen_config_stamp,
                    watcher.as_mut(),
                    &mut current_watched_paths,
                    state_store.clone(),
                    history_store.clone(),
                )
                .await?;
            }
        }
    }

    let _ = scheduler.shutdown().await;
    Ok(())
}

pub async fn register_tasks(
    scheduler: &JobScheduler,
    config: &AppConfig,
    notifications: Option<NotificationsConfig>,
    inherited_env: BTreeMap<String, String>,
    state_store: std::sync::Arc<RuntimeStateStore>,
    history_store: std::sync::Arc<HistoryStore>,
) -> Result<HashMap<String, Uuid>> {
    let mut registered = HashMap::new();
    for task in &config.tasks {
        if !task.enabled {
            continue;
        }
        register_task(
            scheduler,
            task.clone(),
            notifications.clone(),
            inherited_env.clone(),
            state_store.clone(),
            history_store.clone(),
            &mut registered,
        )
        .await?;
    }
    Ok(registered)
}

async fn register_task(
    scheduler: &JobScheduler,
    task: TaskConfig,
    notifications: Option<NotificationsConfig>,
    inherited_env: BTreeMap<String, String>,
    state_store: std::sync::Arc<RuntimeStateStore>,
    history_store: std::sync::Arc<HistoryStore>,
    registered: &mut HashMap<String, Uuid>,
) -> Result<()> {
    let task_id = task.id.clone();
    let job = job_for_task(
        task,
        notifications,
        inherited_env,
        state_store,
        history_store,
    )?;
    let job_id = scheduler.add(job).await?;
    registered.insert(task_id, job_id);
    Ok(())
}

fn job_for_task(
    task: TaskConfig,
    notifications: Option<NotificationsConfig>,
    inherited_env: BTreeMap<String, String>,
    state_store: std::sync::Arc<RuntimeStateStore>,
    history_store: std::sync::Arc<HistoryStore>,
) -> Result<Job> {
    let max_running = task.concurrency.max_running as usize;
    let control = std::sync::Arc::new(TaskExecutionControl {
        semaphore: std::sync::Arc::new(Semaphore::new(max_running)),
        replace_state: Mutex::new(ReplaceState::default()),
    });
    match task.schedule.clone() {
        ScheduleConfig::Cron { expr, timezone } => {
            let task = std::sync::Arc::new(task);
            let inherited_env = std::sync::Arc::new(inherited_env);
            match timezone {
                Some(timezone) => Ok(Job::new_async_tz(
                    expr.as_str(),
                    timezone.parse::<Tz>()?,
                    move |_id, _lock| {
                        Box::pin(schedule_task(
                            task.clone(),
                            notifications.clone(),
                            inherited_env.clone(),
                            control.clone(),
                            state_store.clone(),
                            history_store.clone(),
                        ))
                    },
                )?),
                None => Ok(Job::new_async(expr.as_str(), move |_id, _lock| {
                    Box::pin(schedule_task(
                        task.clone(),
                        notifications.clone(),
                        inherited_env.clone(),
                        control.clone(),
                        state_store.clone(),
                        history_store.clone(),
                    ))
                })?),
            }
        }
        ScheduleConfig::Interval { seconds } => {
            let task = std::sync::Arc::new(task);
            let inherited_env = std::sync::Arc::new(inherited_env);
            Ok(Job::new_repeated_async(
                Duration::from_secs(seconds),
                move |_id, _lock| {
                    Box::pin(schedule_task(
                        task.clone(),
                        notifications.clone(),
                        inherited_env.clone(),
                        control.clone(),
                        state_store.clone(),
                        history_store.clone(),
                    ))
                },
            )?)
        }
    }
}

async fn schedule_task(
    task: std::sync::Arc<TaskConfig>,
    notifications: Option<NotificationsConfig>,
    inherited_env: std::sync::Arc<BTreeMap<String, String>>,
    control: std::sync::Arc<TaskExecutionControl>,
    state_store: std::sync::Arc<RuntimeStateStore>,
    history_store: std::sync::Arc<HistoryStore>,
) {
    match task.concurrency.policy {
        ConcurrencyPolicy::Allow | ConcurrencyPolicy::Forbid => {
            let Some(permit) = try_acquire_task_slot(task.as_ref(), control.semaphore.clone())
            else {
                return;
            };
            let handle = spawn_scheduled_task(
                task,
                notifications,
                inherited_env,
                permit,
                state_store,
                history_store,
            );
            drop(handle);
        }
        ConcurrencyPolicy::Replace => {
            let handle = spawn_replace_task(
                task,
                notifications,
                inherited_env,
                control,
                state_store,
                history_store,
            );
            drop(handle);
        }
    }
}

fn try_acquire_task_slot(
    task: &TaskConfig,
    semaphore: std::sync::Arc<Semaphore>,
) -> Option<OwnedSemaphorePermit> {
    match semaphore.try_acquire_owned() {
        Ok(permit) => Some(permit),
        Err(_) => {
            warn!(
                task_id = %task.id,
                policy = ?task.concurrency.policy,
                max_running = task.concurrency.max_running,
                "task skipped because concurrency limit reached"
            );
            None
        }
    }
}

fn spawn_scheduled_task(
    task: std::sync::Arc<TaskConfig>,
    notifications: Option<NotificationsConfig>,
    inherited_env: std::sync::Arc<BTreeMap<String, String>>,
    _permit: OwnedSemaphorePermit,
    state_store: std::sync::Arc<RuntimeStateStore>,
    history_store: std::sync::Arc<HistoryStore>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let outcome = match task_runner::run_task_with_retry_guarded_and_env(
            task.clone(),
            inherited_env.clone(),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(error) => task_runner::TaskOutcome::panic(&task.id, &error.to_string()),
        };
        if let Err(error) = state_store.record(&task.id, &outcome) {
            tracing::error!(task_id = %task.id, error = %error, "failed to persist runtime state");
        }
        if let Err(error) = history_store.record(&task.id, &outcome) {
            tracing::error!(task_id = %task.id, error = %error, "failed to persist history");
        }
        if let Err(error) = maybe_send_task_notification(
            notifications.as_ref(),
            task.as_ref(),
            &outcome,
            Some(inherited_env.as_ref()),
        )
        .await
        {
            tracing::error!(task_id = %task.id, error = %error, "failed to send task notification");
        }
        if !outcome.success() {
            tracing::error!(task_id = %task.id, error = %outcome.summary(), "scheduled task failed");
        }
    })
}

fn spawn_replace_task(
    task: std::sync::Arc<TaskConfig>,
    notifications: Option<NotificationsConfig>,
    inherited_env: std::sync::Arc<BTreeMap<String, String>>,
    control: std::sync::Arc<TaskExecutionControl>,
    state_store: std::sync::Arc<RuntimeStateStore>,
    history_store: std::sync::Arc<HistoryStore>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let (generation, cancel_receiver) = register_replace_request(control.clone()).await;
        let permit = match control.semaphore.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => return,
        };
        if !replace_request_is_current(control.clone(), generation).await {
            return;
        }
        let outcome = match task_runner::run_task_with_retry_guarded_and_env_with_cancel(
            task.clone(),
            inherited_env.clone(),
            cancel_receiver,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(error) => task_runner::TaskOutcome::panic(&task.id, &error.to_string()),
        };
        if let Err(error) = state_store.record(&task.id, &outcome) {
            tracing::error!(task_id = %task.id, error = %error, "failed to persist runtime state");
        }
        if let Err(error) = history_store.record(&task.id, &outcome) {
            tracing::error!(task_id = %task.id, error = %error, "failed to persist history");
        }
        if let Err(error) = maybe_send_task_notification(
            notifications.as_ref(),
            task.as_ref(),
            &outcome,
            Some(inherited_env.as_ref()),
        )
        .await
        {
            tracing::error!(task_id = %task.id, error = %error, "failed to send task notification");
        }
        if !outcome.success() {
            tracing::error!(task_id = %task.id, error = %outcome.summary(), "scheduled task failed");
        }
        clear_replace_request(control, generation).await;
        drop(permit);
    })
}

async fn register_replace_request(
    control: std::sync::Arc<TaskExecutionControl>,
) -> (u64, oneshot::Receiver<()>) {
    let mut state = control.replace_state.lock().await;
    if let Some(cancel) = state.cancel.take() {
        let _ = cancel.send(());
    }
    state.generation += 1;
    let generation = state.generation;
    let (cancel_tx, cancel_rx) = oneshot::channel();
    state.cancel = Some(cancel_tx);
    (generation, cancel_rx)
}

async fn replace_request_is_current(
    control: std::sync::Arc<TaskExecutionControl>,
    generation: u64,
) -> bool {
    let state = control.replace_state.lock().await;
    state.generation == generation
}

async fn clear_replace_request(control: std::sync::Arc<TaskExecutionControl>, generation: u64) {
    let mut state = control.replace_state.lock().await;
    if state.generation == generation {
        state.cancel = None;
    }
}

#[cfg(test)]
async fn execute_scheduled_task(task: std::sync::Arc<TaskConfig>) {
    let control = std::sync::Arc::new(TaskExecutionControl {
        semaphore: std::sync::Arc::new(Semaphore::new(task.concurrency.max_running as usize)),
        replace_state: Mutex::new(ReplaceState::default()),
    });
    let permit = try_acquire_task_slot(task.as_ref(), control.semaphore.clone()).expect("permit");
    let state_store = std::sync::Arc::new(
        RuntimeStateStore::load(
            tempfile::tempdir()
                .expect("tempdir")
                .path()
                .join("tasks.state.yaml"),
        )
        .expect("state store"),
    );
    let history_store = std::sync::Arc::new(
        HistoryStore::from_config_path(
            &tempfile::tempdir()
                .expect("tempdir")
                .path()
                .join("tasks.yaml"),
        )
        .expect("history store"),
    );
    let handle = spawn_scheduled_task(
        task,
        None,
        std::sync::Arc::new(BTreeMap::new()),
        permit,
        state_store,
        history_store,
    );
    let _ = handle.await;
}

pub fn enabled_task_count(config: &AppConfig) -> usize {
    config.tasks.iter().filter(|task| task.enabled).count()
}

pub fn ensure_enabled_tasks(config: &AppConfig) -> Result<()> {
    let count = enabled_task_count(config);
    if count == 0 {
        return Err(anyhow!("no enabled tasks configured"));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConfigFileStamp {
    modified: SystemTime,
    len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfigInputsStamp(Vec<(PathBuf, Option<ConfigFileStamp>)>);

fn config_file_stamp(path: &Path) -> Result<Option<ConfigFileStamp>> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(ConfigFileStamp {
            modified: metadata.modified()?,
            len: metadata.len(),
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn normalize_watch_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn config_inputs_stamp(config_path: &Path, env_files: &[PathBuf]) -> Result<ConfigInputsStamp> {
    let mut paths = vec![config_path.to_path_buf()];
    paths.extend(env_files.iter().cloned());
    paths.sort();
    paths.dedup();

    let mut stamps = Vec::with_capacity(paths.len());
    for path in paths {
        stamps.push((path.clone(), config_file_stamp(&path)?));
    }
    Ok(ConfigInputsStamp(stamps))
}

fn watched_paths(config_path: &Path, env_files: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = vec![config_path.to_path_buf()];
    paths.extend(env_files.iter().cloned());
    paths.sort();
    paths.dedup();
    paths
}

fn create_config_watcher(
    watched_paths: &[PathBuf],
    watch_tx: mpsc::UnboundedSender<Event>,
) -> Result<RecommendedWatcher> {
    let mut watcher =
        notify::recommended_watcher(move |result: notify::Result<Event>| match result {
            Ok(event) => {
                let _ = watch_tx.send(event);
            }
            Err(error) => {
                warn!(error = %error, "config watcher error");
            }
        })?;
    sync_watcher_paths(&mut watcher, &BTreeSet::new(), watched_paths)?;
    Ok(watcher)
}

fn sync_watcher_paths(
    watcher: &mut RecommendedWatcher,
    current_paths: &BTreeSet<PathBuf>,
    next_paths: &[PathBuf],
) -> Result<BTreeSet<PathBuf>> {
    let next_dirs = next_paths
        .iter()
        .map(|path| {
            path.parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        })
        .collect::<BTreeSet<_>>();

    for path in current_paths.difference(&next_dirs) {
        watcher.unwatch(path)?;
    }
    for path in next_dirs.difference(current_paths) {
        watcher.watch(path, RecursiveMode::NonRecursive)?;
    }

    Ok(next_dirs)
}

fn event_targets_config_path(paths: &[PathBuf], watched_paths: &[PathBuf]) -> bool {
    paths
        .iter()
        .any(|path| watched_paths.iter().any(|watched| watched == path))
}

async fn drain_config_events(
    watch_rx: &mut mpsc::UnboundedReceiver<Event>,
    watched_paths: &[PathBuf],
) {
    while let Ok(event) = watch_rx.try_recv() {
        if event_targets_config_path(&event.paths, watched_paths) {
            continue;
        }
    }
}

async fn reload_if_needed(
    scheduler: &JobScheduler,
    config_path: &Path,
    current_config: &mut LoadedConfig,
    registered_tasks: &mut HashMap<String, Uuid>,
    last_seen_config_stamp: &mut ConfigInputsStamp,
    watcher: Option<&mut RecommendedWatcher>,
    current_watched_paths: &mut Vec<PathBuf>,
    state_store: std::sync::Arc<RuntimeStateStore>,
    history_store: std::sync::Arc<HistoryStore>,
) -> Result<()> {
    let next_stamp = config_inputs_stamp(config_path, &current_config.resolved_env_files)?;
    if next_stamp == *last_seen_config_stamp {
        return Ok(());
    }

    match LoadedConfig::load(config_path).and_then(|config| {
        config.validate()?;
        Ok(config)
    }) {
        Ok(next_config) => {
            let force_reload_all = current_config.app.notifications
                != next_config.app.notifications
                || current_config.app.env_files != next_config.app.env_files
                || current_config.env != next_config.env;
            let plan = build_reload_plan(&current_config.app, &next_config.app, force_reload_all);
            if plan.has_changes() {
                let removed_count = plan.remove.len();
                let added_count = plan.add.len();
                let updated_count = plan.update.len();
                apply_reload_plan(
                    scheduler,
                    registered_tasks,
                    plan,
                    next_config.app.notifications.clone(),
                    next_config.env.clone(),
                    state_store,
                    history_store,
                )
                .await?;
                info!(
                    removed = removed_count,
                    added = added_count,
                    updated = updated_count,
                    "reloaded scheduler config"
                );
            } else {
                info!("config changed but produced no scheduler changes");
            }
            let next_watched_paths = watched_paths(config_path, &next_config.resolved_env_files);
            if let Some(watcher) = watcher {
                let current_dirs = current_watched_paths
                    .iter()
                    .map(|path| {
                        path.parent()
                            .unwrap_or_else(|| Path::new("."))
                            .to_path_buf()
                    })
                    .collect::<BTreeSet<_>>();
                let _ = sync_watcher_paths(watcher, &current_dirs, &next_watched_paths)?;
            }
            *current_watched_paths = next_watched_paths;
            persist_last_good_config(config_path, &next_config.app)?;
            *last_seen_config_stamp =
                config_inputs_stamp(config_path, &next_config.resolved_env_files)?;
            *current_config = next_config;
        }
        Err(error) => {
            *last_seen_config_stamp = next_stamp;
            warn!(error = %error, "ignoring invalid config update");
        }
    }

    Ok(())
}

#[derive(Debug)]
struct ReloadPlan {
    remove: Vec<String>,
    add: Vec<TaskConfig>,
    update: Vec<String>,
}

impl ReloadPlan {
    fn has_changes(&self) -> bool {
        !self.remove.is_empty() || !self.add.is_empty() || !self.update.is_empty()
    }
}

fn build_reload_plan(current: &AppConfig, next: &AppConfig, force_reload_all: bool) -> ReloadPlan {
    if force_reload_all {
        return ReloadPlan {
            remove: current
                .tasks
                .iter()
                .filter(|task| task.enabled)
                .map(|task| task.id.clone())
                .collect(),
            add: next
                .tasks
                .iter()
                .filter(|task| task.enabled)
                .cloned()
                .collect(),
            update: next
                .tasks
                .iter()
                .filter(|task| task.enabled)
                .map(|task| task.id.clone())
                .collect(),
        };
    }

    let current_tasks = current
        .tasks
        .iter()
        .map(|task| (task.id.clone(), task))
        .collect::<HashMap<_, _>>();
    let next_tasks = next
        .tasks
        .iter()
        .map(|task| (task.id.clone(), task))
        .collect::<HashMap<_, _>>();

    let mut remove = Vec::new();
    let mut add = Vec::new();
    let mut update = Vec::new();

    for (task_id, current_task) in &current_tasks {
        match next_tasks.get(task_id) {
            None => {
                if current_task.enabled {
                    remove.push(task_id.clone());
                }
            }
            Some(next_task) if current_task != next_task => {
                update.push(task_id.clone());
                if next_task.enabled {
                    add.push((*next_task).clone());
                }
            }
            _ => {}
        }
    }

    for (task_id, next_task) in &next_tasks {
        if !current_tasks.contains_key(task_id) && next_task.enabled {
            add.push((*next_task).clone());
        }
    }

    ReloadPlan {
        remove,
        add,
        update,
    }
}

async fn apply_reload_plan(
    scheduler: &JobScheduler,
    registered_tasks: &mut HashMap<String, Uuid>,
    plan: ReloadPlan,
    notifications: Option<NotificationsConfig>,
    inherited_env: BTreeMap<String, String>,
    state_store: std::sync::Arc<RuntimeStateStore>,
    history_store: std::sync::Arc<HistoryStore>,
) -> Result<()> {
    for task_id in plan.remove.iter().chain(plan.update.iter()) {
        if let Some(job_id) = registered_tasks.remove(task_id) {
            scheduler.remove(&job_id).await?;
        }
    }

    for task in plan.add {
        register_task(
            scheduler,
            task,
            notifications.clone(),
            inherited_env.clone(),
            state_store.clone(),
            history_store.clone(),
            registered_tasks,
        )
        .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use tempfile::tempdir;
    use tokio::sync::{Mutex, Semaphore};
    use tokio::time::sleep;
    use tokio_cron_scheduler::JobScheduler;

    use super::{
        ReplaceState, TaskExecutionControl, build_reload_plan, enabled_task_count,
        event_targets_config_path, execute_scheduled_task, register_tasks, schedule_task,
        spawn_scheduled_task, try_acquire_task_slot,
    };
    use crate::config::{
        AppConfig, CommandConfig, ConcurrencyConfig, ConcurrencyPolicy, RetryConfig,
        ScheduleConfig, TaskConfig,
    };
    use crate::history::HistoryStore;
    use crate::state::RuntimeStateStore;

    #[test]
    fn counts_only_enabled_tasks() {
        let config = AppConfig {
            version: 1,
            env_files: Vec::new(),
            notifications: None,
            tasks: vec![
                sample_task("job-1", true, ScheduleConfig::Interval { seconds: 30 }),
                sample_task("job-2", false, ScheduleConfig::Interval { seconds: 30 }),
            ],
        };

        assert_eq!(enabled_task_count(&config), 1);
    }

    #[tokio::test]
    async fn registers_only_enabled_tasks() {
        let scheduler = JobScheduler::new().await.expect("scheduler");
        let state_store = std::sync::Arc::new(test_state_store());
        let history_store = std::sync::Arc::new(test_history_store());
        let config = AppConfig {
            version: 1,
            env_files: Vec::new(),
            notifications: None,
            tasks: vec![
                sample_task("job-1", true, ScheduleConfig::Interval { seconds: 30 }),
                sample_task("job-2", false, ScheduleConfig::Interval { seconds: 30 }),
            ],
        };

        let count = register_tasks(
            &scheduler,
            &config,
            None,
            BTreeMap::new(),
            state_store,
            history_store,
        )
        .await
        .expect("register tasks");

        assert_eq!(count.len(), 1);
    }

    #[tokio::test]
    async fn registers_cron_tasks() {
        let scheduler = JobScheduler::new().await.expect("scheduler");
        let state_store = std::sync::Arc::new(test_state_store());
        let history_store = std::sync::Arc::new(test_history_store());
        let config = AppConfig {
            version: 1,
            env_files: Vec::new(),
            notifications: None,
            tasks: vec![sample_task(
                "job-1",
                true,
                ScheduleConfig::Cron {
                    expr: "0/30 * * * * *".to_string(),
                    timezone: None,
                },
            )],
        };

        let count = register_tasks(
            &scheduler,
            &config,
            None,
            BTreeMap::new(),
            state_store,
            history_store,
        )
        .await
        .expect("register tasks");

        assert_eq!(count.len(), 1);
    }

    #[test]
    fn reload_plan_adds_removes_and_updates_tasks_incrementally() {
        let current = AppConfig {
            version: 1,
            env_files: Vec::new(),
            notifications: None,
            tasks: vec![
                sample_task("keep", true, ScheduleConfig::Interval { seconds: 30 }),
                sample_task("remove", true, ScheduleConfig::Interval { seconds: 30 }),
                sample_task("update", true, ScheduleConfig::Interval { seconds: 30 }),
                sample_task("disable", true, ScheduleConfig::Interval { seconds: 30 }),
            ],
        };
        let next = AppConfig {
            version: 1,
            env_files: Vec::new(),
            notifications: None,
            tasks: vec![
                sample_task("keep", true, ScheduleConfig::Interval { seconds: 30 }),
                sample_task("update", true, ScheduleConfig::Interval { seconds: 60 }),
                sample_task("disable", false, ScheduleConfig::Interval { seconds: 30 }),
                sample_task("add", true, ScheduleConfig::Interval { seconds: 15 }),
            ],
        };

        let plan = build_reload_plan(&current, &next, false);

        assert!(plan.remove.contains(&"remove".to_string()));
        assert!(plan.update.contains(&"update".to_string()));
        assert!(plan.update.contains(&"disable".to_string()));
        assert!(plan.add.iter().any(|task| task.id == "add"));
        assert!(plan.add.iter().any(|task| task.id == "update"));
        assert!(!plan.remove.contains(&"keep".to_string()));
    }

    #[test]
    fn watcher_matches_only_target_config_path() {
        let config_path = PathBuf::from("/tmp/taskd/tasks.yaml");
        let paths = vec![
            PathBuf::from("/tmp/taskd/tasks.yaml"),
            PathBuf::from("/tmp/taskd/tasks.yaml.tmp"),
        ];

        assert!(event_targets_config_path(
            &paths,
            std::slice::from_ref(&config_path)
        ));
    }

    #[test]
    fn watcher_ignores_other_files_in_same_directory() {
        let config_path = PathBuf::from("/tmp/taskd/tasks.yaml");
        let paths = vec![
            PathBuf::from("/tmp/taskd/tasks.state.yaml"),
            PathBuf::from("/tmp/taskd/other.yaml"),
        ];

        assert!(!event_targets_config_path(
            &paths,
            std::slice::from_ref(&config_path)
        ));
    }

    #[tokio::test]
    async fn scheduled_command_failure_does_not_panic() {
        let task = std::sync::Arc::new(TaskConfig {
            id: "job-1".to_string(),
            name: "job-1".to_string(),
            enabled: true,
            concurrency: ConcurrencyConfig::default(),
            retry: RetryConfig::default(),
            schedule: ScheduleConfig::Interval { seconds: 30 },
            notify: None,
            command: CommandConfig {
                program: "/definitely/missing/taskd-bin".to_string(),
                args: Vec::new(),
                workdir: None,
                timeout_seconds: None,
                env: BTreeMap::new(),
            },
        });

        execute_scheduled_task(task).await;
    }

    #[tokio::test]
    async fn scheduled_task_retries_and_eventually_succeeds() {
        let dir = tempdir().expect("tempdir");
        let marker = dir.path().join("attempt.txt");
        let task = std::sync::Arc::new(TaskConfig {
            id: "job-1".to_string(),
            name: "job-1".to_string(),
            enabled: true,
            concurrency: ConcurrencyConfig::default(),
            retry: RetryConfig {
                max_attempts: 1,
                delay_seconds: 1,
            },
            schedule: ScheduleConfig::Interval { seconds: 30 },
            notify: None,
            command: CommandConfig {
                program: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    format!(
                        "if [ -f \"{0}\" ]; then exit 0; else touch \"{0}\"; exit 7; fi",
                        marker.display()
                    ),
                ],
                workdir: None,
                timeout_seconds: None,
                env: BTreeMap::new(),
            },
        });
        let state_dir = tempdir().expect("tempdir");
        let history_dir = tempdir().expect("tempdir");
        let state_store = std::sync::Arc::new(
            RuntimeStateStore::load(state_dir.path().join("tasks.state.yaml"))
                .expect("state store"),
        );
        let history_store = std::sync::Arc::new(
            HistoryStore::from_config_path(&history_dir.path().join("tasks.yaml"))
                .expect("history store"),
        );
        let permit = try_acquire_task_slot(task.as_ref(), std::sync::Arc::new(Semaphore::new(1)))
            .expect("permit");

        let handle = spawn_scheduled_task(
            task.clone(),
            None,
            std::sync::Arc::new(BTreeMap::new()),
            permit,
            state_store.clone(),
            history_store.clone(),
        );
        let _ = handle.await;

        let state = crate::state::load_runtime_state(&state_dir.path().join("tasks.state.yaml"))
            .expect("load state");
        assert_eq!(
            state.tasks["job-1"].last_status,
            crate::task_runner::TaskRunStatus::Success
        );

        let history = history_store
            .list_task_history("job-1", 10)
            .expect("load history");
        assert_eq!(history.len(), 1);
        assert!(history[0].summary.contains("succeeded after 2 attempts"));
    }

    #[tokio::test]
    async fn scheduled_tasks_run_concurrently() {
        let start = Instant::now();
        let first_task = std::sync::Arc::new(shell_task("job-1", "sleep 1", 1));
        let second_task = std::sync::Arc::new(shell_task("job-2", "sleep 1", 1));
        let first_state_store = std::sync::Arc::new(test_state_store());
        let second_state_store = std::sync::Arc::new(test_state_store());
        let first_history_store = std::sync::Arc::new(test_history_store());
        let second_history_store = std::sync::Arc::new(test_history_store());
        let first_permit =
            try_acquire_task_slot(first_task.as_ref(), std::sync::Arc::new(Semaphore::new(1)))
                .expect("permit");
        let second_permit =
            try_acquire_task_slot(second_task.as_ref(), std::sync::Arc::new(Semaphore::new(1)))
                .expect("permit");
        let first = spawn_scheduled_task(
            first_task,
            None,
            std::sync::Arc::new(BTreeMap::new()),
            first_permit,
            first_state_store,
            first_history_store,
        );
        let second = spawn_scheduled_task(
            second_task,
            None,
            std::sync::Arc::new(BTreeMap::new()),
            second_permit,
            second_state_store,
            second_history_store,
        );

        let _ = first.await;
        let _ = second.await;

        assert!(start.elapsed() < Duration::from_millis(1800));
    }

    #[tokio::test]
    async fn same_task_respects_max_running_limit() {
        let task = shell_task("job-1", "sleep 1", 1);
        let semaphore = std::sync::Arc::new(Semaphore::new(task.concurrency.max_running as usize));

        let first = try_acquire_task_slot(&task, semaphore.clone());
        let second = try_acquire_task_slot(&task, semaphore);

        assert!(first.is_some());
        assert!(second.is_none());
    }

    #[tokio::test]
    async fn same_task_can_run_up_to_three_instances() {
        let task = shell_task("job-1", "sleep 1", 3);
        let semaphore = std::sync::Arc::new(Semaphore::new(task.concurrency.max_running as usize));

        let first = try_acquire_task_slot(&task, semaphore.clone());
        let second = try_acquire_task_slot(&task, semaphore.clone());
        let third = try_acquire_task_slot(&task, semaphore.clone());
        let fourth = try_acquire_task_slot(&task, semaphore);

        assert!(first.is_some());
        assert!(second.is_some());
        assert!(third.is_some());
        assert!(fourth.is_none());
    }

    #[tokio::test]
    async fn replace_policy_cancels_previous_run_and_keeps_latest() {
        let dir = tempdir().expect("tempdir");
        let output = dir.path().join("replace-output.txt");
        let task = std::sync::Arc::new(TaskConfig {
            id: "job-1".to_string(),
            name: "job-1".to_string(),
            enabled: true,
            concurrency: ConcurrencyConfig {
                policy: ConcurrencyPolicy::Replace,
                max_running: 1,
            },
            retry: RetryConfig::default(),
            schedule: ScheduleConfig::Interval { seconds: 30 },
            notify: None,
            command: CommandConfig {
                program: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    format!("sleep 1; echo run >> {}", output.display()),
                ],
                workdir: None,
                timeout_seconds: None,
                env: BTreeMap::new(),
            },
        });
        let control = std::sync::Arc::new(TaskExecutionControl {
            semaphore: std::sync::Arc::new(Semaphore::new(1)),
            replace_state: Mutex::new(ReplaceState::default()),
        });
        let state_store = std::sync::Arc::new(test_state_store());
        let history_store = std::sync::Arc::new(test_history_store());

        schedule_task(
            task.clone(),
            None,
            std::sync::Arc::new(BTreeMap::new()),
            control.clone(),
            state_store.clone(),
            history_store.clone(),
        )
        .await;
        sleep(Duration::from_millis(100)).await;
        schedule_task(
            task,
            None,
            std::sync::Arc::new(BTreeMap::new()),
            control,
            state_store,
            history_store,
        )
        .await;
        sleep(Duration::from_millis(2500)).await;

        let body = fs::read_to_string(output).expect("output file");
        assert_eq!(body.lines().count(), 1);
    }

    fn sample_task(id: &str, enabled: bool, schedule: ScheduleConfig) -> TaskConfig {
        TaskConfig {
            id: id.to_string(),
            name: id.to_string(),
            enabled,
            concurrency: ConcurrencyConfig::default(),
            retry: RetryConfig::default(),
            schedule,
            notify: None,
            command: CommandConfig {
                program: "/bin/echo".to_string(),
                args: vec!["ok".to_string()],
                workdir: None,
                timeout_seconds: None,
                env: BTreeMap::new(),
            },
        }
    }

    fn shell_task(id: &str, body: &str, max_running: u8) -> TaskConfig {
        TaskConfig {
            id: id.to_string(),
            name: id.to_string(),
            enabled: true,
            concurrency: ConcurrencyConfig {
                policy: if max_running == 1 {
                    ConcurrencyPolicy::Forbid
                } else {
                    ConcurrencyPolicy::Allow
                },
                max_running,
            },
            retry: RetryConfig::default(),
            schedule: ScheduleConfig::Interval { seconds: 30 },
            notify: None,
            command: CommandConfig {
                program: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), body.to_string()],
                workdir: None,
                timeout_seconds: None,
                env: BTreeMap::new(),
            },
        }
    }

    fn test_state_store() -> RuntimeStateStore {
        let dir = tempdir().expect("tempdir");
        RuntimeStateStore::load(dir.path().join("tasks.state.yaml")).expect("state store")
    }

    fn test_history_store() -> HistoryStore {
        let dir = tempdir().expect("tempdir");
        HistoryStore::from_config_path(&dir.path().join("tasks.yaml")).expect("history store")
    }
}
