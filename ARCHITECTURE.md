# Devboule architecture

How Devboule is built. Every structural claim below carries the **file, and the symbol inside it**, that
was read in this tree; where a line number is printed instead, it is because the subject has no name
of its own (a comment, a match arm, a table row, a value), and that number was re-read and printed by
`scout/cleanup-docs/remeasure.py` in the tree that ships it. Where a statement is a code reading
rather than a runtime observation, it says so.

This document is a reading, not an artefact of one commit. Until 21 September 2026 it opened by naming
`f3647ee` as its verification point, and that line had stopped being true: it was written against a
17,859-line `crates/devboule-daemon/src/session.rs` on 13 September, extended twice since (`a417b57`,
17 September) without re-deriving its numbers, and by the time it was corrected two refactors were
behind it — `session.rs` had become one module of a 47-file family, and `server.rs` had become a
`server/` directory of 16 submodules. Seven of the anchors it claimed had been verified at `f3647ee`
were already wrong at `f3647ee` itself. The 21 September pass converted every anchor that could take a
symbol to one, re-measured the rest against the tree, and is recorded here rather than silently
applied: the numbers below are the tree at `00b0042`.

Two conventions:

- Paths are relative to the repository root.
- Section 2 restates a lifecycle measurement made in a design note that does not ship with this
  repository. That measurement was taken while four daemon files were being edited, so **every line
  number reproduced here was re-checked against a quiet tree** and the verified number is the one
  printed.
- A bare filename beside a symbol (`session.rs`) resolves against the full path given for that sentence
  group; where a name exists in more than one crate — `session.rs`, `lib.rs`, `server.rs`, `spawn.rs`
  and `mod.rs` all do — the path is written out in full instead.

## 1. The shape

Four pieces, one wire.

| Piece | Lives in | Runs where |
| --- | --- | --- |
| Surfaces, all rendering | `src/` (React + TypeScript) | the app process, in WebView2 |
| Tauri host and RPC edge | `src-tauri/` | the app process |
| Daemon | `crates/devboule-daemon/` | **its own `devboule-daemon.exe`** |
| Wire types | `crates/devboule-protocol/` | compiled into both |

Tauri v2 is the shell: `src-tauri/Cargo.toml:23` pins `tauri = "=2.11.5"`, and
`src-tauri/tauri.conf.json:3` names the product. The frontend is a React app built by Vite; the app
mounts one surface at a time from a registry (`src/types/surface.ts:22`, `src/app/App.tsx:101`).

