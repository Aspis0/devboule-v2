# Devboule Plugin Development Guidelines

> Audience: anyone who wants to build a plugin for Devboule — from a static
> tool panel to a full domain application (like Polis or an image-analysis
> studio) that embeds into the Devboule shell.
>
> Status: v1 draft, 2026-09-07. Authored from a full read of the plugin stack
> (`src-tauri/src/plugins/*`, `crates/devboule-plugin-rpc/*`,
> `src/features/plugins/*`) and the daemon agent surface
> (`crates/devboule-daemon/*`). Every claim is backed by file:line evidence
> gathered by explorers; where the platform has a GAP, this document says so
> explicitly and proposes the capability roadmap instead of pretending.

---

## 0. Ecosystem philosophy (read this first)

Devboule is an orchestrator base + specialist apps that live inside it. The
marketplace model (see `marketplace-ideas.md`) is deliberately conservative:

| Good | What it is | Price | Review bar |
|------|-----------|-------|-----------|
| **Skill** | One `SKILL.md` (+ optional scripts), loaded in the workspace | Free | Automated scan if it ships `scripts/` |
| **Skill pack** | A curated bundle of skills that work together | Cheap one-shot | Read as a unit, no silent network |
| **Plugin** | An **out-of-process app**: UI in a sandboxed iframe + optional native backend binary, a surface in the shell | Software pricing | Human review, same confinement as first-party |

A plugin is not a prompt. It is a process the app runs and an iframe the app
serves. The trust bar is correspondingly higher, and the platform enforces it
mechanically (digest verification, capability negotiation, sandboxing) — not
by promising.

---

## 1. What a Devboule plugin is

```
Devboule shell (Tauri)
│
├── PluginRegistry ──── scans <app-data>/plugins/ at startup
│     scan → parse plugin.json → list files → SHA-256 verify every digest
│     (symlinks/junctions rejected, ≤10,000 files, ≤2 GiB total, ≤64 MiB/asset)
│
├── Asset server ────── custom scheme: http://plugin.localhost/{pluginId}/{path}
│     serves ONLY files listed in the manifest (digest-verified)
│     per-plugin CSP, CORS header for ES modules, /__selftest.js probe
│
├── Plugin UI ───────── cross-origin <iframe sandbox="allow-scripts allow-same-origin">
│     │                 loaded from plugin.localhost — NO Tauri IPC access
│     │  postMessage    versioned bridge { v:1, id, kind: invoke|result|error }
│     ▼
├── Host bridge (pluginBridge.ts) ── allowlist: sessions.watch, oracle.search
│     │                                  (unknown methods → plugin backend pipe)
├── Backend process ─── OPTIONAL native exe spawned by the host
│     │                 named pipe, framed protocol, capability negotiation
│     ▼                 Windows Job Object (KILL_ON_JOB_CLOSE) = orphan-proof
│   (your backend serves methods to the HOST — e.g. city.get, findings.get)
│
└── Daemon (named pipe) ── sessions, provider catalog, oracle
      (currently Tauri-only; see §7 for the plugin-facing roadmap)
```

Key facts an author must internalize:

1. **Two processes, three trust boundaries.** Your UI runs in a sandboxed
   cross-origin iframe; your backend (if any) is a separate exe the HOST
   spawns; the daemon is a third process you never talk to directly today.
2. **Everything is capability-gated.** Your manifest declares capabilities;
   the host grants a subset (intersection at handshake; bridge allowlist for
   the UI; project-open conditions for workspace caps).
3. **Digests are the trust root.** Every file's SHA-256 lives in
   `plugin.json`; the asset server refuses anything not listed. No digest, no
   serving — this is why the manifest must be generated, never hand-edited.

---

## 2. Plugin anatomy

```
my-plugin/
  plugin.json              # manifest — the ONLY recognized manifest name
  ui/
    index.html             # UI entry (must be .html/.htm, must be in files)
    index.js               # any modules/assets your UI needs
  [my-backend.exe]         # optional backend binary (entry.backend)
```

