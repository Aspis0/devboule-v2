# Devboule

**A desktop workspace for the coding agents you already have.**

Devboule does not ship a model, an agent, or an API key. It drives the coding
agent CLIs already installed on your machine — the ones you use in a terminal
today — and gives them a place to live: real terminals, sessions owned by
something other than the window they happen to be drawn in, and a view of what
each agent is actually doing.

The idea is that the interesting part was never the chat box. It is everything
around it: knowing where an agent is working, what it changed, whether it is
still alive, and being able to walk away and come back.

Built with Rust and TypeScript. Tauri v2 for the shell, React for the surface,
a background daemon in Rust for anything that has to outlive the window.

## Status

**Early, and openly unfinished.** This repository is under active development
and nothing here is stable yet — not the interfaces, not the storage formats,
not the layout. It is public because there is no reason for it not to be, not
because it is ready to use.

What works today:

- **Workspace** — the main surface, with terminal sessions attached to real
  PTYs and agent sessions owned by the daemon. The tab strip also carries
  sessions recovered from a daemon that died, so their transcript is one click
  away instead of buried in History. Swiping a tab reveals what it would do —
  archive to the right, delete to the left — and neither happens immediately:
  the tab goes away, a five-second undo window opens, and only when it expires
  does the daemon hear about it. Archiving kills the process and keeps the
  session and its transcript; deleting removes the row.
- **Terminal sessions** — owned by a background daemon rather than by the view,
  so moving to another surface, detaching and coming back leaves the process
  untouched. Output is journalled, so reattaching replays what was missed
  rather than showing a blank screen.

  On a clean quit Devboule asks the daemon to shut down, and the daemon flushes
  the journal before it accepts. If the app instead goes away without asking — a
  crash, a kill — the daemon keeps running for as long as a session is alive,
  and the next start rejoins that daemon rather than starting a second one.
  Either way the transcript survives: a session whose process is gone comes back
  as `Recovered` and replays from the journal rather than pretending to be
  alive. The process does not come back with it. Every provider lives inside a
  Windows job object that kills it when the daemon exits, so resuming a
  provider's own session is a separate, explicit act — replaying is free and
  happens by itself, resuming starts a process and waits for you to ask. It
  exists today for **ACP, Claude, Codex and Pi** sessions: Claude resumes by
  handing the CLI back its own history file, Codex by asking the app-server for
  its thread id, Pi by handing its CLI the session id it keeps on disk.
- **Agent conversation** — real IPC sessions on the daemon. Claude, Codex
  (app-server) and pi run through native adapters; other agents speak ACP.
  Each provider declares its own permission modes (for Claude: plan, always
  ask, accept edits, auto, bypass; for Codex: read-only, auto, auto-review,
  full access) and the session starts in the mode you pick. Permission
  requests are brokered by the daemon and answered from one card in the chat,
  which shows the agent's own description of the command, never the
  provider's internal wording. Tool calls render as rows with an icon, a
  summary, the files they touched and a failure mark; consecutive tool calls
  collapse into one row with a summary such as "Edited 1 file, ran 2
  commands". Known limit, measured on Windows: in Codex's default (sandboxed)
  mode the shell fails to start inside Codex's own Windows sandbox, so no
  command runs; full access works. This is Codex-side (the same sandbox fails
  from the Codex CLI too), and the app does not work around it.
- **Paired devices** — real. A laptop or phone pairs over a Tailscale tailnet
  with an eight-character code shown on one side and typed on the other; the
  code expires, wrong attempts are rate-limited, and the two sides then prove
  they hold it through a PAKE before exchanging identities inside a Noise
  session. Every later connection is authenticated by the key pinned
  at pairing *and* by Tailscale agreeing that the address is the one the pairing
  was made from. Each paired device holds a set of capabilities — `view`,
  `send`, `answer_permissions`, `create_sessions`, `roster`, `search` and
  `admin` — shown
  as toggles on its row and revocable at any time. A new pairing starts with all
  of them, so a paired phone is a full client: it lists and attaches to
  sessions, sends prompts and steers a running turn, answers permission cards
  (at most three undecided ones per device), creates sessions, and, with
  `admin`, reaches the rest of what the app can ask — settings, projects,
  journal, shutdown, the agent tools. The Oracle semantic search is not part of
  that remainder: it rides its own `search` switch, so a paired device's agent
  may semantically search this machine's code — snippets, paths, line ranges —
  only while that switch is on for that device. A new pairing is born holding
  `search` too (the same "the phone is mine" default as everything else), and
  the switch is how it is taken back from one device alone. The one thing it
  cannot do is change who
  else may enter: starting or completing a pairing, changing a device's
  capabilities and revoking a device stay on this machine, because those decide
  which devices are trusted rather than what a trusted one may do. Without
  `admin` a device is back to the act-named capabilities alone, and a device
  whose toggles are all off can still view its own sessions.
  Both ends need Tailscale; without it the daemon stays local and says so in
  Settings rather than failing to start.
