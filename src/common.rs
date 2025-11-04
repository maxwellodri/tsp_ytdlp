use tokio::process::Command;
use crate::Config;

pub const APP: &str = "tsp_ytdlp";

// Shared types and functions

/// Format bytes into human-readable size (KB, MB, GB)
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}
pub async fn send_critical_notification(url: &str, message: &str, config: &Config) {
    // Always log the notification message for testing validation
    tracing::debug!(
        "CRITICAL notification for URL '{}': {}",
        url,
        message
    );

    // Only execute the actual command if notifications are enabled
    if config.should_notify_send {
        // Create notification ID from URL hash (similar to bash script)
        let notification_id = format!("{:x}", md5::compute(url.as_bytes()));
        let notification_hint = format!("string:x-canonical-private-synchronous:{notification_id}");

        // Use spawn() instead of output().await to make this non-blocking
        match Command::new("notify-send")
            .args([
                "-h",
                &notification_hint,
                "-u",
                "critical", // Critical urgency level
                "-t",
                "0", // Unlimited timeout (0ms means no timeout)
                message,
            ])
            .spawn()
        {
            Ok(_child) => {
                tracing::debug!("Critical notification sent (fire-and-forget)");
            }
            Err(e) => {
                tracing::warn!("Failed to spawn notify-send: {}", e);
            }
        }
    } else {
        tracing::debug!("Notifications disabled in config, skipping notify-send command");
    }
}

pub async fn send_notification(url: &str, message: &str, timeout_ms: Option<u32>, config: &Config) {
    // Always log the notification message for testing validation
    tracing::debug!(
        "Notification for URL '{}' (timeout: {:?}ms): {}",
        url,
        timeout_ms,
        message
    );

    // Only execute the actual command if notifications are enabled
    if config.should_notify_send {
        // Create notification ID from URL hash (similar to bash script)
        let notification_id = format!("{:x}", md5::compute(url.as_bytes()));
        let notification_hint = format!("string:x-canonical-private-synchronous:{notification_id}");

        let timeout = timeout_ms.unwrap_or(0);
        let timeout_string = timeout.to_string();

        // Use spawn() instead of output().await to make this non-blocking
        match Command::new("notify-send")
            .args(["-h", &notification_hint, "-t", &timeout_string, message])
            .spawn()
        {
            Ok(_child) => {
                tracing::debug!("Notification sent (fire-and-forget)");
            }
            Err(e) => {
                tracing::warn!("Failed to spawn notify-send: {}", e);
            }
        }
    } else {
        tracing::debug!("Notifications disabled in config, skipping notify-send command");
    }
}