### Manifest schema (manifestVersion 1)

```jsonc
{
  "manifestVersion": 1,              // REQUIRED, must be 1
  "id": "my-plugin",                 // REQUIRED, == directory name,
                                     // (?=.{1,64}$)(?!-)(?!.*-$)[a-z0-9-]+
                                     // (leading/trailing dash rejected)
  "name": "My Plugin",               // REQUIRED, ≤128 chars, no control chars
  "version": "0.1.0",                // REQUIRED, non-empty, <=128 chars;
                                     // semver is a CONVENTION, not validated
  "entry": {
    "ui": "ui/index.html",           // REQUIRED (.html/.htm, must be in files)
    "backend": "my-backend.exe"      // OPTIONAL (must be in files if present)
  },
  "capabilities": ["oracle.search"], // OPTIONAL, ≤64 names (open set, see §6)
  "files": {                         // REQUIRED, EXHAUSTIVE, lowercase hex SHA-256
    "ui/index.html": "d85909…",
    "ui/index.js":  "6582cb…"
  }
}
```

Platform bounds (`src-tauri/src/plugins/manifest.rs`):

| Bound | Value |
|-------|-------|
| Manifest size | 1 MiB |
| Files per plugin | 10,000 |
| Total plugin size | 2 GiB |
| Per-asset size | 64 MiB (backend binary exempt) |

### Rules that get plugins rejected

- Directory name ≠ `id` → rejected at discovery.
- A file on disk NOT listed in `files` → rejected (the scan is exhaustive).
- Symlink/junction anywhere in the tree → rejected (NTFS reparse points
  included).
- Digest mismatch → rejected before anything is served.
- Symlink escape, `..` segments, backslash/colon in asset paths → refused at
  the asset server, per request.

---

## 3. Quickstart: the minimal UI plugin (3 files)

Copy the shape of `plugins/hello/`:

1. Create the directory and manifest:

```json
{
  "manifestVersion": 1,
  "id": "my-plugin",
  "name": "My Plugin",
  "version": "0.1.0",
  "entry": { "ui": "ui/index.html" },
  "capabilities": [],
  "files": {
    "ui/index.html": "<sha256>",
    "ui/index.js": "<sha256>"
  }
}
```

2. Write `ui/index.html` — copy-paste starting point:

```html
<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><style>
  body { margin: 0; font-family: system-ui, sans-serif; background: #F5F1E8;
         color: #1C1A17; padding: 16px; }
  button { padding: 8px 14px; border-radius: 8px; border: 0;
           background: #C8532B; color: #fff; cursor: pointer; }
</style></head>
<body>
  <h1>My Plugin</h1>
  <button id="go">Ping the host</button>
  <pre id="out"></pre>
  <script type="module" src="./index.js"></script>
</body>
</html>
```

   and `ui/index.js` — the bridge call pattern (versioned postMessage):

```js
let seq = 0;
const pending = new Map();

export function invoke(method, payload = {}) {
  const id = crypto.randomUUID();
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    window.parent.postMessage(
      { v: 1, id, kind: "invoke", method, payload },
      "http://tauri.localhost",          // the shell's origin
    );
  });
}

window.addEventListener("message", (event) => {
  if (event.origin !== "http://tauri.localhost") return;  // origin gate
  const msg = event.data;
  if (msg?.kind !== "result" && msg?.kind !== "error") return;
  const p = pending.get(msg.id);
  if (!p) return;
  pending.delete(msg.id);
  msg.kind === "result" ? p.resolve(msg.value) : p.reject(new Error(msg.message));
});

document.getElementById("go")?.addEventListener("click", async () => {
  try {
    const feed = await invoke("sessions.watch", {});
    document.getElementById("out").textContent =
      JSON.stringify(feed, null, 2);
  } catch (e) {
    document.getElementById("out").textContent = String(e);
  }
});
```

   (Empty `capabilities: []` means the host denies `sessions.watch` — declare
   the capabilities you call. `ping` needs nothing.)

