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

## Checking what rendered

`artifactRenderCritic.ts` measures the artifact instead of trusting the prose the agent wrote
about it. It cannot use the display frame: that frame is `sandbox=""` with an opaque origin,
which is the point of it, and nothing can read back what rendered there. So it renders the
same markup a second time in a hidden frame at `sandbox="allow-scripts"` and nothing else,
with `<script>` elements and `on*` attributes stripped first, its own CSP delivered inside the
document, results validated both against `frame.contentWindow` and by shape, and a 1.5 second
timeout after which nothing is shown. The measurement frame is 700 × 500 — the same size as
the artifact node on the canvas — so it measures the viewport the user is actually looking at.

Four checks, chosen because each is decidable from the rendered box rather than from taste:
text contrast against SC 1.4.3, pointer targets against SC 2.5.8, content overflow, and focus
indicators against SC 1.4.11.

**The pointer-target check measures the element unioned with its label**, because the common
"visually hidden input inside a big label" pattern would otherwise generate pure noise. Two
real artifacts settled the rule in opposite directions: a Settings screen whose switch is a
1 × 1 checkbox inside a 350.8 × 68 label produces nothing, and a desktop mock whose 14 × 14
checkboxes sit in 131.7 × 20 labels produces two findings, because 20 CSS px is genuinely under
the 24 the criterion asks for.

**The focus check is static analysis of the stylesheets, and that is a measured constraint
rather than a preference.** Focusing an element inside the hidden frame moves the _parent_
page's `document.activeElement` to the iframe, and removing the frame afterwards drops it to
`body`. Probed in Chromium 152 with a control: with no `focus()` call the parent keeps both
its active element and its text caret; with one, it loses them. The critic runs immediately
after a generation, which is exactly when the user may be typing the next prompt, so it never
calls `focus()`, `blur()` or `showPicker()`. Three reasons are reported: an outline below 3:1,
an outline removed with nothing declared in its place, and a rule whose focus selector shares
its declarations with a _static_ selector some element already matches — that last one catches
the case where the item carrying `aria-current="page"` looks identical focused and unfocused.
`:hover` is deliberately excluded from that collision test, because sharing a block with a
transient state is correct and extremely common.

**Two defects found by measuring rather than by reading the report**, both recorded because
the shape of them recurs. The first version emitted its measurement source as raw text with no
`<script>` element and prepended it ahead of the artifact's doctype, so the script never ran
and the document fell into quirks mode — and it failed silently, because a timeout is designed
to show nothing. 650 tests were green: they asserted the pure functions and never the
assembled document, which is the only thing a browser sees. The second was a false positive in
the collision test, which fired on a button that had a perfectly good focus ring, because the
check ran per rule and never asked whether a _different_ rule supplied the indicator. A
warning on correct output is the expensive failure for this component: it teaches the reader
to dismiss the card, and then the real finding is invisible too. Both cases are now tests that
drive the assembled document.

**What it does not measure.** When an ancestor carries a `background-image`, the contrast check
returns nothing rather than guessing at a colour underneath. Measuring against the real pixels
would need a screenshot taken from outside the frame — a Tauri or CDP capture — because
rasterising the DOM from inside is blocked by the frame's own CSP and taints the canvas.
Re-rendering with a library instead would produce an approximation wearing the costume of a
measurement, which is worse than the gap.

## Design doctrine

`skillLoader.ts` composes craft doctrine into one block of prompt text,
`builtInSkills.ts` discovers the sections, and `groundedPrompt` in `agentHost.ts` sends it
with every agent generation. **Which sections go is the user's choice**, in three modes:
priority — request every section including ones added later, then send the most important
sections that fit and declare the rest omitted; manual — exactly the ones ticked; or automatic
— a short pre-flight turn asks the agent which apply to this request, and those are composed on
top of a baseline that is never routed. Any failure of that question falls back to requesting
every section, with the same priority and ceiling behavior, never to none.

`AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS` holds that baseline, currently `anti-ai-slop` alone,
and the pre-flight prompt does not offer it as a choice. It was measured at two selections in
fifteen requests across three runs — first in the list, last in the list, and with a
description rewritten to state its breadth outright — because a relevance ranking under a cap
rewards specificity, so a section that applies to everything loses to sections that apply to
this. That is structural and no wording fixes it. `MAX_AUTOMATIC_SKILL_SECTIONS` is four,
bounded by the composed budget rather than chosen. Measured exhaustively over the current
corpus: of the 220 possible four-section selections every one composes with nothing dropped,
and of the 495 possible five-section selections 8 fit while 487 overflow. Four is therefore
the largest cap under which everything the router can choose arrives intact — at five, 98% of
generations would silently discard the router's own last choice and attach a notice saying
the doctrine is incomplete. The cap moved rather than the deliberate 12,000-character ceiling,
and the routed maximum is derived from the cap minus the baseline count so the two cannot
drift.

**That justification has already gone stale once, which is worth more than the number
itself.** The cap was set against an eleven-section corpus, on the finding that no five
sections could fit _at all_: the cheapest five then composed to 9,741 characters with one
dropped. Two smaller sections were added hours later and the claim quietly became false — the
cheapest five now compose to 11,975 and fit. The decision survived and its real reason turned
out to be stronger, but the sentence justifying it did not, and no test would have caught the
prose going wrong. The invariant in `agentHost.test.tsx` guards the cap itself, composing the
most expensive selection the cap allows and asserting nothing is dropped, so it holds whatever
the corpus becomes. The arithmetic quoted in this file does not: recompute it whenever a
section is added, removed or resized.

The choice persists through `surface_settings_get`/`set` rather than `localStorage`, which
the project does use elsewhere for per-model effort preferences. This one feeds prompt
composition rather than UI appearance, so it is product configuration and should outlive a
cleared webview store. `designSettings.ts` validates it on read: any malformed shape falls
back to the default, and a slug that no longer resolves is dropped while its siblings
survive, because uninstalling a bundle must not break a selection.

**A skill is a file, not a feature**: the sections live in `craft/` as markdown with four
front-matter fields — `slug` (must match the filename), `title`, `description`, `requires`
— discovered with a glob. `description` is the field that will decide relevance when there
are more sections than a prompt can carry; it is capped at 300 characters and it is the
same field Cursor's agent-requested rules and the Agent Skills standard use for that job,
both of which chose a sentence over a category taxonomy. Every value has to stay on one
line: the parser wants a key per line and says so when it does not get one. Adding one is dropping in a file, which is the shape the marketplace will need
to distribute them. Nothing is ever executed: doctrine is markdown that becomes prompt
text. There are thirteen sections today, totalling 31,264 characters of body, which is
more than twice what the ceiling composes — the corpus is a library to select from rather
than a block to send whole.

**`form-validation` and `cognition` were condensed from OpenDesign's corpus, and the
condensation is the work.** Their upstream files are 17,407 and 17,635 bytes; the parsed
bodies we ship are 2,396 and 2,315 characters, so roughly a seventh survives. The two sides
are measured differently — a file on disk against what `parseSkillFile` returns — so read the
ratio as an order of magnitude, not a figure. What survives is the checkable rules; what
went is the discussion. Both were given explicit ownership boundaries, because a section
that repeats another wastes one of the four slots automatic mode can send and invites the
two to contradict each other: `form-validation` owns only _when_ validation fires and _how_
an error is wired to its field — `state-coverage` still owns which states exist, `microcopy`
the words, `accessibility` the conformance floor — and `cognition` owns perception, choice
and memory while deferring distance to `spacing` and target size to `accessibility` and
`icons`.

Both were then measured against the router rather than assumed to be reachable, because a
section nothing selects is a section that does not exist. `form-validation` ranked second of
three for a sign-in request; `cognition` ranked first for a pricing page with four plans to
compare and second for a dashboard to scan. Neither appeared where it did not belong —
`cognition` was not chosen for the sign-in, `form-validation` not for either scanning task.
That refuted a prediction made before they were written: `cognition`'s description is close
to universal, and the baseline measurement above says a relevance ranking punishes breadth,
so it was expected to be unselectable. It was not. Breadth in the _subject_ is survivable
when the description names a concrete trigger — here "compare, choose" — and the earlier
result is narrower than it first appeared.

Both were appended to the end of `BUILT_IN_SKILL_PRIORITY` rather than placed by importance.
`all` is the default mode and truncation keeps the head of that list, so inserting them
higher would have pushed `accessibility` out of the block every existing user receives.
Measured after the change: `all` still composes `anti-ai-slop`, `typography`, `color` and
`accessibility`, exactly as before.

