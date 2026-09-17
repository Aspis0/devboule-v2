# Devboule architecture

How Devboule is built at `f3647ee`. Every structural claim below carries a `file:line` that was read
in this tree; where a statement is a code reading rather than a runtime observation, it says so.

Two conventions:

- Paths are relative to the repository root.
- Section 2 restates a lifecycle measurement made in a design note that does not ship with this
  repository. That measurement was taken while four daemon files were being edited, so **every line
  number reproduced here was re-checked against a quiet tree** and the verified number is the one
  printed.

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
copies"). Protocol version 5 (`crates/devboule-protocol/src/lib.rs:144`), with `PROTOCOL_MIN_VERSION`
also 5 — the two are equal so a v4 peer is refused at the handshake instead of dying at the first frame.

### The daemon is a separate process

The GUI never runs the server in-process. That is stated in the manifest
(`src-tauri/Cargo.toml:31`, which pulls `devboule-daemon` with `default-features = false`, with the
comment "the GUI never runs the server in-process") and enforced by the build: the whole serving half
of the daemon crate is behind the `server` feature (`crates/devboule-daemon/src/lib.rs:6-83`), and
`main.rs:17-18` makes a binary built without it a compile error rather than an in-process server.

They meet on a Windows named pipe. The pipe name is derived, not configured: the runtime directory is
normalised (separators and case) and hashed with FNV-1a into sixteen hex characters, giving
`\\.\pipe\devboule-<hash>` (`crates/devboule-daemon/src/paths.rs:60-77`). `std`'s `DefaultHasher` is
deliberately not used, because it is seeded per process and would put the two ends on different pipes
(`paths.rs:5-8`). The pipe is created with a current-user-only security descriptor, `PIPE_TYPE_BYTE`,
and `PIPE_REJECT_REMOTE_CLIENTS`, with up to sixteen instances
(`crates/devboule-daemon/src/transport/windows_pipe.rs:80-112`, `:49`, `:105`).

The daemon binary is found by the app at start-up: an explicit `DEVBOULE_DAEMON`, else a sibling of
the app executable, else `target/{debug,release}/devboule-daemon.exe`
(`src-tauri/src/client/mod.rs:1498-1528`; the daemon's own client has the same rule at
`crates/devboule-daemon/src/spawn.rs:20-37`). In development the daemon is built before the frontend
runs (`src-tauri/tauri.conf.json:9`, `beforeDevCommand`); there is no bundling step
(`tauri.conf.json:70-71`, `bundle.active = false`).

### What the daemon owns that the app does not

- **Every child process and its lifetime.** PTY terminals and provider CLIs are spawned by the daemon
  (`crates/devboule-daemon/src/session.rs:7253-7283` for a PTY), each in a Windows Job Object (§2).
- **The journal.** SQLite, WAL mode, `journal.db` beside the lock file
  (`crates/devboule-daemon/src/paths.rs:51-55`), schema version 10
  (`crates/devboule-daemon/src/journal.rs:54`).
- **The MCP broker.** A loopback HTTP listener with one bearer token per session
  (`crates/devboule-daemon/src/mcp_broker.rs:1-7`, `:40`, `:228`).
- **The peer listener.** A TCP listener on the tailnet, everything inside Noise
  (`crates/devboule-daemon/src/peer_transport.rs:1-6`), started best-effort at boot
  (`crates/devboule-daemon/src/server.rs:1369-1375`).
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
(`src-tauri/src/client/mod.rs:792-812`). The supervisor's connect step is `connect_once`, which builds
the owner from the current user's SID plus `app-<pid>`, resolves the daemon binary, and calls
`connect_or_spawn` with that binary (`src-tauri/src/client/mod.rs:1486-1496`).

**How an already-running daemon is found.** By connecting, not by looking for a process. The
connect-or-spawn loop tries to connect *first* and spawns only if that fails
(`crates/devboule-daemon/src/client.rs:1274-1283`); the address is the deterministic pipe name
(`paths.rs:75-77`). A second daemon is harmless: the single-instance lock is an exclusive
`LockFileEx` on `daemon.lock` (`crates/devboule-daemon/src/lock.rs:26-38`), and the loser of that lock
prints a human sentence and **exits 0**, because "nothing to do" is not "failure"
(`crates/devboule-daemon/src/main.rs:23-32`). Note the lock file's existence is not the lock — the OS
releases it when the process dies, so a stale file is not a deadlock (`lock.rs:1-2`).

**The spawn itself.** `spawn_daemon` sets `DEVBOULE_RUNTIME_DIR`, nulls the three standard streams and
passes `CREATE_NO_WINDOW` (`crates/devboule-daemon/src/spawn.rs:41-43`, `:61-82`). The retry budget is
50 attempts 100 ms apart (`client.rs:29-30`, `:1295`), and the handshake has its own 2 s timeout
(`client.rs:26`).

**On quit.** One `RunEvent::Exit` handler calls `oracle.shutdown()`, `DaemonBridge::shutdown()` and
`PluginRuntime::stop_all()` (`src-tauri/src/lib.rs:118-125`). The bridge stops its thread, sends the
`Shutdown` RPC and joins with a 1.5 s budget (`src-tauri/src/client/mod.rs:874-893`, `:27`);
`DaemonClient::shutdown` requires `accepted: true` (`crates/devboule-daemon/src/client.rs:130-137`);
the daemon's dispatch **flushes the journal before it accepts**, so the reply is the app's last
guarantee that the transcript is on disk (`crates/devboule-daemon/src/server.rs:2258-2263`).
`run_windows` then wakes from `wait_until_shutdown` (`server.rs:379-390`), flushes again, shuts the
listener, stops the peer listener and bounded-joins the accept thread (`server.rs:1377-1392`), and
`main` returns (`main.rs:21-22`). There is no window-close or exit-requested handler anywhere in the
tree, so "closing the last window quits the app" is not something this code shows.