3. Compute digests and fill `files` (PowerShell:
   `Get-FileHash -Algorithm SHA256 <file>`; POSIX: `sha256sum <file>`).

4. Install: either drop the directory into `<app-data>/plugins/` (the scan
   picks it up on next launch), or call the `plugin_install` Tauri command —
   `{ id: "my-plugin", source: "C:/path/to/my-plugin" }` — which stages,
   verifies, and atomically swaps.

5. Debugging: the plugin inventory reports WHY a plugin was refused
   (manifest parse error, unknown field, digest mismatch, unlisted file,
   symlink) — the strings come from `manifest.rs` / `discovery.rs` and are
   surfaced by the plugin management UI. Install location: Devboule's
   app-data dir, `plugins/` subdirectory (on Windows:
   `%APPDATA%/<app-identifier>/plugins/<id>/`).

Your UI is now served at `http://plugin.localhost/my-plugin/ui/index.html`
(Windows shape; `plugin://localhost/…` on macOS/Linux) and mounted by the
shell in a sandboxed iframe.

**You can go surprisingly far with UI-only**: everything client-side works
(fetch to external APIs is subject to the per-plugin CSP — `connect-src
'self'` by default, so plan for a backend if you need network).

---

## 4. Real application plugins (the Polis pattern)

When your tool needs a build chain, assets, and/or a native backend, follow
`plugins/polis/`:

1. **Own `package.json`** — Vite + TypeScript, standard tooling, tests with
   Vitest. Keep the plugin self-contained (own lockfile).
2. **Build to `dist/`**, then **stage** into a directory named after the
   plugin id and **generate the manifest** — never compute digests by hand.
   The repo ships a shared producer: `scripts/make-plugin-manifest.mjs`
   (stages build output + copies the backend binary + writes `plugin.json`
   with all digests). Polis wraps it in
   `plugins/polis/scripts/write-plugin-manifest.mjs` with a `manifest` npm
   script. Make `manifest` part of your `build` pipeline so digests can never
   drift from artifacts.
3. **Backend binary** (optional): a Rust binary using
   `crates/devboule-plugin-rpc` (`PluginBackend::listen(pipe_name)`), speaking
   the framed protocol. The host spawns it suspended, assigns it to a Job
   Object, connects over a unique named pipe, sends `Hello` with the granted
   capabilities, and you reply with yours. Serve `Invoke` requests; keep
   payloads ≤1 MiB in both directions.
4. **Tests**: Polis runs Vitest with happy-dom — plugin UIs are testable like
   any frontend.

Polis is the reference implementation — read it before inventing structure.

---

## 5. The host bridge (UI ↔ shell)

Your iframe talks to the shell over a versioned `postMessage` protocol
(`src/features/plugins/pluginBridge.ts`):

```
{ v: 1, id: "<uuid>", kind: "invoke", method: "...", payload: {...} }
→ { v: 1, id: "<same>", kind: "result"|"error", value?|message? }
```

Events arrive as `{ v:1, kind:"event", id: "<subscription-id>", event:
"sessions.update", value: { sessions: PluginSession[] } }` — note the event
`id` identifies your SUBSCRIPTION (echo it in the unsubscribe invoke), and
both events and replies carry data in `value`, not `payload`. Replies:
`{ v:1, id: "<request-uuid>", kind:"result", value: {...} }` or
`{ v:1, id, kind:"error", message }`.

### Currently served methods

| Method | Behavior | Limits |
|--------|----------|--------|
| `sessions.watch` | Roster snapshots (`PluginSession[]`) pushed every 5 s | State changes only — no live transcript streaming (yet, §7) |
| `oracle.search` | Query routed to the Oracle index | 1 in flight; query ≤4096 chars; 20 s timeout |
| *(anything else)* | Routed to YOUR backend over the named pipe (`plugin_invoke`) | Method must be covered by a capability in your manifest; backend must be spawned (the shell leases it automatically while the surface is open) |