The protocol crate is the only place the wire types are declared, and the dependency is deliberate:
`src-tauri/Cargo.toml:28` ("Shared wire types. The daemon and this crate must not declare their own
copies"). Protocol version 5 (`crates/devboule-protocol/src/lib.rs`, `PROTOCOL_VERSION`), with
`PROTOCOL_MIN_VERSION` also 5 — the two are equal so a v4 peer is refused at the handshake instead of
dying at the first frame.

### The daemon is a separate process

The GUI never runs the server in-process. That is stated in the manifest
(`src-tauri/Cargo.toml:31`, which pulls `devboule-daemon` with `default-features = false`, with the
comment "the GUI never runs the server in-process") and enforced by the build: the whole serving half
of the daemon crate is behind the `server` feature (the `#[cfg(feature = "server")]` module block in
`crates/devboule-daemon/src/lib.rs`), and `crates/devboule-daemon/src/main.rs:17-18` makes a binary built without it a compile
error rather than an in-process server.

They meet on a Windows named pipe. The pipe name is derived, not configured: the runtime directory is
normalised (separators and case) and hashed with FNV-1a into sixteen hex characters, giving
`\\.\pipe\devboule-<hash>` (`crates/devboule-daemon/src/paths.rs:60-77`). `std`'s `DefaultHasher` is
deliberately not used, because it is seeded per process and would put the two ends on different pipes
(`paths.rs:5-8`). The pipe is created with a current-user-only security descriptor, `PIPE_TYPE_BYTE`,
and `PIPE_REJECT_REMOTE_CLIENTS`, with up to sixteen instances
(`crates/devboule-daemon/src/transport/windows_pipe.rs`, `NamedPipeListener::bind`,
`PipeSecurity::current_user_only`, `CreateNamedPipeW`, `MAX_INSTANCES`).

The daemon binary is found by the app at start-up: an explicit `DEVBOULE_DAEMON`, else a sibling of
the app executable, else `target/{debug,release}/devboule-daemon.exe`
(`src-tauri/src/client/mod.rs`, `locate_daemon_binary`; the daemon's own client has the same rule at
`crates/devboule-daemon/src/spawn.rs:20-37`). In development the daemon is built before the frontend
runs (`src-tauri/tauri.conf.json:9`, `beforeDevCommand`); there is no bundling step
(`bundle.active: false` in `src-tauri/tauri.conf.json`).

### What the daemon owns that the app does not

- **Every child process and its lifetime.** PTY terminals and provider CLIs are spawned by the daemon
  (`crates/devboule-daemon/src/provider.rs`, `open_pty_session`, for a PTY), each in a Windows Job
  Object (§2).
- **The journal.** SQLite, WAL mode, `journal.db` beside the lock file
  (`crates/devboule-daemon/src/paths.rs:51-55`), schema version 14
  (`crates/devboule-daemon/src/journal.rs:54`).
- **The MCP broker.** A loopback HTTP listener with one bearer token per session
  (`crates/devboule-daemon/src/mcp_broker.rs:1-7`, `MCP_PATH`, `McpLaunchConfig`).
- **The peer listener.** A TCP listener on the tailnet, everything inside Noise
  (`crates/devboule-daemon/src/peer_transport.rs:1-6`), started best-effort at boot
  (`crates/devboule-daemon/src/server/lifecycle.rs`, `try_start_remote_listener`).
- **Attachments**, under the runtime directory (`crates/devboule-daemon/src/attachment_store.rs:3-8`).
- **The per-provider tool policy** (`crates/devboule-daemon/src/tool_policy.rs:1-13`).
- **Provider discovery**, which is a `PATH` scan plus a launch resolver
  (`crates/devboule-daemon/src/provider_catalog.rs:1-16`).

What stays in the app process: the surfaces, the plugin host and asset server
(`src-tauri/src/plugins/`, managed at `src-tauri/src/lib.rs:22-25`), the local Oracle index and
retrieval engine (`src-tauri/src/oracle/`), and the Tauri command surface itself — 60-odd commands in
one `generate_handler!` list (`src-tauri/src/lib.rs:50-115`). The app is a client of the daemon like
any other, which is why the same `ClientMessage` vocabulary serves it and a paired device (§6).

## 2. Process and lifetime

This section is the workspace report's finding, restated with line numbers re-read in this tree. The
report is a code reading: nothing in it — or here — was executed, and the Windows behaviours it cites
are asserted by the code's own comments and API choices rather than observed on a bench (report §5).

**Who spawns the daemon.** The app. `DaemonBridge::start()` is managed state built at Tauri build
time (`src-tauri/src/lib.rs:20`) and it spawns a `daemon-client` supervisor thread
(`src-tauri/src/client/mod.rs`, `supervisor`). The supervisor's connect step is `connect_once`, which
builds the owner from the current user's SID plus `app-<pid>`, resolves the daemon binary through
`locate_daemon_binary`, and calls `connect_or_spawn` with that binary
(`src-tauri/src/client/mod.rs`, `connect_once`).

**How an already-running daemon is found.** By connecting, not by looking for a process. The
connect-or-spawn loop tries to connect *first* and spawns only if that fails
(`crates/devboule-daemon/src/client.rs`, `connect_or_spawn_with`); the address is the deterministic
pipe name (`paths.rs:75-77`). A second daemon is harmless: the single-instance lock is an exclusive
`LockFileEx` on `daemon.lock` (`crates/devboule-daemon/src/lock.rs`, `SingleInstanceLock::acquire`,
`try_lock_exclusive`), and the loser of that lock prints a human sentence and **exits 0**, because
"nothing to do" is not "failure" (`crates/devboule-daemon/src/main.rs:23-32`). Note the lock file's
existence is not the lock — the OS releases it when the process dies, so a stale file is not a
deadlock (`lock.rs:1-2`).

**The spawn itself.** `spawn_daemon` sets `DEVBOULE_RUNTIME_DIR`, nulls the three standard streams and
passes `CREATE_NO_WINDOW` (`crates/devboule-daemon/src/spawn.rs:41-43`, `:61-82`). The retry budget is
`SPAWN_ATTEMPTS` (50) sleeps of `SPAWN_SLEEP` (100 ms) apart
(`crates/devboule-daemon/src/client.rs`), and the handshake has its own 2 s timeout (`client.rs:26`).

**On quit.** One `RunEvent::Exit` handler calls `oracle.shutdown()`, `DaemonBridge::shutdown()` and
`PluginRuntime::stop_all()` (`src-tauri/src/lib.rs:126-131`). The bridge stops its thread, sends the
`Shutdown` RPC and joins within `JOIN_BUDGET` (`src-tauri/src/client/mod.rs`, `DaemonBridge::shutdown`);
`DaemonClient::shutdown` requires `accepted: true` (`crates/devboule-daemon/src/client.rs`, `shutdown`);
the daemon's dispatch **flushes the journal before it accepts**, so the reply is the app's last
guarantee that the transcript is on disk (`crates/devboule-daemon/src/server/dispatch.rs`, the
`ClientMessage::Shutdown` arm). `run_windows` then wakes from `wait_until_shutdown`
(`crates/devboule-daemon/src/server/lifecycle.rs`, `run_windows`;
`crates/devboule-daemon/src/server/state.rs`, `ServerState::wait_until_shutdown`), flushes again, shuts
the listener, stops the peer listener and bounded-joins the accept thread (the teardown block in
`run_windows`), and `main` returns (`crates/devboule-daemon/src/main.rs:21-22`). There is no window-close or exit-requested
handler anywhere in the tree, so "closing the last window quits the app" is not something this code
shows.

**On a crash, and on losing the pipe.** Two different mechanisms on the two sides.

- *App side.* Liveness is checked only here: a `Status` RPC every `PING_PERIOD` (2 s), and after
  `STATUS_FAILURE_THRESHOLD = 3` consecutive failures the reported state becomes `unresponsive`
  (`src-tauri/src/client/mod.rs`, `StatusFailureTracker`, `run_status_loop`). A lost connection is
  treated as the normal handoff back to the connect path, not as an exit
  (`src-tauri/src/client/mod.rs`, `run_supervisor_loop`), which is what re-spawns a daemon that died.
  **Young deaths are braked, old ones are not**: a connected phase shorter than `HEALTHY_CONNECTED`
  counts as a fast failure, and past `FAST_FAILURE_TOLERANCE` the supervisor waits a doubling
  `BACKOFF_BASE…MAX_BACKOFF` before the next attempt, resetting the count once a connection has
  lasted (`src-tauri/src/client/crash_loop.rs`, `CrashLoopBrake`). The loop sleep and the
  `SPAWN_ATTEMPTS` × `SPAWN_SLEEP` connect retries are unchanged underneath.
- *Daemon side.* The daemon never pings the client; it discovers the loss from the failing pipe. One
  cleanup path serves normal disconnects, read/write errors, shutdown and the idle exit: stop the
  request reader, close the outbound queue, flush final events, detach the connection, clear presence,
  and give back the permission card slots the device was holding
  (`crates/devboule-daemon/src/server/connection.rs`, `handle_client`). Then `client_disconnected`
  decrements the client count and arms the idle exit **only** when `clients == 0 && sessions == 0`
  (`crates/devboule-daemon/src/server/state.rs`, `client_disconnected`); the grace is
  `IDLE_SHUTDOWN_GRACE = 1 s` (`crates/devboule-daemon/src/lib.rs:175`) and the timer re-checks its
  generation under the lock, so a reconnect or a newly created session cancels it
  (`crates/devboule-daemon/src/server/lifecycle.rs`, `arm_idle_shutdown`).

**The two things that surprise people.**

1. **Every provider process lives in a Windows Job Object that kills it when the daemon exits.**
   `JobObject::new()` sets `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, `assign()` puts a child in it, and
   `Drop` closes the handle — which closes the job and terminates its members
   (`crates/devboule-daemon/src/process_tree.rs:37-59`, `:77-83`, `:151-155`). Every agent and
   terminal is assigned immediately after spawn: ACP
   `acp_client.rs`, `claude_client.rs`, `codex_client.rs` and `pi_client.rs` each call `assign`
   right after their own spawn, and a PTY assigns in `provider.rs`, `open_pty_session`; the daemon
   holds a job of its own too (`crates/devboule-daemon/src/server/state.rs`, `JobObject::new`). "No
   orphans" is therefore a kernel guarantee, not a cleanup step. The window between `spawn_command`
   and the assignment is known and accepted: closing it completely would need `CREATE_SUSPENDED`,
   which portable-pty does not expose (the comment in `provider.rs`, `open_pty_session`).

2. **Provider sessions are never re-spawned after a restart: the transcript survives, the process does
   not.** The processes were killed by 1. At the next journal open, rows still marked `live` are
   rewritten — `reaped = 1` → `ended`, otherwise `interrupted`
   (`crates/devboule-daemon/src/journal_schema.rs`, `open_connection`) — and `to_session()` maps those
   to `Ended` / `Recovered` with a transcript-integrity verdict (`journal.rs`, `to_session`); the
   replay emits `Recovered` and no exit event (`journal_replay.rs`, `replay_session`). The transcript
   is then replayed from the journal on attach (§3). Starting the provider again is an explicit, separate act: `resume` is
   the only path, and the gate admits **four** families — ACP, Claude, Codex and Pi
   (`crates/devboule-daemon/src/session.rs`, `resume_handle`; the per-family fact is
   `Provider::resumable`, `crates/devboule-daemon/src/provider.rs:240`, answered `true` by
   `AcpProvider`, `ClaudeProvider`, `CodexProvider` and `PiProvider` and `false` by Terminal).
   Claude resumes by handing the CLI back its own history: the daemon finds the
   transcript file for the provider's session id under the Claude home, refuses with a named error
   when it is not there, and passes `--resume` (`crates/devboule-daemon/src/claude_client.rs`,
   `find_claude_history`, `missing_history_error`, `push_resume_flag`). The id it
   builds that path from is validated against a closed alphabet first, because a session id that
   could contain a separator is a path that could leave its root. Codex resumes by
   `thread/resume { threadId }` on the app-server (`codex_client.rs`, the `ThreadRoad`
   handshake): the thread id is the handle the row already stores, the rollout under the human's
   Codex home is the conversation, and no journal history is re-sent. Pi resumes by
   `--session <id>` (`crates/devboule-daemon/src/pi_client.rs`, `spawn_process_resuming`): the id is the `sessionId`
   the handshake reports and the row already stores, pi resolves it against its own session
   directory for the workspace cwd (under the human's `~/.pi`), and no journal history rides
   the launch line either.

**The daemon can outlive the app.** Because the idle exit requires `sessions == 0`
(`crates/devboule-daemon/src/server/state.rs`, `client_disconnected`), a daemon whose app went away
without delivering `Shutdown` — a kill, a crash — keeps running with its live sessions, and the next
app start *rejoins* it instead of restarting it, since the connect is attempted first
(`crates/devboule-daemon/src/client.rs`, `connect_or_spawn_with`). The app's own exit path waits only
`JOIN_BUDGET` (1.5 s) for its shutdown frame (`src-tauri/src/client/mod.rs`, `DaemonBridge::shutdown`),
so "the frame was not delivered" is reachable. Whether that reattach is intended, or whether the app
should prove the daemon is gone before exiting, is an open question, and is not answered by any code
here.

Two smaller facts that belong to this section. There is **no updater in the tree** (`bundle.active =
false` in `src-tauri/tauri.conf.json`; no updater plugin in `src-tauri/Cargo.toml:21-33`), so
"restart the daemon for an app update" has no implementation; a daemon speaking another protocol
version is *refused* with a sentence telling the user to reinstall, not replaced
(`crates/devboule-protocol/src/handshake.rs:110-131`). And attachment folders left by sessions that
never closed are swept at daemon start (`crates/devboule-daemon/src/server/lifecycle.rs`,
`run_windows`).

**There is also an explicit way to kill the daemon**, and it refuses to shoot the wrong process.
`daemon_restart` in the app (`src-tauri/src/client/mod.rs`, `daemon_restart`) calls
`DaemonClient::restart_daemon` (`crates/devboule-daemon/src/client.rs`, `restart_daemon`), which
requires the server PID captured at handshake and re-checks the pipe's identity immediately before
terminating; a changed identity is an error rather than a risk, because a PID alone can be recycled
between the query and the kill (`crates/devboule-daemon/src/transport/windows_pipe.rs`,
`terminate_server_process_if_identity_matches`, `terminate_after_pipe_identity_check`).

## 3. Sessions and the journal

**What a session is.** One row the daemon owns: an id, an optional workspace, the directory the
process actually received, a kind, a title, the provider and the provider's own session id where they
exist, a state, and the two provenance fields — `origin` (local or a paired device, with the role it
was paired as) and `created_by` (the agent that created it, daemon-written and deliberately absent
from the create request so no client can claim a parent). The struct is
`crates/devboule-protocol/src/session.rs:207-322`, and its field comments are the contract: `cwd` is
display-only and never a filesystem key (`:210-216`), and an absent `origin` on the wire means `Local`
while a NULL `origin_kind` in the journal means `Unknown`, which grants a peer nothing (`:244-250`).

One field is an answer rather than a property: `resumable` (`:314-321`). The daemon decides whether a
row can be started again and says so on the wire; the app never re-derives it from kind, state or
columns, and `#[serde(default)]` makes a frame from an older daemon read back as `false`, so the
button stays hidden rather than offered on a guess. The single source is `session_resumable`
(`crates/devboule-daemon/src/provider.rs:319`), which answers `true` only when four things hold at
once: the session is not live, its family is resumable, and both the provider id and the provider's
own session id are present and non-empty.

**Kinds.** Five, serialised as `snake_case` (`crates/devboule-protocol/src/session.rs:17-23`):
`Terminal`, `Acp`, `Claude`, `Pi`, `Codex`. `is_agent()` is true for all but `Terminal` (`:34-43`).
Session ids are composed, not random: `s.<first 16 chars of the owner token>.<unique>`
(`crates/devboule-protocol/src/ids.rs:59-71`). That middle segment looks like an owner and is not one;
§8 records where that once mattered.

**State.** Four states (`crates/devboule-protocol/src/session.rs:460-485`): `Live { generation }`,
`Silent { generation }` (still running, no output for the silence threshold — never an exit),
`Ended { generation, code, integrity }` (the process exited while this daemon was alive) and
`Recovered { generation, integrity }` (the daemon that owned the process is gone). The silence
threshold is 300 s
(`crates/devboule-daemon/src/session_items.rs`, `SESSION_SILENCE_THRESHOLD`) and it produces a banner
event, not a kill (`crates/devboule-daemon/src/session_runtime.rs`, `mark_silent_if_due`).

`Recovered` used to mean "replay only". It no longer does: the variant's own doc now reads "replay
always; resume when the family is resumable" (`crates/devboule-protocol/src/session.rs:475`). Replay is
free — journal bytes, no
process — so it happens by itself; resume allocates a process and stays a deliberate act. Every state
carries a `generation`, and that is what lets a deferred intent belong to an *instance* of a session
rather than to its id: a row that died and came back is not the row the intent was taken against.

**The journal.** SQLite in WAL mode at `<runtime dir>/journal.db`, beside the lock file
(`crates/devboule-daemon/src/paths.rs:51-55`), schema version 14
(`crates/devboule-daemon/src/journal.rs:54`). One writer thread owns it
(`crates/devboule-daemon/src/journal.rs`, `open_with_limits`) with a bounded queue of 1024 commands
(`:63`) and a snapshot of the screen emulator every 64 KiB of output (`:66`). Rows are appended per
session sequence number, so a replay is ordered by the same counter the live events carry.

**Replay on attach.** A session that is not live is hydrated from the journal instead of being
spawned: `attach`/`attach_with_subscription` call `hydrate_transcript`
(`crates/devboule-daemon/src/session.rs`, `attach`, `attach_with_subscription`, `hydrate_transcript`),
and a live agent's replay is pulled one bounded journal page at a time by cursor, with the live
attachment queue left alone until the durable watermark is complete
(`crates/devboule-daemon/src/event_pull.rs`, `pull_events`). The roster that the app sees is the merge
of live entries and journal rows (`crates/devboule-daemon/src/session.rs`, `SessionRegistry::list`),
which is why a session whose process is gone is still listed, in `Recovered` state, with its
transcript available.

**`Recovered` is a conclusion, not a guess.** At journal open the daemon rewrites what it cannot
vouch for: a row still `live` with `reaped = 1` becomes `ended` — the child's exit was observed, the
daemon died during drain — and any other `live` row becomes `interrupted`
(`crates/devboule-daemon/src/journal_schema.rs`, `open_connection`). `to_session()` turns those into
`Ended` and `Recovered` respectively, each with a transcript-integrity verdict
(`crates/devboule-daemon/src/journal.rs`, `to_session`), and the
replay emits `SessionEvent::Recovered` in place of an exit event
(`crates/devboule-daemon/src/journal_replay.rs`, `replay_session`). `Recovered` therefore means "the
process was lost unobserved", which is a stronger and more honest claim than "the session ended".

**Three ways a session stops being on your screen, and they are not the same act.** `SessionDetach`
gives up one subscription and leaves everything running
(`crates/devboule-protocol/src/messages.rs`, `ClientMessage::SessionDetach`). `SessionStop` kills the
process and **keeps the session and its transcript** (`ClientMessage::SessionStop`); it carries a
`subscription_id` because
the caller must be an observer of the session it is stopping. `SessionClose` destroys the session
(`ClientMessage::SessionClose`), and it carries an idempotency key rather than a subscription, because closing twice must not
mean closing something else.

`SessionStop` kills the *tree*, not the root. After `killer.kill()` the daemon also calls
`job.terminate()` on the session's own Job Object (`crates/devboule-daemon/src/session.rs`, `stop`, for
the agent-facing one; `stop_with_subscription` for the subscribed one), because the session is being
preserved and its job therefore stays open — nothing else would reap the descendants a CLI left
behind. This mirrors what the on-OS-death handler already did
(`crates/devboule-daemon/src/session_spawn.rs`, the `set_on_os_death` closure). A killed-but-kept
session is the one the app calls *archive*: the row and its transcript survive, the process does not.

