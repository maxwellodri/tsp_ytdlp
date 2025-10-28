# Bash Command Equivalents for ytdl_tsp

This document provides the raw `yt-dlp` command equivalents for the operations performed by the ytdl_tsp daemon.

## Phase 1: GetName - Fetch Video Metadata

**Purpose:** Retrieve video title and file size without downloading.

```bash
yt-dlp \
  --print filename \
  --print filesize_approx \
  --cookies "$HOME/source/private/cookies.firefox-private.txt" \
  --restrict-filename \
  --ignore-config \
  --no-playlist \
  --simulate \
  -o "%(channel)s_%(title)s" \
  "$URL"
```

**Notes:**
- For non-YouTube URLs, the output format uses `%(title)s` instead of `%(channel)s_%(title)s`
- `--simulate` prevents actual download
- Returns two lines: filename (line 1) and filesize_approx (line 2)

## Phase 2: DownloadVideo - Download the Video

**Purpose:** Download the video with fault tolerance and quality controls.

```bash
yt-dlp \
  --newline \
  --progress \
  --restrict-filename \
  --trim-filenames 200 \
  --ignore-config \
  --no-playlist \
  --merge-output-format mp4 \
  --format "best[height<=?720]" \
  --retries infinite \
  --fragment-retries infinite \
  --retry-sleep linear=1:120:2 \
  --continue \
  --skip-unavailable-fragments \
  -o "/path/to/download/dir/%(title)s.%(ext)s" \
  "$URL"
```

**Optional: With Download Speed Throttling**

```bash
yt-dlp \
  --newline \
  --progress \
  --restrict-filename \
  --trim-filenames 200 \
  --ignore-config \
  --no-playlist \
  --merge-output-format mp4 \
  --format "best[height<=?720]" \
  --retries infinite \
  --fragment-retries infinite \
  --retry-sleep linear=1:120:2 \
  --continue \
  --skip-unavailable-fragments \
  --limit-rate 100K \
  -o "/path/to/download/dir/%(title)s.%(ext)s" \
  "$URL"
```

**Optional: With Custom Fragment Cache Directory**

```bash
# Create unique cache directory for this download
CACHE_DIR="/tmp/yt-dlp-fragments"
URL_HASH=$(echo -n "$URL" | md5sum | cut -d' ' -f1)
UNIQUE_CACHE="$CACHE_DIR/$URL_HASH"
mkdir -p "$UNIQUE_CACHE"

yt-dlp \
  --newline \
  --progress \
  --restrict-filename \
  --trim-filenames 200 \
  --ignore-config \
  --no-playlist \
  --merge-output-format mp4 \
  --format "best[height<=?720]" \
  --retries infinite \
  --fragment-retries infinite \
  --retry-sleep linear=1:120:2 \
  --continue \
  --skip-unavailable-fragments \
  --paths "temp:$UNIQUE_CACHE" \
  -o "/path/to/download/dir/%(title)s.%(ext)s" \
  "$URL"
```

## Unified Download - Single Command Approach

**Purpose:** Get metadata and download in a single yt-dlp call.

```bash
yt-dlp \
  --newline \
  --progress \
  --restrict-filename \
  --trim-filenames 200 \
  --ignore-config \
  --no-playlist \
  --write-info-json \
  --merge-output-format mp4 \
  --format "best[height<=?720]" \
  --retries infinite \
  --fragment-retries infinite \
  --retry-sleep linear=1:120:2 \
  --continue \
  --skip-unavailable-fragments \
  -o "/path/to/download/dir/%(title)s.%(ext)s" \
  "$URL"
```

**Notes:**
- `--write-info-json` creates a `.info.json` file with metadata
- The daemon deletes the `.info.json` file after extracting metadata
- This approach simplifies the two-phase process into one command

## Client Commands

**Add a download to the queue:**
```bash
ytdl_tsp "$URL"
```

**Check status:**
```bash
ytdl_tsp
ytdl_tsp -v  # verbose mode
```

**Remove a task:**
```bash
ytdl_tsp -r <task_id>
```

**Clear completed downloads:**
```bash
ytdl_tsp --clear
```

**Get task info/logs:**
```bash
ytdl_tsp --info <task_id>
```

**Kill the daemon:**
```bash
ytdl_tsp --kill
```

**Start the daemon:**
```bash
ytdl_tsp --daemon
```

## Configuration Options

The daemon respects these configuration options in `~/.config/ytdl_tsp/config.toml`:

- `concurrent_downloads` - Number of simultaneous downloads (0 = unlimited)
- `download_dir` - Default download directory
- `disk_threshold` - Minimum disk space to keep free (MB)
- `should_notify_send` - Enable/disable desktop notifications
- `fragment_cache_dir` - Optional directory for temporary fragment files
- `throttle` - Optional download speed limit (KB/s)

## Custom Directory Resolution Script

**Location:** `~/source/private/get_video_dir.sh`

The daemon uses this optional script to determine custom download directories for non-YouTube URLs. If the script exists and is executable, it will be called with the URL as an argument and should output the desired download directory path.

**Script Interface:**
```bash
# Input: URL as first argument
# Output: Absolute path to download directory (stdout)
# Returns: Exit code 0 on success

~/source/private/get_video_dir.sh "$URL"
```

**Behavior:**
- **YouTube URLs**: Always use `config.download_dir` (script is not called)
- **Non-YouTube URLs**: Call script if it exists and is executable
- **Fallback**: If script doesn't exist, isn't executable, or fails, use `config.download_dir`
- **Path Validation**: Output must start with `/`, `~`, or `$HOME`
- **Directory Creation**: The daemon will create the directory if it doesn't exist

**Example Script:**
```bash
#!/bin/bash
# ~/source/private/get_video_dir.sh

URL="$1"

# Example: Route different sites to different directories
if [[ "$URL" == *"vimeo.com"* ]]; then
    echo "$HOME/Videos/vimeo"
elif [[ "$URL" == *"twitch.tv"* ]]; then
    echo "$HOME/Videos/twitch"
else
    echo "$HOME/Videos/other"
fi
```

**Note:** There's also a local alias at `get_video_dir.sh` in the project root that references this private script.

## Key yt-dlp Options Explained

- `--retries infinite` / `--fragment-retries infinite` - Unlimited retries for maximum reliability
- `--retry-sleep linear=1:120:2` - Linear backoff: start at 1s, max 120s, increment by 2s
- `--continue` - Resume partial downloads
- `--skip-unavailable-fragments` - Skip missing fragments instead of failing
- `--format "best[height<=?720]"` - Get best quality up to 720p (single file with audio+video)
- `--merge-output-format mp4` - Force merge to mp4 format
- `--restrict-filename` - Use ASCII-only filenames safe for all filesystems
- `--trim-filenames 200` - Limit filename length to 200 characters
- `--ignore-config` - Ignore user's yt-dlp config for consistency
- `--no-playlist` - Only download single video, not entire playlists
