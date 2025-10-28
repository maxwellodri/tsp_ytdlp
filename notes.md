* Make use of send_critical_notification (when task fails for any reason) and send_notification for regular updates.
    - This modification should be contained within the Task::transition method. (read common.rs for send_notification/send_critical_notification function details)
    - Queued -> GetName has a timeout of 30seconds
    - GetName -> Download has a timeout of 3seconds
    - Download -> Completed has a timeout of 3seconds
    - _ -> Failed uses the send_critical_notification  rather than send_notification
* create an async fn is_daemon_active() -> bool method; simply check if the socket is active
* Introduce a queued_tasks.json (in $XDG_DATA_HOME/tsp_ytdlp/) when a client attempts to add a task and the daemon is not active, append it to this (create file if doesnt exist)

```rust
struct QueuedTasks {
    urls: Vec<String>
}
```

- on startup read the queued_tasks.json file and copy its urls over to the active Tasks. Then clear the queued_tasks.json (remove json file)
- Ensure we handle adding duplicate urls. If a url already exists as a task -> the daemon doesnt add it as a task. (compare using Task::url())
    * Simply iterate over the current Tasks
- Add a new client operation [--failed]
    - prints only failed tasks
    - Modify the default status cmd (when tsp_ytdlp is run with no args), to filter out failed tasks
    - On the client, check if the program is being piped to another program, if it is, then print the urls 1 per line
    - if it is not being piped pretty print it as a table:
        * id | url
- Modify [--remove] so it is an Option<String> and is parsed in a second non-clap parse step as  comma separated list of ids, containing 1 or more ids, 
    - Following are valid:
    - e.g.: --remove 0
    - e.g.: --remove 0,1,2,3
    - e.g.: -r 0,1,9
