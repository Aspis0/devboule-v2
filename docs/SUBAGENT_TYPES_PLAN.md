# Subagent types as installed data

Workspace must be able to activate a plugin — Design today, Pubvia and others later — as a
subagent, and adding or removing a type must not require editing the daemon.

This document is the plan for that. It is written against the code as measured on
2026-09-17; every claim below carries the file that makes it true, so a reader can check
the premise instead of trusting the sentence.

## 1. What the code already answers

### `SessionKind` is a transport, not a surface

`devboule-protocol/src/session.rs` spells five values — `Terminal`, `Acp`, `Claude`, `Pi`,
`Codex` — and every branch site asks the same question: which binary does this session
speak to. The provider registry binds one implementation per value (`provider.rs`),
`peer_policy.rs` keeps one mode vocabulary per value, `mcp_broker.rs` picks a config path
for Claude, `event_pull.rs` corrects per-protocol projection quirks,
`journal_retention.rs` groups the agent transports against `Terminal`.

None of them asks which part of the product owns the session. That question has no
daemon-side answer today.

`SessionKind` must therefore **not** grow a variant for a subagent type. The journal's
`parse_kind` (`journal.rs`) refuses an unknown kind string with `JournalError::Corrupt`,
so a type written by a newer build would corrupt the row for an older one. Contrast
`SessionOriginKind`, which deliberately maps unknown to `Unknown`: the closed table is a
deliberate choice for transports, and it is the wrong choice for an open set of types.

### How Design is distinguished today

Four things, none of which is a type:

1. A closed frontend key — `SurfaceKey` in `src/types/surface.ts`, six hardcoded values.
2. The profile a child was created from — `AgentCreated.profile` (a **name**) and the
   nullable `profile_id` column (`journal_schema.rs`). This is the only daemon-side trace.
3. A deny list — `ToolOverlay::DESIGN` in `provider_catalog.rs`: no
   `devboule_send_message`, no `devboule_create_agent`, applied when the resolved profile
   carries those names.
4. An app-side feature module, `src/features/design/`, which remembers in module state
   which child it is mirroring.

### The typed return channel exists on the wire and nobody reads it

`SessionEvent::ChildFinished` already carries `artifacts: Vec<FinishArtifact>`, and a
`FinishArtifactPart` already carries `mime_type` plus a
`devboule-attachment:<sessionId>/<digest>` reference (`protocol/src/session.rs`). The
daemon deposits the child's last message into the creator's folder and publishes the
structured twin beside the text envelope.

The app reads none of it. `src/types/ipc.ts` says so in its own words on the
`child_finished` member, and `delegatedDesignMirror.ts` recovers the artifact by
**replaying the child's transcript** and re-extracting fenced HTML — then synthesises the
card metadata it could not recover. Grep finds `devboule-attachment:` in `crates/` and in
the generated samples, and nowhere under `src/` or `src-tauri/`: there is no
read-by-reference door.

### Plugins are a shell concept; the daemon has never heard of them

`src-tauri/src/plugins/{discovery,install,manifest,assets,rpc}.rs` own the manifest, the
digests, the staging directory and the atomic swap. `crates/devboule-daemon/src` has no
plugin subsystem at all. `AGENT_RUN` is declared in `protocol/src/lib.rs` and referenced
nowhere.

So "the registry is populated when a plugin is installed" spans two processes that today
share nothing on this axis. The precedent for a shell-written, daemon-owned store is the
`XxxSet` request family — `AgentProfilesSet`, `DelegationSet`, `ToolPolicySet`.

### Labels are already the right shape, with a contract to respect

`Session.labels` is a `BTreeMap<String, String>` on creation and on the row, persisted in
a `labels TEXT` column with its migrations. The protocol comment states the contract:

> The `devboule.` prefix is reserved — the daemon's own facts are the ones it writes, and
> a caller cannot set or overwrite one.

and, in the same paragraph:

> For humans and for display, and for nothing else: no code in the daemon reads a label to
> decide anything.

Both halves matter. The reserved prefix is exactly the enforcement the second decision
below needs. The display-only half is a constraint on the design: the label may carry the
type forward, but it must not become the thing the daemon consults to decide what a child
may do.

## 2. The decisions

**D1 — A type is a row in an installed registry, not an enum variant.** An enum is a
closed table: every new value is a negotiated dialect change, and an older daemon refuses
it. A registry row can name a plugin the daemon has never heard of.

