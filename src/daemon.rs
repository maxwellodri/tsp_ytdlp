use crate::common::{send_notification, APP};
use crate::task::TaskKind;
use crate::{get_data_dir, ClientRequest, Config, ServerResponse, TaskManager};
use anyhow::Result;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use tracing::{error, info, warn};

pub async fn run_daemon(config: Config) -> Result<()> {
    info!("Starting daemon with config: {:?}", config);

    // Load or create TaskManager
    let manager = Arc::new(Mutex::new(TaskManager::load_from_disk_with_config(None)?));

    // Process queued tasks from offline queueing
    {
        let mut mgr = manager.lock().await;
        if let Ok(queued) = crate::load_queued_tasks()
            && !queued.urls.is_empty() {
                info!(
                    "Processing {} queued tasks from offline queue",
                    queued.urls.len()
                );

                for url in queued.urls {
                    // Check for duplicates
                    if mgr.check_duplicate_url(&url).is_none() {
                        match mgr.add_task(url.clone()) {
                            Ok(task_id) => {
                                info!("Added queued task {} for URL: {}", task_id, url);
                            }
                            Err(e) => {
                                warn!("Failed to add queued task for URL {}: {}", url, e);
                            }
                        }
                    } else {
                        info!("Skipping duplicate queued URL: {}", url);
                    }
                }

                // Save tasks after processing queue
                if let Err(e) = mgr.save_tasks() {
                    error!("Failed to save tasks after processing queue: {}", e);
                }

                // Delete queued_tasks.json file
                let data_dir = crate::get_data_dir();
                let queued_path = data_dir.join("queued_tasks.json");
                if let Err(e) = std::fs::remove_file(&queued_path) {
                    warn!("Failed to delete queued_tasks.json: {}", e);
                } else {
                    info!("Cleared queued_tasks.json");
                }
            }
    }

    // Set up Unix socket
    let socket_path = std::path::Path::new(&config.socket_path);

    // Remove old socket file if it exists
    if socket_path.exists() {
        info!("Removing old socket file at: {:?}", socket_path);
        std::fs::remove_file(socket_path)?;
    }

    // Ensure parent directory exists
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let listener = UnixListener::bind(socket_path)?;
    info!("Daemon listening on socket: {:?}", socket_path);

    // Store socket path for cleanup later
    let socket_path_owned = socket_path.to_path_buf();

    // Clone for the task processor
    let manager_clone = manager.clone();
    let config_clone = config.clone();

    // Spawn task processor loop
    let processor_handle = tokio::spawn(async move {
        task_processor_loop(manager_clone, config_clone).await;
    });

    // Clone for the socket listener
    let manager_clone2 = manager.clone();
    let config_clone2 = config.clone();

    // Spawn socket listener loop
    let listener_handle = tokio::spawn(async move {
        socket_listener_loop(manager_clone2, config_clone2, listener).await;
    });

    // Wait for either task to complete (they shouldn't unless there's an error)
    tokio::select! {
        _ = processor_handle => {
            warn!("Task processor loop exited unexpectedly");
        }
        _ = listener_handle => {
            warn!("Socket listener loop exited unexpectedly");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, shutting down gracefully");
        }
    }

    // Save tasks before exit
    {
        let mgr = manager.lock().await;
        if let Err(e) = mgr.save_tasks() {
            error!("Failed to save tasks on shutdown: {}", e);
        }
    }

    // Send shutdown notification
    send_notification(APP, "Daemon is dead 💀🥺", Some(1500), &config).await;

    // Clean up socket
    let _ = std::fs::remove_file(&socket_path_owned);

    info!("Daemon shut down successfully");
    Ok(())
}

/// Socket listener loop - handles client requests
async fn socket_listener_loop(
    manager: Arc<Mutex<TaskManager>>,
    config: Config,
    listener: UnixListener,
) {
    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let manager_clone = manager.clone();
                let config_clone = config.clone();

                // Spawn a task to handle this client
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, manager_clone, config_clone).await {
                        error!("Error handling client: {}", e);
                    }
                });
            }
            Err(e) => {
                error!("Error accepting connection: {}", e);
            }
        }
    }
}