**The wire names who wrote a user message.** `UserMessageAuthor`
(`crates/devboule-protocol/src/session.rs:1371`) is `human`, `agent` or `creation`, and it is neither
the session's `origin` (where the session came from) nor the envelope's `role`/`from_agent` (the
delivery's connection facts): it names whose words the echo carries. `creation` is its own value even
when a human wrote the initial text, because that line is daemon-composed — standing instructions plus
preamble plus prompt. The app renders by this field and never re-derives authorship from the text;
absent predates the field and reads as `human`.

**`role:` is not the daemon's claim about itself.** The daemon composes that line from the *caller's*
peer record: a session created by a peer paired in the `Daemon` role writes `role: daemon` on the
agent-to-agent envelope it sends, around another agent's words
(`crates/devboule-daemon/src/session_envelopes.rs`, `agent_message_envelope`). So the app must
not read `role: daemon` as "the daemon is speaking" — it did, briefly, and rendered a relayed message
as a daemon notice while dropping the message text. The marker for a notice is a `kind:` line inside
the fixed header: all four notice builders write one, the agent-to-agent envelope deliberately writes
none, and `src/lib/agentDaemonNotice.ts` requires both. A header line counts only before the
`timestamp:` line, so nothing after it — that is, nothing a caller chose — can mint one.


**Retention.** Four limits, all configurable, with these defaults
(`crates/devboule-daemon/src/journal.rs:72-83`): 512 MiB per session, 8 GiB total, 10 000 sessions,
and an age limit of `0` — off. The app exposes them as `journal_usage`,
`journal_retention_get`/`_set` and `session_delete` (`src-tauri/src/lib.rs:68-71`). Deletion is
byte- and count-driven, not idle-driven (`crates/devboule-daemon/src/journal_retention.rs`,
`retain_global`), each deletion writes a
tombstone into `deleted_sessions` so a deliberate loss of history is on record (`delete_session`), and
the age rule skips ACP, Pi and Codex sessions (`retain_global`). The global scan is amortised rather
than run per write: it runs at most once per MiB of journal written
(`crates/devboule-daemon/src/journal_retention.rs`, `RETENTION_GLOBAL_SWEEP_BYTES`).

**Two details worth knowing before you touch any of this.** Child liveness is not derived from pipe
EOF: a sweeper every 2 s duplicates the process handle and waits non-blockingly, so a provider killed
from Task Manager is noticed even while its descendants still hold the pipe open
(`crates/devboule-daemon/src/session_items.rs`, `SESSION_OS_SWEEP_INTERVAL`;
`crates/devboule-daemon/src/session_spawn.rs`, `spawn_os_liveness_sweeper`, `sweep_os_liveness`). And
the per-attachment output budgets
(`PENDING_OUTPUT_BUDGET_BYTES`, `PENDING_OUTPUT_BUDGET_FRAMES`, `COALESCE_*`,
`crates/devboule-daemon/src/session_items.rs`) are what keep a chatty agent from turning into
unbounded memory or an unbounded journal.

## 4. Providers

**The catalog** is a static table plus a `PATH` scan. A `KnownAgent` row carries the id, its aliases,
and up to four launch shapes — `acp_args`, `stream_json_args`, `rpc_args`, `app_server_args` — each
`Option`, so "this agent speaks that dialect" is a fact in one row
(`crates/devboule-daemon/src/provider_catalog.rs`, `KnownAgent`). `KNOWN_AGENTS` (same file) holds
claude, codex, grok, pi, qwen and the rest; two debug-only rows are compiled in under
`debug_assertions` so a test can reach "provider not installed" without depending on the machine (the
`#[cfg(debug_assertions)]` block in the same file). The header records what was borrowed: the alias
table and the
executable-file check are adapted from herdr under Apache-2.0; the launch resolver — PATHEXT
following, then unwrapping an npm `.cmd` shim to `node` plus the package script so `CreateProcess`
never goes through `cmd.exe` — is Devboule's own (`provider_catalog.rs:1-16`).

**Which providers are native, and which speak ACP.** Native adapters exist for exactly three, and the
kind enum mirrors them (`crates/devboule-protocol/src/session.rs:17-23`):

| Provider | Dialect | Catalog evidence | Adapter |
| --- | --- | --- | --- |
| Claude | `stream-json` | `CLAUDE_STREAM_JSON_ARGS` | `claude_client.rs`, `claude_view.rs` |
| Codex | app-server | `app_server_args: Some(&["app-server"])` in the codex row of `KNOWN_AGENTS` | `codex_client.rs`, `codex_view.rs` |
| pi | RPC | `rpc_args: Some(&["--mode", "rpc"])` in the pi row of `KNOWN_AGENTS` | `pi_client.rs`, `pi_view.rs` |
| everything else | ACP | the grok and qwen rows of `KNOWN_AGENTS` | `acp_client.rs`, `acp_host.rs`, `acp_view.rs` |

The decision is made per installed agent by `chat_protocol`
(`crates/devboule-daemon/src/provider_catalog.rs`), which returns `codex-app-server`, `acp`,
`stream-json` or `pi-rpc`
and `None` for a CLI that is installed but not chat-capable, with the explicit tie-break comment
"An agent with both launches is offered as ACP: ACP is the road, stream-json the exception"
(the doc comment above `chat_protocol`). Among ACP agents the default is a separate, explicit
preference order — grok, then qwen,
then gemini — with the reason recorded as a measurement
(`crates/devboule-daemon/src/provider_catalog.rs`, `ACP_PREFERENCE`, `first_acp_available`).

Three registry wrappers are covered by a better native provider and are therefore visible in Settings
but not offered in the workspace picker: `claude-acp`, `codex-acp`, `pi-acp`
(`crates/devboule-daemon/src/provider_catalog.rs`, `REGISTRY_NATIVE_CHAT_COVERAGE`). The reason for
`pi-acp` is the useful one to know: the wrapper
speaks
ACP and reports models, but does not emit `session/request_permission` for native tools, and "a
measured write completed with zero permission requests" (the doc comment above
`REGISTRY_NATIVE_CHAT_COVERAGE`). On the wire this is
`ProviderInfo.pickable` (`crates/devboule-protocol/src/messages.rs`, `ProviderInfo`).

**Where modes and features come from.** Not from the catalog. A provider's modes are whatever the
provider declares at run time: `SessionModeStateView` carries a current mode id and an
`available_modes` list (`crates/devboule-protocol/src/session.rs`, `SessionModeStateView`), populated
from the ACP
session state (`acp_client.rs`, `remember_mode`) or from Claude's control protocol
(`crates/devboule-daemon/src/claude_view.rs`, `mode_state`), and a mode change is an RPC to the
provider
(`acp_client.rs`, `request_set_mode`, `set_mode`; `claude_client.rs`, `set_mode`). The catalog's part
is narrower and only
concerns *created* agents: the preset cell names the mode a child is started in (§7), and the
classifier that decides which mode ids mean "answers in place of the human" is per family —
`mode_is_unattended` (`crates/devboule-daemon/src/provider_catalog.rs`) — because Codex's `auto` and
Claude's `auto` are
one word apart and mean opposite things (the comment above `mode_is_unattended`).

