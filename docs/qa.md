# QA: the evidence bar for every change

A change lands on proof, not on assertion. Four checks decide:

1. Does the change do what it claims?
2. Does everything around it still behave?
3. Was it exercised where it will run?
4. Does the automated coverage prove anything?

The author shows proof for each check the change touches. A request
with no proof does not land.

## Evidence is raw

Proof means artefacts a reviewer can inspect independently:

- The exact commands executed, together with their exit codes.
- New tests plus their outcomes.
- Side-by-side images of before and after, or a recording of the
  interaction.
- Log excerpts and request/response pairs.

Rules:

- Quote commands with exit codes in full. Never report a piped command's
  status: `cmd > /tmp/out.log 2>&1; echo "EXIT=$?"; tail -30 /tmp/out.log`.
  Never write "should work".
- A red step is never waved through without reading its log. Paste the
  relevant log lines and the exit code.
- A known flake is named by its test name. "Flaky" without a name is not
  an explanation.
- Every PR names what was NOT verified: a live run that was impossible,
  a provider that was logged out, a platform not tested. Silence means
  "verified", so silence must be earned.
- No real user content in fixtures. Synthetic fixtures are labelled as
  synthetic in the file or the PR.
- When an agent produced the change, paste the agent's unedited output.
  Paraphrase loses the particulars a reviewer must confirm.

## Questions every PR answers

- What behaviour changed, and what is the red-first proof? Every behaviour
  change needs a test that fails on the base commit for the reported reason.
  The PR states the base commit hash and how the failure was checked.
- What did you open around the change? This codebase composes: a daemon
  change touches every provider, a protocol change touches old and new
  peers, a workspace change touches recovered sessions. Name the neighbours
  you exercised.
- Where did it run? Developed and tested on Windows; other platforms are
  unverified. State what ran on Windows and name anything else as
  not verified.
- Does the coverage prove anything? Tests drive the interface a user or
  caller uses. A test that stubs out the very behaviour under test,
  pins implementation details, or stays green on the unfixed code
  demonstrates nothing.

## Daemon / protocol changes

Required evidence:

- Red-first test failing on the base commit, green after. Quote the base
  hash, the failing output, and the passing output.
- Replay tests drive the real journal replay path: kill or detach, then
  reattach and replay from the journal. Never a self-comparison where
  the test writes and re-reads through the same helper.
- Wire or protocol changes state the protocol version in the PR. Old-peer
  decoding is tested: a new daemon against an old client and an old
  daemon against a new client, or a named reason why one direction is
  impossible.
- Resumed sessions are distinguished from replayed ones: replay is free,
  resume starts a process and is explicit. Tests assert which happened.
- Gate steps green: `cargo fmt --all -- --check`,
  `cargo build --workspace --locked`,
  `cargo check -p devboule-daemon --no-default-features --locked`,
  `cargo clippy --workspace --exclude oracle-core --all-targets --all-features --locked -- -D warnings`,
  `cargo test --workspace --exclude oracle-core --locked`.

## Provider adapter changes (claude, codex, pi, ACP)

Required evidence:

- Manual verification against the real provider CLI by someone who can
  tell that it worked: session start, prompt, permission card, resume.
  Fixtures and mocks by themselves establish only that the test data
  loads.
- The provider modes exercised are named (Claude: `plan`, `default`,
  `acceptEdits`, `auto`, `bypassPermissions`; Codex: `read-only`,
  `auto`, `auto-review`, `full-access`). Untested modes are listed
  under NOT verified.
- Known provider limits are stated, not worked around silently. Example:
  the shell fails to start inside Codex's own Windows sandbox in its
  default mode; the PR says so and names the mode that works
  (`full-access`).
- Gate steps green: same Rust steps as daemon/protocol, plus
  `pnpm exec tsc --noEmit` and `pnpm exec vite build` where the
  frontend renders the provider's states.

## UI changes

Required evidence:

- Verified in the running app over CDP: computed styles, geometry, and
  hover/focus states. Screenshots are judged as a user would judge them.
- Layout shift and alignment are checked on first load, not only when
  warm: material that jumps while data arrives or a row that changes
  height once a badge appears is not done.
- Happy DOM is not verification. It does not run the cascade, and
  `:has()` is always true there. `pnpm run test` passing is necessary
  but never sufficient for a visual claim.
- Neighbouring surfaces opened and named: tab strip, recovered sessions,
  permission cards, settings panels affected by the change.