**On a crash, and on losing the pipe.** Two different mechanisms on the two sides.

- *App side.* Liveness is checked only here: a `Status` RPC every 2 s, and after
  `STATUS_FAILURE_THRESHOLD = 3` consecutive failures the reported state becomes `unresponsive`
  (`src-tauri/src/client/mod.rs:26`, `:1207`, `:1294`). A lost connection is treated as the normal
  handoff back to the connect path, not as an exit (`client/mod.rs:1339-1369`), which is what
  re-spawns a daemon that died. **There is no crash-loop brake**: a daemon that dies immediately after
  spawn is re-spawned on the next loop iteration, paced only by the loop sleep and the 50 × 100 ms
  connect retries.
- *Daemon side.* The daemon never pings the client; it discovers the loss from the failing pipe. One
  cleanup path serves normal disconnects, read/write errors, shutdown and the idle exit: stop the
  request reader, close the outbound queue, flush final events, detach the connection, clear presence,
  and give back the permission card slots the device was holding
  (`crates/devboule-daemon/src/server.rs:1815-1848`). Then `client_disconnected` decrements the client
  count and arms the idle exit **only** when `clients == 0 && sessions == 0` (`server.rs:411-425`);
  the grace is `IDLE_SHUTDOWN_GRACE = 1 s` (`crates/devboule-daemon/src/lib.rs:149`) and the timer
  re-checks its generation under the lock, so a reconnect or a newly created session cancels it
  (`server.rs:1167-1191`).

**The two things that surprise people.**