- **Agents that talk to each other** — the daemon owns a loopback MCP endpoint
  and offers each ACP and Claude session three tools: list the other live agent
  sessions, send one of them a message, and create one. A per-provider tool
  policy decides which of those a provider's sessions are actually served; it
  is stored beside the journal as `tool-policies.json`, written atomically and
  readable only by the current user, and the daemon re-reads it on every call,
  so a toggle takes effect on the agent's next call rather than at its next
  session. The roster tool cannot be turned off: an agent that cannot list its
  siblings cannot be steered at all. The policy is per machine — paired devices
  do not inherit one another's.
- **Agents that create agents** — an agent can call `devboule_create_agent` to
  create a child from one of two presets (`worker`, `design`) in a fixed,
  non-bypass provider mode, and it must ask you first: once per creator session,
  on a card that states the provider, the preset, the mode and every cap as
  `n of m`. The caps are real and are checked before the card is drawn — three
  live children per creator, ten creations an hour, eight live agent sessions
  daemon-wide, two levels of nesting — and the child runs in the caller's
  workspace or the call is refused. When the child finishes, its final message is
  deposited as an artifact in the *creator's* folder and the creator is told.
  The card is the ordinary permission card, so the caps are legible as the
  sentence the daemon writes rather than as a separate panel.
- **Attachments** — images, SVG and markdown are deposited into a store under
  the daemon's runtime directory and never into the workspace, where an
  attachment would look like your own edit and could be committed by accident.
  A stored file is named by the SHA-256 of the bytes actually written, with
  image identity metadata stripped first, and never by the file name you chose:
  the same bytes are the same file, and a prompt or a finish artifact refers to
  it by reference (`devboule-attachment:<session>/<digest>`) rather than by a
  path. The store has a 20 MiB budget; a deposit that would exceed it is refused
  rather than silently truncated.
- **History** — the persisted session journal, with grouping, retention notices,
  deletion, and `Reopen` for resumable ACP sessions.
- **Provider inventory** — the daemon discovers known agent CLIs from `PATH`;
  Settings can refresh the catalog and reports the last measured start outcome.
- **Surface registry** — six surfaces are registered: `workspace`, `polis`
  (supplied by a plugin, so it is absent until that plugin is installed),
  `pubvia` (still a placeholder), `design`, `settings`, and `marketplace`.
- **Plugin host** — installed plugins run in a cross-origin frame and can use
  an out-of-process backend over the plugin RPC protocol.
- **Projects and workspaces** — register a folder with the native picker and it
  is persisted by the daemon, recognised as a git repository or not, and
  survives a restart. Sessions started inside a workspace run in that folder:
  the frontend sends the workspace id, session kind, and selected provider;
  it never sends a PATH. The daemon resolves the directory, then echoes back
  the path the process actually received.
- **Design** — a chat grounded in the repository by Oracle, driving that same
  agent, with a canvas that renders what the agent produced inside a sandboxed
  frame. It chooses its agent and model from the same provider inventory the
  Workspace uses, and reports what it wrote from the structured tool events the
  protocol carries rather than by reading the agent's prose. Finished work
  survives a restart: the history stores an index into the journal that already
  holds the transcript, not a second copy of the artifact. The canvas is for
  looking and pointing, not for dragging. Its layers are the repository's own
  components, each carrying the file it came from, so a request can be scoped to
  one of them. They are drawn as labelled rectangles rather than previews:
  rendering a component means compiling it, and there is no bundler in a
  packaged app. See
  [`src/features/design/README.md`](src/features/design/README.md) for what else
  is not real yet.
- **Settings** — provider inventory, the per-provider tool policy, Oracle
  administration, Projects and Devices are all wired.