**D2 — The registry is written by a human moment, never by an agent.** If an agent could
define a type, it would define one whose deny list is empty and take back the powers it
was denied. Enforcement rides on the existing reserved prefix and on the store being
daemon-owned and written only over the `Set` RPC.

**D3 — The label carries the type; the registry row carries the power.** The daemon
resolves the row **at the moment of creation** — like a profile, never cached — and
applies provider, model, mode and overlay then. The `devboule.type` label is the durable
stamped fact, for display and for the app's surface routing. No daemon decision reads it,
so the protocol comment above stays true.

**D4 — The consent split: the observation floor is auto-accepted at install, the acting
powers are named at install and arrive off.** This is the asymmetry the house already
applies to peers (`PEER_DEFAULT_CAPS = ["view"]`) and to the delegation switch, which
reads **off** when its file is unreadable.

The mechanism is already there and needs no new concept: `ToolOverlay` is a **deny** list
by explicit decision, so "powers off" is literally the row's default content — the type
ships denying every acting tool, and granting one is the human removing a name in
Settings. No allow-list is introduced; a type can never widen what the stored tool policy
already permits.

**D5 — The install card is generated from the table the daemon applies**, and a walking
test demands that every tool the broker serves appears on the card or is denied by the
row. A card assembled by hand is a card that names what nothing delivers.

**D6 — The registry is its own document, not new fields on `agent-profiles.json`.** Three
reasons: `AgentProfilesDocument` is `deny_unknown_fields`, so a field written by a newer
build quarantines the whole file for an older one — and a quarantined profile document
means *no agents may be created*, which is far too large a blast radius for a plugin's
mistake; a plugin must not write into the human's own profile list; and uninstall must be
"drop the rows carrying that plugin id", which a shared file makes delicate.

## 3. The slices

Each slice is one gate and one audit. T5 is independent of T1–T4 and may go first.

**T1 — the store (daemon).** `subagent-types.json` beside `agent-profiles.json`, shaped on
`agent_profiles.rs`: a `SubagentType` row (`id`, `name`, `installed_by` plugin id,
`provider`, `model`, `mode_id`, `tool_overlay`, `cwd_policy`), an ordered document, atomic
DACL'd temp+rename, admission caps, `deny_unknown_fields`, quarantine to **empty** with
the sentence the profile store already uses, `SubagentTypesGet`/`SubagentTypesSet` over
the pipe. No caller yet: this slice ships a store and its tests.

**T2 — creation binds a type (daemon).** `devboule_create_agent` accepts a type name,
resolves the row at creation, refuses an unknown name with a sentence (never a decode
failure), refuses an ambiguous one the way `resolve_profile` already does, applies the
overlay through `ToolOverlay::from_profile_names`, and stamps `devboule.type`. The stamped
key set grows from four to five: extend the test that walks it.

**T3 — install populates the registry (shell).** An optional `subagentTypes` block in
`plugin.json`, parsed under the existing strict manifest rules; the install pipeline calls
`SubagentTypesSet` after the atomic swap, the uninstall path calls it with that plugin's
rows removed. Crash-safety follows the existing restore-on-next-scan path.

**T4 — the consent surfaces (frontend).** The install card generated per D5: the floor
stated plainly, each acting power named with its switch off. A Settings section shaped on
the tool-policy section for granting and revoking afterwards, plus the list of installed
types with their origin plugin.

**T5 — the typed return channel (daemon + shell + frontend).** A read-by-reference door:
an RPC that resolves `devboule-attachment:<sessionId>/<digest>` through the attachment
store with the store's own size as the number that counts, its Tauri command, and a
frontend reader for `ChildFinished.artifacts`. Then `delegatedDesignMirror.ts` stops
replaying the transcript and stops synthesising metadata it can now read.

This is the piece the other four cannot replace: without it every new type re-invents the
Design mirror's trick, and each one gets it wrong in its own way.

## 4. What this plan is honest about

- **The floor is the first power this product grants without a card in front of a person.**
  Every existing grant — pairing caps, permission cards, tool policy, the delegation
  switch — is a human decision recorded at a human moment. The floor is bounded to
  observation, it is named on the install card rather than hidden, and it is revocable in
  the same Settings section that grants the rest. That is the mitigation; it is not the
  same as the property.
- **The registry spans two processes.** Until T3, a type can only be installed by hand.
- **Design keeps its `SurfaceKey`.** Turning the six hardcoded surfaces into data is a
  separate question from turning subagent types into data, and this plan does not touch it.
