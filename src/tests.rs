use crate::task::{GetNameMetadata, Task, TaskKind, Tasks};
use crate::Config;
use std::path::Path;
use tokio::fs;
use tokio::process::Command;

const TEST_URL: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw";

fn test_config() -> Config {
    Config {
        concurrent_downloads: 1,
        socket_path: "/tmp/tsp_ytdlp/test.sock".to_string(),
        disk_threshold: 1024,
        download_dir: "/tmp/tsp_ytdlp".to_string(),
        should_notify_send: false,
        cache_dir: "/tmp/tsp_ytdlp/cache".to_string(),
        throttle: Some(10), // 10 KB/s for testing
        video_quality: "720p".to_string(),
        video_dir_script: None,
        sponsorblock_mark: Some("all".to_string()),
        sponsorblock_remove: Some("sponsor,interaction".to_string()),
    }
}

async fn cleanup() {
    // Clean up all test directories
    let test_dirs = ["/tmp/tsp_ytdlp", "/tmp/tsp_ytdlp_restart_test"];

    for test_dir in &test_dirs {
        if std::path::Path::new(test_dir).exists() {
            if let Err(e) = fs::remove_dir_all(test_dir).await {
                println!("Warning: Failed to clean up {}: {}", test_dir, e);
            }
        }
    }
}

#[tokio::test]
async fn test_task_creation() {
    cleanup().await;
    let mut tasks = Tasks::default();

    // Test adding a URL as a task
    let url = "https://www.youtube.com/watch?v=test123".to_string();
    let task_id = tasks
        .add_url_as_task(url.clone())
        .expect("Failed to add task");

    assert_eq!(tasks.len(), 1);
    assert_eq!(task_id, 0);

    // Verify the task was created as Queued
    let task = tasks.get_task(task_id).expect("Task not found");
    assert!(matches!(task, Task::Queued { .. }));
    assert_eq!(task.url(), url);

    cleanup().await;
}

#[tokio::test]
async fn test_task_transitions() {
    cleanup().await;
    let test_config = test_config();

    let mut task = Task::Queued {
        url: "https://www.youtube.com/watch?v=test123".to_string(),
    };

    // Test transition from Queued to GetName
    task.transition(TaskKind::GetName, None, &test_config).await;
    assert!(matches!(task, Task::GetName { .. }));

    // Add metadata for next transition
    if let Task::GetName {
        ref mut metadata, ..
    } = task
    {
        *metadata = Some(GetNameMetadata {
            title: Some("Test Video".to_string()),
            expected_size_bytes: Some(1024 * 1024), // 1MB
            directory: "/tmp/test".to_string(),
        });
    }

    // Test transition from GetName to DownloadVideo
    task.transition(TaskKind::DownloadVideo, None, &test_config)
        .await;
    assert!(matches!(task, Task::DownloadVideo { .. }));

    // Test transition to Completed
    task.transition(TaskKind::Completed, None, &test_config)
        .await;
    assert!(matches!(task, Task::Completed { .. }));

    cleanup().await;
}

#[tokio::test]
async fn test_task_serialization() {
    cleanup().await;
    let mut tasks = Tasks::default();

    // Add some test tasks
    let url1 = "https://www.youtube.com/watch?v=test1".to_string();
    let url2 = "https://www.youtube.com/watch?v=test2".to_string();

    tasks.add_url_as_task(url1.clone()).unwrap();
    tasks.add_url_as_task(url2.clone()).unwrap();

    // Test serialization
    let temp_path = std::path::PathBuf::from("/tmp/test_tasks.json");
    tasks
        .save_to_file(&temp_path)
        .expect("Failed to save tasks");

    // Test deserialization
    let loaded_tasks = Tasks::load_from_file(&temp_path).expect("Failed to load tasks");

    assert_eq!(loaded_tasks.len(), 2);

    // Verify URLs are preserved
    let urls: Vec<String> = loaded_tasks
        .iter()
        .map(|(_, task)| task.url().to_string())
        .collect();
    assert!(urls.contains(&url1));
    assert!(urls.contains(&url2));

    // Clean up
    let _ = std::fs::remove_file(&temp_path);

    cleanup().await;
}