**The inventory on the wire** is `ProviderInfo` (`crates/devboule-protocol/src/messages.rs`):
executable, `acp_available`, the chat `protocol`, how the row was obtained
(`user-binary` / `npx-wrapper`, `crates/devboule-daemon/src/provider_catalog.rs`,
`ProviderOrigin::as_wire`), the registry launch arguments,
`pickable`, and installed-versus-latest versions. Authentication is deliberately never probed: an
executable on `PATH` is "installed, status unknown", and the status enum has exactly one variant,
`Unknown` (`messages.rs`, `ProviderInfo`; `provider_catalog.rs`, `AuthenticationStatus`). Settings can
refresh the catalog
and install or update an npm-supplied wrapper (`crates/devboule-daemon/src/provider_update.rs`,
`NpmInstallRunner`; the app's commands are `src-tauri/src/lib.rs:95-97`'s `providers_list`,
`providers_refresh` and `provider_update`).

## 5. Permissions

**One broker, two providers.** The permission broker is shared by ACP and Claude stream-json sessions
(`crates/devboule-daemon/src/permission_broker.rs:1`). A provider's ask becomes an ordinary wire
event: `SessionEvent::PermissionRequest` (`crates/devboule-protocol/src/session.rs`, the
`PermissionRequest` variant) carrying the
tool call id, a title, the agent's own description of the command, the options the provider offered,
and an origin stamp. The daemon publishes it on the attached subscription and the app renders it —
`src/components/PermissionCard.tsx`, mounted at `src/features/workspace/Workspace.tsx`
(`WorkspacePermissionCard`) and `src/features/design/DesignSurface.tsx` (the `PermissionCard`
call). The answer returns as
`ClientMessage::SessionPermissionRespond`, which the daemon accepts only from a client that negotiated
the `typed_permissions` capability (`crates/devboule-daemon/src/server/dispatch.rs`,
`typed_permissions_ok`; `crates/devboule-daemon/src/server/connection.rs`, where the capability is
read off the handshake).

**The card is bounded, and the bound is per device.** At most 32 undecided ACP cards exist at once
(`crates/devboule-daemon/src/permission_broker.rs`, `MAX_PENDING_ACP_PERMISSIONS`), and a paired
device may hold at most 3
(`MAX_PENDING_FOR_PEER`, same file). That count is deliberately daemon-wide rather than per session or
per broker: the slot is
reserved *before* the card is inserted, so two cards cannot both see the last slot free, and a
per-session counter once gave a device three cards per session (the comments on
`MAX_PENDING_ACP_PERMISSIONS` and `MAX_PENDING_FOR_PEER`). A slot is
released when the card is decided, when it is cancelled, and when the connection dies — the
disconnect path hands back whatever the device still held (`permission_broker.rs`,
`release_card_slot`, `release_peer_card`; `crates/devboule-daemon/src/server/connection.rs`,
`handle_client`).

**Provenance is on the card, in its own element.** A request raised for, or by, a paired device is
stamped so the card can render a `peer` line; the app's contract says the request's own text must
never be able to imitate it, and that a `local` origin renders no line while an absent one renders
`Origin: unknown` (`src/types/ipc.ts`, the `origin` field of `PermissionRequest`; the daemon side is
the `origin` stamp on the event, `crates/devboule-daemon/src/permission_broker.rs`, `take`).

**The per-provider tool policy** decides which of the daemon's MCP broker tools a provider's sessions
are served (`crates/devboule-daemon/src/tool_policy.rs:1-2`), and it is the mechanism a person uses to
turn agent-to-agent capability off for one provider without touching the others. It is one JSON file
beside the journal, `tool-policies.json` (`tool_policy.rs`, `POLICY_FILE`), written the way the MCP
config is
written: a create-new temp file, a current-user-only DACL applied before its first byte, then a rename
over the target, so a crash leaves either the old policy or the new one and the file is never briefly
readable by another user (the module header, `:8-13`, and `write_policies`). A file that will not
parse is quarantined under a
nonce-bearing name rather than deleted (`crates/devboule-daemon/src/tool_policy.rs`, `quarantine`).
**The read cadence is the point:** the broker
reads the store on every `tools/list` and on every `tools/call`, so a toggle takes effect on the
provider's next call rather than at the next session (`tool_policy.rs:3-7`; the enforcement sites are
`mcp_broker.rs`, `enabled_tool_list` and `tool_call_refusal`). A policy is per device and is not
propagated to paired peers, because what
this machine hands to an agent is a local decision (`tool_policy.rs:15-17`). One name cannot be
disabled: the roster tool, since an agent that cannot list its siblings cannot be steered at all
(`crates/devboule-daemon/src/provider_catalog.rs`, `MCP_ROSTER_TOOL`, and the comment above it). The
app writes the file through `tool_policy_get` / `tool_policy_set`
(`src-tauri/src/lib.rs:88-89`, `src-tauri/src/backend/tool_policy.rs`).

## 6. Peers

**The transport is the tailnet, and only the tailnet.** One listener per tailnet address, and every
byte on it — handshake included — inside a Noise session
(`crates/devboule-daemon/src/peer_transport.rs:1-6`). The default port is 47831 (`:78`),
overridable by `DEVBOULE_PEER_PORT` (`:79`). Tailscale is reached through its LocalAPI (`whois` and
`status`) over the platform's local transport: on Windows that is the `tailscaled` named pipe, opened
*without* explicit SQOS flags, because the IPN server requires the default impersonation level and an
explicit `Identification` makes it answer 401 (a measured result, recorded in the header at
`crates/devboule-daemon/src/tailscale_localapi.rs:1-17`); the client is a hand-written bounded
HTTP/1.1 GET with a 2 s budget, because the daemon is std threads and blocking I/O with no tokio
(`tailscale_localapi.rs:3-14`, `:41`). Nothing here logs the response, since `LoginName` and
`DisplayName` are PII (`:16-17`).

The listener is **best-effort and never blocks start-up**: no Tailscale, no tailnet address or a
missing key leaves the daemon local-only and says why in `Status.remote`
(`crates/devboule-daemon/src/server/lifecycle.rs`, `try_start_remote_listener`). It is also not a
one-shot attempt — the same
function is retried, so a user who starts Tailscale and shows a code again gets a listener rather than
the same refusal until the daemon restarts (the same function, reached from `pairing_address`).

**This device's identity** is a random `device_id`, a Noise static keypair, and a display name
(`crates/devboule-daemon/src/device_identity.rs:1-3`). The id is what peers pin; the key is the
credential bound to it, so a key rotation can keep the id (`:4-5`). The private half never touches
`device.json` or the journal — it lives in the secret store — and its absence is a distinct state,
`RemoteState::KeyMissing`, rather than a reason to mint a new key and silently orphan every pairing
(`:6-8`, the `RemoteState::KeyMissing` variant). `device.json` holds the id, the public key and the
name (the `DeviceFile` struct;
`crates/devboule-daemon/src/paths.rs:14-17`); a fingerprint is derived for display
(`crates/devboule-daemon/src/device_identity.rs`, `key_fingerprint`)
and
display names are validated and bounded to 64 characters (`MAX_DISPLAY_NAME_CHARS`,
`validate_display_name`).

**Pairing** is a short code, a PAKE, and one mutually authenticated exchange
(`crates/devboule-daemon/src/pairing.rs:1-2`). The device that *displays* the code is the responder;
the one that *types* it is the initiator (`:3-4`). The code is 8 characters from a 32-symbol alphabet
with `0`, `1`, `I` and `O` removed, so it can be read off a screen and typed correctly
(`crates/devboule-daemon/src/pairing.rs`, `CODE_ALPHABET`); it
lives 300 s (`CODE_LIFETIME`), a confirmation must be answered within 60 s (`CONFIRM_WINDOW`), at
most 2 pairings are
parked at once (`MAX_PENDING_PAIRINGS`), and 3 wrong codes from one source or 12 in total kill the
code (`WRONG_PER_SOURCE`, `WRONG_TOTAL`, all five in the same file). The
wire is SPAKE2, then HKDF-SHA256 of the PAKE key into a 32-byte PSK, then Noise `XXpsk3` with each
side's long-term static and the prologue `devboule-pair-v1`; inside Noise each side sends
`{device_id, display_name, role, public_key}` and the responder answers `{accepted, reason}`
(`:5-12`). The PAKE is what makes an 8-character code worth its 40 bits: a passive eavesdropper learns
nothing, and an active attacker gets exactly one guess per attempt, each counted against the lockout
(`:20-23`). The exchange is deliberately not a blocked thread: a pairing that needs the local user's
answer parks the socket with a deadline and waits on a channel
(the `# Not a blocked thread` header section).

**How a peer's identity is established on every later connection** — six steps, in this order
(`peer_transport.rs:17-31`):

1. caps, total and per source and per source per minute;
2. `pre_noise_filter` **before any read**: a paired address goes to Noise, a pairing candidate only
   while a code is active, everything else is closed without a read;
3. a pairing candidate is peeked for the `DBP1` magic under a 2 s timeout;
4. a Noise `XX` responder under one 10 s wall-clock deadline, after which **the remote static key must
   match a pinned, non-revoked `peers` row**;
5. `whois` must agree with the binding recorded at pairing-time;
6. only then `handle_client`, with the remote identity.

Step 4 is the authentication; 1–3 are cheap pre-filters, and a source that fails 4 still consumed a
handshake slot, which is why the slot budgets are separate from the connection cap
(`peer_transport.rs:29-31`). The pattern and prologue are constants (`:46-48`), and the handshake
deadline is `HANDSHAKE_DEADLINE = 10 s` (`:71`).

