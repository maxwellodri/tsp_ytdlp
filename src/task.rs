use crate::{
    common::{send_critical_notification, send_notification},
    get_video_dir_for_url, Config,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    Queued,
    GetName,
    DownloadVideo,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskStatus {
    Queued,
    GetName,
    DownloadVideo,
    Completed,
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum Task {
    Queued {
        url: String,
    },
    GetName {
        url: String,
        metadata: Option<GetNameMetadata>,
    },
    DownloadVideo {
        url: String,
        path: PathBuf,
        metadata: DownloadMetadata,
    },
    Completed {
        url: String,
        path: PathBuf,
    },
    Failed {
        url: String,
        human_readable_error: String,
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
            Task::Queued { url } => url,
            Task::GetName { url, .. } => url,
            Task::DownloadVideo { url, .. } => url,
            Task::Completed { url, .. } => url,
            Task::Failed { url, .. } => url,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Task::GetName { .. } | Task::DownloadVideo { .. })
    }

    pub async fn transition(&mut self, next: TaskKind, context: Option<String>, config: &Config) {
        match (&self, next) {
            // Queued → GetName: Fetch metadata using yt-dlp --simulate
            (Task::Queued { url }, TaskKind::GetName) => {
                info!("Transitioning task to GetName for URL: {}", url);

                let url_clone = url.clone();

                // Send notification with MD5-based notification ID (30s timeout)
                send_notification(url, &format!("Processing: {} 🔄", url), Some(30000), config)
                    .await;

                // Determine output template based on URL
                let output_template = if url.contains("youtube.com") || url.contains("youtu.be") {
                    "%(channel)s_%(title)s"
                } else {
                    "%(title)s"
                };

                // Spawn yt-dlp to get metadata
                let result = Command::new("yt-dlp")
                    .args([
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
                        url,
                    ])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .output()
                    .await;

                match result {
                    Ok(output) if output.status.success() => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let lines: Vec<&str> = stdout.lines().collect();

                        if lines.len() >= 2 {
                            let filename = lines[0].trim().to_string();
                            let filesize_str = lines[1].trim();
                            let expected_size_bytes = filesize_str.parse::<u64>().ok();

                            // Get directory for this URL
                            let directory = get_video_dir_for_url(url, config).await;

                            info!(
                                "GetName success: title={}, size={:?}, dir={}",
                                filename, expected_size_bytes, directory
                            );

                            *self = Task::GetName {
                                url: url_clone,
                                metadata: Some(GetNameMetadata {
                                    title: Some(filename),
                                    expected_size_bytes,
                                    directory,
                                }),
                            };
                        } else {
                            error!("GetName failed: unexpected output format");
                            let error_msg = "Failed to parse yt-dlp metadata output".to_string();
                            send_critical_notification(
                                url,
                                &format!("❌ Download failed: {}", error_msg),
                                config,
                            )
                            .await;
                            *self = Task::Failed {
                                url: url_clone,
                                human_readable_error: error_msg,
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
                        send_critical_notification(
                            url,
                            &format!("❌ Download failed: {}", error_msg),
                            config,
                        )
                        .await;
                        *self = Task::Failed {
                            url: url_clone,
                            human_readable_error: error_msg,
                        };
                    }
                    Err(e) => {
                        error!("GetName spawn failed: {}", e);
                        let error_msg = format!("Failed to spawn yt-dlp: {}", e);
                        send_critical_notification(
                            url,
                            &format!("❌ Download failed: {}", error_msg),
                            config,
                        )
                        .await;
                        *self = Task::Failed {
                            url: url_clone,
                            human_readable_error: error_msg,
                        };
                    }
                }
            }

            // GetName → DownloadVideo: Check disk space and start download
            (Task::GetName { url, metadata }, TaskKind::DownloadVideo) => {
                info!("Transitioning task to DownloadVideo for URL: {}", url);

                let url_clone = url.clone();
                let metadata = match metadata {
                    Some(m) => m.clone(),
                    None => {
                        error!("GetName metadata is None, cannot transition to DownloadVideo");
                        *self = Task::Failed {
                            url: url_clone,
                            human_readable_error: "Missing metadata from GetName phase".to_string(),
                        };
                        return;
                    }
                };

                // Check disk space using df command
                let available_mb = match get_disk_space(&metadata.directory).await {
                    Ok(mb) => mb,
                    Err(e) => {
                        let error_msg = format!("Failed to check disk space: {}", e);
                        error!("{}", error_msg);
                        send_critical_notification(
                            url,
                            &format!("❌ Download failed: {}", error_msg),
                            config,
                        )
                        .await;
                        *self = Task::Failed {
                            url: url_clone,
                            human_readable_error: error_msg,
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
                    send_critical_notification(
                        url,
                        &format!("❌ Download failed: {}", error_msg),
                        config,
                    )
                    .await;
                    *self = Task::Failed {
                        url: url_clone,
                        human_readable_error: error_msg,
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
                let unique_cache = cache_dir.join(&url_hash);
                let is_restart = unique_cache.exists();

                // Send notification with title (different message for restart vs fresh download)
                let title_display = title.as_deref().unwrap_or("video");
                let notification_message = if is_restart {
                    format!("Resuming download: {} 🔄", title_display)
                } else {
                    format!("Downloading: {} 🎬", title_display)
                };
                send_notification(url, &notification_message, Some(3000), config).await;

                *self = Task::DownloadVideo {
                    url: url_clone.clone(),
                    path: PathBuf::from(&directory)
                        .join(format!("{}.mp4", title.as_deref().unwrap_or("download"))),
                    metadata: DownloadMetadata {
                        title: title.clone(),
                        expected_size_bytes: expected_size,
                        directory: directory.clone(),
                        started_at: Some(std::time::Instant::now()),
                        process_id: None,
                        log_file: None,
                    },
                };

                // Note: Actual download will be spawned and polled by daemon
                // This transition just sets up the state
            }

            // DownloadVideo → Completed: This shouldn't happen in practice since we transition directly
            (Task::DownloadVideo { url, path, .. }, TaskKind::Completed) => {
                info!("Transitioning task to Completed for URL: {}", url);
                *self = Task::Completed {
                    url: url.clone(),
                    path: path.clone(),
                };
            }

            // Any → Failed: Mark task as failed with error message
            (task, TaskKind::Failed) => {
                let url = task.url().to_string();
                let error_msg = context.unwrap_or_else(|| "Unknown error".to_string());
                error!("Task failed for URL {}: {}", url, error_msg);
                send_critical_notification(
                    &url,
                    &format!("❌ Download failed: {}", error_msg),
                    config,
                )
                .await;
                *self = Task::Failed {
                    url,
                    human_readable_error: error_msg,
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
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SerializableTask {
    pub url: String,
    pub task_kind: TaskKind,
}

/// Result from a spawned task operation
#[derive(Debug)]
pub enum TaskOperationResult {
    GetNameComplete(GetNameMetadata),
    DownloadComplete(PathBuf),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SerializableTasks {
    pub tasks: Vec<SerializableTask>,
}

#[derive(Debug, Default)]
pub struct Tasks {
    task_list: BTreeMap<u64, Task>,
    index_counter: u64,
    active_tasks: HashMap<u64, JoinHandle<Result<TaskOperationResult>>>,
    status_channels: HashMap<u64, tokio::sync::watch::Sender<TaskStatus>>,
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

    pub fn add_url_as_task(&mut self, url: String) -> Result<u64, String> {
        // Check for duplicate URLs
        for (existing_id, task) in self.task_list.iter() {
            if task.url() == url {
                return Err(format!("URL already exists with task ID {}", existing_id));
            }
        }

        // Create new task with next ID
        let task_id = self.index_counter;
        self.index_counter += 1;

        let task = Task::Queued { url };
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
                    },
                );
                info!("Recovered failed task {} for URL: {}", idx, task.url);
            });

        // Priority 2: Add DownloadVideo tasks as Queued (will be re-promoted)
        serializable
            .tasks
            .iter()
            .filter(|task| matches!(task.task_kind, TaskKind::DownloadVideo))
            .for_each(|task| {
                if !tasks.task_list.values().any(|t| t.url() == task.url) {
                    let idx = tasks.index_counter;
                    tasks.index_counter += 1;
                    tasks.task_list.insert(idx, Task::Queued { url: task.url.clone() });
                    info!("Recovered DownloadVideo task {} as Queued for URL: {} (yt-dlp will resume from fragments)", idx, task.url);
                }
            });

        // Priority 3: Add GetName tasks as Queued (will be re-promoted)
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
                        },
                    );
                    info!(
                        "Recovered GetName task {} as Queued for URL: {}",
                        idx, task.url
                    );
                }
            });

        // Priority 4: Add Queued tasks
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
            Task::Completed { .. } => TaskKind::Completed,
            Task::Failed { .. } => TaskKind::Failed,
        };

        Self {
            url: task.url().to_string(),
            task_kind,
        }
    }
}

// Helper function for touching files to update timestamps
pub async fn touch_file(path: &PathBuf) -> Result<()> {
    let now = filetime::FileTime::now();
    filetime::set_file_times(path, now, now)
        .with_context(|| format!("Failed to set file times for {}", path.display()))?;
    Ok(())
}

/// Spawn and execute a video download task
/// Returns Ok(PathBuf) with final path on success, Err on failure
pub async fn spawn_download_video_task(
    url: String,
    metadata: GetNameMetadata,
    config: Config,
) -> Result<PathBuf> {
    let title = metadata.title.as_deref().unwrap_or("download");

    // Construct final destination path
    let final_path = PathBuf::from(&metadata.directory).join(format!("{}.mp4", title));

    // Determine cache directory for download
    let cache_dir = PathBuf::from(&config.cache_dir);

    // Create unique cache directory for this URL
    let url_hash = format!("{:x}", md5::compute(url.as_bytes()));
    let unique_cache = cache_dir.join(&url_hash);

    tokio::fs::create_dir_all(&unique_cache)
        .await
        .context("Failed to create cache directory")?;

    // Temp download path in cache
    let temp_download_path = unique_cache.join(format!("{}.mp4", title));

    info!(
        "Downloading to cache: {}\nWill move to: {}",
        temp_download_path.display(),
        final_path.display()
    );

    // Build yt-dlp download command
    let mut cmd = Command::new("yt-dlp");
    cmd.args([
        "--newline",
        "--progress",
        "--restrict-filename",
        "--trim-filenames",
        "200",
        "--ignore-config",
        "--no-playlist",
        "--merge-output-format",
        "mp4",
        "--format",
        "best[height<=?720]",
        "--retries",
        "infinite",
        "--fragment-retries",
        "infinite",
        "--retry-sleep",
        "linear=1:120:2",
        "--continue",
        "--skip-unavailable-fragments",
    ]);

    // Set cache directory as home path for downloads and temp for fragments
    cmd.args(["--paths", &format!("home:{}", unique_cache.display())]);
    cmd.args([
        "--paths",
        &format!("temp:{}", unique_cache.join("fragments").display()),
    ]);

    // Add throttle if configured
    if let Some(throttle_kb) = config.throttle {
        cmd.args(["--limit-rate", &format!("{}K", throttle_kb)]);
    }

    // Set output filename
    cmd.args(["-o", &format!("{}.mp4", title)]);
    cmd.arg(&url);

    // Spawn the download process
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let output = cmd.output().await.context("Failed to spawn yt-dlp")?;

    // Check exit status
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let error_msg = stderr.lines().next().unwrap_or("unknown error");
        return Err(anyhow::anyhow!("yt-dlp failed: {}", error_msg));
    }

    info!(
        "Download completed successfully to cache: {}",
        temp_download_path.display()
    );

    // Verify temp file exists
    if !temp_download_path.exists() {
        return Err(anyhow::anyhow!(
            "Downloaded file not found in cache: {}",
            temp_download_path.display()
        ));
    }

    // Ensure final destination directory exists
    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("Failed to create destination directory")?;
    }

    // Move file from cache to final destination
    info!("Moving file from cache to: {}", final_path.display());
    if let Err(e) = tokio::fs::rename(&temp_download_path, &final_path).await {
        // If rename fails (different filesystems), try copy + delete
        warn!("Rename failed, trying copy: {}", e);
        tokio::fs::copy(&temp_download_path, &final_path)
            .await
            .context("Failed to copy file to destination")?;

        // Successfully copied, clean up
        let _ = tokio::fs::remove_file(&temp_download_path).await;
        let _ = tokio::fs::remove_dir_all(&unique_cache).await;
    } else {
        // Successfully moved, clean up cache directory
        let _ = tokio::fs::remove_dir_all(&unique_cache).await;
    }

    info!(
        "File successfully moved to final destination: {}",
        final_path.display()
    );

    Ok(final_path)
}

/// Get available disk space in megabytes for the given path
/// Returns Ok(available_mb) with the available space, or Err if unable to determine
async fn get_disk_space(path: &str) -> anyhow::Result<u32> {
    let mut current_path = std::path::Path::new(path);

    // Find first existing parent directory
    while !current_path.exists() {
        if let Some(parent) = current_path.parent() {
            current_path = parent;
        } else {
            anyhow::bail!("Cannot determine disk space for path");
        }
    }

    // Use df command to check available space
    let output = Command::new("df")
        .arg("--output=avail")
        .arg("--block-size=1M") // Output in megabytes
        .arg(current_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        anyhow::bail!("df command failed");
    }

    let output_str = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = output_str.lines().collect();

    if lines.len() >= 2 {
        if let Ok(available_mb) = lines[1].trim().parse::<u32>() {
            return Ok(available_mb);
        }
    }

    anyhow::bail!("Failed to parse df output")
}
