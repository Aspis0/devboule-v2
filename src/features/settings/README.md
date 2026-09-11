# Settings surface

Six tabs: General, Projects, Oracle, Providers & models, Devices, and
Diagnostics.

Five tabs are backed by typed daemon IPC: **Providers & models** lists the
agent CLIs found on PATH and offers refresh plus consent-gated npm
install/update (`providersList`, `providersRefresh`, `providerUpdate` in
`SettingsSurface.tsx`), **Oracle** embeds the Oracle panel, whose values
and actions all come through the typed Oracle IPC wrappers, **General**
contains only the journal-usage and journal-retention controls on real IPC
(`JournalRetentionPanel.tsx`), **Diagnostics** reports live daemon
health through the typed `daemonDiagnostics` command
(`DiagnosticsPanel.tsx`), and **Devices** shows this device's identity,
both pairing directions, the pairings waiting for confirmation here, and
the paired list with its capability toggles and revoke
(`DevicesPanel.tsx`).

**Projects** lists the daemon's persisted projects through `projects_list`
and registers new ones through the same native-picker flow as the
Workspace sidebar.

No mock data is left in this surface. The two controls that promised
features this product does not have — "Lock app" and "+ Pair a device" —
were removed rather than left drawn with no handler; pairing now has real
commands behind it.