**What a capability is.** A capability names an *act* a paired device may ask for — or, for the
sixth, the surface no act-name covers. The list is one
constant on the wire: `PEER_CAPS` is six names — `view`, `send`, `answer_permissions`,
`create_sessions`, `roster` and `admin` (`crates/devboule-protocol/src/messages.rs`, `PEER_CAPS`) —
and `PEER_DEFAULT_CAPS` is the same six, so a device is born holding everything and a person narrows
it per device. The default is written once, at the pairing that creates the peer row (`pairing.rs`,
its only production use), so a device paired before 2026-09-21 keeps the narrower set it was paired
with: the panel draws its `admin` switch off, and turning it on there is the grant. That is the
owner's decision of 2026-09-21: it revoked the old global deny list, under
which anything no capability named was refused to every peer. `validate_caps` is what still refuses to
leave a `Client` without `view`. A capability is deliberately not a scope: which sessions an
allowed request reaches is decided elsewhere, by the owner projection in `server.rs` and the origin
branch of `check_user_owner` (`peer_policy.rs:14-18`). The gate itself is a closed match with **no
`_` arm** over every `ClientMessage` variant, so adding a variant without deciding its peer policy is
a compile error (`peer_policy.rs:1-8`, `peer_allows`). Consequences of that design, all in the same
file:
`view` is the one capability `validate_caps` refuses to strip (`crates/devboule-daemon/src/pairing.rs`,
`validate_caps`); `admin` is the one that opens everything the act-named five do not, from `Status`
to `Shutdown` (`peer_policy.rs`, `CAP_ADMIN`); the five permission-model frames stay refused *whatever
a peer holds* (below); and the role a device was paired as does not
decide anything
here — the capability set does (the `peer_allows` doc comment).

A peer's set is stored per device in the journal's `peers` table
(`crates/devboule-daemon/src/journal.rs`, `peers_list`, `upsert_peer`, `set_peer_caps`,
`revoke_peer`). Revocation holds at the last place a
frame could still leave: a connection whose caps were dropped or that was just revoked gets no closing
flush at all (`crates/devboule-daemon/src/server/connection.rs`, `handle_client`, the `revoked` flag
it passes to `flush_final_events`).

**In the app**, all of this is the Devices panel in Settings
(`src/features/settings/DevicesPanel.tsx`, the module doc above `DevicesPanel`): this device's identity
and fingerprint, the two
pairing directions — show a code, or type one — the confirmations this device still owes, and the
paired list with online/offline, role, per-device capability toggles and an inline revoke (`showCode`,
`copyFingerprint`, and the pending-confirmation block in `DevicesPanel`). The daemon owns every fact;
the panel never derives a device id from a name, never
invents a reason string, and never answers a permission or a pairing on the far side's behalf
(the same module doc). It polls `devices_list` every 2 s, and every 1 s while a code is on screen or a
confirmation is pending, because those are the states that change under the user's eyes
(`src/features/settings/DevicesPanel.tsx`, `POLL_IDLE_MS`, `POLL_ACTIVE_MS`).
The capability labels are exactly the wire names (`CAP_ORDER`, `CAP_LABELS`, same file), and the two
roles are
described in the
panel as "a phone or laptop of yours that views and steers this device" and "another devboule that
this one may talk to as a machine" (`ROLE_OPTIONS`).

**Plainly: what a paired device can and cannot do.**

- *Can* — with `view`: list sessions, attach to one, and list devices
  (`ClientMessage::SessionsList`, `ClientMessage::SessionAttach`, `ClientMessage::DevicesList` in
  `peer_policy.rs`). With `send`: send a prompt, steer a running turn, send an
  agent message, deposit an attachment, and change a session's mode (`ClientMessage::SessionSend`,
  `ClientMessage::AgentMessageSend`, `ClientMessage::SessionDeposit`,
  `ClientMessage::SessionSetMode`). With
  `answer_permissions`: answer permission cards (`ClientMessage::SessionPermissionRespond`) — under the
  per-device budget of three from §5. With
  `create_sessions`: create sessions (`ClientMessage::SessionCreate`). With `roster`: list the peer's
  own live agents (`ClientMessage::PeerAgentsList`). Every one of these is a per-device toggle the
  user can revoke.
- *Can, with `admin`* — everything else this machine's app can ask, because a paired device is a full
  client: `Status` and `DaemonDiagnostics`, `Shutdown`, the session verbs outside the view/send pair
  (`SessionClaim`, `SessionResume`, `SessionReportAgent`, `SessionDetach`, `SessionClose`,
  `SessionStop`, `SessionResize`, `SessionInterrupt`, `SessionSetModel`), the watch set
  (`SessionsWatch`/`Unwatch`/`Presence`), the journal (`JournalUsage`, `JournalRetentionGet`,
  `JournalRetentionSet`), the deposited-bytes read (`SessionAttachmentRead`), the settings stores
  (`ToolPolicyGet`/`Set`, `AgentProfilesGet`/`Set`, `DelegationGet`/`Set`, `ProviderVocabularyGet`),
  projects and workspaces (`ProjectsList`, `ProjectAdd`, `WorkspacesList`, `WorkspaceCreate`,
  `WorkspaceDelete`), providers (`ProvidersList`, `ProvidersRefresh`, `ProviderUpdate`), `Invoke`, and
  the MCP bridge's destructive tools. The bridge draws the same line: every tool's wire act is judged
  by the one table (`peer_policy.rs`, `mcp_tool_wire`), so a device holding `admin` reaches every
  served tool — `devboule_stop_agent`, `devboule_close_agent`, the three project-graph tools, the
  model half of `devboule_set_agent_profile` — and one without it is refused those with that
  capability's name (`provider_catalog.rs`, `MCP_BROKER_TOOLS`).
- *Cannot* — the three acts that decide **who may enter this machine**: start, complete or confirm a
  pairing (`ClientMessage::PairingStart`, `PairingComplete`, `PairingConfirm`), change a device's
  capability set (`PeerSetCaps`) and revoke a device (`PeerRevoke`). Those five frames are refused to
  every capability set there is, `admin` included — the whole remaining `Deny` in `peer_allows`. The
  reason is not "a paired device is not the app": it is that the trusted set is the human's. A peer
  that could pair another device could hand out the permissions it holds, and one that could change
  caps or revoke could rewrite the set that decides who is in. The owner's decision of 2026-09-21
  opens everything else and leaves these five local; opening them is a decision to take deliberately,
  not a consequence of granting `admin`. A peer also does
  not inherit anything local: a
  tool policy is this machine's own decision and is never propagated to a peer
  (`tool_policy.rs:15-17`).
- Two refusals sit outside the capability table and are unchanged by that decision, because they are
  about *what a session does* rather than who may ask: a frame from a paired device that *carries* an
  attachment is refused (`crates/devboule-daemon/src/server/peer_gate.rs`,
  `peer_refusal_before_mode`, temporary until the deposit budget's caller lands), and a paired device
  never drives a session into a mode that would run without asking this machine's user
  (`peer_gate.rs`, `peer_mode_refusal_for_conn`, §8b A5/R3).
- *Scope* is separate from permission: an allowed request still has to reach a session, and which
  sessions a peer can reach is decided by the owner projection plus the session's recorded `origin`
  (`peer_policy.rs:14-16`; the `origin` contract is
  `crates/devboule-protocol/src/session.rs:45-53`). A `Daemon`-role peer's ownership scope reads that
  origin rather than re-deriving it.

## 7. Agents that create agents

**The channel is the MCP broker.** Each live session that is allowed one gets a bearer token and a
loopback HTTP MCP endpoint (`/mcp`), served by the daemon
(`crates/devboule-daemon/src/mcp_broker.rs:1-7`, `MCP_PATH`, `McpLaunchConfig`); the config is written
for the
provider, never read from the client (the `mcp_launch` doc on `McpLaunchConfig`). Registration is the
provider's own answer, one answer for every site that asks: `Provider::hosts_mcp`
(`crates/devboule-daemon/src/provider.rs:217`) is `true` for **four** of the five families —
`AcpProvider`, `ClaudeProvider`, `CodexProvider` and `PiProvider` — and `false` for `Terminal`, and
`hosts_mcp(kind)` (`crates/devboule-daemon/src/mcp_broker.rs:139`) is the single shim the gates read,
so a pi or codex agent does host the daemon's MCP tools today. What stays narrower on purpose is the
*first prompt*: `mcp_gates_first_prompt` (`mcp_broker.rs:154`) is ACP and Claude only, because a
carrier that is best-effort and slow (Codex measured ~7.4 s against a dead broker) must not make an
outage of the broker an outage of a healthy child. The catalog keeps the cells anyway, "so adding a
non-ACP transport does not silently change a decision" (the comment above the preset cells in
`crates/devboule-daemon/src/provider_catalog.rs`).

