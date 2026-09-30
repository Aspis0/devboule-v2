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
crate was intentionally not a workspace member, so its dev-dependencies stayed
out of this workspace's lockfile. (Patch 0005 below removed those
dev-dependencies, so the in-crate tests now run under
`cargo test -p portable-pty --offline`.)

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
the filter. A malformed entry is dropped silently at spawn: no logger is
installed in a Devboule build, so the vendored crate's `log` records go
nowhere, and this drop logs nothing even so.

remove when: 0003 is removed (this patch stacks on it), or upstream covers the
same hardening.

verification: same daemon-side tests as 0003, plus the drift guard
(`vendored_portable_pty_tree_matches_its_declared_patches`,
`vendored_cmdbuilder_is_pristine_plus_declared_patches`,
`vendored_portable_pty_resolves_to_the_vendored_tree`) in the same file.

## 0005 app-local ConPTY loader

status: active

patch: `vendor/patches/portable-pty/0005-app-local-conpty-loader.patch`

translated from: herdr's `vendor/portable-pty/src/win/psuedocon.rs` loader at
commit `3150bd92` (Apache-2.0); the pinned package values and the
verify-then-stage shape come from herdr's `packaging/windows/conpty.json` and
`scripts/package_windows_conpty.py` / `.ps1` at the same commit (see
`packaging/windows/conpty.json` and `scripts/stage-conpty.ps1` in this
repository).

upstream pr: none

vendored base: `portable-pty 0.9.0` + 0003 and 0004 above

local files:

- `vendor/portable-pty/Cargo.toml`
- `vendor/portable-pty/src/win/conpty_loader.rs` (new file)
- `vendor/portable-pty/src/win/mod.rs`
- `vendor/portable-pty/src/win/psuedocon.rs`

reason: portable-pty 0.9 opened `conpty.dll` by bare name, so the Windows
DLL search order — PATH and the current directory included — decided what
loaded into whichever process owns the pseudoconsoles. The loader now derives
an absolute candidate beside `current_exe` and loads it with `LoadLibraryExW`
restricted to the DLL's own directory plus System32; anything short of a
loadable, symbol-complete table falls back to kernel32 cleanly, without a
panic and without a partially populated table, and the failed load keeps its
reason for `conpty_source()` to report. herdr's reparse-point rejection is
kept for the two staged files: a `conpty.dll` or `OpenConsole.exe` that is a
symlink or junction is not trusted, and the bundle is all-or-nothing — the
inbox path is used unless both files are present. The staged layout is
herdr's deployed one (`conpty/conpty.dll` plus `conpty/x64/OpenConsole.exe`
inside the executable's directory, as `herdr-src/distribution/install.ps1`
verifies), and other architectures never consult a bundle, matching herdr's
`x86_64` gate. The crate has no logger; the choice is reported through
`conpty_source()` for the daemon to surface through its own log. The
manifest changes exist so the loader's in-crate tests can run:
`cargo test -p portable-pty` refuses any non-workspace package that declares
dev-dependencies, and the two declared here served only the `whoami_async`
example, whose declaration is dropped with them. `shared_library` is dropped
because the loader replaced its last use, and the `winapi` `libloaderapi`
feature covers `LoadLibraryExW` and friends.

remove when: upstream portable-pty stops opening `conpty.dll` by bare name or
gains an equivalent app-local option.

verification: `cargo test --workspace --exclude oracle-core --locked` runs
the daemon-side tests in `crates/devboule-daemon/src/portable_pty_tests.rs`
— the production-seam ones (a real pseudoconsole through `openpty` on the
inbox table whose `cmd /c exit 0` child must exit within 30 s, and the
ignored app-local twin that runs when a bundle is staged beside the test
executable), the daemon-side patch pins, and the drift guard. The vendored
crate keeps only what the public API cannot reach, under
`cargo test -p portable-pty --offline`: `choose_conpty` with synthetic
directories (absent bundle, missing executable path, current-directory decoy,
unloadable DLL, symbol-less library, missing host — each pinning its fallback
and its `conpty_source` phrase), the both-files gate, and the reparse-point
rejection (mask arithmetic without privileges, plus the end-to-end symlink
leg where the machine grants the privilege). The selection scenarios
deliberately stay in the vendored crate: the selection is a process-wide
one-shot keyed on `current_exe`'s directory, so a daemon lib test can
exercise exactly one outcome (the gate covers it — the inbox phrase, asserted
through the production `openpty`), and these scenarios need directories a
daemon test cannot point the selector at.