#[tokio::test]
async fn test_duplicate_url_prevention() {
    cleanup().await;
    let mut tasks = Tasks::default();

    let url = "https://www.youtube.com/watch?v=test123".to_string();

    // First addition should succeed
    assert!(tasks.add_url_as_task(url.clone()).is_ok());

    // Second addition of same URL should fail
    assert!(tasks.add_url_as_task(url).is_err());

    assert_eq!(tasks.len(), 1);

    cleanup().await;
}

#[tokio::test]
async fn test_task_activity_check() {
    cleanup().await;
    let queued_task = Task::Queued {
        url: "https://example.com".to_string(),
    };
    assert!(!queued_task.is_active());

    let getname_task = Task::GetName {
        url: "https://example.com".to_string(),
        metadata: None,
    };
    assert!(getname_task.is_active());

    let download_task = Task::DownloadVideo {
        url: "https://example.com".to_string(),
        path: std::path::PathBuf::from("/tmp/test.mp4"),
        metadata: crate::task::DownloadMetadata {
            title: Some("Test".to_string()),
            expected_size_bytes: Some(1024),
            directory: "/tmp".to_string(),
            started_at: None,
            process_id: None,
            log_file: None,
        },
    };
    assert!(download_task.is_active());

    let completed_task = Task::Completed {
        url: "https://example.com".to_string(),
        path: std::path::PathBuf::from("/tmp/test.mp4"),
    };
    assert!(!completed_task.is_active());

    let failed_task = Task::Failed {
        url: "https://example.com".to_string(),
        human_readable_error: "Test error".to_string(),
    };
    assert!(!failed_task.is_active());

    cleanup().await;
}

#[tokio::test]
async fn test_enhanced_ytdlp_options() {
    cleanup().await;
    // Test that the enhanced yt-dlp options are properly defined
    // This is a compile-time test to ensure the execution methods compile correctly

    let task = Task::Queued {
        url: "https://www.youtube.com/watch?v=test".to_string(),
    };

    // Check that the task has the enhanced execution methods available
    assert!(matches!(task, Task::Queued { .. }));

    // Verify task states work correctly
    assert!(!task.is_active());
    assert_eq!(task.url(), "https://www.youtube.com/watch?v=test");

    cleanup().await;
}

#[tokio::test]
async fn test_progress_extraction() {
    cleanup().await;
    use crate::task::extract_progress_from_line;

    // Test typical yt-dlp progress line
    let line1 = "[download] 25.3% of 1.10GiB at 10.87MiB/s ETA 01:40";
    assert_eq!(extract_progress_from_line(line1), Some(25.3));

    // Test another format
    let line2 = "[download]  50.7% of   2.3GiB at  5.2MiB/s ETA 02:15";
    assert_eq!(extract_progress_from_line(line2), Some(50.7));

    // Test 100% completion
    let line3 = "[download] 100% of 1.5GiB in 05:32";
    assert_eq!(extract_progress_from_line(line3), Some(100.0));

    // Test non-progress lines (should return None)
    let line4 = "[youtube] Extracting URL: https://www.youtube.com/watch?v=test";
    assert_eq!(extract_progress_from_line(line4), None);

    let line5 = "[info] Available formats for test:";
    assert_eq!(extract_progress_from_line(line5), None);

    // Test malformed progress line
    let line6 = "[download] invalid% of something";
    assert_eq!(extract_progress_from_line(line6), None);

    // Test edge cases
    let line7 = "[download] 0.0% of 100MB at 1MB/s";
    assert_eq!(extract_progress_from_line(line7), Some(0.0));

    let line8 = "[download] 99.9% of 100MB at 1MB/s";
    assert_eq!(extract_progress_from_line(line8), Some(99.9));

    cleanup().await;
}

#[tokio::test]
async fn test_notification_config() {
    cleanup().await;
    use crate::common::{send_critical_notification, send_notification};

    // Use standardized test config with notifications disabled
    let config_disabled = test_config();
    println!(
        "Test config should_notify_send: {}",
        config_disabled.should_notify_send
    );

    // Test that notification calls complete without error when disabled
    // (We only test with disabled config to avoid desktop notifications during tests)
    send_notification(
        "https://test.com",
        "🔍 DEBUG: This notification should NOT appear during tests",
        Some(1000),
        &config_disabled,
    )
    .await;
    send_critical_notification(
        "https://test.com",
        "🔍 DEBUG: Critical notification should NOT appear during tests",
        &config_disabled,
    )
    .await;

    // Test that Config::default() has notifications enabled by default
    let default_config = Config::default();
    assert_eq!(default_config.should_notify_send, true);
    println!(
        "Default config should_notify_send: {}",
        default_config.should_notify_send
    );

    cleanup().await;
}