**What a Codex child inherits by running in the human's home.** A Codex session keeps the human's
real `~/.codex`: the daemon deliberately mints **no** per-session `CODEX_HOME`, because a redirected
home carries no `auth.json` (measured: every turn answers 401 "Missing bearer or basic
authentication") and the rollout this family resumes lives under the home that wrote it
(`crates/devboule-daemon/src/codex_client.rs`, the `mcp_launch` doc and `spawn_process`). Three
consequences follow, and they are the operational price of resumable Codex threads:

- MCP servers the user has already configured in that home stay available to the Devboule session,
  beside the Devboule override the launch line adds as `-c mcp_servers.<name>.url=...`.
- The provider's history and rollout are **not** isolated per Devboule session: they live where the
  human's own Codex runs live.
- Codex **can** rewrite `~/.codex/auth.json` during authentication operations — that is Codex
  writing to its own home, not the daemon writing to it, and it is not something that happens on
  every session: the daemon never touches that file itself.

And the older rows do not come back. A Codex row created while the retired per-session home existed
is still **re-readable** — the journal holds its transcript — but **not resumable**: its rollout sits
in a `devboule-codex-home-<…>` tree, and the daemon's own startup sweep deletes every one of those on
the next start (`crates/devboule-daemon/src/mcp_broker.rs`, `cleanup_stale_configs`). There is no
migration, so for those rows the history shows and the resume button cannot honestly be offered.

**Eleven tools**, in `tools/list` order, from one table that the Settings panel reads too, so the panel
and the wire cannot disagree (`crates/devboule-daemon/src/provider_catalog.rs`, `MCP_BROKER_TOOLS`):

| Tool | Names | Disableable by policy? |
| --- | --- | --- |
| `devboule_list_agents` | the roster: siblings, their state, their creator, their depth | **No** — an agent that cannot list its siblings cannot be steered at all (the `MCP_ROSTER_TOOL` comment, and `is_tool_enabled`) |
| `devboule_list_devices` | the paired devices this machine knows: id, name, role, online — answered from the daemon's own `peers` rows, never dialled | Yes |
| `devboule_list_peer_agents` | the agents running right now on one paired device, by `deviceId`: one dial per call | Yes |
| `devboule_list_profiles` | the ticked profiles agents may create from | **No** — without it creation is undiscoverable (`is_tool_enabled` is the other always-on name) |
| `devboule_send_message` | send to one live session | Yes |
| `devboule_create_agent` | create a child from a ticked profile and give it an initial prompt | Yes, deliberately (`MCP_BROKER_TOOLS`) |
| `devboule_set_agent_profile` | move a child onto a ticked profile | Yes |
| `devboule_answer_permission` | answer one delegated permission card | Yes |
| `devboule_agent_activity` | one agent's derived activity plus recent kinds, metadata only | Yes |
| `devboule_stop_agent` | kill one own child's process tree, keeping its row and transcript | Yes |
| `devboule_close_agent` | end one own child's session; history keeps the transcript | Yes |

**The creation call.** The caller is the session whose bearer authenticated the connection — "there is
no `from_session` parameter to lie about" (the doc on `create_agent`, `crates/devboule-daemon/src/mcp_broker.rs`).
The order is fixed and stated
in the code: resolve the **profile** the caller named from the profiles the human ticked, read now
(the provider, the model, the mode, the features and the tool overlay come from there and never from
the caller), reserve the budget, raise the creation card once per creator session, create
through the ordinary `SessionCreate` path with the creator's own origin and owner, and answer
`{sessionId, taskId, contextId, displayName, state: "submitted"}`
(`crates/devboule-daemon/src/mcp_broker.rs`, `create_agent`, `resolve_profile`, `created_result`).
Details that matter:

- **Depth comes from the registration, not the request**: a session at depth 2 may not create
  whatever it says (`create_agent`; the cap is `MAX_AGENT_DEPTH` in
  `crates/devboule-daemon/src/session_registry_state.rs`).
- **The child runs where the caller does**: a workspace must be the caller's own or the call is
  refused, and the resolved working directory must stay inside it, before anything is spent
  (`create_agent`, `resolve_child_cwd`).
- **A retry creates nothing.** An MCP `tools/call` has no idempotency parameter, so a retry is
  identified by the frame's own id plus a fingerprint of everything the answer depends on; a key
  reused with a different payload is a conflict, not a retry, and the key is held *before* the store is
  read so a second in-flight call is refused without spending a slot (`create_agent`,
  `creation_retry_key`, `hold_creation_key`).
- **The input schema is closed on purpose** (`additionalProperties: false`), and there is deliberately
  no `mode` parameter: the profile chooses the mode (`agent_create_input_schema`, and the
  `additionalProperties: false` schema block in `mcp_broker.rs`).

**Profiles are the table a creation names; presets are the retired one.** `AGENT_PRESETS`
(`crates/devboule-daemon/src/provider_catalog.rs`) still holds exactly `worker` and `design`, and an
unknown preset is refused by name rather than resolved by default — but nothing on the creation path
reads it any more: the tool's only identity parameter is `profile`, resolved through `resolve_profile`
against the store the human ticked, and `AGENT_PRESETS` is exercised by the catalog's own tests. Each
legacy preset was a list of provider cells — pi/`ask`, codex/`auto`, claude/`default`, and
`default` for the ACP providers the catalog publishes. `design` used the same
modes with the design tool overlay. Worker carried one preamble, and it is worth reading: *"You were
created by another agent; report your result in your final message."* (the `PRESET_WORKER_CELLS`
preamble) — it says nothing
about files on purpose, because the finish hook deposits the message itself and a preamble that told
the child to write files would be asking for the same artifact twice, in the one place the child can
put it out of reach. A design creation has no frozen preamble at all: the app composes it per
request and it travels in `initialPrompt` (`agent_create_input_schema`).

**The caps.** Four limits, all daemon constants, all enforced at admission before the card exists:

| Cap | Value | Where |
| --- | --- | --- |
| live children per creator | 3 | `session_registry_state.rs`, `MAX_LIVE_CHILDREN_PER_CREATOR`; enforced in `session_children.rs`, `reserve_agent_creation` |
| creations per one-hour window | 10 | `session_registry_state.rs`, `MAX_CREATIONS_PER_WINDOW` (`CREATION_WINDOW` is the hour); enforced in `reserve_agent_creation` |
| live agent sessions, daemon-wide | 8 | `session_registry_state.rs`, `MAX_LIVE_AGENT_SESSIONS`; enforced in `reserve_agent_creation` |
| nesting depth | 2 | `session_registry_state.rs`, `MAX_AGENT_DEPTH`; enforced in `reserve_agent_creation` |

The slot is taken **before** the card is raised, so two creations racing on one session cannot both see
the third slot free, and the in-flight reservations are keyed by the child session id they reserved —
so releasing one can never subtract a neighbour's creation (`session_registry_state.rs`,
`AgentCreationTicket`; `session_children.rs`, `reserve_agent_creation`, `release_agent_creation`,
`note_pending_child`). The numbers the
card shows are read from that same reservation (the `max_*` fields `reserve_agent_creation` copies into
the request), which is the point: a person decides
against the budget that is actually about to be spent rather than against a configuration claim
(`crates/devboule-protocol/src/session.rs`, `CreateAgentCaps`). Depth is judged on the child's own
depth
(`reserve_agent_creation`), and the once-per-creator-session accept lives in the same locked entry
(`session_children.rs`, `accept_agent_creation`).

**The creation card is an ordinary permission card.** It is a `SessionEvent::PermissionRequest` with
the `create_agent` payload filled in — the same pending entry, the same allow/deny frame, the same
origin stamp and the same per-device budget as any other card — because a second variant would have to
re-implement all of that (`crates/devboule-daemon/src/mcp_broker.rs`, `creation_card`; the payload type
is `CreateAgentCard` in
`crates/devboule-protocol/src/session.rs:598`). The daemon composes the title
`Create an agent: <title> (<profile>)`, a description sentence that states the provider, model, mode,
thinking, features, the auto-accept judgement, the labels and every cap as `n of m`, and two options,
"Create once" and "Deny" (`creation_card`). The
caps are in the text *and* in the payload, both from one reservation, "the text is what a person reads,
the payload is what a surface renders" (the `creation_card` comment; the payload type is
`CreateAgentCaps` in the protocol crate).

One honest gap to know about: the typed `createAgent` payload exists on the wire and in the TypeScript
types (`src/types/ipc.ts`, `CreateAgentCard` and `CreateAgentCaps`) but **no component reads it** — the
card renders through the
ordinary `PermissionCard`, so what a person actually sees is the daemon's description sentence and the
card's own allow/deny buttons.

**The agent profile store, and the table it replaced.** A creation names a **profile** from the list
`crates/devboule-daemon/src/agent_profiles.rs` holds — the ordered list a human ticked, plus one block
of standing instructions, described in that module's own header. It is one JSON document beside the
journal, written the way the tool policy file is written — a create-new temp file, a current-user-only
DACL applied on Windows **before its first byte**, then a rename over the target (the same module
header). A crash therefore leaves the old list or the new one and never half of either, and a document
that decides what an agent may do is never briefly readable by another user.

It is read **at the moment it is used** — at the moment a creation resolves a profile, and at the
moment the standing instructions apply (the module header) — so a human's edit takes effect on the
next creation rather than on the next session. Both are capped: 8 KiB of standing instructions
(`MAX_STANDING_INSTRUCTIONS_BYTES` in that file), well under the frame cap.

The failure direction is the part worth keeping in mind. A document that cannot be read, parsed or
admitted is **quarantined** exactly as a corrupt tool-policy file is — renamed aside first, never
deleted (`crates/devboule-daemon/src/agent_profiles.rs:144`, the doc on `load`, and `load` itself) —
and the store then holds an **empty list and empty standing instructions**, never the last good copy
(`crates/devboule-daemon/src/agent_profiles.rs`, the `load` doc). An unreadable file means "no agent
may be created and no standing instructions", not "carry on with what we had": see §5, where the
permission dimension is closed on purpose. A profile's **id** is what a running child records, so a
rename cannot move a running child onto a different profile
(`crates/devboule-daemon/src/agent_profiles.rs`, the comment in `check_profile`).

**The finish report and the deposit.** When a child finishes, the daemon reports to the creator
(`crates/devboule-daemon/src/session_children.rs`, `report_child_finish`, `report_child_finish_with`),
publishes `ChildFinished` (`crates/devboule-daemon/src/session_runtime.rs`, `publish_child_finished`;
the envelope, the state derived from the child provider's own stop reason, and the two functions that
derive it are all in `crates/devboule-daemon/src/session_envelopes.rs`:
`agent_finished_envelope`, `child_finish_state`, `stop_reason_state`) and deposits the
child's whole last `AgentMessage` into the **creator's** folder
(`crates/devboule-daemon/src/session_messaging.rs`, `deposit_child_message`) as markdown named
`agent-finished.md`, refusing anything over the artifact cap (`MAX_AGENT_ARTIFACT_BYTES` in
`crates/devboule-daemon/src/session_registry_state.rs`). The artifact is
named by reference and never by path: `devboule-attachment:<sessionId>/<digest>` (the reference
`deposit_child_message` builds), which
is exactly the reference type the wire carries (`FinishArtifact { artifact_id, parts }`,
`crates/devboule-protocol/src/session.rs:669-674`, its parts at `:644-665`; the event is
`SessionEvent::ChildFinished`). The Design surface records a
finished child in history by that reference rather than a second copy
(`src/features/design/childFinishedHistory.ts`, the module doc above
`recordChildFinishedHistory`).

