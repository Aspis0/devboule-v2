# Settings surface

Seven tabs: General, Projects, Oracle, Providers & models, Devices, Labs, and
Diagnostics.

Two tabs are backed by typed daemon IPC: **Providers & models** lists the agent
CLIs found on PATH and offers refresh plus consent-gated npm install/update
(`providersList`, `providersRefresh`, `providerUpdate` in
`SettingsSurface.tsx`), and **Oracle** embeds the Oracle panel, whose values
and actions all come through the typed Oracle IPC wrappers.

Two more are partially real: **General** contains journal-usage and
journal-retention controls on real IPC (`JournalRetentionPanel.tsx`) next to
mock rows from `mockData.ts`, and **Diagnostics** reports live daemon health
through the typed `daemonDiagnostics` command (`DiagnosticsPanel.tsx`).

**Projects** lists the daemon's persisted projects through `projects_list` and
registers new ones through the same native-picker flow as the Workspace
sidebar. Its `MOCK_WORKTREE_DEFAULTS` rows are still hardcoded, because
`worktree` isolation does not exist yet for them to describe.

The rest is still mock data from `mockData.ts` with no command behind it:
**Devices** renders `MOCK_DEVICES` and **Labs** renders `MOCK_LABS`. The two
controls that promised features this product does not have — "Lock app" and
"+ Pair a device" — were removed rather than left drawn with no handler.
