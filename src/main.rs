mod client;
mod common;
mod daemon;
mod task;

#[cfg(test)]
mod tests;

use anyhow::Result;
use clap::{Parser, Subcommand};
use common::APP;
use directories::{BaseDirs, ProjectDirs};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use task::{Task, TaskKind, Tasks};
use tokio::process::Command as TokioCommand;
use tracing::{info, warn};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

use crate::common::send_notification;

#[derive(Serialize, Deserialize, Debug)]
pub enum ClientRequest {
    Add { url: String },
    Remove { id: u64 },
    Clear,
    Status { verbose: bool },
    Kill { sender_pid: u32 },
    Info { id: u64 },
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ServerResponse {
    Success {
        message: String,
    },
    Error {
        message: String,
    },
    Status {
        queued_tasks: Vec<TaskSummary>,
        current_tasks: Vec<TaskSummary>,
        completed_tasks: Vec<TaskSummary>,
        failed_tasks: Vec<TaskSummary>,
        uptime_seconds: u64,
        config_summary: ConfigSummary,
    },
    Info {
        log_file_path: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TaskSummary {
    pub id: u64,
    pub url: String,
    pub title: Option<String>,
    pub path: Option<String>,
    pub task_type: String,
    pub error: Option<String>,
    pub timestamp: Option<String>,
    pub elapsed_seconds: Option<u64>,
    pub is_paused: bool,
    pub paused_reason: Option<String>,
    pub progress_percent: Option<f32>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ConfigSummary {
    pub concurrent_downloads: u8,
    pub disk_threshold: u32,
    pub socket_path: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Config {
    pub concurrent_downloads: u8,
    pub socket_path: String,
    pub disk_threshold: u32,      // Megabytes to keep in reserve
    pub download_dir: String,     // Default directory for downloads
    pub should_notify_send: bool, // Whether to send desktop notifications
    pub cache_dir: String,        // Directory for temporary files (fragments, json, etc)
    pub throttle: Option<u32>,    // Download speed limit in KB/s, None = unlimited
    pub video_quality: String, // Video quality: "480p", "720p", "1080p", "1440p", "2160p", "best", or custom
}

impl Default for Config {
    fn default() -> Self {
        let default_socket_path = get_default_socket_path();
        let default_download_dir = get_default_video_dir();
        let default_cache_dir = get_default_cache_dir();
        Self {
            concurrent_downloads: 1,
            socket_path: default_socket_path,
            disk_threshold: 1024 * 2, // 2GB default
            download_dir: default_download_dir,
            should_notify_send: true,          // Default to enabled
            cache_dir: default_cache_dir,      // Use XDG cache dir
            throttle: None,                    // Unlimited download speed by default
            video_quality: "720p".to_string(), // 720p default
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct QueuedTasks {
    pub urls: Vec<String>,
}

#[derive(Debug)]
pub enum TaskByUrl {
    Exists { id: u64, kind: TaskKind },
    DoesntExist,
}

#[derive(Debug)]
pub struct TaskManager {
    pub tasks: Tasks,
    pub config: Config,
    pub start_time: std::time::Instant,
}

#[derive(Parser)]
#[command(name = APP)]
#[command(about = "yt-dlp task spooler")]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,

    #[arg(help = "URL to download")]
    pub url: Option<String>,

    #[arg(short, long, help = "Show verbose status information")]
    pub verbose: bool,

    #[arg(long, help = "Kill the running daemon")]
    pub kill: bool,

    #[arg(long, help = "Remove task(s) by ID (comma-separated for multiple)")]
    pub remove: Option<String>,

    #[arg(long, help = "Show detailed info/logs for a task")]
    pub info: Option<u64>,

    #[arg(long, help = "Clear all completed downloads")]
    pub clear: bool,

    #[arg(long, help = "Show only failed tasks")]
    pub failed: bool,

    #[arg(long, global = true, help = "Path to custom config file")]
    pub config: Option<String>,
}

#[derive(Subcommand)]
pub enum Command {
    #[command(about = "Start the daemon")]
    Daemon {
        #[arg(long, help = "Run daemon in a detached tmux session")]
        tmux: bool,
        #[arg(long, help = "Kill existing daemon before starting")]
        kill: bool,
    },
}

/// Parse comma-separated list of task IDs
pub fn parse_id_list(input: &str) -> Result<Vec<u64>, String> {
    input
        .split(',')
        .map(|s| {
            s.trim()
                .parse::<u64>()
                .map_err(|e| format!("Invalid task ID '{}': {}", s.trim(), e))
        })
        .collect()
}

pub fn get_default_socket_path() -> String {
    let project_dirs =
        ProjectDirs::from("", "", APP).expect("Could not determine project directories");

    let runtime_dir = project_dirs
        .runtime_dir()
        .unwrap_or_else(|| project_dirs.cache_dir());

    std::fs::create_dir_all(runtime_dir).expect("Could not create runtime directory");
    runtime_dir
        .join(format!("{APP}.sock"))
        .to_string_lossy()
        .to_string()
}

pub fn get_socket_path() -> PathBuf {
    let config = load_config();
    PathBuf::from(&config.socket_path)
}

pub fn get_data_dir() -> PathBuf {
    // For development/testing, if we have a custom download dir in /tmp, use that as base for data too
    if let Ok(test_data_dir) = std::env::var("TSP_YTDLP_DATA_DIR") {
        let data_dir = PathBuf::from(test_data_dir);
        std::fs::create_dir_all(&data_dir).expect("Could not create test data directory");
        return data_dir;
    }

    let project_dirs =
        ProjectDirs::from("", "", APP).expect("Could not determine project directories");

    let data_dir = project_dirs.data_dir();
    std::fs::create_dir_all(data_dir).expect("Could not create data directory");
    data_dir.to_path_buf()
}

pub fn get_config_path() -> PathBuf {
    let base_dirs = BaseDirs::new().expect("Could not determine base directories");

    let config_dir = base_dirs.config_dir().join(APP);
    std::fs::create_dir_all(&config_dir).expect("Could not create config directory");
    config_dir.join("config.toml")
}

pub fn load_config_from_path(custom_path: Option<&str>) -> Config {
    let config_path = if let Some(path) = custom_path {
        PathBuf::from(path)
    } else {
        get_config_path()
    };

    if config_path.exists() {
        match std::fs::read_to_string(&config_path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(config) => {
                    tracing::debug!("Loaded config from: {:?}", config_path);
                    return config;
                }
                Err(e) => {
                    warn!("Failed to parse config file: {}, using defaults", e);
                }
            },
            Err(e) => {
                warn!("Failed to read config file: {}, using defaults", e);
            }
        }
    } else if custom_path.is_none() {
        // Only create default config file if using default path (not for custom configs)
        let default_config = Config::default();
        let config_content = format!(
            r#"# {} Configuration File
# Number of concurrent downloads (0 = unlimited)
concurrent_downloads = {}

# Unix socket path for daemon communication
socket_path = "{}"

# Disk space threshold in MB to keep in reserve
disk_threshold = {}

# Default directory for downloads (fallback when get_video_dir.sh doesn't provide one)
download_dir = "{}"

# Directory for temporary files (json, part files, etc)
cache_dir = "{}"

# Whether to send desktop notifications (true/false)
should_notify_send = {}

# Download speed throttle in KB/s (optional, unlimited if not set)
# throttle = 1500

# Video quality setting: "480p", "720p", "1080p", "1440p", "2160p", "best", or custom format
video_quality = "{}"
"#,
            APP,
            default_config.concurrent_downloads,
            default_config.socket_path,
            default_config.disk_threshold,
            default_config.download_dir,
            default_config.cache_dir,
            default_config.should_notify_send,
            default_config.video_quality
        );

        if let Err(e) = std::fs::write(&config_path, config_content) {
            eprintln!("Failed to create default config file: {e}");
        } else {
            tracing::debug!("Created default config at: {:?}", config_path);
        }
    }

    Config::default()
}

pub fn load_config() -> Config {
    load_config_from_path(None)
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskManager {
    pub fn new() -> Self {
        let config = load_config();
        Self {
            tasks: Tasks::default(),
            config,
            start_time: std::time::Instant::now(),
        }
    }

    pub fn new_with_config(config: Config) -> Self {
        Self {
            tasks: Tasks::default(),
            config,
            start_time: std::time::Instant::now(),
        }
    }

    pub fn load_from_disk() -> Result<Self> {
        Self::load_from_disk_with_config(None)
    }

    pub fn load_from_disk_with_config(custom_config_path: Option<&str>) -> Result<Self> {
        let data_dir = get_data_dir();
        let tasks_path = data_dir.join("tasks.json");
        let config = load_config_from_path(custom_config_path);

        let tasks = if tasks_path.exists() {
            info!("Loading tasks from unified tasks.json at: {:?}", tasks_path);
            let loaded_tasks = Tasks::load_from_file(&tasks_path)?;
            info!("Loaded {} tasks from disk", loaded_tasks.len());
            loaded_tasks
        } else {
            info!(
                "tasks.json not found at {:?}, starting with empty task list",
                tasks_path
            );
            Tasks::default()
        };

        Ok(TaskManager {
            tasks,
            config,
            start_time: std::time::Instant::now(),
        })
    }

    pub fn add_task(&mut self, url: String) -> Result<u64, String> {
        self.tasks.add_url_as_task(url)
    }

    pub fn remove_task(&mut self, id: u64) -> bool {
        self.tasks.remove_task(id)
    }

    pub fn save_tasks(&self) -> Result<()> {
        let data_dir = get_data_dir();
        let tasks_path = data_dir.join("tasks.json");
        self.tasks.save_to_file(&tasks_path)
    }

    pub fn check_duplicate_url(&self, url: &str) -> Option<String> {
        for (id, task) in self.tasks.iter() {
            if task.url() == url {
                return Some(format!("URL already exists with task ID {id}"));
            }
        }
        None
    }

    /// Check if a URL already exists and return its ID and task kind
    pub fn find_task_by_url(&self, url: &str) -> TaskByUrl {
        for (id, task) in self.tasks.iter() {
            if task.url() == url {
                let kind = match task {
                    task::Task::Queued { .. } => TaskKind::Queued,
                    task::Task::GetName { .. } => TaskKind::GetName,
                    task::Task::DownloadVideo { .. } => TaskKind::DownloadVideo,
                    task::Task::Completed { .. } => TaskKind::Completed,
                    task::Task::Failed { .. } => TaskKind::Failed,
                };
                return TaskByUrl::Exists { id: *id, kind };
            }
        }
        TaskByUrl::DoesntExist
    }

    pub fn get_status(&self, _verbose: bool) -> ServerResponse {
        let uptime_seconds = self.start_time.elapsed().as_secs();

        let mut queued_tasks = Vec::new();
        let mut current_tasks = Vec::new();
        let mut completed_tasks = Vec::new();
        let mut failed_tasks = Vec::new();

        for (id, task) in self.tasks.iter() {
            let summary = TaskSummary {
                id: *id,
                url: task.url().to_string(),
                title: self.get_task_title(task),
                path: self.get_task_path(task),
                task_type: self.get_task_type(task),
                error: self.get_task_error(task),
                timestamp: None,       // Could be enhanced later
                elapsed_seconds: None, // Could be enhanced later
                is_paused: false,
                paused_reason: None,
                progress_percent: self.get_task_progress(task),
            };

            match task {
                Task::Queued { .. } => queued_tasks.push(summary),
                Task::GetName { .. } | Task::DownloadVideo { .. } => current_tasks.push(summary),
                Task::Completed { .. } => completed_tasks.push(summary),
                Task::Failed { .. } => failed_tasks.push(summary),
            }
        }

        let config_summary = ConfigSummary {
            concurrent_downloads: self.config.concurrent_downloads,
            disk_threshold: self.config.disk_threshold,
            socket_path: self.config.socket_path.clone(),
        };

        ServerResponse::Status {
            queued_tasks,
            current_tasks,
            completed_tasks,
            failed_tasks,
            uptime_seconds,
            config_summary,
        }
    }

    fn get_task_title(&self, task: &Task) -> Option<String> {
        match task {
            Task::GetName { metadata, .. } => metadata.as_ref().and_then(|m| m.title.clone()),
            Task::DownloadVideo { metadata, .. } => metadata.title.clone(),
            _ => None,
        }
    }

    fn get_task_path(&self, task: &Task) -> Option<String> {
        match task {
            Task::DownloadVideo { path, .. } => Some(path.to_string_lossy().to_string()),
            Task::Completed { path, .. } => Some(path.to_string_lossy().to_string()),
            _ => None,
        }
    }

    fn get_task_type(&self, task: &Task) -> String {
        match task {
            Task::Queued { .. } => "Queued".to_string(),
            Task::GetName { .. } => "GetName".to_string(),
            Task::DownloadVideo { .. } => "DownloadVideo".to_string(),
            Task::Completed { .. } => "Completed".to_string(),
            Task::Failed { .. } => "Failed".to_string(),
        }
    }

    fn get_task_error(&self, task: &Task) -> Option<String> {
        match task {
            Task::Failed {
                human_readable_error,
                ..
            } => Some(human_readable_error.clone()),
            _ => None,
        }
    }

    fn get_task_progress(&self, task: &Task) -> Option<f32> {
        match task {
            Task::DownloadVideo { path, metadata, .. } => {
                if let Some(expected_bytes) = metadata.expected_size_bytes {
                    // Try to get file size from various possible locations
                    let current_bytes = std::fs::metadata(path)
                        .or_else(|_| std::fs::metadata(path.with_extension("part")))
                        .or_else(|_| {
                            let part_path = format!("{}.part", path.display());
                            std::fs::metadata(part_path)
                        })
                        .map(|m| m.len())
                        .ok()?;

                    Some((current_bytes as f64 / expected_bytes as f64 * 100.0) as f32)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn clear_completed(&mut self) {
        // Remove all completed tasks
        let completed_ids: Vec<u64> = self
            .tasks
            .iter()
            .filter_map(|(id, task)| {
                if matches!(task, Task::Completed { .. }) {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();

        for id in completed_ids {
            self.tasks.remove_task(id);
        }
    }
}

pub fn get_default_video_dir() -> String {
    std::env::var("HOME")
        .map(|home| format!("{home}/Videos/youtube"))
        .unwrap_or_else(|_| "/tmp/videos".to_string())
}

pub fn get_default_cache_dir() -> String {
    let project_dirs =
        ProjectDirs::from("", "", APP).expect("Could not determine project directories");
    project_dirs.cache_dir().to_string_lossy().to_string()
}

/// Check if the daemon is currently running by attempting to connect to its Unix socket
pub async fn is_daemon_active(config: &Config) -> bool {
    use tokio::net::UnixStream;
    UnixStream::connect(&config.socket_path).await.is_ok()
}

/// Load queued tasks from disk
pub fn load_queued_tasks() -> Result<QueuedTasks> {
    let data_dir = get_data_dir();
    let queued_path = data_dir.join("queued_tasks.json");

    if !queued_path.exists() {
        return Ok(QueuedTasks::default());
    }

    let content = std::fs::read_to_string(&queued_path)?;
    if content.trim().is_empty() {
        return Ok(QueuedTasks::default());
    }

    let queued: QueuedTasks = serde_json::from_str(&content)?;
    Ok(queued)
}

/// Append a URL to the queued tasks file
pub async fn append_to_queued_tasks(url: String, config: &Config) -> Result<()> {
    let data_dir = get_data_dir();
    let queued_path = data_dir.join("queued_tasks.json");

    let mut queued = load_queued_tasks().unwrap_or_default();
    queued.urls.push(url.clone());
    let content = serde_json::to_string_pretty(&queued)?;
    std::fs::write(&queued_path, content)?;
    send_notification(&url, &format!("Added {url} to queued tasks\ndaemon is not running 🤨"), timeout_ms, config)

    Ok(())
}

/// Cleanup cache directory for a given URL
pub async fn cleanup_cache_for_url(url: &str, config: &Config) {
    // Determine cache directory (same logic as in download)
    let cache_dir = PathBuf::from(&config.cache_dir);

    // Compute URL hash
    let url_hash = format!("{:x}", md5::compute(url.as_bytes()));
    let unique_cache = cache_dir.join(&url_hash);

    // Remove cache directory if it exists
    if unique_cache.exists() {
        match tokio::fs::remove_dir_all(&unique_cache).await {
            Ok(_) => {
                info!("Cleaned up cache for URL: {}", url);
            }
            Err(e) => {
                warn!("Failed to cleanup cache for URL {}: {}", url, e);
            }
        }
    }
}

pub async fn get_video_dir_for_url(url: &str, config: &Config) -> String {
    // Use config's download_dir as default
    let default_dir = &config.download_dir;

    // For YouTube URLs, always use default
    if url.contains("youtube.com") {
        info!(
            "YouTube URL detected, using default directory: {}",
            default_dir
        );
        return default_dir.clone();
    }

    // Check if custom script exists and is executable
    let script_path = std::env::var("HOME")
        .map(|home| format!("{home}/source/private/get_video_dir.sh"))
        .unwrap_or_else(|_| "/tmp/nonexistent".to_string());

    let script_path_obj = std::path::Path::new(&script_path);

    // Check if script exists
    if !script_path_obj.exists() {
        info!(
            "get_video_dir.sh not found at {}, using default directory: {}",
            script_path, default_dir
        );
        return default_dir.clone();
    }

    // Check if script is executable (Unix permissions)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = script_path_obj.metadata() {
            let permissions = metadata.permissions();
            if permissions.mode() & 0o111 == 0 {
                warn!(
                    "get_video_dir.sh at {} is not executable, using default directory: {}",
                    script_path, default_dir
                );
                return default_dir.clone();
            }
        }
    }

    // Try to execute the script
    match TokioCommand::new(&script_path)
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
    {
        Ok(output) => {
            if output.status.success() {
                let custom_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();

                if !custom_dir.is_empty() && custom_dir != *default_dir {
                    // Validate the path - must start with / or ~ or $HOME
                    if custom_dir.starts_with('/')
                        || custom_dir.starts_with('~')
                        || custom_dir.starts_with("$HOME")
                    {
                        // Expand $HOME if present
                        let expanded_dir = if custom_dir.starts_with("$HOME") {
                            custom_dir.replace(
                                "$HOME",
                                &std::env::var("HOME")
                                    .expect("HOME environment variable must be set"),
                            )
                        } else if custom_dir.starts_with('~') {
                            custom_dir.replace(
                                "~",
                                &std::env::var("HOME")
                                    .expect("HOME environment variable must be set"),
                            )
                        } else {
                            custom_dir
                        };

                        // Try to create the directory
                        match tokio::fs::create_dir_all(&expanded_dir).await {
                            Ok(_) => {
                                info!(
                                    "Custom directory detected for URL: {} -> {}",
                                    url, expanded_dir
                                );
                                return expanded_dir;
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to create custom directory: {}, using default: {}",
                                    e, default_dir
                                );
                            }
                        }
                    } else {
                        warn!(
                            "get_video_dir.sh returned invalid directory path: {}, using default",
                            custom_dir
                        );
                    }
                } else {
                    info!("get_video_dir.sh returned empty/default result, using default directory for URL: {}", url);
                }
            } else {
                let error = String::from_utf8_lossy(&output.stderr);
                warn!(
                    "get_video_dir.sh failed: {}, using default directory",
                    error
                );
            }
        }
        Err(e) => {
            warn!(
                "Failed to execute get_video_dir.sh: {}, using default directory",
                e
            );
        }
    }

    info!("Using default directory: {}", default_dir);
    default_dir.clone()
}

pub fn get_progress_file_path(url: &str) -> String {
    let url_md5 = format!("{:x}", md5::compute(url.as_bytes()));
    format!("/tmp/{APP}/progress_{url_md5}.txt")
}

pub async fn validate_download_dir(download_dir: &str) -> Result<()> {
    tracing::debug!("Validating download directory: {}", download_dir);

    // Create directory if it doesn't exist
    match tokio::fs::create_dir_all(download_dir).await {
        Ok(()) => {
            tracing::debug!("Download directory created/verified: {}", download_dir);
        }
        Err(e) => {
            anyhow::bail!(
                "Failed to create download directory '{}': {}",
                download_dir,
                e
            );
        }
    }

    // Check write permissions by creating a temporary file
    let temp_file_path = std::path::Path::new(download_dir).join(format!(".{APP}_write_test"));
    match tokio::fs::write(&temp_file_path, "test").await {
        Ok(()) => {
            // Clean up the test file
            let _ = tokio::fs::remove_file(&temp_file_path).await;
            tracing::debug!("Download directory is writable: {}", download_dir);
            Ok(())
        }
        Err(e) => anyhow::bail!(
            "Download directory '{}' is not writable: {}",
            download_dir,
            e
        ),
    }
}

pub async fn validate_ytdlp() -> Result<()> {
    tracing::debug!("Validating yt-dlp installation...");

    match TokioCommand::new("yt-dlp")
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
    {
        Ok(output) => {
            if output.status.success() {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                tracing::debug!("yt-dlp found: version {}", version);
                Ok(())
            } else {
                let error = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("yt-dlp command failed: {}", error)
            }
        }
        Err(e) => anyhow::bail!(
            "yt-dlp not found in PATH: {}. Please install yt-dlp to use this daemon.",
            e
        ),
    }
}

pub async fn validate_curl() -> Result<()> {
    tracing::debug!("Validating curl installation...");

    match TokioCommand::new("curl")
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
    {
        Ok(output) => {
            if output.status.success() {
                let output_str = String::from_utf8_lossy(&output.stdout);
                let version = output_str.lines().next().unwrap_or("unknown");
                tracing::debug!("curl found: {}", version);
                Ok(())
            } else {
                let error = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("curl command failed: {}", error)
            }
        }
        Err(e) => anyhow::bail!(
            "curl not found in PATH: {}. Please install curl for network connectivity checks.",
            e
        ),
    }
}

pub fn init_tracing() -> Result<()> {
    let base_dirs =
        BaseDirs::new().ok_or_else(|| anyhow::anyhow!("Could not determine base directories"))?;

    let cache_dir = base_dirs.cache_dir().join(APP);
    std::fs::create_dir_all(&cache_dir)?;

    // File appender for logs
    let file_appender = RollingFileAppender::new(Rotation::DAILY, &cache_dir, format!("{APP}.log"));

    // Initialize tracing with both stdout and file output
    // Ensure info logs go to stdout in both debug and release modes
    let stdout_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(format!("{APP}=info")));

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_target(false)
                .with_thread_ids(true)
                .with_ansi(true) // Enable colors for stdout
                .with_writer(std::io::stdout)
                .with_filter(stdout_filter), // stdout gets info level (or env override)
        )
        .with(
            fmt::layer()
                .with_target(false)
                .with_thread_ids(true)
                .with_ansi(false)
                .with_writer(file_appender)
                .with_filter(EnvFilter::new(format!("{APP}=debug"))), // file gets debug level
        )
        .init();

    info!(
        "Logging initialized - logs will be written to: {:?}",
        cache_dir.join(format!("{APP}.log"))
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();
    let config = load_config_from_path(args.config.as_deref());

    match args.command {
        Some(Command::Daemon { tmux, kill }) => {
            // Handle --kill flag first (before --tmux processing)
            if kill {
                let binary_path =
                    std::env::current_exe().expect("Failed to get current executable path");

                println!("Killing existing daemon...");
                let kill_status = std::process::Command::new(&binary_path)
                    .arg("--kill")
                    .status();

                match kill_status {
                    Ok(_) => {
                        // Continue with daemon startup after kill
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to run kill command: {}", e);
                    }
                }
            }

            if tmux {
                // Check if tmux session already exists
                let check_status = std::process::Command::new("tmux")
                    .args(["has-session", "-t", "tsp_ytdlp"])
                    .stderr(std::process::Stdio::null())
                    .status();

                match check_status {
                    Ok(status) if status.success() => {
                        eprintln!("Error: tmux session 'tsp_ytdlp' already exists");
                        eprintln!("Attach with: tmux attach -t tsp_ytdlp");
                        std::process::exit(1);
                    }
                    Ok(_) => {
                        // Session doesn't exist, create it
                        println!("Launching daemon in session tsp_ytdlp");
                        let binary_path =
                            std::env::current_exe().expect("Failed to get current executable path");

                        // Get all args except the binary name and filter out --tmux and --kill
                        let filtered_args: Vec<String> = std::env::args()
                            .skip(1)
                            .filter(|arg| arg != "--tmux" && arg != "--kill")
                            .collect();

                        // Build tmux command
                        let mut tmux_cmd = std::process::Command::new("tmux");
                        tmux_cmd.args([
                            "new-session",
                            "-d",
                            "-s",
                            "tsp_ytdlp",
                            "-n",
                            "tsp-ytdlp daemon",
                            "--",
                        ]);
                        tmux_cmd.arg(binary_path);
                        tmux_cmd.args(&filtered_args);

                        // Use exec to replace current process
                        use std::os::unix::process::CommandExt;
                        let err = tmux_cmd.exec();
                        // If we get here, exec failed
                        eprintln!("Failed to exec tmux: {}", err);
                        std::process::exit(1);
                    }
                    Err(e) => {
                        eprintln!("Error: Failed to check tmux session: {}", e);
                        eprintln!("Is tmux installed?");
                        std::process::exit(1);
                    }
                }
            } else {
                // Run as daemon normally
                init_tracing()?;
                info!("Starting {} daemon", APP);
                validate_ytdlp().await?;
                validate_curl().await?;
                validate_download_dir(&config.download_dir).await?;

                daemon::run_daemon(config).await?;
            }
        }
        None => {
            // Client mode
            if args.kill {
                client::run_client_kill(&config).await?;
            } else if let Some(ref remove_str) = args.remove {
                let ids = parse_id_list(remove_str).map_err(|e| anyhow::anyhow!(e))?;
                client::run_client_remove(ids, &config).await?;
            } else if let Some(id) = args.info {
                client::run_client_info(id, &config).await?;
            } else if args.clear {
                client::run_client_clear(&config).await?;
            } else if let Some(url) = args.url {
                client::run_client_add(url, &config).await?;
            } else {
                // Default: show status
                if args.failed {
                    client::run_client_failed(&config).await?;
                } else {
                    client::run_client_status(args.verbose, &config, true).await?;
                }
            }
        }
    }

    Ok(())
}