What is still mock: the Workspace side panels — Changes, Files, the interactive
app preview and the pull request panel — render hardcoded examples and say so on
screen; there is no git status or file tree on the wire yet. Worktree isolation
now exists in the daemon, which can add a `git worktree` beside the project and
refuses only when the project's live git state cannot host one, but nothing in
the app asks for one yet, so every workspace is currently the project folder
itself.

Developed and tested on Windows. Tauri itself is cross-platform, but no other
platform has been verified, so treat them as unsupported for now.

## Oracle

Oracle is the foundation the rest leans on: a local RAG over the code and files
in your repositories, meant to help agents find their way around a codebase
instead of guessing at it. Ask where something lives, what calls what, or which
file to open first, and get an answer grounded in citations you can follow.

It is meant to be suggestive rather than binding. An agent is free to ignore it
and read the repository itself, the way it would without any of this. Oracle
points at places worth opening; it does not stand between the agent and the
files, and it is never the authority on what the code says — the code is. A
stale index should cost a wasted lookup, never a confident wrong answer.

It is meant to run locally: indexing and retrieval on your machine, with the
index belonging to the project it describes.

The Oracle panel is wired to the local engine. It downloads the models it needs,
indexes the selected folder, and returns ranked source pointers through typed
Oracle IPC.

Agents reach Oracle through the Devboule MCP broker, and the two halves have
different requirements. The project-graph tools (`devboule_project_neighborhood`,
`devboule_project_imports`, `devboule_project_importers`) are answered by the
Daemon and work with the desktop app closed. The semantic search tool
(`devboule_oracle_search`) is answered by this app's own engine: the index and
the local models live here, so it works only while the desktop app is running -
leave it open, or minimize it to the tray. With the app closed the tool says so,
in a sentence that names the app; it does not fail silently and it never answers
from another project's index.

A paired device reaches the same tool only while that device holds the `search`
capability: on by default at pairing, toggled per device in Settings - Devices,
and refused by name (`capability 'search' was not negotiated`) when it is off —
local sessions on this machine are never judged.

### Using Oracle

Oracle is for the person reading a codebase, not just for configuring the app:

1. Choose the folder you want Oracle to understand.
2. Oracle automatically downloads its two local models — about **34 MB** for
   embeddings and **5 MB** for reranking. They land in
   `<the folder you chose>/oracle-data/models/`, so each indexed folder keeps
   its own copy. This repository already carries that copy for itself, which
   is most of its clone size; the weights are third-party and are attributed
   in [THIRD_PARTY.md](THIRD_PARTY.md).
3. Start the first index pass. Reading and chunking a large folder can take
   several minutes.
4. When indexing finishes, ask a question in natural language. Oracle returns
   file paths, line ranges, and source spans to open; it does not generate an
   answer in place of the source.

If setup fails:

- **No network:** reconnect, then retry the model download.
- **Folder unreadable:** choose a directory you can open and read.
- **Disk full:** free at least 40 MB, then retry.

The panel keeps these failures visible and distinguishes an empty index from a
query with no matching source spans. If the optional reranker is unavailable,
Oracle still searches densely and says so explicitly; retry it when the model
download can complete.

### Optional query-time reranking

Oracle automatically downloads the reranker alongside the embedding model on
first setup. It reorders the dense candidate set with a local ONNX
cross-encoder. The bundle is loaded from
`<oracle-data-root>/models/ms-marco-TinyBERT-L-2-v2`, or from
`ORACLE_RERANKER_MODEL_DIR`. If that directory is absent, retrieval is exactly
the dense path and the stored index is unchanged; the panel reports this
degraded mode rather than hiding it. `ORACLE_RERANK_CANDIDATES` controls the
query-time depth (default 50).

The model bundle must describe its own non-inferable facts in
`model_config.json`, for example:

```json
{
  "id": "ms-marco-TinyBERT-L-2-v2",
  "onnx_graph": "onnx/model_int8.onnx",
  "tokenizer_file": "tokenizer.json",
  "max_seq_tokens": 512,
  "pair": {
    "mode": "tokenizer_pair",
    "first": "query",
    "second": "document"
  }
}
```

The reranker is query-time metadata and is intentionally not part of the
embedding recipe: it does not change vectors, ANN contents, or index
compatibility.

## Polis

Polis draws a repository as an isometric city. Directories become districts,
files become buildings, dependencies become roads. As the code changes, the
city changes with it.

It sounds like a toy and it is partly a toy, but a large codebase is genuinely
hard to hold in your head, and a map you can recognise at a glance turns out to
be a real way to navigate one — and a way to point an agent at a place rather
than a path.

