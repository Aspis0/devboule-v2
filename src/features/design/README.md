# Design surface

A chat grounded in the repository, driving a real ACP agent that writes in the first
workspace it finds, with a canvas that renders what the agent produced. It is not an editor: you
can look and point, not drag.

## The host contract

`designHost.ts` defines one object with three members, of which only `loadDocument` is
required. `saveDocument` and `generate` are optional, and **an absent capability removes
its own UI** — a host with no `saveDocument` has no Save control at all, not a disabled
one. Honesty is a property of the type rather than of copy somebody has to remember to
keep accurate.

`App.tsx` picks a host at mount by asking what actually exists:

| Host              | Chosen when                                  | Capabilities                            |
| ----------------- | -------------------------------------------- | --------------------------------------- |
| `agentHost`       | Oracle can answer **and** a workspace exists | load, generate (ACP; no save)           |
| `oracleHost`      | Oracle can answer, no workspace              | load, generate (Oracle search; no save) |
| demo (`mockData`) | otherwise                                    | load, generate, save (fixtures)         |

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

**That `<meta>` is load-bearing, not belt-and-braces.** Measured in WebView2 on
2026-09-06, with a control in the same run:

| Frame                              | `data:` image | remote image | request reached a local listener |
| ---------------------------------- | ------------- | ------------ | -------------------------------- |
| `sandbox=""` **with** our meta CSP | rendered      | blocked      | no                               |
| `sandbox=""` **without** it        | rendered      | loaded       | yes                              |

The second row is the point. **A sandbox does not block passive subresource loads** — it
governs scripting, navigation and forms, not fetches — so without the meta policy an
`<img src="https://…">` in agent-generated markup would be a live outbound channel from
the user's machine. The control proves the request path was reachable and the detector
worked, so the blocked case is a real block and not a false negative. The first row also
shows the policy does not over-block: `img-src data:` still renders, as intended.

Do not remove the meta CSP on the grounds that the sandbox already covers it. It does not.

The `<iframe>` keeps `pointer-events: none` and the artifact's content wrapper is
`inert`, so a click always belongs to the app and never to generated markup, and keyboard
focus cannot descend into the frame. Artifacts are capped at 256 KiB, with a card that
says so rather than an application that stops responding.

## Design doctrine

`skillLoader.ts` composes craft doctrine into one block of prompt text,
`builtInSkills.ts` discovers the sections, and `groundedPrompt` in `agentHost.ts` sends it
with every agent generation. **There is no selection yet** — every built-in section goes, always.
Letting the user choose is the next slice, and it needs somewhere to persist the choice:
the app has no preference store in the frontend, and `ARCHITETTURA.md` names
`<app_config_dir>/oracle-settings.json` as the model to copy, which is a command pair in
`src-tauri/` rather than anything reachable from here.

**A skill is a file, not a feature**: the sections live in `craft/` as markdown with three
front-matter fields — `slug` (must match the filename), `title`, `requires` — discovered
with a glob. Adding one is dropping in a file, which is the shape the marketplace will need
to distribute them. Nothing is ever executed: doctrine is markdown that becomes prompt
text.

**The doctrine is palette-agnostic, and that is a rule about content, not a style.** The
agent writes into the user's project, so binding its output to Devboule's terracotta would
be a defect — an all-black site and an all-green app are both legitimate requests. Rules
about craft travel; a brand does not. Where the user's own tokens should ground a
generation, they come from their repository through the Oracle, and when there are none the
agent has to say what it chose rather than invent a palette in silence.

**Two ceilings, both measured.** A condensed craft section weighs about 1,900 characters,
so `DOCTRINE_SECTION_CEILING_CHARS` (2,500) forces first-party content to condense, while
`DOCTRINE_CEILING_CHARS` (8,000, roughly 2,000 tokens) bounds the composed block. They are
deliberately different numbers: one constant serving both would let a single section pass
the strict check and then consume the whole block, silently dropping every other section.
Characters are a conservative proxy for tokens and no tokeniser is added for this.

**When the ceiling does cut something, the block says so.** Truncation removes whole
sections, never part of a rule, and the composed text carries a notice that what it holds
is not the complete doctrine — silence would let the model infer it received everything.
The notice has a length, so its budget is reserved before the fit rather than appended
after it: otherwise announcing the truncation would cause more of it.

**Doctrine is reference material, and the prompt is built so that it cannot be anything
else.** The block is fenced between `===== BEGIN DESIGN DOCTRINE (reference material) =====`
and its closing delimiter, with a line saying it is not a request from the user. Two
properties carry the weight. The fence sits **after** the grounding and **before** the
output constraints — the single fenced block, "scripts will not run", "only the last block
is used" — so our constraints have the last word in the prompt and no text inside the block
can read as replacing them; a test asserts the index ordering, not just the presence.
And any occurrence of either delimiter inside the composed text is replaced before
embedding, because a bundle able to print the closing delimiter could make the rest of its
text read as instructions to the host.

That neutralisation matches the delimiters exactly. An imitation with different spacing or
casing is not caught, and no textual fence can be made airtight — the defences that do not
depend on a model's reading are that the block is bounded by the ceiling, that the section
is omitted entirely when it would be empty, and that nothing in a bundle is ever executed.

**The runtime is tolerant and the repository is strict**, and both halves are needed. A
downloaded bundle referencing a section this build does not have must not take the surface
down, so `buildSkillBlock` skips malformed files, unknown `requires` and cycles. Our own
content is held to `validateSections`, which a test runs over `craft/` on every CI run, so
first-party doctrine cannot lose a section to a typo. Taking only the tolerant half gives
silent drops; taking only the strict half breaks installed bundles on upgrade.

Two practical notes for anyone adding a section. `oxfmt` formats markdown under `src/`, so
the formatter's output is canonical here and tables get padded — which costs characters
against the ceiling, and is a reason to prefer prose. And the content is adapted from the
MIT-licensed [refero_skill](https://github.com/referodesign/refero_skill) (© 2026 Refero);
see `THIRD_PARTY.md`.

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
`snapAdvanced.ts`, `multiResize.ts` and `snap.ts` were deleted for this reason and should not return.

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
