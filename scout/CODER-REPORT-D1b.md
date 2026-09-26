# Coder report D1b — card waits and caller cancellation

## Worktree

- Repository: `devboule-v2-queue-ui`
- Branch: `fix/card-bridge-timeouts`
- Starting HEAD: `2a416398f54c0c9a4657d474d241de14aee6673a`
- No stash, push, or new branch used.

## Changes

- Added `MCP_CARD_WAIT_TOOLS` in `crates/devboule-daemon/src/provider_catalog.rs:315` as the daemon-owned set of tools that can wait on consent: create agent, create/archive workspace, and create/send-keys/kill terminal. The pi template mirrors these exact tool strings in `crates/devboule-daemon/src/pi_client.rs:353`; a test compares the mirror to the catalog.
- Pi calls for those tools keep the pi-provided abort signal but skip our 30-second timer. Every other MCP fetch remains bounded at 30 seconds (`pi_client.rs:369`). When a call fetch is aborted, pi posts `notifications/cancelled` with the original JSON-RPC request ID (`pi_client.rs:439`). The notification attempt itself has a one-second bound.
- Claude's generated MCP server entry now sets `timeout` to `2,147,483,647` ms (`mcp_broker/mod.rs:418`), the maximum accepted by Claude Code's signed 32-bit timer cap from the recon. Claude's per-server `timeout` key raises its first-byte and idle floors for this server only. This is the largest supported value, not a literal infinite timeout.
- Codex launch config sets `mcp_servers.devboule.tool_timeout_sec` to `18446744073709549568.0` (`codex_client.rs:337,358`), the largest representable `f64` below `2^64` and accepted by Codex's `Duration::try_from_secs_f64` config parser. The decimal point is intentional: Codex reads this field as seconds through `Option<Duration>`, not a TOML integer. The setting is scoped to our `devboule` server. Codex documents `tool_timeout_sec` as the per-server tool-call timeout (default 60 seconds); its source declares `Option<Duration>` with seconds deserialization: [Codex MCP docs](https://developers.openai.com/codex/mcp), [Codex config source](https://github.com/openai/codex/blob/main/codex-rs/config/src/mcp_types.rs#L2020-L2022), [seconds deserializer](https://github.com/openai/codex/blob/main/codex-rs/config/src/mcp_types.rs#L2686-L2725). The absolute maximum is derived from that source type/parser, not stated as a numeric cap in the docs.
- `notifications/cancelled` is no longer dropped (`mcp_broker/dispatch.rs:42`). A scoped MCP-call context captures `(session ID, JSON-RPC request ID)` while the synchronous `tools/call` handler runs. Host permission cards registered during that call retain the same pair. An authenticated cancellation POST resolves through its own bearer registration and attempts to take only a host card matching both values (`permission_broker.rs:854`). Non-matching and already-finished IDs are no-ops. Human Allow and cancellation share the broker's atomic take, so only one wins. Cancellation completes the normal permission path with outcome `cancelled`, publishes the resolved card, and wakes the tool handler with `HostDecision::Cancelled`; the gated operation therefore refuses before its act.
- **Connection loss choice:** no HTTP socket-drop detector was added. FIN/half-close is not proof the caller abandoned its wait; treating it as cancellation risks withdrawing a card from a live caller. This implementation relies on explicit `notifications/cancelled`. The recon verified Claude's SDK emits it. I inspected the installed qwen bundle `@qwen-code/qwen-code/chunks/chunk-4F7GQGXB.js`: its SDK cancellation closure sends the notification with `messageId` at line 63257, is invoked on the request signal at line 63290, and from the timeout handler at line 63294. Silent connection drops and Codex's timeout signal remain unverified; those clients can leave a card pending if they abandon without sending cancellation.
- grok's separate 30-minute `ask_user_question` timeout remains unchanged; this is A2b-3 P2-1.

## Tests and checks

Commands ran from the repository root. Cargo commands used `CARGO_BUILD_JOBS=4` and were serialized under the local cargo-slot mutex.

- `cargo fmt --all` — exit 0.
- `cargo fmt --all -- --check` — exit 0.
- `git diff --check` — exit 0.
- `cargo test -p devboule-daemon --lib mcp_cancel --locked -- --test-threads=1` — exit 0; 2 passed. Covers session/request scoping, no-op mismatches, journal outcome `cancelled`, and the Allow race.
- `cargo test -p devboule-daemon --lib pi_bridge_card_wait_ignores_injected_bound_but_other_tools_keep_it --locked -- --test-threads=1` — exit 0; pi card call succeeds after a 15 ms injected bound while a non-card call aborts at that bound.
- `cargo test -p devboule-daemon --lib codex_mcp_launch_rides_config_overrides_and_keeps_the_token_in_env --locked -- --test-threads=1` — exit 0.
- `cargo test -p devboule-daemon --lib claude_bearer_file_is_removed_with_the_session_guard --locked -- --exact mcp_broker::tests::claude_bearer_file_is_removed_with_the_session_guard` — exit 0; also asserts generated config includes the per-server timeout.
- `cargo test -p devboule-daemon --lib permission --locked -- --test-threads=4` — exit 0; 98 passed.
- `cargo test -p devboule-daemon --lib pi_client --locked -- --test-threads=4` — exit 0; 80 passed.
- `cargo test -p devboule-daemon --lib mcp_ --locked -- --test-threads=4` — exit 101; 250 passed and one installed-Codex smoke test failed at `initialize` (`live_codex_reports_a_configured_but_dead_broker_honestly`). A direct rerun of that test also exited 101. The smoke test starts Codex without the generated MCP launch config, so it does not exercise the new timeout. Its stderr is suppressed by the existing test.
- `cargo clippy --workspace --exclude oracle-core --all-targets --all-features --locked -- -D warnings` — exit 0.
- `cargo check -p devboule-daemon --no-default-features --locked` — exit 0.
- `cargo check -p devboule --lib --locked` — exit 0; the Tauri client-only library check.
- `pnpm run test` — 2,970 tests passed across 158 files. The runner printed its existing Node engine mismatch warning (installed 26.7.0 vs requested >=26.8.1) and Happy DOM teardown diagnostics, but Vitest completed successfully.

An initial targeted cargo compile caught a moved-value error while building the call context; the context is now computed before moving the responder, and the targeted suites above pass.

## Limits not verified

- No live Claude or Codex long-wait tool call was run. Claude's configured maximum is finite (~24.8 days). ACP has no timeout field, and provider-side disconnect behavior remains as stated in the recon.
- The broad `mcp_` filter's live Codex smoke test still fails independently during initialization; it needs a separate environment-focused diagnosis.
- No full `cargo test` was run.