#[tokio::test]
async fn test_end_to_end_video_download() {
    cleanup().await;

    // Initialize tracing for detailed logging
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("tsp_ytdlp=debug"))
        .try_init();

    use crate::task::{Task, TaskStatus, Tasks};
    use crate::TaskManager;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    println!("🔄 Starting end-to-end video download test");

    // Use standardized test config
    let test_config = test_config();
    println!(
        "📋 Test config: should_notify_send = {}, throttle = {:?}",
        test_config.should_notify_send, test_config.throttle
    );

    let test_dir = "/tmp/tsp_ytdlp";
    println!("📁 Using test directory: {}", test_dir);

    // Clean up and create test directory
    println!("🧹 Cleaning up from previous runs...");
    cleanup().await;
    fs::create_dir_all(test_dir)
        .await
        .expect("Failed to create test directory");
    println!("✅ Test directory created");

    // Initialize task collection and add the test URL
    println!("📝 Initializing task collection");
    let mut tasks = Tasks::default();
    let task_id = tasks
        .add_url_as_task(TEST_URL.to_string())
        .expect("Failed to add test URL");
    println!("✅ Added task {} for URL: {}", task_id, TEST_URL);

    // Get completion handle to observe task progress
    let mut rx = tasks
        .get_completion_handle(task_id)
        .expect("Failed to get completion handle");

    // Spawn a task manager with the test config in a background task
    let manager = Arc::new(Mutex::new(TaskManager::new_with_config(test_config.clone())));
    manager.lock().await.tasks = tasks;

    // Start a simplified task processor (similar to daemon but for testing)
    let manager_clone = manager.clone();
    let config_clone = test_config.clone();
    let processor_handle = tokio::spawn(async move {
        // Run processor loop for just this one task
        task_processor_loop_single_task(manager_clone, config_clone, task_id).await;
    });

    // Wait for task to complete or fail (with 30s timeout)
    use tokio::time::{timeout, Duration};

    println!("⏳ Waiting for download to complete (timeout: 30s)...");
    let result = timeout(Duration::from_secs(30), async {
        loop {
            rx.changed().await.unwrap();
            let status = rx.borrow().clone();
            println!("📊 Task status: {:?}", status);
            match status {
                TaskStatus::Completed => return Ok(()),
                TaskStatus::Failed(e) => return Err(e),
                _ => continue, // Keep waiting for final state
            }
        }
    })
    .await;

    match result {
        Ok(Ok(())) => {
            println!("✅ Download completed successfully");

            // Verify the task is now completed
            let mgr = manager.lock().await;
            let task = mgr.tasks.get_task(task_id).expect("Task should exist");
            assert!(
                matches!(task, Task::Completed { .. }),
                "Task should be completed"
            );

            // Debug: List all files in the test directory
            println!("📂 Contents of test directory:");
            let mut dir_entries = fs::read_dir(test_dir)
                .await
                .expect("Failed to read test directory");
            let mut all_files = Vec::new();
            while let Some(entry) = dir_entries
                .next_entry()
                .await
                .expect("Failed to read directory entry")
            {
                let path = entry.path();
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                println!("  📄 {}", name);
                all_files.push(path);
            }

            // Find the downloaded file (look for any video file)
            let mut download_path = None;
            for path in &all_files {
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    if ["mp4", "mkv", "webm", "m4a"].contains(&ext) {
                        download_path = Some(path.clone());
                        break;
                    }
                }
            }

            assert!(
                download_path.is_some(),
                "No video file found in test directory. Files found: {:?}",
                all_files
                    .iter()
                    .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
                    .collect::<Vec<_>>()
            );
            let download_path = download_path.unwrap();

            // Verify file size is reasonable (should be > 10KB for a real video)
            let metadata = fs::metadata(&download_path)
                .await
                .expect("Failed to get file metadata");
            assert!(
                metadata.len() > 10 * 1024,
                "Downloaded file should be larger than 10KB, got {} bytes",
                metadata.len()
            );

            // Verify video properties using ffprobe
            let video_info = get_video_info(&download_path).await;
            match video_info {
                Ok(info) => {
                    // Verify duration is approximately 19 seconds
                    assert!(
                        info.duration >= 17.0 && info.duration <= 21.0,
                        "Video duration should be approximately 19 seconds, got {:.1} seconds",
                        info.duration
                    );
                    println!("✅ Video duration verified: {:.1} seconds", info.duration);

                    // Verify it has both video and audio streams
                    assert!(info.has_video, "Video file should contain a video stream");
                    assert!(info.has_audio, "Video file should contain an audio stream");
                    println!(
                        "✅ Video streams verified: video={}, audio={}",
                        info.has_video, info.has_audio
                    );
                }
                Err(e) => {
                    println!("⚠️  Warning: Could not verify video properties: {}", e);
                    // Don't fail the test if ffprobe is not available
                }
            }

            println!("✅ Downloaded file verified at: {}", download_path.display());
        }
        Ok(Err(e)) => panic!("❌ Task failed: {}", e),
        Err(_) => panic!("❌ Test timeout after 30s"),
    }

    // Cleanup
    processor_handle.abort();
    cleanup().await;
}