- Gate steps green: `pnpm exec tsc --noEmit`, `pnpm run lint`,
  `pnpm run test`, `pnpm exec vite build`, `pnpm run format:check`.

## Tests-only changes

Required evidence:

- The new test fails on the base commit for the reported reason (red
  first), or the PR explains why the test cannot fail there, such as a
  harness-only fix.
- Targeted filters while iterating, then the full gate before asking
  for review: one full frontend run (`pnpm run test`) plus the Rust
  steps the change touches.
- The `#[ignore]`d ConPTY / named-pipe tests stay out of the PR gate.
  They run on the weekly `ignored-tests-informational` schedule, or by
  hand with `cargo test --workspace --locked -- --ignored
  --test-threads=4` after touching daemon session code.
- Gate steps green: the steps covering what the tests exercise
  (frontend tests for `pnpm run test`, Rust tests for
  `cargo test --workspace --exclude oracle-core --locked`).

## Docs / comments-only changes

Required evidence:

- A token or AST compare showing zero non-comment changes, quoted in
  the PR.
- `cargo clippy` green: a `//` line between `///` lines fails clippy,
  so the lint is the check, not the eye.
- `pnpm run format:check` green where the formatter covers the files;
  markdown outside its scope is stated as such.
- Comments explain WHY or a trap, never restate the code. A comment
  declaring an invariant must be true; the PR names how it was checked.
- Gate steps green: `pnpm run format:check`,
  `cargo fmt --all -- --check`, `cargo clippy --workspace --exclude
  oracle-core --all-targets --all-features --locked -- -D warnings`.

## Dependency changes

Required evidence:

- `.github/workflows/ci.yml` is the authority. These steps stay last
  because they reach the registries: `pnpm run check:dependency-majors`,
  `pnpm run check:third-party`, `pnpm audit --audit-level high`,
  `cargo audit`. A registry outage must never mark our own compile
  and test steps as never executed.
- Direct dependencies stay on the latest stable. An exception lives
  inline in `scripts/check-direct-dependency-majors.mjs` with both
  `reason` and `exitCondition`, stating why the lag exists and what
  removes it.
- `pnpm install --frozen-lockfile` green, plus
  `pnpm run check:shrinkwrap` proving no npm shrinkwrap leaked in.
- `cargo build --workspace --locked` green; oracle-core serially:
  `cargo clippy -p oracle-core --locked --all-targets -- -D warnings`
  and `cargo test -p oracle-core --locked -j 1`.
- Licence inventory current: `pnpm run check:third-party` green, and
  THIRD_PARTY.md updated where the check requires it.

## When a change is rejected

- No evidence: claims without commands, exit codes, tests, or
  screenshots.
- Coverage that is not real: mocked behaviour, internals asserted,
  passing against the broken code, Happy DOM offered as visual proof,
  self-comparison offered as replay proof.
- Red waved through: a failing step without its log read and quoted,
  a flake without a test name, a piped command's status reported as
  the command's status.
- Undeclared gaps: unverified providers, platforms, or modes missing
  from the NOT verified list.
- Protocol change without a version and without older-peer decoding.
- Real user content in a fixture, or an unlabelled synthetic one.
- Clean-code violations (below).

## Clean code

Write clean code. Never create a God file: a file may hold one responsibility, and the test of it is that you can name that responsibility in one phrase. Line count is a smell, not the rule — ~250 is indicative, ~350 is fine, and if you notice you are moving lines only to make a number work, stop and leave the file alone. No new file beyond ~350. No file, new or existing, that mixes responsibilities until it stops being readable. Test files follow the same logic: if a test file becomes an indistinguishable list of cases, split it by topic, not by line count. Pre-existing excess is declared, not refactored. Minimal imports: no globs, no preventive pub, no convenience re-exports. No types or APIs nobody constructs yet. Comments only for WHY or a trap, never restating the code — and if a comment declares an invariant, that invariant must be true. No secret values. Tests: targeted filters while iterating, one full frontend run (vitest) at the end, then the full gate before asking for review — and never report a piped command's status: `cmd > /tmp/out.log 2>&1; echo "EXIT=$?"; tail -30 /tmp/out.log`, quoting the exit code. Report file:line, commands with exit codes, hashes, and what you could not verify.

## Gates

To verify locally, run the steps of `.github/workflows/ci.yml`: run
every step even after a red one, and record the exit code of each.
CI adds the `polis-plugin` and `oracle-core-check` jobs and the
scheduled-only `ignored-tests-informational` job. `.github/workflows/ci.yml`
is the authority; this document names the steps, it does not replace them.
