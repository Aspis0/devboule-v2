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

The rest is still mock data from `mockData.ts` with no command behind it:
**Projects** renders `MOCK_PROJECTS` and `MOCK_WORKTREE_DEFAULTS`, **Devices**
renders `MOCK_DEVICES`, and **Labs** renders `MOCK_LABS`.