**Supervision: what a creator can see, and one notice it is sent.** `devboule_agent_activity` answers
for one live agent of the same owner — the roster's scope, because reading is not destroying and the
roster already lists them. The answer is metadata only: the derived headline (`working` / `idle` /
`blocked` / `unknown`), the hook's own last state beside it, the idle age, and a bounded tail of
recent event *kinds* with their sequence numbers and timestamps. No transcript text, ever. The
headline is derived from facts the daemon holds — liveness, a running turn, a parked permission card
— and never merged with the hook map, which keeps its own `seq` discipline
(`crates/devboule-daemon/src/agent_activity.rs`). The recent tail is an in-memory ring of 64 marks,
not a journal query: the read takes two locks and touches no rows. `devboule_agent_activity` is served
by `agent_activity` in `crates/devboule-daemon/src/session_children.rs`.

A child that has been *working* with nothing published for `CHILD_QUIET_AFTER` (20 minutes) earns its
creator one envelope, `kind: agent_quiet`, once per quiet spell; movement re-arms it. It is a notice
and never an action: the child's turn, its cards and its brakes are untouched, and the test asserts
exactly that. This is also why it is delivered without steering. A steer's *refusal* path is an
interrupt, and an ACP creator cannot take a steer — nothing in `crates/devboule-daemon/src/acp_client.rs`
overrides `clone_steerer`, whose trait declaration and default live in
`crates/devboule-daemon/src/session_items.rs` and which Claude, Codex and Pi all override —
so routing a routine notice through the steer path would have cancelled the creator's own turn and
dropped its pending cards — the most destructive act in the system, on the wrong session, every twenty
minutes. `deliver_notice_to_creator` exists so that cannot be reached by passing the wrong boolean.
What the notice cannot tell you is stated in the code: a model thinking hard, a long build and a
wedged process look identical from outside, which is the whole reason it reports and never acts.


**Supervision's acting half, and the scope it is not allowed to exceed.** `devboule_stop_agent` kills
one child's process tree and keeps its row and transcript; `devboule_close_agent` ends the session and
leaves the transcript in history. Reading is scoped to the roster, but acting is **narrower on
purpose**: both resolve their target through `resolve_own_child`
(`crates/devboule-daemon/src/session.rs`), which admits
only live sessions whose `created_by` is the caller. A grandchild is not a child, a sibling is not a
child, and a parent is certainly not.

`close` had no parentage check of any kind before this — only `check_user_owner` — so the scope check
is added here rather than assumed. An invented id, the caller's parent, a live session of the user
that the caller did not create, and another account's session all answer with one sentence and one
code, so the tool tells nothing about what exists. Two honest limits on that: the caller's own child
that has exited but not been reaped is still `Live`, so it resolves and answers "stopped"; and
`devboule_list_agents` already hands every agent the ids of every live agent of the same owner, so
existence was never a secret these sentences kept. They are kept because they stay right if the roster
ever narrows.

The predicate behind all of it is written once (`crates/devboule-daemon/src/session_items.rs`,
`is_child_of`) and called from every
site that needs it. It had been four textual copies; a scope rule spelled five times is a scope rule
that will be wrong in one of them. Both verbs are local by construction: the tool door judges them as
`SessionStop` and `SessionClose`, which every peer is denied unconditionally whatever its capabilities,
and a test walks the closed capability table for both roles rather than sampling one name.

**Lineage is daemon-written.** `created_by` is deliberately absent from `SessionCreate` so no client
can claim a parent — the field is not there, and the comment that stands where it would be says so —
and `display_name` is set once at creation and is not renamable
(`crates/devboule-protocol/src/messages.rs`, the `SessionCreate` variant and the two comments on its
`display_name` field).

**A child's powers are a birth fact, and they survive the daemon.** A creation resolves a tool
overlay — a deny-list of tool names the child will not be offered and cannot call — and the journal
row keeps it (`sessions.overlay`, schema v13). A resume reads it back in the same lookup that reads
`created_by`; `resumed_lineage` takes no profile store, which is the structural reason it cannot
re-resolve. That is deliberate and it cost a column: re-resolving from `profile_id` would ask a
**mutable** store a question whose answer can change, and when the profile has since been edited or
deleted the honest fallback is "no overlay" — which is exactly the silent escalation this avoids. A
row written before v13 carries no recorded restriction, even if the child was born restricted:
the overlay column did not exist to record it, so a resumed pre-v13 child comes back
unrestricted — only the depth cap holds it, at the closed end for rows that still name a
creator. There is no backfill, because a backfill could only manufacture a restriction
nobody recorded.

## 8. Attachments

**Where the bytes live.** Under the daemon's runtime directory, one folder per session, and never
inside the workspace — a workspace is a git checkout the user reads from `git status` and the Changes
panel, and an attachment written there would appear as the user's own edit and could be committed by
accident (`crates/devboule-daemon/src/attachment_store.rs:3-8`). The layout is

```
<runtime dir>/attachments/<session id>/<sha256>.<ext>
```

**Content addressing does three jobs at once** (`attachment_store.rs:8-20`). The name is the SHA-256
of the bytes that were actually written, plus an extension taken from the MIME type — and for a JPEG
or a PNG those are the decoded bytes with their identity metadata removed
(`crate::raster_metadata`), "so the name answers for what is on disk rather than for what arrived"
(`:10-14`). The user's file name is deliberately not in the path: it comes from outside and may contain
`..`, a path separator or a drive letter, while a digest cannot express any of them, so traversal is
not possible to express (`:15-18`). And the same bytes are the same path, so re-materializing the same
image — a second turn with the same picture, or a replay of history that rebuilds it — reuses the file
instead of leaving another copy behind (`:18-20`). The digest length is asserted to be 64 hex
characters before any string may be joined to a path (`:75-80`), and the four stored extensions are
`png`, `jpg`, `svg`, `md` (`:95`).

**Budgets.** Five limits, all on the wire, all enforced before bytes are written:

| Limit | Value | Where |
| --- | --- | --- |
| one store (one account) | 20 MiB | `crates/devboule-protocol/src/lib.rs`, `MAX_ATTACHMENT_OWNER_BYTES` |
| attachments per prompt | 4 | `lib.rs`, `MAX_ATTACHMENT_COUNT` |
| one attachment's base64 data | 192 KiB | `lib.rs`, `MAX_ATTACHMENT_DATA_BYTES` |
| a prompt's inline attachments together | 384 KiB | `lib.rs`, `MAX_ATTACHMENTS_TOTAL_BYTES` |
| stored references per prompt | 200 | `lib.rs`, `MAX_ATTACHMENT_REFERENCES` |

"This store" is one account's store: the runtime directory is `%LOCALAPPDATA%\Devboule`, so the budget
is per-user by construction and needs no key at all (`attachment_store.rs:22-29`). The header records
that a key once existed and was wrong, and it is worth reading before touching session ids: the middle
segment of a session id *looked* like an owner and is not one — it is one connection's client token cut
to sixteen characters, so one user running two clients would have had two budgets of twenty megabytes,
and two clients whose tokens share sixteen leading characters would have shared one (`:31-37`; the id
composition is `crates/devboule-protocol/src/ids.rs:59-71`, and §3 warns about the same field).

The running total is held in memory rather than walked per question, because a forty-page deck is two
hundred deposits and walking the store under its single write lock would put every session's attachment
work behind that walk; the tree stays the truth and the cache is derived from it
(`crates/devboule-daemon/src/attachment_store.rs:39-44`, the doc on `StoreState`). A folder
the walk could not read makes the total *unknown* rather than zero, and an unknown total is refused —
"a number below the truth admits the bytes the limit exists to refuse"
(`crates/devboule-daemon/src/attachment_store.rs:44-47`).

**How a prompt names a stored file.** Inline bytes are `PromptAttachment { name, mime_type, data }`,
where `data` is base64 and the comment is explicit: "Never a path"
(`crates/devboule-protocol/src/messages.rs`, `PromptAttachment`). A stored file is named by reference
instead, in
exactly one spelling — `devboule-attachment:<sessionId>/<digest>`
(the reference `crates/devboule-protocol/src/session.rs` spells in the `FinishArtifact` doc) — carried
on the request as
`attachment_references` (`messages.rs`, `ClientMessage::SessionDeposit`) and validated before use,
including a total-size check
against the same 20 MiB budget (`crates/devboule-protocol/src/attachments.rs`, the validation that
compares against `MAX_ATTACHMENT_OWNER_BYTES`). A deposit is
`ClientMessage::SessionDeposit` and answers `SessionDeposited` with that reference
(`messages.rs`, `DaemonMessage::SessionDeposited`), which is the value the finish artifact of §7 reuses.
Folders left by sessions that
never closed are swept at daemon start (`crates/devboule-daemon/src/server/lifecycle.rs`, `run_windows`).

## 9. Surfaces

**The registry is a typed array, and navigation is generated from it.** `SurfaceKey` is a union of
six names — `workspace`, `polis`, `pubvia`, `design`, `settings`, `marketplace` — and each entry in
`SURFACES` carries a label, an eyebrow, a description and a tone
(`src/types/surface.ts:1-66`). A surface that comes from a plugin names its plugin id, which is also
the directory name under the app's plugin folder, and such a surface is absent until the user installs
it, so the navigation shows it as something to add rather than something to open
(`src/types/surface.ts:11-19`; `polis` is the one that does this today, `:30-37`). The app maps every
key to a component in one record (`src/app/App.tsx:101-108`): `pubvia` is a literal placeholder, the
other five are real surfaces loaded lazily. `Shell.tsx` derives its keyboard reachability from the same
array, which is what keeps a new surface from having to be registered twice
(`src/app/Shell.tsx:6`, `:14`).

