# TODO
* Add a check for losing internet access (e.g. check archlinux.org)
    * Store as an Arc<AtomicBool>
    * Poll every N for internet access
    * Store N as internet_poll_rate = N (where N is in milliseconds, default 10 000 or 10s )
    * make the client display this when querying status (but displayed only when  no internet)
* Add a check to fix a rare bug: sometimes all tasks can be queued, but none are active. The fix is to add a check in the daemon if there are 0 tasks active, and some queued, start the first task - this needs to be done in such a way that no race conditions happen. i believe we can do this as part of daemon.rs
* Add a --queued flag that checks:
  - if the daemon is not active read queued.json then show queued urls
   - if the daemon is active, ignore the flag and proceed as  aregular tsp_ytdlp status query
## TODO Advanced
* For now keep the actual yt-dlp commands for GetName and DownloadVideo hard coded but consider what params we should expose beyond sponsorblock?
    -  Should we just allow yt-dlp commands to be configurable?
    - If we go down the route of exposing specific options to be passed to yt-dlp they should be in the form yt-dlp already expects
* Refactor client::run_client_status so that we deduplicate the println!() stmts that are duplicated because of if verbose {} the solution is to format!() the shared strings before the if verbose {} check then use them in that check in both locations
    * these format!() expr should be created at the top of the run_client_status fn