Bridge integrity: every inbound message is checked at THREE checkpoints —
(1) `origin` AND `event.source === iframe.contentWindow` match,
(2) the method must appear in the manifest's declared capabilities,
(3) the Rust side re-checks the grant at invoke time (`method_is_granted`).
The host never calls into your frame except via `postMessage`.

### Surfaces (nav integration) — current limitation, read carefully

Plugin surfaces in the shell nav are **not self-registering** today:
`SurfaceKey` is a hardcoded 5-member union (`workspace`, `polis`, `pubvia`,
`design`, `settings`) with fixed arc positions (`src/types/surface.ts`,
`src/app/Shell.tsx`). Only definitions with a `plugin` field participate in
install/open flows (currently `polis` only), and the crescent geometry does
NOT compute positions dynamically (a sixth point at current spacing overshoots
the arc). Practically:

- Your plugin installs and runs; the USER reaches it through the plugin
  management UI, not through a nav point.
- Becoming a first-class nav surface is a host-side code change (extend
  `SurfaceKey` + `SurfaceDefinition` + make the arc dynamic). The planned
  Marketplace crescent point follows the same path.
- Design your UI to work as a full-surface app when given the slot — the
  component mapping (`SURFACE_COMPONENTS`) renders you as
  `<PluginSurface>` (full-area iframe).
- Your frame inherits NO host theming (cross-origin): bring your own visual
  system. Polis does exactly this.

### Practical guidance

- Derive provider info client-side when you only need display data:
  `deriveSessionProvider(title)` extracts it from session titles.
- Treat `sessions.watch` snapshots as eventual-consistent (5 s poll). Do not
  build tight-loop UIs on them.
- If your tool needs data the host does not serve, that is what the backend
  process is for — but note the backend serves the HOST, it cannot reach the
  daemon either (see §7).

---

## 6. Out-of-process backends (optional native superpowers)

Use a backend when your plugin needs: heavy computation (Pixi/D3 is UI-side;
parsing/segmentation/indexing belongs in the exe), access to local files the
user opened (via the `workspace.root` capability), or long-lived native state.

Contract:

- Implement `PluginBackend::listen(pipe_name)` from `devboule-plugin-rpc`,
  accept one connection, answer the handshake, then serve
  `ClientMessage::Invoke { id, method, payload }` with
  `DaemonMessage::InvokeResult { id, value }` / `Error`.