1. **Every provider process lives in a Windows Job Object that kills it when the daemon exits.**
   `JobObject::new()` sets `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, `assign()` puts a child in it, and
   `Drop` closes the handle — which closes the job and terminates its members
   (`crates/devboule-daemon/src/process_tree.rs:37-59`, `:77-83`, `:151-155`). Every agent and
   terminal is assigned immediately after spawn: ACP `acp_client.rs:456-467`, Claude
   `claude_client.rs:221-232`, Codex `codex_client.rs:185-196`, Pi `pi_client.rs:305-317`, a PTY
   `session.rs:7282-7283`; the daemon holds a job of its own too (`server.rs:242`). "No orphans" is
   therefore a kernel guarantee, not a cleanup step. The window between `spawn_command` and the
   assignment is known and accepted: closing it completely would need `CREATE_SUSPENDED`, which
   portable-pty does not expose (`session.rs:7253-7257`).

2. **Provider sessions are never re-spawned after a restart: the transcript survives, the process does
   not.** The processes were killed by 1. At the next journal open, rows still marked `live` are
   rewritten — `reaped = 1` → `ended`, otherwise `interrupted`
   (`crates/devboule-daemon/src/journal_schema.rs:184-194`) — and `to_session()` maps those to
   `Ended` / `Recovered` with a transcript-integrity verdict (`journal.rs:398-418`); the replay emits
   `Recovered` and no exit event (`journal_replay.rs:374-392`). The transcript is then replayed from
   the journal on attach (§3). Starting the provider again is an explicit, separate act: `resume` is
   the only path, and the gate admits **two** families — ACP and Claude
   (`resume_handle`, `session.rs:9500`; the per-family fact is `Provider::resumable`,
   `provider.rs:240`, answered `true` by `AcpProvider` and `ClaudeProvider` and `false` by Pi, Codex
   and Terminal). Codex is refused outright and Pi is deliberately excluded "until Pi resume is
   designed end to end" — pi can resume on its own wire, so that exclusion is a decision, not a
   limitation. Claude resumes by handing the CLI back its own history: the daemon finds the
   transcript file for the provider's session id under the Claude home, refuses with a named error
   when it is not there, and passes `--resume` (`claude_client.rs:390`, `:411`, `:429`). The id it
   builds that path from is validated against a closed alphabet first, because a session id that
   could contain a separator is a path that could leave its root.

**The daemon can outlive the app.** Because the idle exit requires `sessions == 0` (`server.rs:415`),
a daemon whose app went away without delivering `Shutdown` — a kill, a crash — keeps running with its
live sessions, and the next app start *rejoins* it instead of restarting it, since the connect is
attempted first (`client.rs:1274-1283`). The app's own exit path waits only 1.5 s for its shutdown
frame (`src-tauri/src/client/mod.rs:27`, `:884-892`), so "the frame was not delivered" is reachable.
Whether that reattach is intended, or whether the app should prove the daemon is gone before exiting,
is an open question, and is not answered by any code here.

Two smaller facts that belong to this section. There is **no updater in the tree** (`bundle.active =
false`, `tauri.conf.json:70-71`; no updater plugin in `src-tauri/Cargo.toml:21-33`), so "restart the
daemon for an app update" has no implementation; a daemon speaking another protocol version is
*refused* with a sentence telling the user to reinstall, not replaced
(`crates/devboule-protocol/src/handshake.rs:110-131`). And attachment folders left by sessions that
never closed are swept at daemon start (`server.rs:1346-1360`).

**There is also an explicit way to kill the daemon**, and it refuses to shoot the wrong process.
`daemon_restart` in the app (`src-tauri/src/client/mod.rs:1199-1205`) calls
`DaemonClient::restart_daemon` (`crates/devboule-daemon/src/client.rs:142-170`), which requires the
server PID captured at handshake and re-checks the pipe's identity immediately before terminating; a
changed identity is an error rather than a risk, because a PID alone can be recycled between the query
and the kill (`crates/devboule-daemon/src/transport/windows_pipe.rs:295-324`, `:257`).

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
(`crates/devboule-daemon/src/provider.rs:318`), which answers `true` only when four things hold at
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
threshold is 300 s (`crates/devboule-daemon/src/session.rs:200`) and it produces a banner event, not a
kill (`crates/devboule-daemon/src/session_runtime.rs:1916`).

`Recovered` used to mean "replay only". It no longer does: the variant's own doc now reads "replay
always; resume when the family is resumable" (`session.rs:475`). Replay is free — journal bytes, no
process — so it happens by itself; resume allocates a process and stays a deliberate act. Every state
carries a `generation`, and that is what lets a deferred intent belong to an *instance* of a session
rather than to its id: a row that died and came back is not the row the intent was taken against.

**The journal.** SQLite in WAL mode at `<runtime dir>/journal.db`, beside the lock file
(`crates/devboule-daemon/src/paths.rs:51-55`), schema version 10
(`crates/devboule-daemon/src/journal.rs:54`). One writer thread owns it
(`crates/devboule-daemon/src/journal.rs:1398`) with a bounded queue of 1024 commands (`:63`) and a
snapshot of the screen emulator every 64 KiB of output (`:66`). Rows are appended per session
sequence number, so a replay is ordered by the same counter the live events carry.

**Replay on attach.** A session that is not live is hydrated from the journal instead of being
spawned: `attach`/`attach_with_subscription` call `hydrate_transcript`
(`crates/devboule-daemon/src/session.rs:3307-3338`, `:3567`), and a live agent's replay is pulled one
bounded journal page at a time by cursor, with the live attachment queue left alone until the durable
watermark is complete (`crates/devboule-daemon/src/event_pull.rs:476-485`). The roster that the app
sees is the merge of live entries and journal rows (`session.rs:5064`), which is why a session whose
process is gone is still listed, in `Recovered` state, with its transcript available.

**`Recovered` is a conclusion, not a guess.** At journal open the daemon rewrites what it cannot
vouch for: a row still `live` with `reaped = 1` becomes `ended` — the child's exit was observed, the
daemon died during drain — and any other `live` row becomes `interrupted`
(`crates/devboule-daemon/src/journal_schema.rs:184-194`). `to_session()` turns those into `Ended` and
`Recovered` respectively, each with a transcript-integrity verdict (`journal.rs:398-418`), and the
replay emits `SessionEvent::Recovered` in place of an exit event
(`journal_replay.rs:374-392`). `Recovered` therefore means "the process was lost unobserved", which is
a stronger and more honest claim than "the session ended".

**Three ways a session stops being on your screen, and they are not the same act.** `SessionDetach`
gives up one subscription and leaves everything running (`messages.rs:255`). `SessionStop` kills the
process and **keeps the session and its transcript** (`:272`); it carries a `subscription_id` because
the caller must be an observer of the session it is stopping. `SessionClose` destroys the session
(`:266`), and it carries an idempotency key rather than a subscription, because closing twice must not
mean closing something else.

`SessionStop` kills the *tree*, not the root. After `killer.kill()` the daemon also calls
`job.terminate()` on the session's own Job Object (`session.rs:4801`, `:4847`), because the session is
being preserved and its job therefore stays open — nothing else would reap the descendants a CLI left
behind. This mirrors what the on-OS-death handler already did (`:8874`). A killed-but-kept session is
the one the app calls *archive*: the row and its transcript survive, the process does not.

**The wire names who wrote a user message.** `UserMessageAuthor`
(`crates/devboule-protocol/src/session.rs:1364`) is `human`, `agent` or `creation`, and it is neither
the session's `origin` (where the session came from) nor the envelope's `role`/`from_agent` (the
delivery's connection facts): it names whose words the echo carries. `creation` is its own value even
when a human wrote the initial text, because that line is daemon-composed — standing instructions plus
preamble plus prompt. The app renders by this field and never re-derives authorship from the text;
absent predates the field and reads as `human`.


**Retention.** Four limits, all configurable, with these defaults
(`crates/devboule-daemon/src/journal.rs:72-83`): 512 MiB per session, 8 GiB total, 10 000 sessions,
and an age limit of `0` — off. The app exposes them as `journal_usage`,
`journal_retention_get`/`_set` and `session_delete` (`src-tauri/src/lib.rs:68-71`). Deletion is
byte- and count-driven, not idle-driven (`journal_retention.rs:263-322`), each deletion writes a
tombstone into `deleted_sessions` so a deliberate loss of history is on record (`:406`), and the age
rule skips ACP sessions (`:285-295`). The global scan is amortised rather than run per write: it runs
at most once per MiB of journal written (`:14-17`).

**Two details worth knowing before you touch any of this.** Child liveness is not derived from pipe
EOF: a sweeper every 2 s duplicates the process handle and waits non-blockingly, so a provider killed
from Task Manager is noticed even while its descendants still hold the pipe open
(`crates/devboule-daemon/src/session.rs:185`, `:7085-7105`). And the per-attachment output budgets
(`PENDING_OUTPUT_BUDGET_BYTES`, `PENDING_OUTPUT_BUDGET_FRAMES`, `COALESCE_*`,
`crates/devboule-daemon/src/session.rs:146-172`) are what keep a chatty agent from turning into
unbounded memory or an unbounded journal.

## 4. Providers

**The catalog** is a static table plus a `PATH` scan. A `KnownAgent` row carries the id, its aliases,
and up to four launch shapes — `acp_args`, `stream_json_args`, `rpc_args`, `app_server_args` — each
`Option`, so "this agent speaks that dialect" is a fact in one row
(`crates/devboule-daemon/src/provider_catalog.rs:47-61`). `KNOWN_AGENTS`
(`provider_catalog.rs:87`) holds claude, codex, grok, pi, qwen and the rest; two debug-only rows are
compiled in under `debug_assertions` so a test can reach "provider not installed" without depending on
the machine (`:166-192`). The header records what was borrowed: the alias table and the
executable-file check are adapted from herdr under Apache-2.0; the launch resolver — PATHEXT
following, then unwrapping an npm `.cmd` shim to `node` plus the package script so `CreateProcess`
never goes through `cmd.exe` — is Devboule's own (`provider_catalog.rs:1-16`).

**Which providers are native, and which speak ACP.** Native adapters exist for exactly three, and the
kind enum mirrors them (`crates/devboule-protocol/src/session.rs:17-23`):

| Provider | Dialect | Catalog evidence | Adapter |
| --- | --- | --- | --- |
| Claude | `stream-json` | `CLAUDE_STREAM_JSON_ARGS` `provider_catalog.rs:69-85` | `claude_client.rs`, `claude_view.rs` |
| Codex | app-server | `app_server_args: Some(&["app-server"])` `:103` | `codex_client.rs`, `codex_view.rs` |
| pi | RPC | `rpc_args: Some(&["--mode", "rpc"])` `:122` | `pi_client.rs`, `pi_view.rs` |
| everything else | ACP | grok `:109`, qwen `:132` | `acp_client.rs`, `acp_host.rs`, `acp_view.rs` |

The decision is made per installed agent by `chat_protocol`
(`provider_catalog.rs:983-998`), which returns `codex-app-server`, `acp`, `stream-json` or `pi-rpc`
and `None` for a CLI that is installed but not chat-capable, with the explicit tie-break comment
"An agent with both launches is offered as ACP: ACP is the road, stream-json the exception"
(`:982`). Among ACP agents the default is a separate, explicit preference order — grok, then qwen,
then gemini — with the reason recorded as a measurement (`:730-735`, `first_acp_available:1001-1010`).

Three registry wrappers are covered by a better native provider and are therefore visible in Settings
but not offered in the workspace picker: `claude-acp`, `codex-acp`, `pi-acp`
(`provider_catalog.rs:711-728`). The reason for `pi-acp` is the useful one to know: the wrapper speaks
ACP and reports models, but does not emit `session/request_permission` for native tools, and "a
measured write completed with zero permission requests" (`:718-722`). On the wire this is
`ProviderInfo.pickable` (`crates/devboule-protocol/src/messages.rs:1087-1090`).

**Where modes and features come from.** Not from the catalog. A provider's modes are whatever the
provider declares at run time: `SessionModeStateView` carries a current mode id and an
`available_modes` list (`crates/devboule-protocol/src/session.rs:992-998`), populated from the ACP
session state (`acp_client.rs:1062`, `:1683-1711`) or from Claude's control protocol
(`claude_client.rs:172`), and a mode change is an RPC to the provider
(`acp_client.rs:1226`, `:1331`; `claude_client.rs:1032`). The catalog's part is narrower and only
concerns *created* agents: the preset cell names the mode a child is started in (§7), and the
classifier that decides which mode ids mean "answers in place of the human" is per family —
`mode_is_unattended` (`provider_catalog.rs:361-374`) — because Codex's `auto` and Claude's `auto` are
one word apart and mean opposite things (`:350-357`).

**The inventory on the wire** is `ProviderInfo` (`crates/devboule-protocol/src/messages.rs:1064-1101`):
executable, `acp_available`, the chat `protocol`, how the row was obtained
(`user-binary` / `npx-wrapper`, `provider_catalog.rs:745-748`), the registry launch arguments,
`pickable`, and installed-versus-latest versions. Authentication is deliberately never probed: an
executable on `PATH` is "installed, status unknown", and the status enum has exactly one variant,
`Unknown` (`messages.rs:1064-1065`; `provider_catalog.rs:737-741`). Settings can refresh the catalog
and install or update an npm-supplied wrapper (`provider_update.rs:27`; the app's commands
`providers_list`, `providers_refresh`, `provider_update` at `src-tauri/src/lib.rs:88-90`).

## 5. Permissions

**One broker, two providers.** The permission broker is shared by ACP and Claude stream-json sessions
(`crates/devboule-daemon/src/permission_broker.rs:1`). A provider's ask becomes an ordinary wire
event: `SessionEvent::PermissionRequest` (`crates/devboule-protocol/src/session.rs:708`) carrying the
tool call id, a title, the agent's own description of the command, the options the provider offered,
and an origin stamp. The daemon publishes it on the attached subscription and the app renders it —
`src/components/PermissionCard.tsx`, mounted at `src/features/workspace/Workspace.tsx:819` and
`src/features/design/DesignSurface.tsx:3087`. The answer returns as
`ClientMessage::SessionPermissionRespond`, which the daemon accepts only from a client that negotiated
the `typed_permissions` capability (`crates/devboule-daemon/src/server.rs:1601`, `:2249`).

**The card is bounded, and the bound is per device.** At most 32 undecided ACP cards exist at once
(`permission_broker.rs:14`), and a paired device may hold at most 3
(`:29`). That count is deliberately daemon-wide rather than per session or per broker: the slot is
reserved *before* the card is inserted, so two cards cannot both see the last slot free, and a
per-session counter once gave a device three cards per session (`:18-29`, `:56-60`). A slot is
released when the card is decided, when it is cancelled, and when the connection dies — the
disconnect path hands back whatever the device still held (`:95-102`, `server.rs:1845-1847`).

**Provenance is on the card, in its own element.** A request raised for, or by, a paired device is
stamped so the card can render a `peer` line; the app's contract says the request's own text must
never be able to imitate it, and that a `local` origin renders no line while an absent one renders
`Origin: unknown` (`src/types/ipc.ts:105-115`; the daemon side is
`permission_broker.rs:700-750`).

**The per-provider tool policy** decides which of the daemon's MCP broker tools a provider's sessions
are served (`crates/devboule-daemon/src/tool_policy.rs:1-2`), and it is the mechanism a person uses to
turn agent-to-agent capability off for one provider without touching the others. It is one JSON file
beside the journal, `tool-policies.json` (`tool_policy.rs:30`), written the way the MCP config is
written: a create-new temp file, a current-user-only DACL applied before its first byte, then a rename
over the target, so a crash leaves either the old policy or the new one and the file is never briefly
readable by another user (`:8-13`, `:457-470`). A file that will not parse is quarantined under a
nonce-bearing name rather than deleted (`:391-448`). **The read cadence is the point:** the broker
reads the store on every `tools/list` and on every `tools/call`, so a toggle takes effect on the
provider's next call rather than at the next session (`:3-7`; the enforcement sites are
`mcp_broker.rs:98-100`). A policy is per device and is not propagated to paired peers, because what
this machine hands to an agent is a local decision (`tool_policy.rs:15-17`). One name cannot be
disabled: the roster tool, since an agent that cannot list its siblings cannot be steered at all
(`provider_catalog.rs:214-217`). The app writes the file through `tool_policy_get` / `tool_policy_set`
(`src-tauri/src/lib.rs:86-87`, `src-tauri/src/backend/tool_policy.rs`).

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
(`crates/devboule-daemon/src/server.rs:1369-1375`). It is also not a one-shot attempt — the same
function is retried, so a user who starts Tailscale and shows a code again gets a listener rather than
the same refusal until the daemon restarts (`server.rs:1371-1374`).

**This device's identity** is a random `device_id`, a Noise static keypair, and a display name
(`crates/devboule-daemon/src/device_identity.rs:1-3`). The id is what peers pin; the key is the
credential bound to it, so a key rotation can keep the id (`:4-5`). The private half never touches
`device.json` or the journal — it lives in the secret store — and its absence is a distinct state,
`RemoteState::KeyMissing`, rather than a reason to mint a new key and silently orphan every pairing
(`:6-8`, `:113`). `device.json` holds the id, the public key and the name (`:100`;
`crates/devboule-daemon/src/paths.rs:14-17`); a fingerprint is derived for display (`:205`) and
display names are validated and bounded to 64 characters (`:236`, `:270`).

**Pairing** is a short code, a PAKE, and one mutually authenticated exchange
(`crates/devboule-daemon/src/pairing.rs:1-2`). The device that *displays* the code is the responder;
the one that *types* it is the initiator (`:3-4`). The code is 8 characters from a 32-symbol alphabet
with `0`, `1`, `I` and `O` removed, so it can be read off a screen and typed correctly (`:53-56`); it
lives 300 s (`:62`), a confirmation must be answered within 60 s (`:64`), at most 2 pairings are
parked at once (`:67`), and 3 wrong codes from one source or 12 in total kill the code (`:70-77`). The
wire is SPAKE2, then HKDF-SHA256 of the PAKE key into a 32-byte PSK, then Noise `XXpsk3` with each
side's long-term static and the prologue `devboule-pair-v1`; inside Noise each side sends
`{device_id, display_name, role, public_key}` and the responder answers `{accepted, reason}`
(`:5-12`). The PAKE is what makes an 8-character code worth its 40 bits: a passive eavesdropper learns
nothing, and an active attacker gets exactly one guess per attempt, each counted against the lockout
(`:14-16`). The exchange is deliberately not a blocked thread: a pairing that needs the local user's
answer parks the socket with a deadline and waits on a channel
(`pairing.rs:23-30`).

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

**What a capability is.** A capability names an *act* a paired device may ask for. The list is one
constant on the wire: `PEER_CAPS = ["view", "send", "answer_permissions", "create_sessions"]`
(`crates/devboule-protocol/src/messages.rs:52`). It is deliberately not a scope: which sessions an
allowed request reaches is decided elsewhere, by the owner projection in `server.rs` and the origin
branch of `check_user_owner` (`peer_policy.rs:14-16`). The gate itself is a closed match with **no
`_` arm** over every `ClientMessage` variant, so adding a variant without deciding its peer policy is
a compile error (`peer_policy.rs:1-8`, `:101`). Consequences of that design, all in the same file:
`view` is the one capability `validate_caps` refuses to strip (`:123-125`); `Status`, pairing,
capability changes and the tool bridge stay refused to a peer *whatever it holds*, because no
capability names those acts (`:12-14`); and the role a device was paired as does not decide anything
here — the capability set does (`:96-101`).

A peer's set is stored per device in the journal's `peers` table (`crates/devboule-daemon/src/journal.rs:1872`,
`upsert_peer:1917`, `set_peer_caps:2014`, `revoke_peer:1999`). Revocation holds at the last place a
frame could still leave: a connection whose caps were dropped or that was just revoked gets no closing
flush at all (`crates/devboule-daemon/src/server.rs:1821-1829`).

**In the app**, all of this is the Devices panel in Settings
(`src/features/settings/DevicesPanel.tsx:24-26`): this device's identity and fingerprint, the two
pairing directions — show a code, or type one — the confirmations this device still owes, and the
paired list with online/offline, role, per-device capability toggles and an inline revoke (`:216-300`,
`:519-670`). The daemon owns every fact; the panel never derives a device id from a name, never
invents a reason string, and never answers a permission or a pairing on the far side's behalf
(`:33-35`). It polls `devices_list` every 2 s, and every 1 s while a code is on screen or a
confirmation is pending, because those are the states that change under the user's eyes (`:28-39`).
The capability labels are exactly the wire names (`:66-73`), and the two roles are described in the
panel as "a phone or laptop of yours that views and steers this device" and "another devboule that
this one may talk to as a machine" (`:53-64`).

**Plainly: what a paired device can and cannot do.**

- *Can* — with `view`: list sessions, attach to one, and list devices
  (`peer_policy.rs:116`, `:119`, `:125`). With `send`: send a prompt, steer a running turn, send an
  agent message, deposit an attachment, and change a session's mode (`:127-142`). With
  `answer_permissions`: answer permission cards — under the per-device budget of three from §5. With
  `create_sessions`: create sessions. Every one of these is a per-device toggle the user can revoke.
- *Cannot* — anything no capability names, which is the whole administrative surface: `Status`,
  pairing itself, changing capabilities, the MCP tool bridge, journal retention and deletion, and the
  app-only commands (`peer_policy.rs:12-14`, `:147`). A peer also does not inherit anything local: a
  tool policy is this machine's own decision and is never propagated to a peer
  (`tool_policy.rs:15-17`).
- *Scope* is separate from permission: an allowed request still has to reach a session, and which
  sessions a peer can reach is decided by the owner projection plus the session's recorded `origin`
  (`peer_policy.rs:14-16`; the `origin` contract is
  `crates/devboule-protocol/src/session.rs:45-53`). A `Daemon`-role peer's ownership scope reads that
  origin rather than re-deriving it.

## 7. Agents that create agents

**The channel is the MCP broker.** Each live session that is allowed one gets a bearer token and a
loopback HTTP MCP endpoint (`/mcp`), served by the daemon
(`crates/devboule-daemon/src/mcp_broker.rs:1-7`, `:40`, `:228`); the config is written for the
provider, never read from the client (`:236-239`). It is registered for `SessionKind::Acp` and
`SessionKind::Claude` only (`crates/devboule-daemon/src/session.rs:3217`, `:7449`), so a pi or codex
agent gets no MCP tools today; the catalog says so and keeps the cells anyway, "so adding a non-ACP
transport does not silently change a decision" (`provider_catalog.rs:440-446`).

**Seven tools**, in `tools/list` order, from one table that the Settings panel reads too, so the panel
and the wire cannot disagree (`provider_catalog.rs:194-212`):

| Tool | Names | Disableable by policy? |
| --- | --- | --- |
| `devboule_list_agents` | the roster: siblings, their state, their creator, their depth | **No** — an agent that cannot list its siblings cannot be steered at all (`:214-217`) |
| `devboule_send_message` | send to one live session | Yes |
| `devboule_create_agent` | create a child from a preset and give it an initial prompt | Yes, deliberately (`:219-226`) |
| `devboule_list_profiles` | the ticked profiles agents may create from | **No** — without it creation is undiscoverable |
| `devboule_answer_permission` | answer one delegated permission card | Yes |
| `devboule_set_agent_profile` | move a child onto a ticked profile | Yes |
| `devboule_agent_activity` | one agent's derived activity plus recent kinds, metadata only | Yes |

**The creation call.** The caller is the session whose bearer authenticated the connection — "there is
no `from_session` parameter to lie about" (`mcp_broker.rs:1108-1110`). The order is fixed and stated
in the code: resolve the preset from the closed table (the mode and the overlay come from there and
never from the caller), reserve the budget, raise the creation card once per creator session, create
through the ordinary `SessionCreate` path with the creator's own origin and owner, and answer
`{sessionId, displayName, state: "submitted"}` (`:1112-1116`). Details that matter:

- **Depth comes from the registration, not the request**: a session at depth 2 may not create
  whatever it says (`:1196-1198`).
- **The child runs where the caller does**: a workspace must be the caller's own or the call is
  refused, and the resolved working directory must stay inside it, before anything is spent
  (`:1179-1195`).
- **A retry creates nothing.** An MCP `tools/call` has no idempotency parameter, so a retry is
  identified by the frame's own id plus a fingerprint of everything the answer depends on; a key
  reused with a different payload is a conflict, not a retry, and the key is held *before* the store is
  read so a second in-flight call is refused without spending a slot (`:1137-1178`).
- **The input schema is closed on purpose** (`additionalProperties: false`), and there is deliberately
  no `mode` parameter: the preset chooses the mode (`:228-235`).

**Presets are a closed table.** `AGENT_PRESETS` (`provider_catalog.rs:567`) holds exactly `worker`
and `design` (`:467-469`); an unknown preset is refused by name rather than resolved by default
(`:564-566`). Each preset is a list of provider cells — pi/`ask`, codex/`auto`, claude/`default`, and
`default` for the ACP providers the catalog publishes (`:482-522`, `:524-562`). `design` uses the same
modes with the design tool overlay. Worker carries one preamble, and it is worth reading: *"You were
created by another agent; report your result in your final message."* (`:478-480`) — it says nothing
about files on purpose, because the finish hook deposits the message itself and a preamble that told
the child to write files would be asking for the same artifact twice, in the one place the child can
put it out of reach (`:471-477`). Design has no frozen preamble at all: the app composes it per
request and it travels in `initialPrompt` (`:524-529`).

**The caps.** Four limits, all daemon constants, all enforced at admission before the card exists:

| Cap | Value | Where |
| --- | --- | --- |
| live children per creator | 3 | `session.rs:1473`, enforced `:5249` |
| creations per one-hour window | 10 | `session.rs:1475-1477`, enforced `:5255` |
| live agent sessions, daemon-wide | 8 | `session.rs:1482`, enforced `:5236` |
| nesting depth | 2 | `session.rs:1480`, enforced `:5222` |

The slot is taken **before** the card is raised, so two creations racing on one session cannot both see
the third slot free, and the in-flight reservations are keyed by the child session id they reserved —
so releasing one can never subtract a neighbour's creation (`session.rs:1518-1541`). The numbers the
card shows are read from that same reservation (`:5300-5311`), which is the point: a person decides
against the budget that is actually about to be spent rather than against a configuration claim
(`crates/devboule-protocol/src/session.rs:419-424`). Depth is judged on the child's own depth
(`:9715-9721`), and the once-per-creator-session accept lives in the same locked entry
(`session.rs:1532-1536`).

**The creation card is an ordinary permission card.** It is a `SessionEvent::PermissionRequest` with
the `create_agent` payload filled in — the same pending entry, the same allow/deny frame, the same
origin stamp and the same per-device budget as any other card — because a second variant would have to
re-implement all of that (`mcp_broker.rs:1307-1313`; the payload type is
`crates/devboule-protocol/src/session.rs:438-459`). The daemon composes the title
`Create an agent: <title> (<preset>)`, a description sentence that states the provider, preset, mode
and every cap as `n of m`, and two options, "Create once" and "Deny" (`mcp_broker.rs:1314-1360`). The
caps are in the text *and* in the payload, both from one reservation, "the text is what a person reads,
the payload is what a surface renders" (`:1311-1313`).

One honest gap to know about: the typed `createAgent` payload exists on the wire and in the TypeScript
types (`src/types/ipc.ts:116-137`) but **no component reads it** — the card renders through the
ordinary `PermissionCard`, so what a person actually sees is the daemon's description sentence and the
card's own allow/deny buttons.

**The agent profile store.** A creation names a *preset* today, but the list that will replace
presets already exists: `crates/devboule-daemon/src/agent_profiles.rs` holds the ordered list of
profiles a human ticked, plus one block of standing instructions. It is one JSON document beside the
journal, written the way the tool policy file is written — a create-new temp file, a current-user-only
DACL applied on Windows **before its first byte**, then a rename over the target (`:11-12`). A crash
therefore leaves the old list or the new one and never half of either, and a document that decides
what an agent may do is never briefly readable by another user.

It is read **at the moment it is used** — at the moment a creation resolves a profile, and at the
moment the standing instructions apply (`:6`) — so a human's edit takes effect on the next creation
rather than on the next session. Both are capped: 8 KiB of standing instructions
(`MAX_STANDING_INSTRUCTIONS_BYTES`, `:72`), well under the frame cap (`:59`).

The failure direction is the part worth keeping in mind. A document that cannot be read, parsed or
admitted is **quarantined** exactly as a corrupt tool-policy file is — renamed aside first, never
deleted (`:144`, `:157-169`, `:501`) — and the store then holds an **empty list and empty standing
instructions**, never the last good copy (`:22-25`). An unreadable file means "no agent may be created
and no standing instructions", not "carry on with what we had": see §5, where the permission dimension
is closed on purpose. A profile's **id** is what a running child records, so a rename cannot move a
running child onto a different profile (`:331`).

**The finish report and the deposit.** When a child finishes, the daemon reports to the creator
(`crates/devboule-daemon/src/session.rs:6009`, `:6027`), publishes `ChildFinished` (`:6106`, envelope
`:6351`, with the state derived from the child provider's own stop reason `:6433`) and deposits the
child's whole last `AgentMessage` into the **creator's** folder (`:4246`) as markdown named
`agent-finished.md` (`:4260-4264`), refusing anything over the artifact cap (`:4254`). The artifact is
named by reference and never by path: `devboule-attachment:<sessionId>/<digest>` (`:4269-4272`), which
is exactly the reference type the wire carries (`FinishArtifact { artifact_id, parts }`,
`crates/devboule-protocol/src/session.rs:669-672`, its parts at `:644-659`; the event at `:853`). The Design surface records a
finished child in history by that reference rather than a second copy
(`src/features/design/childFinishedHistory.ts:80`).

**Supervision: what a creator can see, and one notice it is sent.** `devboule_agent_activity` answers
for one live agent of the same owner — the roster's scope, because reading is not destroying and the
roster already lists them. The answer is metadata only: the derived headline (`working` / `idle` /
`blocked` / `unknown`), the hook's own last state beside it, the idle age, and a bounded tail of
recent event *kinds* with their sequence numbers and timestamps. No transcript text, ever. The
headline is derived from facts the daemon holds — liveness, a running turn, a parked permission card
— and never merged with the hook map, which keeps its own `seq` discipline
(`crates/devboule-daemon/src/agent_activity.rs`). The recent tail is an in-memory ring of 64 marks,
not a journal query: the read takes two locks and touches no rows.

A child that has been *working* with nothing published for `CHILD_QUIET_AFTER` (20 minutes) earns its
creator one envelope, `kind: agent_quiet`, once per quiet spell; movement re-arms it. It is a notice
and never an action: the child's turn, its cards and its brakes are untouched, and the test asserts
exactly that. This is also why it is delivered without steering. A steer's *refusal* path is an
interrupt, and an ACP creator cannot take a steer (`acp_client.rs` does not override `clone_steerer`),
so routing a routine notice through the steer path would have cancelled the creator's own turn and
dropped its pending cards — the most destructive act in the system, on the wrong session, every twenty
minutes. `deliver_notice_to_creator` exists so that cannot be reached by passing the wrong boolean.
What the notice cannot tell you is stated in the code: a model thinking hard, a long build and a
wedged process look identical from outside, which is the whole reason it reports and never acts.


**Lineage is daemon-written.** `created_by` is deliberately absent from `SessionCreate` so no client
can claim a parent, and `display_name` is set once at creation and is not renamable
(`crates/devboule-protocol/src/session.rs:220-235`).

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
| one store (one account) | 20 MiB | `crates/devboule-protocol/src/lib.rs:354` |
| attachments per prompt | 4 | `lib.rs:233` |
| one attachment's base64 data | 192 KiB | `lib.rs:242` |
| a prompt's inline attachments together | 384 KiB | `lib.rs:305` |
| stored references per prompt | 200 | `lib.rs:331` |

"This store" is one account's store: the runtime directory is `%LOCALAPPDATA%\Devboule`, so the budget
is per-user by construction and needs no key at all (`attachment_store.rs:22-29`). The header records
that a key once existed and was wrong, and it is worth reading before touching session ids: the middle
segment of a session id *looked* like an owner and is not one — it is one connection's client token cut
to sixteen characters, so one user running two clients would have had two budgets of twenty megabytes,
and two clients whose tokens share sixteen leading characters would have shared one (`:31-37`; the id
composition is `crates/devboule-protocol/src/ids.rs:59-71`, and §3 warns about the same field).

The running total is held in memory rather than walked per question, because a forty-page deck is two
hundred deposits and walking the store under its single write lock would put every session's attachment
work behind that walk; the tree stays the truth and the cache is derived from it (`:39-44`). A folder
the walk could not read makes the total *unknown* rather than zero, and an unknown total is refused —
"a number below the truth admits the bytes the limit exists to refuse" (`:44-47`).

**How a prompt names a stored file.** Inline bytes are `PromptAttachment { name, mime_type, data }`,
where `data` is base64 and the comment is explicit: "Never a path"
(`crates/devboule-protocol/src/messages.rs:110-115`). A stored file is named by reference instead, in
exactly one spelling — `devboule-attachment:<sessionId>/<digest>`
(`crates/devboule-protocol/src/session.rs:465`) — carried on the request as
`attachment_references` (`messages.rs:297`) and validated before use, including a total-size check
against the same 20 MiB budget (`crates/devboule-protocol/src/attachments.rs:210-248`). A deposit is
`ClientMessage::SessionDeposit` (`messages.rs:324`) and answers `SessionDeposited` with that reference
(`messages.rs:887`), which is the value the finish artifact of §7 reuses. Folders left by sessions that
never closed are swept at daemon start (`crates/devboule-daemon/src/server.rs:1346-1360`).

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
(`:60`, `:109-115`), and the commands are `surface_settings_get` / `surface_settings_set`
(`src-tauri/src/lib.rs:106-107`). A malformed file is an error that preserves the file rather than a
silent reset (`src-tauri/src/surface_settings.rs` tests, `:213`).

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
`client_read_loop` (`crates/devboule-daemon/src/client.rs:1351`), the handler calls are inline at
`:1372` (roster snapshots) and `:1400` (event envelopes), and the blocking roundtrip a handler would
enter is `roundtrip_with_deadline` (`:1055-1091`). The recommended fix is to enqueue to a
dedicated dispatcher instead of invoking handlers on the reader; a documentation-only prohibition is
insufficient for the attached Workspace flow. **As far as this tree shows, that fix has not been
made.**

**`session.rs` is one file of about 17,900 lines with no provider trait.** There are separate client
and view modules per provider family (`claude_client.rs`, `codex_client.rs`, `pi_client.rs`,
`acp_client.rs` and their `*_view.rs` counterparts), but the registry and everything kind-dependent
live in a single file that branches on `SessionKind` (`crates/devboule-daemon/src/session.rs:3167`,
`:4616`, `:6201`). The traits the daemon does define are for transports, the secret store, the
registry fetchers and the npm runner (`transport.rs:35`, `peer_transport.rs:702`,
`secret_store.rs:52`, `registry.rs:45`) — none of them is a provider abstraction. Anything that
touches all providers is therefore a search across 17,859 lines.

**There is no updater, so there is no update path.** No updater plugin in
`src-tauri/Cargo.toml:21-33`, `bundle.active = false` (`src-tauri/tauri.conf.json:70-71`). A daemon
speaking a different protocol version is refused rather than replaced
(`crates/devboule-protocol/src/handshake.rs:110-131`).

**There is no crash-loop brake.** A daemon that dies immediately after spawn is re-spawned by the
supervisor on its next iteration, paced only by the loop sleep and the 50 × 100 ms connect retries
(`src-tauri/src/client/mod.rs:1339-1369`; `crates/devboule-daemon/src/client.rs:29-30`).

**`resume` exists only for ACP providers.** `resume_handle` refuses Codex outright and excludes Pi
deliberately (`crates/devboule-daemon/src/session.rs:8039-8065`), so a Claude, Pi or Codex session
that was lost with its process cannot be brought back by the app — only replayed.

Six further questions that no code here answers. They are recorded as questions because that is what the evidence supports:

1. **App update vs a running older daemon.** With no updater and no "kill on version mismatch" path,
   what is supposed to happen when a newer app is installed while an older daemon still owns the pipe?
2. **Whose Job Object the daemon is in.** `spawn.rs:39-40` says the daemon "is allowed to die when
   Windows tears down this job", but no code in `src-tauri/src` creates a job or assigns the daemon to
   one — the spawn path sets only `CREATE_NO_WINDOW` (`spawn.rs:76-80`). If the app is not itself in
   such a job, a killed app is expected to leave the daemon running with its live sessions.
3. **Reattach after an undelivered `Shutdown`.** The app waits 1.5 s
   (`src-tauri/src/client/mod.rs:27`) and then exits; the daemon, holding a live session, never
   idle-exits (`server.rs:415`), so the next start rejoins it. Intended, or should the app prove the
   daemon is gone first?
4. **What ends a running session.** In this code only an explicit `SessionStop` / `SessionClose`
   (`session.rs:3729`, `:3827`) or the process dying does. There is no idle or quota policy for live
   sessions; silence produces a banner, not an end (`session.rs:182`). Whether that is the rule, or a
   policy is still to be written, is not stated anywhere in the tree.
5. **Two app instances.** A second app process computes the same pipe name and connects first
   (`paths.rs:75-77`), the daemon accepts up to sixteen pipe instances
   (`transport/windows_pipe.rs:49`), and sessions are scoped to the owner user. Whether a second app
   instance should see and drive the first one's sessions, or "one app at a time" is assumed somewhere
   outside this code, is not stated.
6. **The spawn-then-assign window.** A child is assigned to its job right after spawn; closing the
   window completely would need `CREATE_SUSPENDED`, which portable-pty does not expose
   (`session.rs:7253-7257`). Whether that window is covered by a test that kills the daemon inside it,
   or is accepted, is not recorded.

One more gap, found while reading and not from a report: the typed `createAgent` payload has no
consumer in the UI (§7), and the MCP broker — and therefore every agent-to-agent feature — is
registered for ACP and Claude sessions only (`session.rs:3217`, `:7449`).

---

This document describes what the code does, not what anyone intended. The intent lives in design
notes that are **not part of this repository**, and in the headers of the daemon's own modules, which
are unusually forthcoming about what was measured and what was decided against. Where a module header
and this document disagree, the header was written first and this document is a reading of the code.







