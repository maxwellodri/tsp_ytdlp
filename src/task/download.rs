use crate::Config;
use crate::task::GetNameMetadata;
use crate::task::MediaType;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{info, warn};

/// Spawn and execute a video download task
/// Returns Ok(PathBuf) with final path on success, Err on failure
pub async fn spawn_download_media_task(
    url: String,
    media_type: MediaType,
    metadata: GetNameMetadata,
    config: Config,
) -> Result<PathBuf> {
    let title = metadata.title.as_deref().unwrap_or("download");

    // Determine file extension based on media type
    let file_ext = match media_type {
        MediaType::Audio => "ogg",
        MediaType::Video => "mp4",
    };

    // Construct final destination path
    let final_path = PathBuf::from(&metadata.directory).join(format!("{}.{}", title, file_ext));

    // Determine cache directory for download
    let cache_dir = PathBuf::from(&config.cache_dir);

    // Create unique cache directory for this URL with human-readable name
    let url_hash = format!("{:x}", md5::compute(url.as_bytes()));
    let sanitized_title = sanitize_title(title);
    let cache_dir_name = format!("{}-{}", url_hash, sanitized_title);
    let unique_cache = cache_dir.join(&cache_dir_name);

    tokio::fs::create_dir_all(&unique_cache)
        .await
        .context("Failed to create cache directory")?;

    // Temp download path in cache
    let temp_download_path = unique_cache.join(format!("{}.{}", title, file_ext));

    info!(
        "Downloading {} to cache: {}\nWill move to: {}",
        media_type,
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
    ]);

    match media_type {
        MediaType::Audio => {
            cmd.args([
                "--extract-audio",
                "--audio-format",
                "vorbis",
                "--format",
                "bestaudio/best",
            ]);
        }
        MediaType::Video => {
            cmd.args([
                "--merge-output-format",
                "mp4",
                "--format",
                "best[height<=?720]",
            ]);
        }
    }

    cmd.args([
        "--retries",
        "infinite",
        "--fragment-retries",
        "infinite",
        "--retry-sleep",
        "linear=1:120:2",
        "--continue",
        "--skip-unavailable-fragments",
        "--parse-metadata",
        "webpage_url:%(comment)s",
        "--embed-metadata",
    ]);

    // Add SponsorBlock options
    match media_type {
        MediaType::Audio => {
            // For audio, remove ALL sponsorblock categories (ignore config)
            cmd.args(["--sponsorblock-remove", "all"]);
        }
        MediaType::Video => {
            // For video, use config settings
            if let Some(mark) = &config.sponsorblock_mark
                && !mark.is_empty()
            {
                cmd.args(["--sponsorblock-mark", mark]);
            }
            if let Some(remove) = &config.sponsorblock_remove
                && !remove.is_empty()
            {
                cmd.args(["--sponsorblock-remove", remove]);
            }
        }
    }

    // Add cookies if available
    if let Some(cookie_file) = &config.cookies_file {
        cmd.args(["--cookies", cookie_file]);
    }

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
    cmd.args(["-o", &format!("{}.{}", title, file_ext)]);
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

    // Find the actual downloaded file (yt-dlp may normalize the filename)
    // Search for video files starting with the title prefix
    let actual_file = if temp_download_path.exists() {
        temp_download_path.clone()
    } else {
        // Search for video files in cache directory that start with the title
        let mut found_file: Option<PathBuf> = None;
        if let Ok(mut entries) = tokio::fs::read_dir(&unique_cache).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    // Check if it's a media file starting with our title
                    let file_extensions = match media_type {
                        MediaType::Audio => vec![".ogg"],
                        MediaType::Video => vec![".mp4", ".mkv", ".webm"],
                    };
                    if file_name.starts_with(title)
                        && file_extensions.iter().any(|ext| file_name.ends_with(ext))
                    {
                        found_file = Some(path);
                        break;
                    }
                }
            }
        }
        found_file.ok_or_else(|| {
            anyhow::anyhow!(
                "Downloaded file not found in cache. Expected: {}, searched in: {}",
                temp_download_path.display(),
                unique_cache.display()
            )
        })?
    };

    // Ensure final destination directory exists
    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("Failed to create destination directory")?;
    }

    // Move file from cache to final destination
    info!(
        "Moving file from {} to: {}",
        actual_file.display(),
        final_path.display()
    );
    if let Err(e) = tokio::fs::rename(&actual_file, &final_path).await {
        // If rename fails (different filesystems), try copy + delete
        warn!("Rename failed, trying copy: {}", e);
        tokio::fs::copy(&actual_file, &final_path)
            .await
            .context("Failed to copy file to destination")?;

        // Successfully copied, clean up
        let _ = tokio::fs::remove_file(&actual_file).await;
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

/// Sanitize a title for use in a directory name
/// Removes/replaces special characters and limits length
fn sanitize_title(title: &str) -> String {
    title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .chars()
        .take(100) // Limit to 100 characters
        .collect()
}