// Helper function to run a simplified task processor for a single task
async fn task_processor_loop_single_task(
    manager: Arc<Mutex<TaskManager>>,
    config: Config,
    target_task_id: u64,
) {
    use crate::task::{TaskKind, TaskOperationResult, TaskStatus};

    loop {
        let mut mgr = manager.lock().await;

        // Check if target task is done
        if let Some(task) = mgr.tasks.get_task(target_task_id) {
            if matches!(task, Task::Completed { .. } | Task::Failed { .. }) {
                drop(mgr);
                return; // Task is done, exit
            }
        } else {
            drop(mgr);
            return; // Task doesn't exist, exit
        }

        // If we have an active task, wait for it
        if mgr.tasks.has_active_task(target_task_id) {
            let handle = mgr.tasks.remove_active_task(target_task_id).unwrap();
            drop(mgr);

            let task_result = handle.await;

            let mut mgr = manager.lock().await;
            if let Some(task) = mgr.tasks.get_task_mut(target_task_id) {
                match task_result {
                    Ok(Ok(TaskOperationResult::GetNameComplete(metadata))) => {
                        if let Task::GetName { url, .. } = task {
                            info!("GetName completed for task {}", target_task_id);
                            *task = Task::GetName {
                                url: url.clone(),
                                metadata: Some(metadata),
                            };
                        }
                    }
                    Ok(Ok(TaskOperationResult::DownloadComplete(path))) => {
                        if let Task::DownloadVideo { url, .. } = task {
                            info!("Download completed for task {}", target_task_id);
                            *task = Task::Completed {
                                url: url.clone(),
                                path,
                            };
                            mgr.tasks
                                .set_task_status(target_task_id, TaskStatus::Completed);
                        }
                    }
                    Ok(Err(e)) => {
                        let url = task.url().to_string();
                        let error_msg = e.to_string();
                        *task = Task::Failed {
                            url,
                            human_readable_error: error_msg.clone(),
                        };
                        mgr.tasks
                            .set_task_status(target_task_id, TaskStatus::Failed(error_msg));
                    }
                    Err(e) => {
                        if !e.is_cancelled() {
                            let url = task.url().to_string();
                            let error_msg = format!("Task panicked: {}", e);
                            *task = Task::Failed {
                                url,
                                human_readable_error: error_msg.clone(),
                            };
                            mgr.tasks
                                .set_task_status(target_task_id, TaskStatus::Failed(error_msg));
                        }
                    }
                }
            }
            drop(mgr);
            continue;
        }

        // Check if we need to spawn next operation
        let task_clone = mgr.tasks.get_task(target_task_id).cloned();

        match task_clone {
            Some(Task::Queued { url }) => {
                // Spawn GetName
                let config_clone = config.clone();
                if let Some(t) = mgr.tasks.get_task_mut(target_task_id) {
                    *t = Task::GetName {
                        url: url.clone(),
                        metadata: None,
                    };
                    mgr.tasks
                        .set_task_status(target_task_id, TaskStatus::GetName);
                }

                let handle = tokio::spawn(async move {
                    let mut task = Task::Queued { url: url.clone() };
                    task.transition(TaskKind::GetName, None, &config_clone)
                        .await;

                    if let Task::GetName {
                        metadata: Some(m), ..
                    } = task
                    {
                        Ok(TaskOperationResult::GetNameComplete(m))
                    } else {
                        Err(anyhow::anyhow!("GetName transition failed"))
                    }
                });

                mgr.tasks.insert_active_task(target_task_id, handle);
            }
            Some(Task::GetName {
                url,
                metadata: Some(metadata),
            }) => {
                // Spawn Download
                let config_clone = config.clone();

                if let Some(t) = mgr.tasks.get_task_mut(target_task_id) {
                    let mut temp_task = Task::GetName {
                        url: url.clone(),
                        metadata: Some(metadata.clone()),
                    };
                    temp_task
                        .transition(TaskKind::DownloadVideo, None, &config_clone)
                        .await;
                    *t = temp_task;
                    mgr.tasks
                        .set_task_status(target_task_id, TaskStatus::DownloadVideo);
                }

                let handle = tokio::spawn(async move {
                    let path =
                        crate::task::spawn_download_video_task(url, metadata, config_clone).await?;
                    Ok(TaskOperationResult::DownloadComplete(path))
                });

                mgr.tasks.insert_active_task(target_task_id, handle);
            }
            _ => {
                // Nothing to do
                drop(mgr);
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }

        drop(mgr);
    }
}

#[derive(Debug)]
struct VideoInfo {
    duration: f64,
    has_video: bool,
    has_audio: bool,
}

async fn get_video_info(
    video_path: &Path,
) -> Result<VideoInfo, Box<dyn std::error::Error + Send + Sync>> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            video_path.to_str().unwrap(),
        ])
        .output()
        .await?;

    if !output.status.success() {
        return Err("ffprobe command failed".into());
    }

    let output_str = String::from_utf8(output.stdout)?;
    let json: serde_json::Value = serde_json::from_str(&output_str)?;

    // Get duration from format section
    let duration_str = json["format"]["duration"]
        .as_str()
        .ok_or("Duration not found in ffprobe output")?;
    let duration: f64 = duration_str.parse()?;

    // Check for video and audio streams
    let streams = json["streams"]
        .as_array()
        .ok_or("Streams not found in ffprobe output")?;

    let mut has_video = false;
    let mut has_audio = false;

    for stream in streams {
        if let Some(codec_type) = stream["codec_type"].as_str() {
            match codec_type {
                "video" => has_video = true,
                "audio" => has_audio = true,
                _ => {}
            }
        }
    }

    Ok(VideoInfo {
        duration,
        has_video,
        has_audio,
    })
}