- Capability negotiation is an INTERSECTION: you declare what you can serve;
  the host grants what the manifest requested AND policy allows.
  `workspace.root` is granted only when a project is open; derived caps
  (`city.get`, `findings.get`, `finding.inspect` — Polis's set) require it.
- Lifecycle: spawned suspended → assigned to a Windows Job Object with
  `KILL_ON_JOB_CLOSE` (host exit kills you — do not fight it). Environment is
  sanitized (Oracle path overrides stripped). Reference-counted leases in the
  frontend (`acquirePluginBackend`/`release`) mean you can be started and
  stopped as surfaces open and close — persist your own state if you need it
  across leases.
- Ship the exe inside the plugin directory and reference it in
  `entry.backend`; the manifest producer copies the freshest build
  automatically (see Polis's selection script).

Windows-only today (named pipes). Cross-platform transport is a known gap.

---

## 7. Agent integration for domain apps (the roadmap)

This is the section for tools whose core is an AI agent — image analysis
studios, research assistants, code-review dashboards. It states honestly
what exists, what is missing, and the proposed capability additions.

### What the shell can do TODAY (Tauri layer)

| Surface | Where |
|---------|-------|
| Provider catalog: `providers_list` / `providers_refresh` / `provider_update` | `src-tauri/src/backend/providers.rs` → daemon `provider_catalog` |
| Session lifecycle: `session_create / resume / attach / send / interrupt / set_model / permission_respond / close`, `sessions_list / watch` | `src-tauri/src/backend/session.rs` → daemon `SessionRegistry` |
| Protocols: `claude` = stream-json; `grok/qwen/gemini` = ACP; `codex/pi` = not chat-capable (per `KNOWN_AGENTS`) | `provider_catalog::chat_protocol()` |
| Streaming: `SessionEvent` envelopes (22+ variants) over Tauri Channels; `generation` distinguishes process recreations | daemon → shell push |
| Permissions: `PermissionRequest` event → `session_permission_respond(AllowOnce | Deny)` | `PermissionBroker` |

### What a plugin can reach TODAY

| Need | State |
|------|-------|
| List agents (provider catalog) | ❌ Tauri-only (`sessions.watch` gives roster, not catalog) |
| One-shot agent request **with an image** | ❌ `session_send` is text-only, 64 KB, no attachment field in the protocol |
| Long interactive session (create/send/interrupt) | ❌ Tauri-only |
| Live event streaming (transcript, tool calls) | ❌ plugins get roster snapshots only |
| Permission prompts | ❌ not interceptable from a plugin frame |
| Oracle search / roster watch | ✅ bridge |

### The capability roadmap (proposed)

The protocol already declares the intent — `AGENT_RUN` exists as a capability
constant (`protocol/lib.rs:141`) but is granted nowhere. The platform needs
these host-side additions, in this order:

1. **`providers.list` (bridge method)** — read-only provider catalog through
   the existing daemon RPC. Smallest possible step; unblocks pickers.
2. **`agents.run` (capability + bridge method)** — managed one-shot requests:
   the host creates a hidden session (or uses a one-shot protocol adapter),
   streams transcript events to the plugin frame, enforces timeouts, and
   tears down. **Requires an attachment protocol first** — `SessionSend`
   needs a media field (`{ mime, bytes | path }`) or an out-of-band file
   channel; vision is the point for domain apps.
3. **`events.subscribe` (bridge method)** — live `SessionEvent` stream for
   plugin-owned sessions, with the same envelope semantics as the shell
   (generation-aware).
4. **`permissions.pass`** — permission requests routed to the owning plugin
   frame for user interaction, with the host still logging outcomes.
5. **Vision capability declaration** — extend `KNOWN_AGENTS`-adjacent data
   with a per-provider `image: bool` (ACP already carries
   `PromptCapabilities.image` at `initialize`; stream-json providers need a
   static flag or a probe), so pickers can badge VLM support instead of
   guessing.

Design constraints the additions must respect (non-negotiable, they are what
makes the trust model work):

- All agent traffic stays host-mediated. Plugins never get daemon pipe
  access (`ClientMessage::Invoke` is explicitly `Unimplemented` daemon-side).
- PROPOSED (not current behavior): sessions created by a plugin are
  plugin-scoped — visible in the roster (labeled), closed when the surface
  closes, counted against a concurrency budget.
- Attachments are path-based (host-copied into the session workspace), size-
  and MIME-capped — the plugin never streams raw bytes into the daemon.
- Every new capability is deny-by-default and appears in the manifest.

### Interim patterns (until the roadmap lands)

- **UI-only plugin + own backend that shells out itself**: your backend exe
  is a normal process — it CAN spawn its own CLI integrations (your own
  provider detection, your own one-shot adapters). This is exactly the
  AiMacro pattern (§10): it works TODAY, at the cost of duplicating provider
  discovery inside the plugin. Acceptable for a first version; migrate to
  `providers.list`/`agents.run` when available.
- **Roster-driven UI**: build on `sessions.watch` for anything
  session-aware that the USER drives from the shell (your plugin renders and
  reacts; the shell owns the session).
- **Oracle-backed search panels**: `oracle.search` is production-ready for
  codebase Q&A surfaces.

---

## 8. Security & trust model (what the platform enforces for you)

- **Files**: SHA-256 digest per file, verified at scan/install; asset server
  refuses unlisted files. Manifest and artifacts must be produced together
  (§4) — hand-edited manifests are the #1 rejection cause.
- **UI containment**: cross-origin sandboxed iframe; no Tauri IPC from plugin
  frames (a GHSA-documented boundary); per-plugin CSP
  (`default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; …;
  frame-ancestors <shell origins>`); double-canonicalized asset paths
  against symlink escape.
- **Processes**: backends run under Job Objects (orphan-proof), spawn-time
  environment sanitization, capability intersection at handshake, 1 MiB
  payload caps.
- **Data**: `workspace.root` requires an open project; derived capabilities
  require it too. Oracle queries are length- and time-capped.
- **Honest limits (know them, design for them)**: digests provide
  INTEGRITY, not authenticity — anyone who can write the install directory
  can rewrite `plugin.json` (`manifest.rs` says so verbatim); there are no
  signatures in v1. Backends are ordinary user processes inside a kill-Job —
  no AppContainer/restricted token. Skills' `scripts/` have NO confinement
  (no manifest, no capabilities, no iframe) — the audit's largest unresolved
  trust gap. There is no revocation mechanism (refund ≠ uninstall). These
  gaps are marketplace-roadmap items, not excuses.
- **Marketplace (design)**: git-catalog distribution (a `marketplace.json` in
  a repo), identity = GitHub, paid plugins via Stripe Connect with human
  review; digests + review are the trust pair. No Devboule SSO, no promise of
  payouts without a license-check story.

Do not try to bypass any of these — they are the reason a reviewed plugin can
be trusted with a surface in the shell.

---

## 9. Packaging & distribution checklist

### How install works today (single path: local folder)

```
user "＋" → folder picker → copy to plugins-staging/<id>
  → verify(staging): manifest parse → EXACT file-list match → SHA-256 per file
  → swap_into_place (atomic rename, rollback on failure)
  → restore_interrupted_swaps on next scan (crash mid-swap is survivable)
```

The backend binary is RE-VERIFIED at spawn time, not just at install. The
pipeline is deliberately shaped so a remote source can bolt on later:
"fetch an archive, unpack to staging, then everything from verify onwards is
unchanged" (`install.rs` header). That seam is where the marketplace
fetcher will land — catalog schema, download, and signatures are the
documented gaps before it exists.

### Pre-submission checklist

- [ ] `plugin.json` generated by the manifest producer (digests match
      artifacts by construction)
- [ ] `id` == directory name, `[a-z0-9-]`, stable across versions
- [ ] Entry UI loads in a RUNNING Devboule build — `plugin.localhost` is a
      Tauri-registered scheme, not something a plain browser resolves. For a
      fast iteration loop, serve `ui/` with any static dev server and apply
      the same CSP by hand; the authoritative test is always inside the shell
      (`http://plugin.localhost/{id}/…`)
- [ ] Capabilities: minimal set; every entry justified in the listing
- [ ] Backend (if any): fresh binary copied by the producer; graceful
      behavior when a capability is NOT granted (the intersection can shrink)
- [ ] Version bumps change digests → re-verify on update; the install flow
      (staging → verify → rename) is atomic
- [ ] Listing assets + license; skills' licenses must explicitly allow
      redistribution if you ship them inside a pack
- [ ] Human review artifacts for paid plugins (threat model: what your
      backend does with `workspace.root`, network egress, child processes)

---

## 10. Case study: AiMacro as a Devboule plugin

AiMacro (Fiji/ImageJ macro studio — AI vision → analysis spec → detector →
Fiji macro → critic → batch) is the reference for "domain app with its own
agent stack" becoming a plugin. Mapping of what exists to plugin anatomy:

| AiMacro today | As a plugin |
|---------------|-------------|
| Dioxus/WASM UI in Tauri webview | `ui/` bundle — Dioxus builds to WASM/JS the same way Vite does; served from `plugin.localhost/aimacro/` |
| Node Express sidecar (Fiji runner, Python kernel, pipeline orchestrator, `:8787`) | `entry.backend` native service (or keep Node: the platform spawns exes; a small native supervisor that owns the Node process keeps the Job Object discipline) |
| `api_proxy` HTTP bridge (reqwest, SSRF-guarded) | **⚠️ Requires a host change today.** Plugin CSP is fixed (`connect-src 'self'`) and the manifest rejects unknown fields, so pointing the UI at `http://127.0.0.1:8787` does NOT work out of the box. Interim options: (a) move the loopback calls INTO the named-pipe backend (the host-mediated, reviewable path — the backend talks to the sidecar on the UI's behalf), or (b) propose a reviewed per-plugin CSP extension as a platform capability. Do not assume either exists today. |
| Provider detection (`providers.rs`: PATH scan + cmd-shim unwrap, ported from devboule) | **Deleted** in favor of `providers.list` once §7-1 lands; interim: keep the ported scan inside the backend |
| `cliProvider.ts` one-shot adapters (claude stream-json, ACP, codex exec, pi) | **Deleted** in favor of `agents.run` once §7-2 lands (image attachments included); interim: keep adapters in the sidecar — they already work and carry the vision-probe gate |
| Vision probe (`verifyVision`, known-color PNG) | Stays — it is a domain-quality gate regardless of transport |
| Asset protocol for image previews | Maps to the plugin asset server (digest-verified previews) or keeps blob URLs |

The interim shape (plugin owns its agent stack behind its backend) is
exactly what §7's "interim pattern" sanctions; the destination shape
(host-mediated agents via `providers.list` + `agents.run` + `events.subscribe`)
is where every domain plugin converges — one provider stack, one permission
surface, one place to audit.

---

## 11. Mobile surfaces & remote control (companion-app roadmap)

> Key references (verified 2026-09-07):
> - MCP Apps, SEP-1865 (Final): https://modelcontextprotocol.io/seps/1865-mcp-apps-interactive-user-interfaces-for-mcp
>   and the normative spec https://github.com/modelcontextprotocol/ext-apps/blob/main/specification/2026-01-26/apps.mdx
> - MCP-UI SDK: https://mcpui.dev - https://github.com/MCP-UI-Org/mcp-ui
> - Adaptive Cards schema explorer: https://adaptivecards.io/explorer/
> - JSON Schema 2020-12: https://json-schema.org/specification
> - Tailscale Serve (identity headers): https://tailscale.com/docs/features/tailscale-serve
> - OAuth DPoP: https://www.rfc-editor.org/rfc/rfc9449.html - mTLS binding: https://www.rfc-editor.org/rfc/rfc8705.html
> - Tauri v2 mobile: https://v2.tauri.app/develop/plugins/develop-mobile/

The requirement: a future Devboule phone app (Tailscale-connected, like
Paseo's) must **discover and command any installed plugin without shipping
per-plugin mobile code**. The researched answer is a **two-tier model**:

### Tier 1 — schema-driven command floor (the compatibility contract)

Optional manifest extension (old manifests stay valid):

```jsonc
{
  "commands": [{
    "id": "aimacro.run-detection",
    "name": "Run detection",
    "description": "user-facing summary",
    "inputSchema":  { "$schema": "https://json-schema.org/draft/2020-12/schema", "type": "object" },
    "outputSchema": { "$schema": "https://json-schema.org/draft/2020-12/schema" },
    "effects": "read | write | dangerous",   // drives phone-side confirmation
    "idempotent": false,
    "streaming": true,
    "requires": ["aimacro.backend"],          // existing capability system
    "ui":       { "dialect": "devboule.form/1" },
    "rich":     { "uri": "ui://aimacro/run", "mimeType": "text/html;profile=mcp-app",
                  "sha256": "…" }             // OPTIONAL tier-2
  }]
}
```

Rules: core widget vocabulary only (text/number/bool/enum/date/file/array/
submit); **unknown widget → readable "unsupported control" + raw value, never
a blank screen**; UI dialect versioned separately from JSON Schema (2020-12);
the command must stay callable as a plain form; labels/required/errors live in
the contract; the HOST owns typography/theme. Bounded schemas — no unbounded
`oneOf`, no remote `$ref`, no executable defaults.

### Tier 2 — rich surfaces via MCP Apps (SEP-1865, stable 2026-01-26)

A plugin MAY predeclare a `ui://` HTML resource (`text/html;profile=mcp-app`),
rendered by the host in a sandboxed iframe with the MCP Apps postMessage
JSON-RPC bridge — exact methods per the ext-apps spec: `ui/initialize`,
`ui/notifications/initialized`, `tools/call` (→ the SAME command
authorization path), `resources/read`, `notifications/message`, `ui/message`,
`ui/open-link` (URL policy applies), `ui/notifications/size-changed`, and
`ui/resource-teardown`. Resource digest verified
against the manifest; text/structured output remains MANDATORY so text-only
hosts (and the phone's generic renderer) always work. remote-dom is a later,
allowlisted experiment (the MCP-UI packages are community-maintained and
evolving; treat them as a compatibility layer, not the normative contract).

### Transport & authority

- Desktop is **authoritative**: discovery, authorization, validation, queuing,
  execution, output validation. The phone renders and enqueues.
- Pair over **Tailscale Serve** (tailnet-only, identity headers — never
  Funnel), backend on localhost only. Caveat: traffic from TAGGED devices
  carries no user-identity header, and shared-device users are still
  tailnet-reachable - which is why the application session below is
  mandatory, not optional. Add an application session anyway:
  QR pairing, short-lived access + rotating refresh tokens in
  Keychain/Keystore; optionally DPoP (RFC 9449) / mTLS binding (RFC 8705).
  Tailnet reachability ≠ per-command authorization.
- Discovery: `GET /api/v1/surfaces` → digest-cached catalog (plugin id,
  manifest digest, command schemas, dialect version, authorization-filtered).
- Execution: phone validates locally → `{commandId, input, requestId}` →
  desktop re-checks digest/grant/schema/confirmation policy → durable
  `jobId` → WebSocket for progress/cancel (SSE or polling fallback, per
  Portainer's intermittent-link model); outputs validated against
  `outputSchema` before display.
- **"Installed" ≠ "authorized for remote control"** — two explicit states,
  with per-device grants and revocation.

---

## 12. Author checklist

1. Read `plugins/hello` (minimal) and `plugins/polis` (full app) before
   writing anything.
2. Generate the manifest; never hand-edit digests.
3. Declare the minimal capability set; handle every denial gracefully.
4. Keep the UI CSP-clean (no inline scripts from CDNs; self-hosted assets).
5. No Tauri IPC from the frame — it does not exist for you, by design.
6. Backend: answer the handshake honestly; serve only what you declared; die
   when the Job Object kills you.
7. Test the uninstalled state (the shell ships with zero plugins — your
   first-run experience must still make sense).
8. Write the listing like documentation: what it does, what it needs, what
   it sends over the network.
9. Version semantically; every release re-runs the manifest producer.
10. If your tool is agent-centric, read §7 twice: build on the interim
    pattern, but design the UI so the migration to `providers.list` /
    `agents.run` is a transport swap, not a rewrite.

---

*This document is generated from the devboule-v2 codebase (commit-era
2026-09-07). When the platform changes, the explorers' file:line citations
above are the diff anchors: `src-tauri/src/plugins/*`,
`crates/devboule-plugin-rpc/*`, `src/features/plugins/pluginBridge.ts`,
`crates/devboule-daemon/src/provider_catalog.rs`, `crates/devboule-protocol/*`.*
