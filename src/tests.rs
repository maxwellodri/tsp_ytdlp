use crate::Config;
use crate::task::{GetNameMetadata, MediaType, Task, TaskKind, Tasks};
use std::num::NonZeroU64;
use tokio::fs;

fn test_config() -> Config {
    Config {
        concurrent_downloads: NonZeroU64::new(1),
        socket_path: "/tmp/tsp_ytdlp/test.sock".to_string(),
        disk_threshold: 1024,
        download_dir: "/tmp/tsp_ytdlp".to_string(),
        should_notify_send: false,
        cache_dir: "/tmp/tsp_ytdlp/cache".to_string(),
        throttle: Some(100),
        video_quality: "720p".to_string(),
        video_dir_script: None,
        sponsorblock_mark: Some("all".to_string()),
        sponsorblock_remove: Some("sponsor,interaction".to_string()),
        cookies_file: None,
    }
}

async fn cleanup() {
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

    let url = "https://www.youtube.com/watch?v=test123".to_string();
    let task_id = tasks
        .add_url_as_task(url.clone(), MediaType::Video, None)
        .expect("Failed to add task");

    assert_eq!(tasks.len(), 1);
    assert_eq!(task_id, 0);

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
        url: "https://www.youtube.com/watch?v=jNQXAC9IVRw".to_string(),
        media_type: MediaType::Video,
        download_dir: None,
    };

    task.transition(TaskKind::GetName, None, &test_config).await;
    assert!(matches!(task, Task::GetName { .. }));

    if let Task::GetName {
        ref mut metadata, ..
    } = task
    {
        *metadata = Some(GetNameMetadata {
            title: Some("Test Video".to_string()),
            expected_size_bytes: Some(1024 * 1024),
            directory: "/tmp/test".to_string(),
        });
    }

    task.transition(TaskKind::DownloadVideo, None, &test_config)
        .await;
    assert!(matches!(task, Task::DownloadVideo { .. }));

    task.transition(TaskKind::Completed, None, &test_config)
        .await;
    assert!(matches!(task, Task::Completed { .. }));

    cleanup().await;
}

#[tokio::test]
async fn test_task_serialization() {
    cleanup().await;
    let mut tasks = Tasks::default();

    let url1 = "https://www.youtube.com/watch?v=test1".to_string();
    let url2 = "https://www.youtube.com/watch?v=test2".to_string();

    tasks
        .add_url_as_task(url1.clone(), MediaType::Video, None)
        .unwrap();
    tasks
        .add_url_as_task(url2.clone(), MediaType::Video, None)
        .unwrap();

    let temp_path = std::path::PathBuf::from("/tmp/test_tasks.json");
    tasks
        .save_to_file(&temp_path)
        .expect("Failed to save tasks");

    let loaded_tasks = Tasks::load_from_file(&temp_path).expect("Failed to load tasks");

    assert_eq!(loaded_tasks.len(), 2);

    let urls: Vec<String> = loaded_tasks
        .iter()
        .map(|(_, task)| task.url().to_string())
        .collect();
    assert!(urls.contains(&url1));
    assert!(urls.contains(&url2));

    let _ = std::fs::remove_file(&temp_path);

    cleanup().await;
}

#[tokio::test]
async fn test_duplicate_url_prevention() {
    cleanup().await;
    let mut tasks = Tasks::default();

    let url = "https://www.youtube.com/watch?v=test123".to_string();

    assert!(
        tasks
            .add_url_as_task(url.clone(), MediaType::Video, None)
            .is_ok()
    );
    assert!(tasks.add_url_as_task(url, MediaType::Video, None).is_err());

    assert_eq!(tasks.len(), 1);

    cleanup().await;
}

#[tokio::test]
async fn test_enhanced_ytdlp_options() {
    cleanup().await;

    let task = Task::Queued {
        url: "https://www.youtube.com/watch?v=test".to_string(),
        media_type: MediaType::Video,
        download_dir: None,
    };

    assert!(matches!(task, Task::Queued { .. }));
    assert!(!task.is_active());
    assert_eq!(task.url(), "https://www.youtube.com/watch?v=test");

    cleanup().await;
}

#[tokio::test]
async fn test_notification_config() {
    cleanup().await;
    use crate::common::{send_critical_notification, send_notification};

    let config_disabled = test_config();
    println!(
        "Test config should_notify_send: {}",
        config_disabled.should_notify_send
    );

    send_notification(
        "https://test.com",
        "DEBUG: This notification should NOT appear during tests",
        Some(1000),
        &config_disabled,
    )
    .await;
    send_critical_notification(
        "https://test.com",
        "DEBUG: Critical notification should NOT appear during tests",
        &config_disabled,
    )
    .await;

    let default_config = Config::default();
    assert_eq!(default_config.should_notify_send, true);
    println!(
        "Default config should_notify_send: {}",
        default_config.should_notify_send
    );

    cleanup().await;
}

#[tokio::test]
async fn test_task_with_notification_config() {
    cleanup().await;
    let config_disabled = test_config();

    let mut task = Task::Queued {
        url: "https://www.youtube.com/watch?v=jNQXAC9IVRw".to_string(),
        media_type: MediaType::Video,
        download_dir: None,
    };

    task.transition(TaskKind::GetName, None, &config_disabled)
        .await;
    assert!(matches!(task, Task::GetName { .. }));

    task.transition(
        TaskKind::Failed,
        Some("Custom error".to_string()),
        &config_disabled,
    )
    .await;
    assert!(
        matches!(task, Task::Failed { human_readable_error, .. } if human_readable_error == "Custom error")
    );

    let mut tasks = Tasks::default();
    let result = tasks.add_url_as_task("https://test.com".to_string(), MediaType::Video, None);
    assert!(result.is_ok());

    cleanup().await;
}
