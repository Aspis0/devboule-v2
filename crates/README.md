# Rust crates

| Crate | Role |
| --- | --- |
| `devboule-protocol` | Wire types shared by the app and the daemon. No I/O. |
| `devboule-daemon` | Daemon binary plus the blocking client used by `src-tauri`. |
| `oracle-core` | Local index and retrieval (chunking, embeddings, Lance/SQLite). Not an HTTP/MCP server and not an answerer. |
