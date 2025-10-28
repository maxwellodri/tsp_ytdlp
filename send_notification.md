//GetName enter notification:
// Send notification with  MD5-based notification ID (30s timeout)
// it will be overwritten when transitioning 
send_notification(&url, &format!("Processing: {url} 🔄"), Some(30000), config).await;


For Task::DownloadVideo -> we can send another notification to overwrite and use the title
                        // Send "Downloading" notification with timeout (replaces GetName notification)
                        send_notification(
                            url,
                            &format!("Downloading: {title} 🎬"),
                            Some(3000),
                            config,
                        )


* On Completion:
                // Send completion notification with filename
                let filename_display = path
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or(metadata.title.as_deref().unwrap_or("video"));

                send_notification(
                    url,
                    &format!("Download completed: {filename_display} 🤗✅"),
                    Some(5000),
                    config,
                )
* send notification if url already queued/finished:
                    send_notification(
                        &url,
                        &format!("URL already queued/finished: {url} 🕺"),
                        Some(1200),
                        &config,
                    )

