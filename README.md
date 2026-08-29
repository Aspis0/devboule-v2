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
  PTYs.
- **Terminal sessions** — owned by a background daemon rather than by the view,
  so moving to another surface, detaching and coming back leaves the process
  untouched. Output is journalled, so reattaching replays what was missed
  rather than showing a blank screen.

  Sessions do not outlive the application yet: on exit, Devboule asks the
  daemon to shut down. Surviving a full restart is the next step, and it is
  what the daemon and the journal were built for.
- **Settings** — providers, projects, devices and Oracle administration.

What is drawn but not wired: the agent conversation and the provider inventory
still read from fixed sample data. The boundary between those and real IPC is
deliberate and marked in the source.

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

### Using Oracle

Oracle is for the person reading a codebase, not just for configuring the app:

1. Choose the folder you want Oracle to understand.
2. Oracle automatically downloads its two local models — about **34 MB** for
   embeddings and **5 MB** for reranking.
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

Not yet ported into this repository.

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

Requirements are pinned in the repository: Node **26.7.0** (`.nvmrc`), pnpm
**10.33.2** (`packageManager`), Rust **1.97.1** (`rust-toolchain.toml`). Tauri
also needs the usual platform prerequisites — see the
[Tauri v2 prerequisites](https://v2.tauri.app/start/prerequisites/).

```sh
pnpm install
pnpm tauri dev      # run the app
```

Other useful commands:

```sh
pnpm build          # type-check and build the frontend
pnpm lint           # oxlint
pnpm test           # vitest
pnpm run check:dependency-majors
cargo test --workspace
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

The Cargo workspace holds the Tauri application (`src-tauri`) plus two crates:
`devboule-protocol` for the wire types shared with the daemon, and
`devboule-daemon` for the session host itself.

## Licence

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

Third-party dependencies, their licences and the attribution required by them
are inventoried in [THIRD_PARTY.md](THIRD_PARTY.md), which covers linked
crates and packages as well as the fonts and assets bundled in the
application.
