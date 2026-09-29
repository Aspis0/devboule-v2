# portable-pty local patches

This file tracks intentional local changes applied on top of the vendored
`portable-pty` source. Remove a patch only when the upstream crate contains an
equivalent fix or exposes an option that keeps the same behavior.

## 0003 reject malformed Windows environments

status: active

patch: `vendor/patches/portable-pty/0003-reject-malformed-windows-environments.patch`

herdr issue: https://github.com/herdrdev/herdr/issues/3430

upstream discussions:

- https://github.com/wezterm/wezterm/issues/4364
- https://github.com/warpdotdev/warp/commit/2992d02e3e38af697c83a3bd6f20003f54ffe066

upstream pr: none

vendored base: `portable-pty 0.9.0`

local files:

- `vendor/portable-pty/src/cmdbuilder.rs`

reason: Windows process environments may contain a registry value with an
empty name or a non-string type. `winreg 0.10` converts `REG_MULTI_SZ` to an
`OsString` that can contain embedded nulls. `portable-pty` serialized those
values unchanged, producing an invalid environment block that makes
`CreateProcessW` fail with `ERROR_INVALID_PARAMETER` (87). Import only Windows
environment string types and omit entries that cannot form one complete
`name=value\0` record.

remove when: upstream `portable-pty` both imports only valid Windows environment
string types and prevents malformed names or values from corrupting the process
environment block, or Devboule replaces this launch path.

verification: the daemon-side regression tests in
`crates/devboule-daemon/src/portable_pty_tests.rs`
(`pty_environment_block_omits_entries_without_a_valid_record`,
`registry_string_values_reject_malformed_types`). They run inside the normal
workspace gate (`cargo test --workspace --exclude oracle-core --locked`) through
the `[patch.crates-io]`-resolved crate our spawns link, so no isolated copy is
needed. The patch's own in-crate test is superseded by that port: the vendored
crate is intentionally not a workspace member, so its dev-dependencies stay out
of this workspace's lockfile and its in-crate test cannot run in-tree.

## 0004 local env hardening (on top of 0003)

status: active

patch: `vendor/patches/portable-pty/0004-local-env-hardening.patch`

vendored base: `portable-pty 0.9.0` + 0003 above

local files:

- `vendor/portable-pty/src/cmdbuilder.rs`

reason: local Devboule follow-up, not upstream. Removes the logged registry
values from the two trace lines (names only), rejects a bare `=` key that
cannot form a valid record, and widens `reg_value_to_string` and
`environment_block` to `pub` so the daemon-side regression test can reach
the filter. A malformed entry is dropped silently at spawn: the vendored
crate has no log sink in a Devboule build (no `log` dependency, no logger,
daemon stderr discarded), so a visible notice waits for the daemon log sink.

remove when: 0003 is removed (this patch stacks on it), or upstream covers the
same hardening.

verification: same daemon-side tests as 0003, plus the drift guard
(`vendored_portable_pty_tree_matches_its_declared_patches`,
`vendored_cmdbuilder_is_pristine_plus_declared_patches`,
`vendored_portable_pty_resolves_to_the_vendored_tree`) in the same file.