**What the workspace strip shows, and what a swipe on it means.** The strip carries live and silent
sessions, and **recovered** ones as well (`src/features/workspace/workspaceSessions.ts:76-86`): a
recovered row costs nothing to show because attaching to it is reading — replay from the journal, no
process — so it comes back by itself, in a diminished state with its transcript readable and its
composer disabled. Ended rows stay in History; they have nothing left to come back to.

A swipe on a tab reveals the act underneath — **Archive to the right, Delete to the left** — and
releasing past `SWIPE_COMMIT_PX = 90` commits it (`SessionTabSwipe.tsx:1-13`). The gesture never calls
the daemon. It records an intent, hides the tab, and opens one undo window of `UNDO_WINDOW_MS = 5000`
(`pendingSessionActions.ts:12`, which owns the only such timer in the tree). Two details are load-
bearing and were both paid for. Pointer capture is taken when the press becomes a drag, never on
`pointerdown`: capturing on press retargets WebView2's compatibility mouse events and the click never
reaches the tab button inside (`SessionTabSwipe.tsx:20-28`). And the deferred intent is owned above
the surface that can unmount, because a flush in an unmount cleanup would fire the destructive act
every time the user navigates away — only `beforeunload` is the app closing. The intent is keyed by
the session's `generation`, so an intent taken against one instance is void if the row died and came
back.


**Per-surface settings are opaque on purpose.** Each surface stores one JSON document at
`<app_config_dir>/surface-settings/<surfaceId>.json`; the backend never inspects the value's shape — it
stores whatever JSON arrives, verbatim, and hands it back unchanged
(`src-tauri/src/surface_settings.rs:1-7`). Surface ids are validated and the payload is size-bounded
(`:60`, `:109-115`, where the payload cap is `MAX_SURFACE_SETTINGS_BYTES`), and the commands are
`surface_settings_get` / `surface_settings_set`
(`src-tauri/src/lib.rs:113-114`). A malformed file is an error that preserves the file rather than a
silent reset (`src-tauri/src/surface_settings.rs:213`, the malformed-file test).

**What Design is.** Design is a chat grounded in the repository that drives the *same* daemon and the
same agent CLIs the Workspace uses, with a canvas that renders what the agent produced inside a
sandboxed frame. Its own README states the boundary in its first sentence: "It is not an editor: you
can look and point, not drag" (`src/features/design/README.md:1-4`). The host contract is one object
with three members, of which only `loadDocument` is required; `saveDocument` and `generate` are
optional, and **an absent capability removes its own UI** rather than disabling a control, so honesty
about what a host can do is a property of the type rather than of copy someone must remember to keep
accurate (`src/features/design/README.md:6-13`). `App.tsx` always mounts the agent host
(`src/app/App.tsx:8`, `:47`), so generation grounds on the attached folder's Oracle index or runs
ungrounded when no folder is attached, and the global Oracle index state never decides which host the
user gets (`src/features/design/README.md:15-17`). Its layers are the repository's own components
rather than previews, because rendering a component means compiling it and there is no bundler in a
packaged app; the layers are listed in the README's Status section and in
`src/features/design/README.md`.

## 10. Where the edges are

Measured but unresolved. Everything here is taken from the two workspace reports named in the
preamble, or from a line read in this tree, and is listed as an open edge rather than a defect claim.

**A client handler can deadlock the attachment stream.**
A measurement taken at `786e717` found a
client-side reentrancy cycle, not a daemon subscription bug: an `EventHandler` is invoked *inline on
the client's only reader thread*, so if a handler synchronously calls `sessions_list()` on the same
client, the call waits for a reply that its own reader thread must read. The verdict was that
the daemon keeps sending and the client simply stops reading — "the apparent 'daemon stopped
publishing' boundary is the handler call". In this tree the reader is the single
`client_read_loop` (`crates/devboule-daemon/src/client.rs`), which calls each handler inline from that
loop (the roster-snapshot arm and the event-envelope arm), and the blocking roundtrip a handler would
enter is `roundtrip_with_deadline` (same file). The recommended fix is to enqueue to a
dedicated dispatcher instead of invoking handlers on the reader; a documentation-only prohibition is
insufficient for the attached Workspace flow. **As far as this tree shows, that fix has not been
made.**

**`session.rs` is one module in a family, and the provider trait exists.** The split moved the
registry's kind-dependent code out of the old single file: `crates/devboule-daemon/src/session.rs` is
3,418 lines today, with 47 `session*.rs` siblings beside it (`#[path]` modules, 32,801 lines across
the family). And the class-level seam is real:
`crates/devboule-daemon/src/provider.rs:121` declares a `Provider` trait, with one implementation per
spawn family — ACP, Claude, Pi, Codex and Terminal — and a `ProviderRegistry` the spawn road resolves
through, so "anything that touches all providers" is a call through that trait rather than a search
across the tree. The traits the daemon also defines are for transports, the secret store, the
registry fetchers and the npm runner (`transport.rs:35`, `crates/devboule-daemon/src/peer_transport.rs`,
`PeerTransport`,
`secret_store.rs:52`, `registry.rs:45`).

**There is no updater, so there is no update path.** No updater plugin in
`src-tauri/Cargo.toml:21-33`, `bundle.active` is `false` in `src-tauri/tauri.conf.json`. A daemon
speaking a different protocol version is refused rather than replaced
(`crates/devboule-protocol/src/handshake.rs:110-131`).

**Young daemon deaths are braked.** A daemon that dies soon after each spawn is re-spawned by the
supervisor, but past `FAST_FAILURE_TOLERANCE` consecutive fast failures the loop waits a doubling
`BACKOFF_BASE`…`MAX_BACKOFF` before the next attempt, and a connected phase that lasted
`HEALTHY_CONNECTED` resets the count (`src-tauri/src/client/crash_loop.rs`, `CrashLoopBrake`;
called from `src-tauri/src/client/mod.rs`, `run_supervisor_loop`). The loop sleep and the
`SPAWN_ATTEMPTS` × `SPAWN_SLEEP` connect retries sit underneath it
(`crates/devboule-daemon/src/client.rs`).

**`resume` exists for four families: ACP, Claude, Codex and Pi.** `resume_handle` admits every family
whose `Provider::resumable()` answers `true`, and Terminal is the one that answers `false`
(`crates/devboule-daemon/src/session.rs`, `resume_handle`; `crates/devboule-daemon/src/provider.rs`,
`resumable`). Pi resumes by `--session <id>` against its own session directory, which is why §2 names
it with the other three. Codex resume loads the
thread by `threadId` from the human's real Codex home, which is also why that family keeps no
per-session `CODEX_HOME` and writes no carrier file (the broker rides `-c` overrides on the launch
line).

Six further questions that no code here answers. They are recorded as questions because that is what the evidence supports:

1. **App update vs a running older daemon.** With no updater and no "kill on version mismatch" path,
   what is supposed to happen when a newer app is installed while an older daemon still owns the pipe?
2. **Whose Job Object the daemon is in.** `crates/devboule-daemon/src/spawn.rs:39-40` says the daemon
   "is allowed to die when Windows tears down this job", but no code in `src-tauri/src` creates a job
   or assigns the daemon to one — the spawn path sets only `CREATE_NO_WINDOW`
   (`crates/devboule-daemon/src/spawn.rs:76-80`). If the app is not itself in
   such a job, a killed app is expected to leave the daemon running with its live sessions.
3. **Reattach after an undelivered `Shutdown`.** The app waits `JOIN_BUDGET` (1.5 s)
   (`src-tauri/src/client/mod.rs`) and then exits; the daemon, holding a live session, never
   idle-exits (`crates/devboule-daemon/src/server/state.rs`, `client_disconnected`), so the next start
   rejoins it. Intended, or should the app prove the
   daemon is gone first?
4. **What ends a running session.** In this code only an explicit `SessionStop` / `SessionClose`
   (`crates/devboule-daemon/src/session.rs`, `stop`, `close`) or the process dying does. There is no
   idle or quota policy for live
   sessions; silence produces a banner, not an end (`crates/devboule-daemon/src/session_items.rs`,
   `SESSION_SILENCE_THRESHOLD`). Whether that is the rule, or a
   policy is still to be written, is not stated anywhere in the tree.
5. **Two app instances.** A second app process computes the same pipe name and connects first
   (`paths.rs:75-77`), the daemon accepts up to sixteen pipe instances
   (`transport/windows_pipe.rs:49`, `MAX_INSTANCES`), and sessions are scoped to the owner user.
   Whether a second app
   instance should see and drive the first one's sessions, or "one app at a time" is assumed somewhere
   outside this code, is not stated.
6. **The spawn-then-assign window.** A child is assigned to its job right after spawn; closing the
   window completely would need `CREATE_SUSPENDED`, which portable-pty does not expose
   (`crates/devboule-daemon/src/provider.rs`, `open_pty_session`). Whether that window is covered by a
   test that kills the daemon inside it,
   or is accepted, is not recorded.

One more gap, found while reading and not from a report: the typed `createAgent` payload has no
consumer in the UI (§7), and the MCP broker — and therefore every agent-to-agent feature — reaches
every agent family except `Terminal` (`crates/devboule-daemon/src/mcp_broker.rs`, `hosts_mcp`; the
distinction between hosting tools and gating the first prompt on them is §7's).

---

This document describes what the code does, not what anyone intended. The intent lives in design
notes that are **not part of this repository**, and in the headers of the daemon's own modules, which
are unusually forthcoming about what was measured and what was decided against. Where a module header
and this document disagree, the header was written first and this document is a reading of the code.







