# TODO
* Add a check for losing internet access (e.g. check archlinux.org)
    * Store as an Arc<AtomicBool>
    * Poll every N for internet access
    * Store N as internet_poll_rate = N (where N is in milliseconds, default 10 000 or 10s )
    * make the client display this when querying status (but displayed only when  no internet)
* Add a --queued flag that checks:
    * if the daemon is not active read queued.json then show queued urls
    * if the daemon is active, ignore the flag and proceed as  aregular tsp_ytdlp status query
## TODO Advanced
* For now keep the actual yt-dlp commands for GetName and DownloadVideo hard coded but consider what params we should expose beyond sponsorblock?
    - Should we just allow yt-dlp commands to be configurable?
    - If we go down the route of exposing specific options to be passed to yt-dlp they should be in the form yt-dlp already expects
* Add options for handling music, e.g. use get_video_dir.sh (how since we currently rely on an echo -> expect a comma separated ? json?) or a --music flag to treat the file as a music file (strip video and use special directory)
* If the size value returned is None (from Task::GetName), adjust the client code to display the size of the video fragments currently downloaded (when downloading) - if it has returned a value show the size of the video file fragments + the % downloaded (e.g. current_size / total_size - size is in an integer so account for that)
* Refactor client::run_client_status so that we deduplicate the println!() stmts that are duplicated because of if verbose {} the solution is to format!() the shared strings before the if verbose {} check then use them in that check in both locations
    * these format!() expr should be created at the top of the run_client_status fn

