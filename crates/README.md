# Rust crates

| Crate | Role |
| --- | --- |
| `devboule-protocol` | Wire types shared by the app and the daemon. No I/O. |
| `devboule-daemon` | Daemon binary plus the blocking client used by `src-tauri`. |
| `oracle-core` | Local index and retrieval (chunking, embeddings, Lance/SQLite). Not an HTTP/MCP server and not an answerer. |
| `devboule-plugin-rpc` | Host side of the plugin-backend conversation: named pipe, handshake with plugin-scoped capabilities, invoke. Process membership is a Windows Job Object with `KILL_ON_JOB_CLOSE`, so an orphaned backend cannot outlive the host. |
| `polis-backend` | The Polis plugin's backend process. The city graph lives here, never in the host. |
| `devboule-augur` | Review findings as a plain library — detectors and a ledger, nothing that draws. Linked by the Polis backend; the app does not link it. |

The last three exist to keep plugin work out of the host process. A plugin backend is a
separate binary reached over a pipe, so a plugin that hangs, crashes or leaks does so in
its own process and is killed with it.