**The provenance scheme has no tier for empirical findings, and `cognition` exposed that.**
`STANDARD` wants a clause and a conformance level; `CONVENTION` wants two or more named,
independent design systems. A replicated psychology result is neither, so Fitts, Hick and
Hyman, and Iyengar and Lepper are all labelled `OPINION` — the same word as an aesthetic
preference. The prose carries the provenance instead, naming the researchers inline, and the
section's "What is contested" paragraph does the corrective work: Miller's 7±2 is about
chunk capacity and not a menu limit, Hick does not license a universal three-to-five cap, and
Fitts predicts a speed–accuracy trade-off rather than a pixel rule. A fourth tier would be
more honest, and it would mean re-auditing all thirteen sections; the flattening is recorded
here rather than fixed.

**The conformance target this doctrine writes to is WCAG 2.2 Level AA**, verified against
the W3C rather than inherited: 2.2 is the current Recommendation and conforming to it also
conforms to 2.1 and 2.0, so it covers the US Title II baseline (WCAG 2.1 AA, deadlines
2027-04-26 and 2028-04-26 by total population) and the Section 508 baseline (still WCAG 2.0
AA in the published revision) while matching the direction of EN 301 549 V4.1.1. That is a
conformance target for an interface, **not** a claim of legal compliance: EN 301 549 carries
requirements beyond WCAG, and applicability depends on the whole product and its
jurisdiction. Two details worth keeping straight, because a secondary source we read had both
wrong: EN 301 549 V4.1.1 is _published_ (ETSI, 2026-09) but not yet _cited in the Official
Journal_, so V3.2.1 and WCAG 2.1 remain the EU legal reference; and Section 508 is
coordinated with EN 301 549 but not harmonised with it — the Access Board expressly declined
to incorporate it by reference.

`doctrineLint.test.ts` guards the corpus, and it is deliberately narrow. It checks that every
WCAG citation carries a conformance level, that every description ends in the Apply-whenever
sentence the router matches on, that both ceilings hold with the constants imported rather
than retyped, and that no truncation marker or TODO survived. It does **not** try to decide
whether a sentence describes craft or issues an instruction: that needs a heuristic which
misfires in both directions, and a check that fires on correct content is worse than no check.
Each rule it does run exists because a defect of that exact shape shipped and was caught by
hand — a correct success-criterion number with the level missing was the most frequent.

**The doctrine is palette-agnostic, and that is a rule about content, not a style.** The
agent writes into the user's project, so binding its output to Devboule's terracotta would
be a defect — an all-black site and an all-green app are both legitimate requests. Rules
about craft travel; a brand does not. Where the user's own tokens should ground a
generation, they come from their repository through the Oracle, and when there are none the
agent has to say what it chose rather than invent a palette in silence.

**Two ceilings, and where their numbers come from.**
`DOCTRINE_SECTION_CEILING_CHARS` (2,500) forces first-party content to condense — it is
binding rather than generous, and a section that outgrows it becomes two sections rather
than a bigger number. `DOCTRINE_CEILING_CHARS` (12,000, roughly 3,000 tokens) bounds the
composed block; when priority/all mode requests the whole corpus, whole sections are dropped
from the tail of the priority order and the block declares what was omitted. They are
deliberately different constants: one serving both jobs would let a single section pass the
strict check and then consume the whole block, silently dropping every other section.

The 12,000 is borrowed rather than derived, and the sources are recorded here so nobody has
to re-derive it. It is the only published hard character cap found in a comparable product:
Windsurf caps a workspace rule file at 12,000 characters and a global rule file at 6,000.
Everyone else publishes lines — Cursor under 500, GitHub Copilot roughly 20-50 for custom
instructions and about 1,000 for a code-review file, Claude Code under 200 per `CLAUDE.md`
with the accompanying statement that longer files consume more context and reduce adherence.
No vendor publishes a measured curve of adherence against prompt length.

The ceiling went 8,000 → 16,000 → 12,000. The first raise was right in direction and wrong
in size: at 8,000 the truncation path could not fire at all, because three sections composed
to 7,211 and the automatic cap was three at the time. The correction downward came from evidence rather
than taste, and from noticing that a bigger corpus is a selection problem, not a budget one.

