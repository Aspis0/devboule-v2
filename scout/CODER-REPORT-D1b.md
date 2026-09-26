# Coder report D1b — card waits and caller cancellation

## Worktree

- Repository: `devboule-v2-queue-ui`
- Branch: `fix/card-bridge-timeouts`
- Starting HEAD: `2a416398f54c0c9a4657d474d241de14aee6673a`
- No stash, push, or new branch used.

## Changes

- Added `MCP_CARD_WAIT_TOOLS` in `crates/devboule-daemon/src/provider_catalog.rs:316` as the daemon-owned set of tools that can wait on consent: create agent, create/archive workspace, and create/send-keys/kill terminal. `bridge_extension()` serializes this table into the pi template at `crates/devboule-daemon/src/pi_client.rs:934`; its test parses the generated set, requires it to be nonempty, and compares exact contents.
- Pi calls for those tools keep the pi-provided abort signal but skip our 30-second timer. Every other MCP fetch remains bounded at 30 seconds (`pi_client.rs:360`). When a call fetch is aborted, pi posts `notifications/cancelled` with the original JSON-RPC request ID (`pi_client.rs:430`). The notification attempt itself has a one-second bound.
- Claude's generated MCP server entry now sets `timeout` to `2,147,483,647` ms (`mcp_broker/mod.rs:538`), the maximum accepted by Claude Code's signed 32-bit timer cap from the recon. Claude's per-server `timeout` key raises its first-byte and idle floors for this server only. This is the largest supported value, not a literal infinite timeout.
- Codex launch config sets `mcp_servers.devboule.tool_timeout_sec` to 2,073,600 seconds (24 days) in `codex_client.rs:339,360`. This is just below Claude's ~24.86-day ceiling and far below `u32::MAX` seconds, a conservative bound for the `Instant + Duration` deadline Codex constructs for each call. The config parser accepts the whole-seconds value as `Duration`; the setting remains scoped to our server. Codex's pinned `rust-v0.157.0` source adds the configured timeout to `Instant` in [`binding.rs`](https://github.com/openai/codex/blob/rust-v0.157.0/codex-rs/codex-mcp/src/binding.rs#L333), then passes the remaining duration to a second deadline construction in [`rmcp_client.rs`](https://github.com/openai/codex/blob/rust-v0.157.0/codex-rs/rmcp-client/src/rmcp_client.rs#L1416). Both additions can panic on overflow; the previous parse-limit-sized timeout is therefore unsafe even though it parses.
- `notifications/cancelled` is no longer dropped (`mcp_broker/dispatch.rs:71`). Each active card-capable call has a registry token keyed by `(session ID, JSON-RPC request ID)`; the token is removed when the handler finishes, and the registry is bounded by the listener's 64-connection cap (`mcp_broker/mod.rs:70,451`). The cancellation POST finds only an active exact key. Host cards retain the exact call token and ID, and the permission table lock orders cancellation against card insertion (`permission_broker.rs:368,870`): a cancel before the card marks the token, while a cancel after insertion atomically takes only that call's card. Duplicate in-flight IDs are refused, completed IDs can be reused, and cancellation cannot select another call's card. Allow and cancel still share the broker's atomic take.
- **Connection loss choice:** no HTTP socket-drop detector was added. FIN/half-close is not proof the caller abandoned its wait; treating it as cancellation risks withdrawing a card from a live caller. This implementation relies on explicit `notifications/cancelled`. The recon verified Claude's SDK emits it. I inspected the installed qwen bundle `@qwen-code/qwen-code/chunks/chunk-4F7GQGXB.js`: its SDK cancellation closure sends the notification with `messageId` at line 63257, is invoked on the request signal at line 63290, and from the timeout handler at line 63294. Silent connection drops and Codex's timeout signal remain unverified; those clients can leave a card pending if they abandon without sending cancellation.
- grok's separate 30-minute `ask_user_question` timeout remains unchanged; this is A2b-3 P2-1.

## Tests and checks

Commands ran from the repository root. Cargo commands used `CARGO_BUILD_JOBS=4` and were serialized under the local cargo-slot mutex.

- `cargo fmt --all` — exit 0.
- `cargo fmt --all -- --check` — exit 0.
- `git diff --check` — exit 0.
- `cargo test -p devboule-daemon --lib mcp_cancel --locked -- --test-threads=1` — exit 0; 3 passed. Covers session/request scoping, cancel-before-card refusal with no card, journal outcome `cancelled`, and the Allow race.
- `cargo test -p devboule-daemon --lib pi_bridge_card_wait_ignores_injected_bound_but_other_tools_keep_it --locked -- --test-threads=1` — exit 0; a carded call gets its answer after a 300 ms injected wait, while a non-carded call still aborts at the 20 ms bound.
- `cargo test -p devboule-daemon --lib codex_mcp_timeout_is_finite_and_fits_instant_on_every_platform --locked -- --test-threads=1` — exit 0; timeout is finite, below `u32::MAX` seconds, and `Instant::checked_add` succeeds. The test was red on starting HEAD `72a593f` because the generated value exceeded the bound.
- `cargo test -p devboule-daemon --lib mcp_cancel_before_host_card_prevents_registration --locked -- --test-threads=1` — exit 0 after the fix. The same test was red on starting HEAD `72a593f`: cancellation before card registration was lost, and the later host card could be allowed.
- `cargo test -p devboule-daemon --lib codex_mcp_launch_rides_config_overrides_and_keeps_the_token_in_env --locked -- --test-threads=1` — exit 0.
- `cargo test -p devboule-daemon --lib claude_bearer_file_is_removed_with_the_session_guard --locked -- --exact mcp_broker::tests::claude_bearer_file_is_removed_with_the_session_guard` — exit 0; also asserts generated config includes the per-server timeout.
- `cargo test -p devboule-daemon --lib permission --locked -- --test-threads=4` — exit 0; 98 passed.
- `cargo test -p devboule-daemon --lib pi_client --locked -- --test-threads=4` — exit 0; 80 passed.
- `cargo test -p devboule-daemon --lib pi_client --locked -- --test-threads=1` — exit 0; 80 passed with Node on PATH.
- `cargo test -p devboule-daemon --lib mcp_ --locked -- --test-threads=1` — exit 0; 218 passed.
- `cargo test -p devboule-daemon --lib broker --locked -- --test-threads=1` — exit 0; 254 passed.
- `cargo test -p devboule-daemon --lib dispatch --locked -- --test-threads=1` — exit 0; 6 passed.
- `cargo test -p devboule-daemon --lib codex --locked -- --test-threads=1` — exit 0; 162 passed.
- `cargo test -p devboule-daemon --lib claude --locked -- --test-threads=1` — exit 0; 141 passed.
- `cargo test -p devboule-daemon --lib permission --locked -- --test-threads=1` — exit 0; 99 passed.
- `cargo test -p devboule-daemon --lib live_codex_reports_a_configured_but_dead_broker_honestly --locked -- --test-threads=1` — exit 0; 1 passed. The test merges the generated carrier argv at `codex_client_tests.rs:1389-1395`; it failed twice before and passed once after, with cause unproven.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — exit 0.
- `cargo check -p devboule-daemon --no-default-features --locked` — exit 0.
- `cargo check -p devboule --lib --locked` — exit 0; the Tauri client-only library check.
- `cargo fmt --all` — exit 0.
- `cargo fmt --all -- --check` — exit 0.
- `git diff --check` — exit 0.
- `pnpm run test` — 2,970 tests passed across 158 files. The runner printed its existing Node engine mismatch warning (installed 26.7.0 vs requested >=26.8.1) and Happy DOM teardown diagnostics, but Vitest completed successfully.

An initial targeted cargo compile caught a moved-value error while building the call context; the context is now computed before moving the responder, and the targeted suites above pass.

## Limits not verified

- No live Claude or Codex long-wait tool call was run. Claude's configured maximum is finite (~24.8 days). ACP has no timeout field, and provider-side disconnect behavior remains as stated in the recon.
- No full `cargo test` was run.

## Fix pass 1

- Starting HEAD: `72a593fa10c817b58f6118c96d625590333f5a71` on `fix/card-bridge-timeouts`.
- F1: Replaced the overflow-prone parse-limit timeout with 24 days; a finite/bound test is red on starting HEAD and green after the fix.
- F2: The call token is shared with cancellation and permission registration; the new cancel-before-card regression is red on starting HEAD and proves the handler refuses without showing a card.
- F3: The pi set is generated from the daemon table and its test checks exact nonempty contents.
- F4: The live smoke test merges the launch carrier; it failed twice before and passed once after, but the cause is unproven.
- F5: Create-agent calls use the shared audit outcome path, so cancelled calls have the same audit row shape as other carded tools.
- F6: Active duplicate request IDs are refused, completed IDs can be reused, and a pending card matches the exact call token.
- F7: The cancel-before-card test uses deterministic ordering with old-HEAD red evidence; pre-existing broker tests still use bounded 1 ms polling, and the pi injected timeout margin is 280 ms.
- F8: Reworded the pi comment to match the carded and non-carded timeout behavior; the Codex 24-day cap comment states why its bound is portable.
- F9: Added the client-only dead-code attributes to the daemon-owned catalog table and helper.
- F10: At pass 1 HEAD, `permission_broker.rs` had 3,399 lines, `pi_client.rs` 3,746, `mcp_broker_tests.rs` 6,411, and `mcp_broker/mod.rs` grew from 773 to 903 lines; no line-shuffling refactor was made.
- Verification: `cargo fmt`, fmt check, diff check, all seven requested filters, the exact live Codex smoke test, full-workspace clippy, and both client-only checks all exited 0. Cargo used `CARGO_BUILD_JOBS=4` under the local cargo-slot mutex. No full cargo test, stash, or push.

## Fix pass 2

- R2: Split cancellation intent from effective cancellation (`mcp_broker/mod.rs:31,57`, `permission_broker.rs:374`). Audit outcome becomes `cancelled` only when card registration is pre-empted or an existing card is withdrawn; the open-gate regression is `caller.rs:222`.
- R2 red with the pass 1 cancellation behavior from `3c65eb8a`: `C:\Users\gualt\AppData\Local\Temp\d1b-fixpass2-r2-red.log` — failed at the audit assertion (`cancelled` vs `ok`). Green after fix: `C:\Users\gualt\AppData\Local\Temp\d1b-fixpass2-r2-green.log` — 1 passed.
- R4: Restored a wrong-session-token matcher negative with a card already waiting (`permission_broker.rs:1841`); its cancellation cannot withdraw the real card.
- Dispatch: Moved the `-32800` cancellation check after peer and policy doors (`dispatch.rs:117`); the duplicate-ID refusal now says only “already in flight” (`dispatch.rs:37`).
- R1 remains unaddressed as requested: clients normally send cancellation after their own timeout, long after the call begins (24 days for Codex; minutes or longer elsewhere).
- F4 correction: the smoke test failed twice before and passed once after; no mechanism establishes the cause.
- F10 correction: pass 1's `permission_broker.rs` count was 3,399, and `mcp_broker/mod.rs` grew 773→903 lines. Current counts are 3,429 and 909 respectively; the other existing large files remain `pi_client.rs` 3,746 and `mcp_broker_tests.rs` 6,411.
- Verification: `pi_client` 80, `mcp_` 219, `broker` 255, `dispatch` 6, `codex` 162 and `permission` 99 passed; Node is on PATH. Both client-only checks, fmt, fmt check and the gate's workspace clippy exited 0. Outputs are in `%TEMP%\d1b-fixpass2-{filter}.log`; each red/green regression log is named above. `CARGO_BUILD_JOBS=4`, cargo slot held for every Cargo command. No full cargo test, stash, or push.
