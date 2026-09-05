# Design surface

A chat grounded in the repository, driving a real ACP agent that writes in the active
worktree, with a canvas that renders what the agent produced. It is not an editor: you
can look and point, not drag.

## The host contract

`designHost.ts` defines one object with three members, of which only `loadDocument` is
required. `saveDocument` and `generate` are optional, and **an absent capability removes
its own UI** — a host with no `saveDocument` has no Save control at all, not a disabled
one. Honesty is a property of the type rather than of copy somebody has to remember to
keep accurate.

`App.tsx` picks a host at mount by asking what actually exists:

| Host              | Chosen when                                  | Capabilities                          |
| ----------------- | -------------------------------------------- | ------------------------------------- |
| `agentHost`       | Oracle can answer **and** a workspace exists | load, generate                        |
| `oracleHost`      | Oracle can answer, no workspace              | load only — view-only by construction |
| demo (`mockData`) | otherwise                                    | load, save (of a fixture)             |

Each announces itself with a disclosure line, so the surface never implies more than the
host behind it can do.

## What a generation does

`agentHost` opens an ACP session through the same daemon and the same commands the
Workspace uses, and reuses `AgentSession` from `src/lib/` rather than reimplementing the
event lifecycle. Oracle search hits are attached to the prompt as grounding, labelled as
search hits and not as files changed.

Afterwards the surface reports which files were **written**, from the structured `kind`
and `locations` the daemon puts on tool events — not from scraping tool titles, which
cannot tell an affirmative write from a negation. A file counts only when a _completed_
tool call had kind `edit`, `delete` or `move`; `kind` alone is intent. Locations on an
update **replace** the collection rather than extending it, so they are tracked per
`toolCallId`. When no locations arrive, the surface says the agent did not report them
instead of guessing.

Two limits are stated rather than hidden. Agents create files through shell constantly,
and those complete as kind `execute` with no locations, so the count can be incomplete
and says so. And a permission request stops the run and sends the user to Workspace —
this surface never answers one.

## The artifact, and why the frame is built the way it is

The agent is asked for a self-contained HTML fragment in a fenced block. It is extracted
from the agent's reply, not read from disk: no IPC command reads an arbitrary file and
the Tauri capabilities were not widened for this. Extraction is scoped to the current
turn, because `AgentSession` items accumulate across turns and a reused session would
otherwise hand a previous generation's artifact to the next one.

The fragment renders in `<iframe sandbox="" srcDoc={...}>` inside the canvas.

**`sandbox=""` is a security boundary, not a style choice.** No `allow-same-origin`, so
the frame has an opaque origin and cannot reach the app, its storage, or the IPC bridge.
No `allow-scripts`, because the artifact does not need them and a capability not granted
is one hostile markup cannot use. A test asserts the exact attribute value: sandbox is
the primary defence, so a refactor that widened it must fail the suite.

**Measured in WebView2 on 2026-09-05: the parent CSP is not inherited by a `srcdoc`
frame here.** An inline script in an unsandboxed srcdoc frame executed and reached the
parent, and a `blob:` image loaded although `img-src` does not allow it. The
specification says policy containers are inherited; this runtime does not do it. Do not
re-derive the opposite conclusion from the spec. The frame therefore carries its own
policy by `<meta>`, with `default-src 'none'` and every directive named explicitly rather
than left to a fallback. An artifact may carry its own policy meta and cannot escape ours
— CSP policies are additive and the most restrictive wins.

The `<iframe>` keeps `pointer-events: none` and the artifact's content wrapper is
`inert`, so a click always belongs to the app and never to generated markup, and keyboard
focus cannot descend into the frame. Artifacts are capped at 256 KiB, with a card that
says so rather than an application that stops responding.

## The canvas

Pan, cursor-anchored wheel zoom and click-to-select come from the ported geometry engine
in `src/lib/canvas/` (`viewportMath`, `hitTest`); the wheel's bounded additive step is
adapted from plat's approach, with no source copied. Pan and zoom live in view state and
are deliberately **outside the undo history**: undo should not rewind where the user was
looking.

Clicking a node scopes the next prompt to it, and the chip and the scope sent to the
agent agree by construction. Selecting the artifact scopes precisely, since it is what
the agent just produced. A repository layer carries its name, kind, and indexed source
path; a demo fixture carries only its name and kind. The surface does not invent a
symbol or line range that the index does not provide.

**There is no dragging and no resizing, and there will not be.** Layers derive from the
repository and nothing writes a moved layer back to the code it stands for.
`snapAdvanced.ts` and `multiResize.ts` were deleted for this reason and should not return.

`src/lib/canvas/snap.ts` was ported alongside them and **is still in the tree, imported by
nothing but its own test**. It serves the same manipulation model, so it is dead by the
same argument; it survived only because it was deleted less thoroughly. Do not wire it up.

## What is not real yet

- **Layers are indexed file entries, not parsed component instances.** The Oracle and agent
  hosts enumerate workspace files from `oracle_files("indexed", page)` and expose `.tsx` and
  `.svg` paths with their extension-derived kind and source provenance. The client places
  them in a deterministic grid; those positions are a layout decision here, not a property
  of the code. Test/spec files and conventional non-source directories are excluded by a
  small path heuristic that can be wrong. The demo host still uses explicit fixture
  rectangles, with no source provenance.
- **There is no diff review.** Workspace's Changes panel is a mockup and no git plumbing
  exists anywhere in the project, so review of what an agent wrote is the user's own git.
- **Save exists only on the demo host**, and saves a fixture.

The plan for the remaining phases lives outside the repository, beside `ARCHITETTURA.md`.
