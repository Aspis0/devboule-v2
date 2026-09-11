# Settings surface

Six tabs: General, Projects, Oracle, Providers & models, Devices, and
Diagnostics.

Four tabs are backed by typed daemon IPC: **Providers & models** lists the
agent CLIs found on PATH and offers refresh plus consent-gated npm
install/update (`providersList`, `providersRefresh`, `providerUpdate` in
`SettingsSurface.tsx`), **Oracle** embeds the Oracle panel, whose values
and actions all come through the typed Oracle IPC wrappers, **General**
contains only the journal-usage and journal-retention controls on real IPC
(`JournalRetentionPanel.tsx`), and **Diagnostics** reports live daemon
health through the typed `daemonDiagnostics` command
(`DiagnosticsPanel.tsx`).

**Projects** lists the daemon's persisted projects through `projects_list`
and registers new ones through the same native-picker flow as the
Workspace sidebar.

The only mock left is **Devices**, which renders the hardcoded
`MOCK_DEVICES` rows: no device or pairing IPC exists yet for a typed
response to replace them. The two controls that promised features this
product does not have — "Lock app" and "+ Pair a device" — were removed
rather than left drawn with no handler.