**The corpus is deliberately larger than the ceiling.** nexu-io/open-design ships about
112,000 characters of comparable doctrine with no budget, no section cap and no truncation
whatsoever, because each skill declares the sections it needs (`od.craft.requires`) and the
daemon injects only those, in full. Selection is what makes a large library affordable. The
ceiling only decides what happens when selection is refused, which is why `all` mode is the
only mode it constrains.

**What the density literature does and does not say.** In ManyIFEval, satisfying _every_
instruction at once collapses as instructions are added — GPT-4o from 94% to 21% between one
and ten — while per-instruction accuracy declines far less. Doctrine is judged on the second
metric, not the first: an artifact that honours eight rules out of twenty-six is better than
one that honours none, whereas a compliance checklist scoring 21% has failed. This is the
reason the corpus is allowed to hold more rules than any model will satisfy jointly, and the
reason instruction count is not the unit the ceiling is written in.

None of the borrowed numbers measured this system, so a narrower version of the experiment
was run against this code. The same request was generated twice, with the first three
sections of the priority order and with the first five. The larger block produced a semantic
table, pointer targets sized against a cited criterion, specific accessible names, a declared
spacing scale and a responsive rule, none of which appeared in the smaller one — and it did
not drift from the brief. That is one request, one model, one run per arm: evidence that more
doctrine can help, not enough to call five an attainable automatic cap or an optimum.

Routing was also measured while evaluating the wider, temporary cap, because it could have
made the selector spray rather than choose. It did not: doubling the routed slots from two
to four left `icons` at two selections in fifteen and `rtl` at one, exactly the requests that
need them, while
`spacing` — which no automatic path had delivered at all, being fifth in priority and never
chosen — rose to seven. It had not been described badly; it had been below the cut. The
instruction to name fewer than the maximum also started to work only at four slots, going
from one answer in fifteen to three, each with a stated reason. Worth watching: `typography`
and `accessibility` now win ten of fifteen, which is close to being a baseline that pays a
routed slot for the privilege.

The full version,
with a fixed task suite at randomised budgets scoring per-rule adherence separately from
all-rules success, has still not been run.

**When the ceiling does cut something, the block says so.** Truncation removes whole
sections from the tail of the priority order, never part of a rule, and the composed text carries
a notice that what it holds is not the complete doctrine — silence would let the model infer it
received everything.
The notice has a length, so its budget is reserved before the fit rather than appended
after it: otherwise announcing the truncation would cause more of it.

**Doctrine is reference material, and the prompt is built so that it cannot be anything
else.** The block is fenced between `===== BEGIN DESIGN DOCTRINE (reference material) =====`
and its closing delimiter, with a line saying it is not a request from the user. Two
properties carry the weight. **The output constraints come first, before the fence, and are
restated once after it.** That ordering was originally the other way round, on the
reasoning that constraints coming last have the last word. A 2026 study of 402 real agent
skills measured the opposite and named it: constraints deferred to a late section are
routinely skipped, because agents act on the first actionable instruction they meet and
reach the later advisory text only after acting (arXiv 2605.13044, pitfall F5, "Detached
Safety Constraints"). Our constraints were sitting exactly where that study found
constraints get ignored. They are now first, and restated after the block so recency does
not favour the untrusted half either; a test asserts the index ordering in the emitted
string, not mere presence. And any occurrence of either delimiter inside the composed text
is replaced before embedding, because a bundle able to print the closing delimiter could
make the rest of its text read as instructions to the host.

That neutralisation matches the delimiters exactly. An imitation with different spacing or
casing is not caught, and no textual fence can be made airtight — the defences that do not
depend on a model's reading are that the block is bounded by the ceiling, that the section
is omitted entirely when it would be empty, and that nothing in a bundle is ever executed.

**Why a fence is worth anything here when the literature says it is not.** The same
survey of Agent Skills concludes that instruction-hierarchy and structured-query defences
are architecturally inapplicable there, because a `SKILL.md` body already occupies the
operator layer — it is _meant_ to direct the agent, so nothing distinguishes an injected
directive from an intended one without a specification of what the skill should do, and no
such specification exists (arXiv 2604.02837). Doctrine is not that. It has a stated role
and it is one sentence: **it describes design, it never directs the agent.** That makes it
data by construction rather than by convention, which is the property the fence needs and
the one Agent Skills cannot have.

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