Polis is ported as an out-of-process plugin, installed as files rather than
compiled in — which is the point: it exercises the path by which the app loads
code it did not build. `PolisSurface` mounts the installed `plugins/polis` build
in a cross-origin iframe served from `http://plugin.localhost`, and
`crates/polis-backend` is the plugin backend process for the city graph and
Augur findings. Buildings are files, sized by line count and tinted by top-level
folder; roads are imports. The city it draws is this repository.

The plugin currently draws a repository fixture extracted by
`plugins/polis/scripts/extract-city-fixture.mjs`. The host's real CKG is not yet
delivered over the plugin bridge.

## Plugins

Devboule is meant to host surfaces that have little to do with each other, run
out of process, and can be developed independently. Polis is one. The next is
**Pubvia**, a tool for academic writing — document model, citation handling,
DOI and Crossref lookups, Word export — which has no relationship to writing
software at all.

That is the point. The plugins these tools usually offer are variations on the
same theme: another model provider, another git view, another diff. The ones
here are meant to be genuinely different kinds of work sharing one shell.

Pubvia is not available yet.

## Building from source

Requirements are pinned in the repository: Node **26.8.1** (`.nvmrc`), pnpm
**10.33.2** (`packageManager`), Rust **1.98.0** (`rust-toolchain.toml`). Tauri
also needs the usual platform prerequisites — see the
[Tauri v2 prerequisites](https://v2.tauri.app/start/prerequisites/).

```sh
pnpm install
pnpm tauri dev      # run the app
```

Day to day:

```sh
pnpm build          # type-check and build the frontend
pnpm lint           # oxlint
pnpm test           # vitest
pnpm run format:check
cargo test --workspace --exclude oracle-core --locked
```

Note the `--exclude oracle-core`. That crate is built and tested on its own,
serially:

```sh
cargo clippy -p oracle-core --locked --all-targets -- -D warnings
cargo test -p oracle-core --locked -j 1
```

The `-j 1` is deliberate. At full parallelism `rustc` has crashed while linking
its integration-test binaries, so `cargo test --workspace` on its own is not a
command that finishes here.

Before opening a pull request, these are the checks worth running locally. They
are not everything CI does — it also runs `cargo audit`, `pnpm audit`, a
third-party licence check, a separate Polis plugin job, and a scheduled job for
the ignored tests. `.github/workflows/ci.yml` is the authority.

```sh
pnpm exec tsc --noEmit
pnpm run lint
pnpm run test
pnpm exec vite build
pnpm run format:check
pnpm run check:shrinkwrap
pnpm run check:dependency-majors
cargo fmt --all -- --check
cargo build --workspace --locked
cargo check -p devboule-daemon --no-default-features --locked
cargo clippy --workspace --exclude oracle-core --all-targets --all-features --locked -- -D warnings
cargo test --workspace --exclude oracle-core --locked
cargo clippy -p oracle-core --locked --all-targets -- -D warnings
cargo test -p oracle-core --locked -j 1
```

The direct dependency major-version check compares npm dependencies with the
registry `latest` tag and Cargo dependencies with crates.io
`max_stable_version`. It covers only direct dependencies declared by this
repository. A crate with no stable release is reported explicitly and is not
compared to a major version; registry failures fail the check.

If a deliberate exception is ever needed, add it inline in
`scripts/check-direct-dependency-majors.mjs` under the relevant `npm` or
`cargo` map. Each entry must include both `reason` and `exitCondition`; the
exception output must state why the lag exists and what will allow its removal.

The Cargo workspace holds the Tauri application (`src-tauri`) plus six crates:
`devboule-protocol` for shared wire types, `devboule-daemon` for the local
daemon and its session and provider runtime, `oracle-core` for local indexing
and retrieval, `devboule-augur` for repository review detectors and findings,
`devboule-plugin-rpc` for the host-to-plugin-backend protocol and lifecycle, and
`polis-backend` for the Polis plugin backend's city graph and Augur findings.
They are inventoried in [`crates/README.md`](crates/README.md). The division
that matters is that plugin work runs in its own process: the Polis backend and
the review detectors it uses are separate binaries reached over a pipe, never
linked into the host.

## Licence

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

Third-party dependencies, their licences and the attribution required by them
are inventoried in [THIRD_PARTY.md](THIRD_PARTY.md), which covers linked
crates and packages as well as the fonts and assets bundled in the
application.
