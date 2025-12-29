use std::{path::PathBuf, process::Stdio};
use tokio::process::Command;
use tracing::{info, warn};
use anyhow::{Context, Result};

use crate::{common::expand_path, Config};

/// Find cache directory by URL hash
/// Searches for directories starting with {hash}
pub async fn find_cache_dir_by_hash(cache_dir: &PathBuf, url_hash: &str) -> Option<PathBuf> {
    if let Ok(mut entries) = tokio::fs::read_dir(cache_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_dir()
                && let Some(dir_name) = path.file_name().and_then(|n| n.to_str())
                    && dir_name.starts_with(url_hash) {
                        return Some(path);
                    }
        }
    }
    None
}

/// Helper function for touching files to update timestamps
pub async fn touch_file(path: &PathBuf) -> Result<()> {
    let now = filetime::FileTime::now();
    filetime::set_file_times(path, now, now)
        .with_context(|| format!("Failed to set file times for {}", path.display()))?;
    Ok(())
}

/// Get available disk space in megabytes for the given path
/// Returns Ok(available_mb) with the available space, or Err if unable to determine
pub async fn get_disk_space(path: &str) -> anyhow::Result<u32> {
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

    if lines.len() >= 2
        && let Ok(available_mb) = lines[1].trim().parse::<u32>() {
            return Ok(available_mb);
        }

    anyhow::bail!("Failed to parse df output")
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

    // Check if custom script path is configured
    let script_path = match &config.video_dir_script {
        Some(path) => match expand_path(path) {
            Ok(expanded) => expanded,
            Err(e) => {
                use tracing::error;
                error!(
                    "Failed to expand script path '{}': {}, using default directory",
                    path, e
                );
                return default_dir.clone();
            }
        },
        None => {
            info!(
                "No video_dir_script configured, using default directory: {}",
                default_dir
            );
            return default_dir.clone();
        }
    };

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
                use tracing::warn;

                warn!(
                    "get_video_dir.sh at {} is not executable, using default directory: {}",
                    script_path, default_dir
                );
                return default_dir.clone();
            }
        }
    }

    // Try to execute the script
    match Command::new(&script_path)
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
