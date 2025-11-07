use crate::{common::{format_bytes, send_notification, APP}, ClientRequest, Config, ServerResponse};
use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// Send request to daemon and receive response
async fn send_request(request: ClientRequest, config: &Config) -> Result<ServerResponse> {
    let socket_path = &config.socket_path;

    // Connect to Unix socket
    let mut stream = UnixStream::connect(socket_path).await.map_err(|e| {
        match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                anyhow::anyhow!(
                    "Daemon is not running. Start it with: {} daemon",
                    std::env::args()
                        .next()
                        .unwrap_or_else(|| "tsp_ytdlp".to_string())
                )
            }
            _ => anyhow::anyhow!("Failed to connect to daemon: {}", e),
        }
    })?;

    // Serialize and send request
    let request_json = serde_json::to_vec(&request)?;
    stream.write_all(&request_json).await?;
    stream.flush().await?;

    // Shutdown write side to signal we're done sending
    stream.shutdown().await?;

    // Read response
    let mut buffer = Vec::new();
    stream.read_to_end(&mut buffer).await?;

    // Deserialize response
    let response: ServerResponse = serde_json::from_slice(&buffer)?;
    Ok(response)
}

/// Add a task to the daemon
pub async fn run_client_add(url: String, start_paused: bool, config: &Config) -> Result<()> {
    // Check if daemon is active
    if !crate::is_daemon_active(config).await {
        // Daemon is not running, queue the task for next startup
        crate::append_to_queued_tasks(url.clone(), config).await?;
        println!("Daemon not running, queued for next startup: {}", url);
        return Ok(());
    }

    // Daemon is running, send add request
    let request = ClientRequest::Add { url, start_paused };
    let response = send_request(request, config).await?;

    match response {
        ServerResponse::Success { message } => {
            println!("{}", message);
            Ok(())
        }
        ServerResponse::Error { message } => {
            eprintln!("Error: {}", message);
            std::process::exit(1);
        }
        _ => {
            eprintln!("Unexpected response from daemon");
            std::process::exit(1);
        }
    }
}

