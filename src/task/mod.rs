use crate::{
    Config,
    common::{format_bytes, send_critical_notification, send_notification},
    task::fs::{find_cache_dir_by_hash, get_disk_space, get_video_dir_for_url},
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
pub mod download;
pub mod fs;
pub mod serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MediaType {
    #[default]
    Video,
    Audio,
}
impl std::fmt::Display for MediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let string = match self {
            MediaType::Video => "Video",
            MediaType::Audio => "Audio",
        };
        write!(f, "{}", string)
    }
}

impl MediaType {
    pub fn emoji(&self) -> &'static str {
        match self {
            MediaType::Video => "🎬",
            MediaType::Audio => "🎵",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    Queued,
    GetName,
    DownloadVideo,
    PausedQueued,
    PausedGetName,
    PausedDownloadVideo,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskStatus {
    Queued,
    GetName,
    DownloadVideo,
    PausedQueued,
    PausedGetName,
    PausedDownloadVideo,
    Completed,
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum Task {
    Queued {
        url: String,
        media_type: MediaType,
        download_dir: Option<String>,
    },
    GetName {
        url: String,
        metadata: Option<GetNameMetadata>,
        media_type: MediaType,
        download_dir: Option<String>,
    },
    DownloadVideo {
        url: String,
        path: PathBuf,
        media_type: MediaType,
        metadata: DownloadMetadata,
        download_dir: Option<String>,
    },
    PausedQueued {
        url: String,
        media_type: MediaType,
        should_auto_resume: bool,
        download_dir: Option<String>,
    },
    PausedGetName {
        url: String,
        metadata: Option<GetNameMetadata>,
        should_auto_resume: bool,
        media_type: MediaType,
        download_dir: Option<String>,
    },
    PausedDownloadVideo {
        media_type: MediaType,
        url: String,
        path: PathBuf,
        metadata: DownloadMetadata,
        should_auto_resume: bool,
        download_dir: Option<String>,
    },
    Completed {
        url: String,
        media_type: MediaType,
        path: PathBuf,
        download_dir: Option<String>,
    },
    Failed {
        media_type: MediaType,
        url: String,
        human_readable_error: String,
        download_dir: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct GetNameMetadata {
    pub title: Option<String>,
    pub expected_size_bytes: Option<u64>,
    pub directory: String,
}

#[derive(Debug, Clone)]
pub struct DownloadMetadata {
    pub title: Option<String>,
    pub expected_size_bytes: Option<u64>,
    pub directory: String,
    pub started_at: Option<std::time::Instant>,
    pub process_id: Option<u32>,
    pub log_file: Option<String>,
}

impl Task {
    pub fn url(&self) -> &str {
        match self {
            Task::Queued { url, .. } => url,
            Task::GetName { url, .. } => url,
            Task::DownloadVideo { url, .. } => url,
            Task::PausedQueued { url, .. } => url,
            Task::PausedGetName { url, .. } => url,
            Task::PausedDownloadVideo { url, .. } => url,
            Task::Completed { url, .. } => url,
            Task::Failed { url, .. } => url,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Task::GetName { .. } | Task::DownloadVideo { .. })
    }

    pub fn is_paused(&self) -> bool {
        matches!(
            self,
            Task::PausedQueued { .. }
                | Task::PausedGetName { .. }
                | Task::PausedDownloadVideo { .. }
        )
    }

    pub fn pause(&mut self) {
        let paused = match self {
            Task::Queued {
                url,
                media_type,
                download_dir,
            } => Task::PausedQueued {
                url: url.clone(),
                media_type: *media_type,
                should_auto_resume: false,
                download_dir: download_dir.clone(),
            },
            Task::GetName {
                url,
                metadata,
                media_type,
                download_dir,
            } => Task::PausedGetName {
                url: url.clone(),
                metadata: metadata.clone(),
                should_auto_resume: false,
                media_type: *media_type,
                download_dir: download_dir.clone(),
            },
            Task::DownloadVideo {
                url,
                path,
                metadata,
                media_type,
                download_dir,
            } => Task::PausedDownloadVideo {
                url: url.clone(),
                path: path.clone(),
                metadata: metadata.clone(),
                should_auto_resume: false,
                media_type: *media_type,
                download_dir: download_dir.clone(),
            },
            // Already paused, completed, or failed - no-op
            _ => return,
        };
        *self = paused;
    }

    pub fn unpause(&mut self) {
        let unpaused = match self {
            Task::PausedQueued {
                url,
                media_type,
                download_dir,
                ..
            } => Task::Queued {
                url: url.clone(),
                media_type: *media_type,
                download_dir: download_dir.clone(),
            },
            Task::PausedGetName {
                url,
                metadata,
                media_type,
                download_dir,
                ..
            } => Task::GetName {
                url: url.clone(),
                metadata: metadata.clone(),
                media_type: *media_type,
                download_dir: download_dir.clone(),
            },
            Task::PausedDownloadVideo {
                url,
                path,
                metadata,
                media_type,
                download_dir,
                ..
            } => Task::DownloadVideo {
                url: url.clone(),
                path: path.clone(),
                metadata: metadata.clone(),
                media_type: *media_type,
                download_dir: download_dir.clone(),
            },
            // Not paused - no-op
            _ => return,
        };
        *self = unpaused;
    }

    pub async fn transition(&mut self, next: TaskKind, context: Option<String>, config: &Config) {
        // Paused tasks cannot transition - return early
        if self.is_paused() {
            warn!("Cannot transition paused task: {:?}", self);
            return;
        }

        match (&self, next) {
            // Queued → GetName: Fetch metadata using yt-dlp --simulate
            (
                Task::Queued {
                    url,
                    media_type,
                    download_dir,
                },
                TaskKind::GetName,
            ) => {
                info!("Transitioning task to GetName for URL: {}", url);

                let url_clone = url.clone();
                let download_dir_clone = download_dir.clone();

                let dir_suffix = download_dir_clone
                    .as_ref()
                    .map(|d| format!(" to {}", d))
                    .unwrap_or_default();
                send_notification(
                    url,
                    &format!("Processing: {}{} 🔄", url, dir_suffix),
                    Some(5000),
                    config,
                )
                .await;

                // Determine output template based on URL and media_type
                let output_template = match media_type {
                    MediaType::Audio => "%(track)_%(artist)s",
                    MediaType::Video => {
                        if url.contains("youtube.com") || url.contains("youtu.be") {
                            "%(channel)s_%(title)s"
                        } else {
                            "%(title)s"
                        }
                    }
                };

                // Spawn yt-dlp to get metadata
                let mut cmd = Command::new("yt-dlp");
                cmd.args([
                    "--print",
                    "filename",
                    "--print",
                    "filesize_approx",
                    "--restrict-filename",
                    "--ignore-config",
                    "--no-playlist",
                    "--simulate",
                    "-o",
                    output_template,
                ]);

                // Add cookies if available
                if let Some(cookie_file) = &config.cookies_file {
                    cmd.args(["--cookies", cookie_file]);
                }

                cmd.arg(url);
                cmd.stdout(Stdio::piped());
                cmd.stderr(Stdio::piped());

                let result = cmd.output().await;

                match result {
                    Ok(output) if output.status.success() => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let lines: Vec<&str> = stdout.lines().collect();

                        if lines.len() >= 2 {
                            let filename = lines[0].trim().trim_end_matches('.').to_string();
                            let filesize_str = lines[1].trim();
                            let expected_size_bytes = filesize_str.parse::<u64>().ok();

                            // Get directory for this URL - use stored download_dir or fall back to script/default
                            let directory = match &download_dir_clone {
                                Some(dir) => dir.clone(),
                                None => get_video_dir_for_url(url, config, *media_type).await,
                            };

                            // Format the log output nicely
                            let size_display = match expected_size_bytes {
                                Some(bytes) => format!("{} ({} bytes)", format_bytes(bytes), bytes),
                                None => "unknown".to_string(),
                            };

                            info!(
                                "GetName completed: '{}' | Size: {} | Directory: {} | Type: {:?}",
                                filename, size_display, directory, media_type
                            );

                            *self = Task::GetName {
                                url: url_clone,
                                media_type: *media_type,
                                metadata: Some(GetNameMetadata {
                                    title: Some(filename),
                                    expected_size_bytes,
                                    directory,
                                }),
                                download_dir: download_dir_clone,
                            };
                        } else {
                            error!("GetName failed: unexpected output format");
                            let error_msg = "Failed to parse yt-dlp metadata output".to_string();
                            let dir_suffix = download_dir_clone
                                .as_ref()
                                .map(|d| format!(" to {}", d))
                                .unwrap_or_default();
                            send_critical_notification(
                                url,
                                &format!("❌ Download failed:{} {}", dir_suffix, error_msg),
                                config,
                            )
                            .await;
                            *self = Task::Failed {
                                url: url_clone,
                                human_readable_error: error_msg,
                                media_type: *media_type,
                                download_dir: download_dir_clone,
                            };
                        }
                    }
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        error!("GetName failed with non-zero exit: {}", stderr);
                        let error_msg = format!(
                            "yt-dlp metadata fetch failed: {}",
                            stderr.lines().next().unwrap_or("unknown error")
                        );
                        let dir_suffix = download_dir_clone
                            .as_ref()
                            .map(|d| format!(" to {}", d))
                            .unwrap_or_default();
                        send_critical_notification(
                            url,
                            &format!("❌ Download failed:{} {}", dir_suffix, error_msg),
                            config,
                        )
                        .await;
                        *self = Task::Failed {
                            url: url_clone,
                            human_readable_error: error_msg,
                            media_type: *media_type,
                            download_dir: download_dir_clone,
                        };
                    }
                    Err(e) => {
                        error!("GetName spawn failed: {}", e);
                        let error_msg = format!("Failed to spawn yt-dlp: {}", e);
                        let dir_suffix = download_dir_clone
                            .as_ref()
                            .map(|d| format!(" to {}", d))
                            .unwrap_or_default();
                        send_critical_notification(
                            url,
                            &format!("❌ Download failed:{} {}", dir_suffix, error_msg),
                            config,
                        )
                        .await;
                        *self = Task::Failed {
                            url: url_clone,
                            human_readable_error: error_msg,
                            media_type: *media_type,
                            download_dir: download_dir_clone,
                        };
                    }
                }
            }

            // GetName → DownloadVideo: Check disk space and start download
            (
                Task::GetName {
                    url,
                    metadata,
                    media_type,
                    download_dir,
                },
                TaskKind::DownloadVideo,
            ) => {
                info!("Transitioning task to DownloadVideo for URL: {}", url);

                let url_clone = url.clone();
                let download_dir_clone = download_dir.clone();
                let mut metadata = match metadata {
                    Some(m) => m.clone(),
                    None => {
                        error!("GetName metadata is None, cannot transition to DownloadVideo");
                        *self = Task::Failed {
                            url: url_clone,
                            human_readable_error: "Missing metadata from GetName phase".to_string(),
                            media_type: *media_type,
                            download_dir: download_dir_clone,
                        };
                        return;
                    }
                };

                if metadata.directory.is_empty() {
                    metadata.directory = get_video_dir_for_url(url, config, *media_type).await;
                    info!("Recovered empty directory, set to: {}", metadata.directory);
                }

                // Check disk space using df command
                let available_mb = match get_disk_space(&metadata.directory).await {
                    Ok(mb) => mb,
                    Err(e) => {
                        let error_msg = format!("Failed to check disk space: {}", e);
                        error!("{}", error_msg);
                        let dir_suffix = download_dir_clone
                            .as_ref()
                            .map(|d| format!(" to {}", d))
                            .unwrap_or_default();
                        send_critical_notification(
                            url,
                            &format!("❌ Download failed:{} {}", dir_suffix, error_msg),
                            config,
                        )
                        .await;
                        *self = Task::Failed {
                            url: url_clone,
                            human_readable_error: error_msg,
                            media_type: *media_type,
                            download_dir: download_dir_clone,
                        };
                        return;
                    }
                };

                // Check if available space is below threshold
                if available_mb < config.disk_threshold {
                    let error_msg = format!(
                        "Disk space below threshold: {}MB < {}MB",
                        available_mb, config.disk_threshold
                    );
                    error!("{}", error_msg);
                    let dir_suffix = download_dir_clone
                        .as_ref()
                        .map(|d| format!(" to {}", d))
                        .unwrap_or_default();
                    send_critical_notification(
                        url,
                        &format!("❌ Download failed:{} {}", dir_suffix, error_msg),
                        config,
                    )
                    .await;
                    *self = Task::Failed {
                        url: url_clone,
                        human_readable_error: error_msg,
                        media_type: *media_type,
                        download_dir: download_dir_clone,
                    };
                    return;
                }

                info!(
                    "Disk space check passed ( {}MB < {}MB )",
                    config.disk_threshold, available_mb
                );

                // Transition to DownloadVideo state - this will be used for status display
                let title = metadata.title.clone();
                let directory = metadata.directory.clone();
                let expected_size = metadata.expected_size_bytes;

                // Check if this is a restart (cache directory with fragments exists)
                let url_hash = format!("{:x}", md5::compute(url.as_bytes()));
                let cache_dir = PathBuf::from(&config.cache_dir);
                let is_restart = find_cache_dir_by_hash(&cache_dir, &url_hash)
                    .await
                    .is_some();

                // Send notification with title (different message for restart vs fresh download)
                let title_display = title.as_deref().unwrap_or("download");
                let dir_suffix = download_dir_clone
                    .as_ref()
                    .map(|d| format!(" to {}", d))
                    .unwrap_or_default();
                let notification_message = if is_restart {
                    format!("Resuming download: {}{} 🔄", title_display, dir_suffix)
                } else {
                    format!(
                        "Downloading: {}{} {}",
                        title_display,
                        dir_suffix,
                        media_type.emoji()
                    )
                };
                send_notification(url, &notification_message, Some(3000), config).await;

                *self = Task::DownloadVideo {
                    url: url_clone.clone(),
                    path: PathBuf::from(&directory).join(format!(
                        "{}.{}",
                        title.as_deref().unwrap_or("download"),
                        match media_type {
                            MediaType::Audio => "ogg",
                            MediaType::Video => "mp4",
                        }
                    )),
                    media_type: *media_type,
                    metadata: DownloadMetadata {
                        title: title.clone(),
                        expected_size_bytes: expected_size,
                        directory: directory.clone(),
                        started_at: Some(std::time::Instant::now()),
                        process_id: None,
                        log_file: None,
                    },
                    download_dir: download_dir_clone,
                };

                // Note: Actual download will be spawned and polled by daemon
                // This transition just sets up the state
            }

            // DownloadVideo → Completed: This shouldn't happen in practice since we transition directly
            (
                Task::DownloadVideo {
                    url,
                    path,
                    media_type,
                    download_dir,
                    ..
                },
                TaskKind::Completed,
            ) => {
                info!("Transitioning task to Completed for URL: {}", url);
                *self = Task::Completed {
                    url: url.clone(),
                    path: path.clone(),
                    media_type: *media_type,
                    download_dir: download_dir.clone(),
                };
            }

            // Any → Failed: Mark task as failed with error message
            (task, TaskKind::Failed) => {
                let url = task.url().to_string();
                let media_type = task.media_type();
                let download_dir = task.download_dir().cloned();
                let error_msg = context.unwrap_or_else(|| "Unknown error".to_string());
                error!("Task failed for URL {}: {}", url, error_msg);
                let dir_suffix = download_dir
                    .as_ref()
                    .map(|d| format!(" to {}", d))
                    .unwrap_or_default();
                send_critical_notification(
                    &url,
                    &format!("❌ Download failed:{} {}", dir_suffix, error_msg),
                    config,
                )
                .await;
                *self = Task::Failed {
                    url,
                    human_readable_error: error_msg,
                    media_type,
                    download_dir,
                };
            }

            // Invalid transitions
            _ => {
                warn!(
                    "Invalid state transition attempted: {:?} -> {:?}",
                    self, next
                );
            }
        }
    }

    pub fn media_type(&self) -> MediaType {
        match self {
            Task::Queued { media_type, .. } => *media_type,
            Task::GetName { media_type, .. } => *media_type,
            Task::DownloadVideo { media_type, .. } => *media_type,
            Task::PausedQueued { media_type, .. } => *media_type,
            Task::PausedGetName { media_type, .. } => *media_type,
            Task::PausedDownloadVideo { media_type, .. } => *media_type,
            Task::Completed { media_type, .. } => *media_type,
            Task::Failed { media_type, .. } => *media_type,
        }
    }

    pub fn download_dir(&self) -> Option<&String> {
        match self {
            Task::Queued { download_dir, .. } => download_dir.as_ref(),
            Task::GetName { download_dir, .. } => download_dir.as_ref(),
            Task::DownloadVideo { download_dir, .. } => download_dir.as_ref(),
            Task::PausedQueued { download_dir, .. } => download_dir.as_ref(),
            Task::PausedGetName { download_dir, .. } => download_dir.as_ref(),
            Task::PausedDownloadVideo { download_dir, .. } => download_dir.as_ref(),
            Task::Completed { download_dir, .. } => download_dir.as_ref(),
            Task::Failed { download_dir, .. } => download_dir.as_ref(),
        }
    }
}

/// Result from a spawned task operation
#[derive(Debug)]
pub enum TaskOperationResult {
    GetNameComplete(GetNameMetadata),
    DownloadComplete(PathBuf),
}

#[derive(Debug, Default)]
pub struct Tasks {
    task_list: BTreeMap<u64, Task>,
    index_counter: u64,
    active_tasks: HashMap<u64, JoinHandle<Result<TaskOperationResult>>>,
    status_channels: HashMap<u64, tokio::sync::watch::Sender<TaskStatus>>,
}

impl Tasks {
    pub fn get_task(&self, id: u64) -> Option<&Task> {
        self.task_list.get(&id)
    }

    pub fn get_task_mut(&mut self, id: u64) -> Option<&mut Task> {
        self.task_list.get_mut(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&u64, &Task)> {
        self.task_list.iter()
    }

    pub fn remove_task(&mut self, id: u64) -> bool {
        // Abort the active task if it exists
        if let Some(handle) = self.active_tasks.remove(&id) {
            handle.abort();
            info!("Aborted active task {}", id);
        }

        self.remove_status_channel(id);
        self.task_list.remove(&id).is_some()
    }

    pub fn abort_active_task(&mut self, id: u64) -> bool {
        if let Some(handle) = self.active_tasks.remove(&id) {
            handle.abort();
            info!("Aborted active task {}", id);
            true
        } else {
            false
        }
    }

    pub fn insert_active_task(&mut self, id: u64, handle: JoinHandle<Result<TaskOperationResult>>) {
        self.active_tasks.insert(id, handle);
    }

    pub fn get_active_task_mut(
        &mut self,
        id: u64,
    ) -> Option<&mut JoinHandle<Result<TaskOperationResult>>> {
        self.active_tasks.get_mut(&id)
    }

    pub fn remove_active_task(
        &mut self,
        id: u64,
    ) -> Option<JoinHandle<Result<TaskOperationResult>>> {
        self.active_tasks.remove(&id)
    }

    pub fn active_task_count(&self) -> usize {
        self.active_tasks.len()
    }

    pub fn has_active_task(&self, id: u64) -> bool {
        self.active_tasks.contains_key(&id)
    }

    pub fn drain_active_tasks(
        &mut self,
    ) -> impl Iterator<Item = (u64, JoinHandle<Result<TaskOperationResult>>)> {
        self.active_tasks.drain()
    }

    pub fn add_url_as_task(
        &mut self,
        url: String,
        media_type: MediaType,
        download_dir: Option<String>,
    ) -> Result<u64, String> {
        // Validate download_dir exists if provided
        if let Some(ref dir) = download_dir {
            let path = std::path::Path::new(dir);
            if !path.exists() {
                return Err(format!("Download directory does not exist: {}", dir));
            }
            if !path.is_dir() {
                return Err(format!("Download path is not a directory: {}", dir));
            }
        }

        for (existing_id, task) in self.task_list.iter() {
            if task.url() == url {
                return Err(format!("URL already exists with task ID {}", existing_id));
            }
        }

        // Create new task with next ID
        let task_id = self.index_counter;
        self.index_counter += 1;

        let task = Task::Queued {
            url,
            media_type,
            download_dir,
        };
        self.task_list.insert(task_id, task);

        // Initialize status channel
        let (tx, _rx) = tokio::sync::watch::channel(TaskStatus::Queued);
        self.status_channels.insert(task_id, tx);

        Ok(task_id)
    }

    pub fn count_active_tasks(&self) -> usize {
        self.task_list
            .values()
            .filter(|task| task.is_active())
            .count()
    }

    pub fn len(&self) -> usize {
        self.task_list.len()
    }

    /// Create a completion handle for observing task status changes
    /// Should be called when a task is first added
    pub fn create_completion_handle(
        &mut self,
        id: u64,
    ) -> tokio::sync::watch::Receiver<TaskStatus> {
        let (tx, rx) = tokio::sync::watch::channel(TaskStatus::Queued);
        self.status_channels.insert(id, tx);
        rx
    }

    /// Get a completion handle for an existing task (if available)
    pub fn get_completion_handle(
        &self,
        id: u64,
    ) -> Option<tokio::sync::watch::Receiver<TaskStatus>> {
        self.status_channels.get(&id).map(|tx| tx.subscribe())
    }

    /// Update task status and notify all observers
    pub fn set_task_status(&mut self, id: u64, status: TaskStatus) {
        if let Some(tx) = self.status_channels.get(&id) {
            let _ = tx.send(status);
        }
    }

    /// Clean up status channel when task is removed
    fn remove_status_channel(&mut self, id: u64) {
        self.status_channels.remove(&id);
    }
}