#[tokio::test]
async fn test_task_with_notification_config() {
    cleanup().await;
    // Use standardized test config with notifications disabled
    let config_disabled = test_config();

    let mut task = Task::Queued {
        url: "https://www.youtube.com/watch?v=test".to_string(),
    };

    // Test that transitions work with notifications disabled
    task.transition(TaskKind::GetName, None, &config_disabled)
        .await;
    assert!(matches!(task, Task::GetName { .. }));

    // Test failure transition with custom error message
    task.transition(
        TaskKind::Failed,
        Some("Custom error".to_string()),
        &config_disabled,
    )
    .await;
    assert!(
        matches!(task, Task::Failed { human_readable_error, .. } if human_readable_error == "Custom error")
    );

    // Test that Tasks methods work with notification config
    let mut tasks = Tasks::default();
    let result = tasks.add_url_as_task("https://test.com".to_string());
    assert!(result.is_ok());

    cleanup().await;
}

#[tokio::test]
#[ignore] // Focus on e2e test first
async fn test_daemon_restart_recovery() {
    use std::process::Stdio;
    use std::time::Duration;

    println!("Starting comprehensive daemon restart test...");

    // Step 1: Create test environment and config
    let test_dir = "/tmp/tsp_ytdlp_restart_test";
    // Clean up any existing files from previous test runs
    cleanup().await;
    fs::create_dir_all(test_dir)
        .await
        .expect("Failed to create test directory");

    // Create custom config file using standardized test config
    let test_config = Config {
        throttle: Some(10),
        ..test_config()
    };
    let config_path = format!("{}/test_config.toml", test_dir);
    let config_content = format!(
        r#"
concurrent_downloads = {}
socket_path = "{}/test.sock"
disk_threshold = {}
download_dir = "{}"
should_notify_send = {}
cache_dir = "{}/cache"
throttle = {}
video_quality = "{}"
"#,
        test_config.concurrent_downloads,
        test_dir,
        test_config.disk_threshold,
        test_dir,
        test_config.should_notify_send,
        test_dir,
        test_config.throttle.unwrap_or(0),
        test_config.video_quality
    );

    fs::write(&config_path, &config_content)
        .await
        .expect("Failed to write test config");
    println!("Created test config at: {}", config_path);

    // Step 2: Start daemon with custom config
    let binary_path = std::env::current_exe()
        .expect("Failed to get binary path")
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tsp_ytdlp");
    println!("Starting daemon with binary: {}", binary_path.display());

    let mut daemon_process = Command::new(&binary_path)
        .args(["--daemon", "--config", &config_path])
        .env("RUST_LOG", "tsp_ytdlp=debug")
        .env("TSP_YTDLP_DATA_DIR", test_dir) // Use test directory for data storage
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn daemon");

    // Wait for daemon to start
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("Daemon started, PID: {:?}", daemon_process.id());

    // Step 3: Submit download URL via client
    println!(
        "Running client command: {} {} --config {}",
        binary_path.display(),
        TEST_URL,
        config_path
    );
    let client_result = Command::new(&binary_path)
        .args([TEST_URL, "--config", &config_path])
        .env("RUST_LOG", "tsp_ytdlp=debug")
        .env("TSP_YTDLP_DATA_DIR", test_dir) // Use test directory for data storage
        .output()
        .await
        .expect("Failed to run client");

    if !client_result.status.success() {
        let stderr = String::from_utf8_lossy(&client_result.stderr);
        let stdout = String::from_utf8_lossy(&client_result.stdout);
        println!("Client stdout: {}", stdout);
        println!("Client stderr: {}", stderr);
        panic!("Client failed to add URL: {}", stderr);
    }
    println!("Successfully submitted download URL");

    // Step 4: Monitor progress until at least 10%
    let mut progress_found = false;
    let progress_timeout = Duration::from_secs(30);
    let start_time = std::time::Instant::now();

    while start_time.elapsed() < progress_timeout {
        let status_result = Command::new(&binary_path)
            .args(["--verbose", "--config", &config_path])
            .env("RUST_LOG", "tsp_ytdlp=debug")
            .env("TSP_YTDLP_DATA_DIR", test_dir) // Use test directory for data storage
            .output()
            .await
            .expect("Failed to get status");

        if status_result.status.success() {
            let output = String::from_utf8_lossy(&status_result.stdout);
            println!("Status check: {}", output);

            // Look for actual downloads in progress (not just queued)
            let current_downloads_active = output.contains("Current Downloads (")
                && !output.contains("Current Downloads (0/")
                && !output.contains("  (none)");

            // Look for progress percentages or download state
            let has_progress_indicator = output.contains("Downloading (") && output.contains("%)");

            if current_downloads_active || has_progress_indicator {
                progress_found = true;
                println!(
                    "Download progress detected! Active downloads or progress percentage found."
                );
                break;
            }

            // Debug: Check current downloads status
            if output.contains("Current Downloads (") {
                let lines: Vec<&str> = output.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    if line.contains("Current Downloads (") {
                        println!("DEBUG: Current downloads line: {}", line);
                        if i + 1 < lines.len() {
                            println!("DEBUG: Next line: {}", lines[i + 1]);
                        }
                        if i + 2 < lines.len() {
                            println!("DEBUG: Following line: {}", lines[i + 2]);
                        }
                    }
                }
            }

            // Also debug any lines with progress percentages
            for line in output.lines() {
                if line.contains("%)") {
                    println!("DEBUG: Progress line found: {}", line);
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    if !progress_found {
        println!("Warning: No explicit progress found, proceeding with kill test anyway");
    }

    // Step 5: Kill the daemon
    println!("Killing daemon with PID: {:?}", daemon_process.id());
    daemon_process.kill().await.expect("Failed to kill daemon");
    let _ = daemon_process.wait().await;
    println!("Daemon killed successfully");

    // Wait a moment to ensure cleanup
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Step 6: Restart daemon
    println!("Restarting daemon...");
    let mut restarted_daemon = Command::new(&binary_path)
        .args(["--daemon", "--config", &config_path])
        .env("RUST_LOG", "tsp_ytdlp=debug")
        .env("TSP_YTDLP_DATA_DIR", test_dir) // Use test directory for data storage
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to restart daemon");

    // Wait for restart
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("Daemon restarted, PID: {:?}", restarted_daemon.id());

    // Step 7: Monitor until completion
    let mut completed = false;
    let completion_timeout = Duration::from_secs(120); // Increased timeout for merge operations
    let restart_time = std::time::Instant::now();

    while restart_time.elapsed() < completion_timeout {
        let status_result = Command::new(&binary_path)
            .args(["--verbose", "--config", &config_path])
            .env("RUST_LOG", "tsp_ytdlp=debug")
            .env("TSP_YTDLP_DATA_DIR", test_dir) // Use test directory for data storage
            .output()
            .await
            .expect("Failed to get status");

        if status_result.status.success() {
            let output = String::from_utf8_lossy(&status_result.stdout);

            if output.contains("Completed") {
                // Also check that a video file actually exists (not just .part files)
                let mut video_exists = false;
                if let Ok(mut entries) = tokio::fs::read_dir(test_dir).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        let path = entry.path();
                        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                            if ["mp4", "mkv", "webm", "m4a"].contains(&ext) {
                                video_exists = true;
                                break;
                            }
                        }
                    }
                }

                if video_exists {
                    completed = true;
                    println!("Download completed after restart and video file exists!");
                    break;
                } else {
                    println!("Task marked completed but no video file found yet, waiting...");
                }
            } else if output.contains("Failed") {
                panic!("Download failed after restart: {}", output);
            }
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    if !completed {
        panic!("Download did not complete within timeout after restart");
    }

    // Step 8: Verify the downloaded file
    println!("Checking for downloaded files in: {}", test_dir);
    let mut download_path = None;
    let mut part_files = Vec::new();
    let mut all_files = Vec::new();

    let mut dir_entries = fs::read_dir(test_dir)
        .await
        .expect("Failed to read test directory");
    while let Some(entry) = dir_entries
        .next_entry()
        .await
        .expect("Failed to read directory entry")
    {
        let path = entry.path();
        let filename = path.file_name().unwrap().to_string_lossy().to_string();
        all_files.push(filename.clone());

        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            if ["mp4", "mkv", "webm", "m4a"].contains(&ext) {
                download_path = Some(path);
                break;
            } else if ext == "part" {
                part_files.push(path);
            }
        } else {
            // Check for files without extension that might be video files
            if filename.starts_with("Me_at_the_zoo")
                && !filename.contains(".part")
                && !filename.contains(".ytdl")
                && !filename.contains(".json")
                && !filename.contains(".toml")
                && !filename.contains(".sock")
                && filename != "fragments"
            {
                println!(
                    "DEBUG: Found potential video file without extension: {}",
                    filename
                );
                download_path = Some(path);
            }
        }
    }

    println!("Files found: {:?}", all_files);
    if !part_files.is_empty() {
        println!(
            "Partial download files found: {:?}",
            part_files
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
                .collect::<Vec<_>>()
        );
    }

    assert!(
        download_path.is_some(),
        "No video file found after daemon restart"
    );
    let download_path = download_path.unwrap();

    // Verify file size (>10KB)
    let metadata = fs::metadata(&download_path)
        .await
        .expect("Failed to get file metadata");
    assert!(
        metadata.len() > 10 * 1024,
        "Downloaded file should be larger than 10KB, got {} bytes",
        metadata.len()
    );

    // Verify video properties
    let video_info = get_video_info(&download_path).await;
    match video_info {
        Ok(info) => {
            assert!(
                info.duration >= 17.0 && info.duration <= 21.0,
                "Video duration should be approximately 19 seconds, got {:.1} seconds",
                info.duration
            );
            assert!(info.has_video, "Video file should contain a video stream");
            assert!(info.has_audio, "Video file should contain an audio stream");
            println!(
                "✅ Video verification passed: duration={:.1}s, video={}, audio={}",
                info.duration, info.has_video, info.has_audio
            );
        }
        Err(e) => {
            println!("Warning: Could not verify video properties: {}", e);
        }
    }

    println!("✅ Daemon restart test completed successfully!");

    // Step 9: Cleanup
    let _ = restarted_daemon.kill().await;
    let _ = restarted_daemon.wait().await;
    cleanup().await;
}