/// Show status of all tasks
pub async fn run_client_status(verbose: bool, config: &Config, filter_failed: bool) -> Result<()> {
    let request = ClientRequest::Status { verbose };
    let response = send_request(request, config).await?;

    match response {
        ServerResponse::Status {
            queued_tasks,
            current_tasks,
            completed_tasks,
            failed_tasks,
            uptime_seconds,
            config_summary,
        } => {
            // Print header (only in verbose mode)
            if verbose {
                println!("\n=== tsp_ytdlp Status ===");
                println!("Uptime: {}s", uptime_seconds);
                let concurrent_display = config_summary.concurrent_downloads
                    .map(|n| n.get().to_string())
                    .unwrap_or_else(|| "unlimited".to_string());
                println!(
                    "Config: {} concurrent downloads, {}MB disk threshold",
                    concurrent_display, config_summary.disk_threshold
                );
                println!();
            }

            if verbose {
                // Verbose mode: queued -> current -> completed (original order)

                // Print queued tasks
                if !queued_tasks.is_empty() {
                    println!("Queued ({}):", queued_tasks.len());
                    for task in &queued_tasks {
                        println!("  #{}: {}", task.id, task.url);
                    }
                    println!();
                }

                // Print current downloads
                if !current_tasks.is_empty() {
                    println!(
                        "Current Downloads ({}{}):",
                        current_tasks.len(),
                        if let Some(concurrent_downloads) = config_summary.concurrent_downloads {
                            format!("/{concurrent_downloads}") 
                        } else {
                            "".to_string()
                        }
                    );
                for task in &current_tasks {
                    print!("  #{}: ", task.id);

                    // Show title if available
                    if let Some(ref title) = task.title {
                        print!("{}", title);
                    } else {
                        print!("{}", task.url);
                    }

                    // Show progress if available
                    if let Some(progress) = task.progress_percent {
                        // Show percentage and current size
                        if let Some(current_bytes) = task.current_size_bytes {
                            print!(" ({:.1}%, {})", progress, format_bytes(current_bytes));
                        } else {
                            print!(" ({:.1}%)", progress);
                        }
                    } else if let Some(current_bytes) = task.current_size_bytes {
                        // No total size, just show current size
                        print!(" ({})", format_bytes(current_bytes));
                    } else {
                        print!(" ({})", task.task_type);
                    }

                    // Show (Paused) suffix if paused
                    if task.is_paused {
                        print!(" (Paused)");
                    }

                    println!();

                    // Show path if verbose
                    if verbose
                        && let Some(ref path) = task.path {
                            println!("      Path: {}", path);
                        }
                }
                println!();
            }

                // Print completed tasks
                if !completed_tasks.is_empty() {
                    println!("Completed ({}):", completed_tasks.len());
                    for task in &completed_tasks {
                        print!("  #{}: ", task.id);

                        if let Some(ref title) = task.title {
                            println!("{}", title);
                        } else {
                            println!("{}", task.url);
                        }

                        if let Some(ref path) = task.path {
                            println!("      Path: {}", path);
                        }
                    }
                    println!();
                }

                // Print failed tasks in verbose mode (only if not filtered out)
                if !filter_failed && !failed_tasks.is_empty() {
                    println!("Failed ({}):", failed_tasks.len());
                    for task in &failed_tasks {
                        println!("  #{}: {}", task.id, task.url);
                        if let Some(ref error) = task.error {
                            println!("      Error: {}", error);
                        }
                    }
                    println!();
                }
            } else {
                // Non-verbose mode: current -> queued -> completed (new order)

                // Print current downloads
                if !current_tasks.is_empty() {
                    println!(
                        "Current Downloads ({}{}):",
                        current_tasks.len(),
                        if let Some(concurrent_downloads) = config_summary.concurrent_downloads {
                            format!("/{concurrent_downloads}") 
                        } else {
                            "".to_string()
                        }
                    );
                    for task in &current_tasks {
                        print!("  #{}: ", task.id);

                        // Show title if available
                        if let Some(ref title) = task.title {
                            print!("{}", title);
                        } else {
                            print!("{}", task.url);
                        }

                        // Show progress if available
                        if let Some(progress) = task.progress_percent {
                            // Show percentage and current size
                            if let Some(current_bytes) = task.current_size_bytes {
                                print!(" ({:.1}%, {})", progress, format_bytes(current_bytes));
                            } else {
                                print!(" ({:.1}%)", progress);
                            }
                        } else if let Some(current_bytes) = task.current_size_bytes {
                            // No total size, just show current size
                            print!(" ({})", format_bytes(current_bytes));
                        } else {
                            print!(" ({})", task.task_type);
                        }

                        // Show (Paused) suffix if paused
                        if task.is_paused {
                            print!(" (Paused)");
                        }

                        println!();
                    }
                }

                // Print queued tasks
                if !queued_tasks.is_empty() {
                    println!("Queued ({}):", queued_tasks.len());
                    for task in &queued_tasks {
                        println!("  #{}: {}", task.id, task.url);
                    }
                }

                // Print completed tasks with both title and URL
                if !completed_tasks.is_empty() {
                    println!("\nCompleted ({}):", completed_tasks.len());
                    for task in &completed_tasks {
                        print!("  #{}: ", task.id);

                        // Show both title and URL in non-verbose mode
                        if let Some(ref title) = task.title {
                            print!("{} ", title);
                        }
                        println!("{}", task.url);
                    }
                }

                // Print failed count only in non-verbose mode (only if not filtered out)
                if !filter_failed && !failed_tasks.is_empty() {
                    println!("Failed ({})", failed_tasks.len());
                }
            }

            // Print summary if no tasks
            if queued_tasks.is_empty()
                && current_tasks.is_empty()
                && completed_tasks.is_empty()
                && failed_tasks.is_empty()
            {
                println!("No tasks");
            }

            Ok(())
        }
        ServerResponse::Error { message } => {
            eprintln!("Error: {}", message);
            std::process::exit(1);
        }
        _ => {
            eprintln!("Unexpected response from daemon");
            std::process::exit(1);
        }
    }
}

/// Show only failed tasks
pub async fn run_client_failed(config: &Config) -> Result<()> {
    let request = ClientRequest::Status { verbose: false };
    let response = send_request(request, config).await?;

    match response {
        ServerResponse::Status { failed_tasks, .. } => {
            if failed_tasks.is_empty() {
                println!("No failed tasks");
                return Ok(());
            }

            // Check if output is being piped
            let is_piped = !atty::is(atty::Stream::Stdout);

            if is_piped {
                // Print URLs one per line (for piping to other programs)
                for task in &failed_tasks {
                    println!("{}", task.url);
                }
            } else {
                // Pretty print as table
                println!("\n=== Failed Tasks ({}) ===\n", failed_tasks.len());
                println!("{:<6} | URL", "ID");
                println!("{:-<6}-+-{:-<60}", "", "");
                for task in &failed_tasks {
                    println!("{:<6} | {}", task.id, task.url);
                    if let Some(ref error) = task.error {
                        println!("{:<6} | Error: {}", "", error);
                    }
                }
            }

            Ok(())
        }
        ServerResponse::Error { message } => {
            eprintln!("Error: {}", message);
            std::process::exit(1);
        }
        _ => {
            eprintln!("Unexpected response from daemon");
            std::process::exit(1);
        }
    }
}