/// Handle a single client connection
async fn handle_client(
    mut stream: UnixStream,
    manager: Arc<Mutex<TaskManager>>,
    config: Config,
) -> Result<()> {
    // Read request from client
    let mut buffer = vec![0u8; 8192];
    let n = stream.read(&mut buffer).await?;

    if n == 0 {
        return Ok(()); // Connection closed
    }

    let request: ClientRequest = serde_json::from_slice(&buffer[..n])?;
    info!("Received request: {:?}", request);

    // Process request
    let response = process_request(request, manager, &config).await;

    // Send response
    let response_json = serde_json::to_vec(&response)?;
    stream.write_all(&response_json).await?;
    stream.flush().await?;

    Ok(())
}

/// Process a client request and return a response
async fn process_request(
    request: ClientRequest,
    manager: Arc<Mutex<TaskManager>>,
    config: &Config,
) -> ServerResponse {
    match request {
        ClientRequest::Add { url } => {
            let mut mgr = manager.lock().await;

            // Check for duplicate and handle failed task re-add
            match mgr.find_task_by_url(&url) {
                crate::TaskByUrl::Exists {
                    id: existing_id,
                    kind: crate::task::TaskKind::Failed,
                } => {
                    // Failed task - remove it, cleanup cache, and re-add
                    info!("Re-adding failed task {} for URL: {}", existing_id, url);

                    // Remove the failed task
                    mgr.remove_task(existing_id);

                    // Release lock before async operations
                    let url_clone = url.clone();
                    let config_clone = config.clone();
                    drop(mgr);

                    // Cleanup cache
                    crate::cleanup_cache_for_url(&url_clone, &config_clone).await;

                    // Re-acquire lock and add new task
                    let mut mgr = manager.lock().await;
                    match mgr.add_task(url_clone.clone()) {
                        Ok(task_id) => {
                            // Save tasks
                            if let Err(e) = mgr.save_tasks() {
                                error!("Failed to save tasks: {}", e);
                            }

                            // Send notification about retry
                            drop(mgr);
                            crate::common::send_notification(
                                &url_clone,
                                &format!("🔄 Retrying failed download (new task ID: {})", task_id),
                                Some(3000),
                                &config_clone,
                            )
                            .await;

                            ServerResponse::Success {
                                message: format!(
                                    "Removed failed task {} and added new task {} for URL: {}",
                                    existing_id, task_id, url_clone
                                ),
                            }
                        }
                        Err(e) => {
                            ServerResponse::Error {
                                message: format!("Failed to re-add task: {}", e),
                            }
                        }
                    }
                }
                crate::TaskByUrl::Exists {
                    id: existing_id,
                    kind,
                } => {
                    // Not a failed task - reject duplicate
                    let url_clone = url.clone();
                    let config_clone = config.clone();
                    drop(mgr);

                    info!("URL already queued/finished: {url_clone}");
                    crate::common::send_notification(
                        &url_clone,
                        &format!("URL already queued/finished: {} 🕺", url_clone),
                        Some(1200),
                        &config_clone,
                    )
                    .await;

                    ServerResponse::Error {
                        message: format!(
                            "URL already exists with task ID {} (state: {:?})",
                            existing_id, kind
                        ),
                    }
                }
                crate::TaskByUrl::DoesntExist => {
                    // No duplicate - add new task
                    match mgr.add_task(url.clone()) {
                        Ok(task_id) => {
                            // Save tasks
                            if let Err(e) = mgr.save_tasks() {
                                error!("Failed to save tasks: {}", e);
                            }

                            // Send notification about new task
                            let url_clone = url.clone();
                            let config_clone = config.clone();
                            drop(mgr);

                            crate::common::send_notification(
                                &url_clone,
                                &format!("Task {} added for URL: {}", task_id, url_clone),
                                Some(3000),
                                &config_clone,
                            )
                            .await;

                            ServerResponse::Success {
                                message: format!("Task {} added for URL: {}", task_id, url_clone),
                            }
                        }
                        Err(e) => ServerResponse::Error {
                            message: format!("Failed to add task: {}", e),
                        },
                    }
                }
            }
        }

        ClientRequest::Remove { id } => {
            let mut mgr = manager.lock().await;

            // Get task URL before removing
            let task_url = mgr.tasks.get_task(id).map(|task| task.url().to_string());

            if mgr.remove_task(id) {
                // Save tasks
                if let Err(e) = mgr.save_tasks() {
                    error!("Failed to save tasks: {}", e);
                }

                // Cleanup cache if we have the URL
                if let Some(url) = task_url {
                    let config_clone = config.clone();
                    drop(mgr);
                    crate::cleanup_cache_for_url(&url, &config_clone).await;

                    ServerResponse::Success {
                        message: format!("Task {} removed and cache cleaned", id),
                    }
                } else {
                    ServerResponse::Success {
                        message: format!("Task {} removed", id),
                    }
                }
            } else {
                ServerResponse::Error {
                    message: format!("Task {} not found", id),
                }
            }
        }

        ClientRequest::Clear => {
            let mut mgr = manager.lock().await;
            mgr.clear_completed();

            // Save tasks
            if let Err(e) = mgr.save_tasks() {
                error!("Failed to save tasks: {}", e);
            }

            ServerResponse::Success {
                message: "Completed tasks cleared".to_string(),
            }
        }

        ClientRequest::Status { verbose } => {
            let mgr = manager.lock().await;
            mgr.get_status(verbose)
        }

        ClientRequest::Info { id } => {
            let mgr = manager.lock().await;

            // Check if task exists
            if mgr.tasks.get_task(id).is_some() {
                // Return log file path
                let log_dir = get_data_dir();
                let log_file_path = log_dir.join(format!("task_{}.log", id));

                ServerResponse::Info {
                    log_file_path: log_file_path.to_string_lossy().to_string(),
                }
            } else {
                ServerResponse::Error {
                    message: format!("Task {} not found", id),
                }
            }
        }

        ClientRequest::Kill { sender_pid: _pid } => {
            info!("Received kill request, initiating shutdown");

            // Save tasks before shutdown
            {
                let mgr = manager.lock().await;
                if let Err(e) = mgr.save_tasks() {
                    error!("Failed to save tasks during kill: {}", e);
                }
            }

            // Send shutdown notification
            send_notification(APP, "Daemon is dead 💀🥺", Some(1500), &config).await;

            // Exit the process
            std::process::exit(0);
        }
    }
}

