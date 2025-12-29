use crate::task::{MediaType, Task, TaskKind, Tasks};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Serialize, Deserialize, Debug)]
pub struct SerializableTask {
    pub url: String,
    pub task_kind: TaskKind,
    pub media_type: MediaType,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SerializableTasks {
    pub tasks: Vec<SerializableTask>,
}

impl Tasks {
    pub fn save_to_file(&self, path: &std::path::Path) -> Result<()> {
        let serializable = SerializableTasks::from(self);
        let content = serde_json::to_string_pretty(&serializable)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn load_from_file(path: &std::path::Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)?;
        if content.trim().is_empty() {
            return Ok(Self::default());
        }

        let serializable: SerializableTasks = serde_json::from_str(&content)?;
        Ok(Self::from(serializable))
    }
}

impl From<&Tasks> for SerializableTasks {
    fn from(tasks: &Tasks) -> Self {
        let tasks = tasks
            .task_list
            .clone()
            .into_values()
            .filter(|task| {
                !matches!(task, Task::Completed { .. }) // Don't serialize completed tasks
            })
            .map(SerializableTask::from)
            .collect::<Vec<_>>();
        Self { tasks }
    }
}

impl From<SerializableTasks> for Tasks {
    fn from(serializable: SerializableTasks) -> Self {
        let mut tasks = Self::default();

        // Add tasks in priority order: Failed, DownloadVideo, GetName, Queued
        // This ensures failed tasks don't get overwritten by recovered tasks

        // Priority 1: Add Failed tasks first (keep as Failed)
        serializable
            .tasks
            .iter()
            .filter(|task| matches!(task.task_kind, TaskKind::Failed))
            .for_each(|task| {
                let idx = tasks.index_counter;
                tasks.index_counter += 1;
                tasks.task_list.insert(
                    idx,
                    Task::Failed {
                        url: task.url.clone(),
                        human_readable_error: "Recovered from previous session".to_string(),
                        media_type: task.media_type,
                    },
                );
                info!("Recovered failed task {} for URL: {}", idx, task.url);
            });

        // Priority 2: Add Paused tasks (keep as paused with should_auto_resume: true for
        // restart)task
        serializable
            .tasks
            .iter()
            .filter(|task| matches!(task.task_kind, TaskKind::PausedQueued))
            .for_each(|task| {
                if !tasks.task_list.values().any(|t| t.url() == task.url) {
                    let idx = tasks.index_counter;
                    tasks.index_counter += 1;
                    tasks.task_list.insert(
                        idx,
                        Task::PausedQueued {
                            url: task.url.clone(),
                            should_auto_resume: true,
                            media_type: task.media_type,
                        },
                    );
                    info!("Recovered paused queued task {} for URL: {}", idx, task.url);
                }
            });

        serializable
            .tasks
            .iter()
            .filter(|task| matches!(task.task_kind, TaskKind::PausedGetName))
            .for_each(|task| {
                if !tasks.task_list.values().any(|t| t.url() == task.url) {
                    let idx = tasks.index_counter;
                    tasks.index_counter += 1;
                    tasks.task_list.insert(
                        idx,
                        Task::PausedGetName {
                            url: task.url.clone(),
                            metadata: None,
                            should_auto_resume: true,
                            media_type: task.media_type,
                        },
                    );
                    info!(
                        "Recovered paused GetName task {} for URL: {}",
                        idx, task.url
                    );
                }
            });

        serializable
            .tasks
            .iter()
            .filter(|task| matches!(task.task_kind, TaskKind::PausedDownloadVideo))
            .for_each(|task| {
                if !tasks.task_list.values().any(|t| t.url() == task.url) {
                    let idx = tasks.index_counter;
                    tasks.index_counter += 1;
                    tasks.task_list.insert(
                        idx,
                        Task::PausedQueued {
                            url: task.url.clone(),
                            should_auto_resume: true,
                            media_type: task.media_type
                        },
                    );
                    info!("Recovered paused DownloadVideo task {} as PausedQueued for URL: {} (will re-fetch metadata)", idx, task.url);
                }
            });

        // Priority 3: Add DownloadVideo tasks as Queued (will be re-promoted)
        serializable
            .tasks
            .iter()
            .filter(|task| matches!(task.task_kind, TaskKind::DownloadVideo))
            .for_each(|task| {
                if !tasks.task_list.values().any(|t| t.url() == task.url) {
                    let idx = tasks.index_counter;
                    tasks.index_counter += 1;
                    tasks.task_list.insert(idx, Task::Queued { url: task.url.clone(), media_type: task.media_type });
                    info!("Recovered DownloadVideo task {} as Queued for URL: {} (yt-dlp will resume from fragments)", idx, task.url);
                }
            });

        // Priority 4: Add GetName tasks as Queued (will be re-promoted)
        serializable
            .tasks
            .iter()
            .filter(|task| matches!(task.task_kind, TaskKind::GetName))
            .for_each(|task| {
                if !tasks.task_list.values().any(|t| t.url() == task.url) {
                    let idx = tasks.index_counter;
                    tasks.index_counter += 1;
                    tasks.task_list.insert(
                        idx,
                        Task::Queued {
                            url: task.url.clone(),
                            media_type: task.media_type,
                        },
                    );
                    info!(
                        "Recovered GetName task {} as Queued for URL: {}",
                        idx, task.url
                    );
                }
            });

        // Priority 5: Add Queued tasks
        serializable
            .tasks
            .iter()
            .filter(|task| matches!(task.task_kind, TaskKind::Queued))
            .for_each(|task| {
                if !tasks.task_list.values().any(|t| t.url() == task.url) {
                    let idx = tasks.index_counter;
                    tasks.index_counter += 1;
                    tasks.task_list.insert(
                        idx,
                        Task::Queued {
                            url: task.url.clone(),
                            media_type: task.media_type,
                        },
                    );
                    info!("Recovered queued task {} for URL: {}", idx, task.url);
                }
            });

        info!(
            "Loaded {} total tasks from serialized data",
            tasks.task_list.len()
        );

        tasks
    }
}

impl From<Task> for SerializableTask {
    fn from(task: Task) -> Self {
        let task_kind = match task {
            Task::Queued { .. } => TaskKind::Queued,
            Task::GetName { .. } => TaskKind::GetName,
            Task::DownloadVideo { .. } => TaskKind::DownloadVideo,
            Task::PausedQueued { .. } => TaskKind::PausedQueued,
            Task::PausedGetName { .. } => TaskKind::PausedGetName,
            Task::PausedDownloadVideo { .. } => TaskKind::PausedDownloadVideo,
            Task::Completed { .. } => TaskKind::Completed,
            Task::Failed { .. } => TaskKind::Failed,
        };

        Self {
            url: task.url().to_string(),
            task_kind,
            media_type: task.media_type(),
        }
    }
}