/// Remove task(s) by ID(s)
pub async fn run_client_remove(ids: Vec<u64>, config: &Config) -> Result<()> {
    let mut had_errors = false;

    for id in ids {
        let request = ClientRequest::Remove { id };
        let response = send_request(request, config).await?;

        match response {
            ServerResponse::Success { message } => {
                println!("{}", message);
            }
            ServerResponse::Error { message } => {
                eprintln!("Error removing task {}: {}", id, message);
                had_errors = true;
            }
            _ => {
                eprintln!("Unexpected response from daemon for task {}", id);
                had_errors = true;
            }
        }
    }

    if had_errors {
        std::process::exit(1);
    }

    Ok(())
}

/// Clear all completed tasks
pub async fn run_client_clear(config: &Config) -> Result<()> {
    let request = ClientRequest::Clear;
    let response = send_request(request, config).await?;

    match response {
        ServerResponse::Success { message } => {
            println!("{}", message);
            Ok(())
        }
        ServerResponse::Error { message } => {
            eprintln!("Error: {}", message);
            std::process::exit(1);
        }
        _ => {
            eprintln!("Unexpected response from daemon");
            std::process::exit(1);
        }
    }
}

/// Kill the daemon
pub async fn run_client_kill(config: &Config) -> Result<()> {
    // Check if daemon is running first
    if !crate::is_daemon_active(config).await {
        println!("Daemon is not running");
        return Ok(());
    }

    let sender_pid = std::process::id();
    let request = ClientRequest::Kill { sender_pid };

    // Send kill request (daemon will exit, so we may not get a response)
    match send_request(request, config).await {
        Ok(ServerResponse::Success { message }) => {
            println!("{}", message);
            send_notification(APP, "Daemon killed 💀", Some(1500), config).await;
        }
        Err(_) => {
            // Expected - daemon likely exited before responding
            println!("Daemon killed");
            send_notification(APP, "Daemon killed 💀", Some(1500), config).await;
        }
        _ => {}
    }

    Ok(())
}

/// Show info/logs for a task
pub async fn run_client_info(id: u64, config: &Config) -> Result<()> {
    let request = ClientRequest::Info { id };
    let response = send_request(request, config).await?;

    match response {
        ServerResponse::Info { log_file_path } => {
            println!("Log file: {}", log_file_path);

            // Try to display the log file if it exists
            if std::path::Path::new(&log_file_path).exists() {
                println!("\n--- Log contents ---");
                match std::fs::read_to_string(&log_file_path) {
                    Ok(contents) => {
                        println!("{}", contents);
                    }
                    Err(e) => {
                        eprintln!("Error reading log file: {}", e);
                    }
                }
            } else {
                println!("(Log file does not exist yet)");
            }

            Ok(())
        }
        ServerResponse::Error { message } => {
            eprintln!("Error: {}", message);
            std::process::exit(1);
        }
        _ => {
            eprintln!("Unexpected response from daemon");
            std::process::exit(1);
        }
    }
}

/// Pause task(s)
pub async fn run_client_pause(ids: Option<Vec<u64>>, config: &Config) -> Result<()> {
    let request = ClientRequest::Pause { ids };
    let response = send_request(request, config).await?;

    match response {
        ServerResponse::Success { message } => {
            println!("{}", message);
            Ok(())
        }
        ServerResponse::Error { message } => {
            eprintln!("Error: {}", message);
            std::process::exit(1);
        }
        _ => {
            eprintln!("Unexpected response from daemon");
            std::process::exit(1);
        }
    }
}

/// Resume task(s)
pub async fn run_client_resume(ids: Option<Vec<u64>>, config: &Config) -> Result<()> {
    let request = ClientRequest::Resume { ids };
    let response = send_request(request, config).await?;

    match response {
        ServerResponse::Success { message } => {
            println!("{}", message);
            Ok(())
        }
        ServerResponse::Error { message } => {
            eprintln!("Error: {}", message);
            std::process::exit(1);
        }
        _ => {
            eprintln!("Unexpected response from daemon");
            std::process::exit(1);
        }
    }
}