/// Task processor loop - executes state transitions for queued tasks
async fn task_processor_loop(manager: Arc<Mutex<TaskManager>>, config: Config) {
    use crate::task::TaskOperationResult;
    use futures::future::select_all;

    info!("Task processor loop started");

    loop {
        // Simplified approach: Extract handles, wait for any to complete, put rest back
        // We accept that we can't easily timeout without losing handles
        let mut mgr = manager.lock().await;

        if mgr.tasks.active_task_count() > 0 {
            // Extract all task IDs and handles
            let entries: Vec<(u64, _)> = mgr.tasks.drain_active_tasks().collect();
            let task_ids: Vec<u64> = entries.iter().map(|(id, _)| *id).collect();
            let handles: Vec<_> = entries.into_iter().map(|(_, h)| h).collect();

            drop(mgr); // Release lock while waiting

            // Wait for ANY task to complete
            let (task_result, index, remaining) = select_all(handles).await;
            let completed_task_id = task_ids[index];

            info!("Task {} operation completed", completed_task_id);

            // Put remaining handles back into HashMap
            let mut mgr = manager.lock().await;
            let remaining_ids: Vec<u64> = task_ids
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != index)
                .map(|(_, id)| *id)
                .collect();
            for (task_id, handle) in remaining_ids.into_iter().zip(remaining) {
                mgr.tasks.insert_active_task(task_id, handle);
            }

            // Update task state based on result
            if let Some(task) = mgr.tasks.get_task_mut(completed_task_id) {
                match task_result {
                    Ok(Ok(TaskOperationResult::GetNameComplete(metadata))) => {
                        // GetName completed successfully - update task with metadata
                        if let crate::task::Task::GetName { url, .. } = task {
                            info!("GetName completed for task {}", completed_task_id);
                            *task = crate::task::Task::GetName {
                                url: url.clone(),
                                metadata: Some(metadata),
                            };
                        }
                    }
                    Ok(Ok(TaskOperationResult::DownloadComplete(path))) => {
                        // Download completed successfully
                        if let crate::task::Task::DownloadVideo { url, metadata, .. } = task {
                            info!("Download completed for task {}", completed_task_id);

                            // Send completion notification with filename
                            let filename_display = path
                                .file_name()
                                .and_then(|f| f.to_str())
                                .unwrap_or(metadata.title.as_deref().unwrap_or("video"));
                            crate::common::send_notification(
                                url,
                                &format!("Download completed: {} 🤗✅", filename_display),
                                Some(5000),
                                &config,
                            )
                            .await;

                            // Touch the file to update timestamp
                            if let Err(e) = crate::task::touch_file(&path).await {
                                error!("Failed to touch completed file {}: {}", path.display(), e);
                            }

                            *task = crate::task::Task::Completed {
                                url: url.clone(),
                                path,
                            };
                            mgr.tasks.set_task_status(
                                completed_task_id,
                                crate::task::TaskStatus::Completed,
                            );
                        }
                    }
                    Ok(Err(e)) => {
                        // Task failed
                        let url = task.url().to_string();
                        error!("Task {} failed: {}", completed_task_id, e);
                        crate::common::send_critical_notification(
                            &url,
                            &format!("❌ Download failed: {}", e),
                            &config,
                        )
                        .await;
                        let error_msg = e.to_string();
                        *task = crate::task::Task::Failed {
                            url,
                            human_readable_error: error_msg.clone(),
                        };
                        mgr.tasks.set_task_status(
                            completed_task_id,
                            crate::task::TaskStatus::Failed(error_msg),
                        );
                    }
                    Err(e) => {
                        // JoinHandle error (task panicked or aborted)
                        if e.is_cancelled() {
                            info!("Task {} was cancelled", completed_task_id);
                            // Task was already removed, no state update needed
                        } else {
                            let url = task.url().to_string();
                            error!("Task {} panicked: {}", completed_task_id, e);
                            let error_msg = format!("Task panicked: {}", e);
                            *task = crate::task::Task::Failed {
                                url,
                                human_readable_error: error_msg.clone(),
                            };
                            mgr.tasks.set_task_status(
                                completed_task_id,
                                crate::task::TaskStatus::Failed(error_msg),
                            );
                        }
                    }
                }
            }

            if let Err(e) = mgr.save_tasks() {
                error!("Failed to save tasks: {}", e);
            }
            drop(mgr);
        } else {
            // No active tasks, just sleep
            drop(mgr);
            sleep(Duration::from_secs(1)).await;
        }

        // Check if we can spawn more tasks
        let mut mgr = manager.lock().await;
        let concurrent_limit = if config.concurrent_downloads == 0 {
            usize::MAX
        } else {
            config.concurrent_downloads as usize
        };

        if mgr.tasks.active_task_count() >= concurrent_limit {
            drop(mgr);
            continue; // At capacity
        }

        // Find next task that needs processing
        let next_task = mgr.tasks.iter().find_map(|(id, task)| {
            if mgr.tasks.has_active_task(*id) {
                return None; // Already being processed
            }

            match task {
                crate::task::Task::Queued { .. } => Some((*id, task.clone())),
                crate::task::Task::GetName {
                    metadata: Some(_), ..
                } => Some((*id, task.clone())),
                crate::task::Task::DownloadVideo { .. } => Some((*id, task.clone())),
                _ => None,
            }
        });

        if let Some((task_id, task)) = next_task {
            info!("Spawning operation for task {}: {:?}", task_id, task);

            match &task {
                crate::task::Task::Queued { url } => {
                    // Spawn GetName operation
                    let url = url.clone();
                    let config_clone = config.clone();

                    // Transition to GetName state immediately
                    if let Some(t) = mgr.tasks.get_task_mut(task_id) {
                        *t = crate::task::Task::GetName {
                            url: url.clone(),
                            metadata: None,
                        };
                        mgr.tasks
                            .set_task_status(task_id, crate::task::TaskStatus::GetName);
                    }
                    if let Err(e) = mgr.save_tasks() {
                        error!("Failed to save tasks: {}", e);
                    }

                    let handle = tokio::spawn(async move {
                        // Execute GetName operation
                        let mut task = crate::task::Task::Queued { url: url.clone() };
                        task.transition(TaskKind::GetName, None, &config_clone)
                            .await;

                        // Extract metadata
                        if let crate::task::Task::GetName {
                            metadata: Some(m), ..
                        } = task
                        {
                            Ok(TaskOperationResult::GetNameComplete(m))
                        } else {
                            Err(anyhow::anyhow!("GetName transition failed"))
                        }
                    });

                    mgr.tasks.insert_active_task(task_id, handle);
                }
                crate::task::Task::GetName {
                    url,
                    metadata: Some(metadata),
                } => {
                    // Spawn DownloadVideo operation
                    let url = url.clone();
                    let metadata = metadata.clone();
                    let config_clone = config.clone();

                    // Transition to DownloadVideo state immediately
                    if let Some(t) = mgr.tasks.get_task_mut(task_id) {
                        let mut temp_task = crate::task::Task::GetName {
                            url: url.clone(),
                            metadata: Some(metadata.clone()),
                        };
                        temp_task
                            .transition(TaskKind::DownloadVideo, None, &config_clone)
                            .await;
                        *t = temp_task;
                        mgr.tasks
                            .set_task_status(task_id, crate::task::TaskStatus::DownloadVideo);
                    }
                    if let Err(e) = mgr.save_tasks() {
                        error!("Failed to save tasks: {}", e);
                    }

                    let handle = tokio::spawn(async move {
                        // Execute download
                        let path =
                            crate::task::spawn_download_video_task(url, metadata, config_clone)
                                .await?;
                        Ok(TaskOperationResult::DownloadComplete(path))
                    });

                    mgr.tasks.insert_active_task(task_id, handle);
                }
                crate::task::Task::DownloadVideo { url, metadata, .. } => {
                    // Resume interrupted download
                    info!("Resuming interrupted download for task {}", task_id);

                    let url = url.clone();
                    let download_metadata = metadata.clone();
                    let config_clone = config.clone();

                    // Send "Resuming download" notification
                    let title_display = download_metadata.title.as_deref().unwrap_or("video");
                    crate::common::send_notification(
                        &url,
                        &format!("Resuming download: {} 🔄", title_display),
                        Some(3000),
                        &config_clone,
                    )
                    .await;

                    // Convert DownloadMetadata to GetNameMetadata for spawn_download_video_task
                    let get_name_metadata = crate::task::GetNameMetadata {
                        title: download_metadata.title.clone(),
                        expected_size_bytes: download_metadata.expected_size_bytes,
                        directory: download_metadata.directory.clone(),
                    };

                    // Save tasks after sending notification
                    if let Err(e) = mgr.save_tasks() {
                        error!("Failed to save tasks: {}", e);
                    }

                    let handle = tokio::spawn(async move {
                        // Execute download (will resume from cache if fragments exist)
                        let path = crate::task::spawn_download_video_task(
                            url,
                            get_name_metadata,
                            config_clone,
                        )
                        .await?;
                        Ok(TaskOperationResult::DownloadComplete(path))
                    });

                    mgr.tasks.insert_active_task(task_id, handle);
                }
                _ => {}
            }
        }
        drop(mgr);
    }
}
