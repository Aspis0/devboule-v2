# Rust crates

| Crate | Role |
| --- | --- |
| `devboule-protocol` | Wire types shared by the app and the daemon. No I/O. |
| `devboule-daemon` | Local daemon and client: named-pipe transport, sessions, journal, and provider discovery. |
| `oracle-core` | Rust-native local indexing and retrieval: chunking, embeddings, Lance/SQLite, and the CLI. Not an answerer. |
| `devboule-augur` | Repository review library: detectors, findings, a SQLite ledger, and SARIF exchange. |
| `devboule-plugin-rpc` | Host/plugin-backend lifecycle over named pipes, including capability handshake, invocation, spawning, and Job Object ownership. |
| `polis-backend` | Polis plugin backend process for the workspace city graph and Augur findings. |

`devboule-plugin-rpc`, `polis-backend` and `devboule-augur` exist to keep plugin work out
of the host process. A plugin backend is a separate binary reached over a pipe, so a plugin
that hangs, crashes or leaks does so in its own process and is killed with it.

## Two directories inside `oracle-core` that are not source

Neither is a crate, and neither is obvious from its name:

- `oracle-core/vendor/esaxx-rs-0.1.10/` — a vendored copy of `esaxx-rs`,
  patched to build against the dynamic CRT (`/MD`) on Windows so it matches the
  prebuilt `ort-sys` binaries. Without the patch the linker fails on a CRT
  mismatch. The patch is declared in the workspace `Cargo.toml`, which also
  records the condition for dropping it.
- `oracle-core/golden/` — a small synthetic corpus and the frozen JSON outputs
  expected from it. It is test data, not fixtures the application ever loads.
