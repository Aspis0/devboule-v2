# Settings surface

Fourteen pages in five groups, behind one left menu (`SettingsSurface.tsx`,
menu catalogue in `settingsMenu.ts`, icons in `menuIcons.tsx`): **This machine**
(Appearance, Layout, Editing, Shortcuts, Notifications, Diagnostics),
**Providers & agents** (Providers, Agent profiles, Usage), **Workspace**
(Projects, Oracle), **Devices** (a static "This PC" row, Paired devices,
Permissions), **About**. The shell renders every page's title and intro; pages
with no function yet render an honest empty state instead of controls.

Panels with function live in `panels/` (`ProvidersPanel.tsx`,
`AgentsPanel.tsx` with `DelegationSetting`, `ProjectsPanel.tsx`); the
Providers/Agents/Projects/Devices/Diagnostics panels are backed by typed
daemon IPC, Appearance by local theme storage, the window-close choice by
surface settings, Default send by local storage, and journal retention by
`journalRetentionGet`/`Set` plus `journalUsage`. Per-provider status and
tool-policy reads live in `providerStatus.ts`.

Each page owns its stylesheet (`providers.css`, `profiles.css`, `devices.css`,
`rows.css` for the shared row pattern, `settingsSwitch.css` for the switch,
`diagnostics.css` for the retry pill, copy note and retention input);
`settings.css` holds base tokens, the shell, and the shared card/stack
primitives every page renders inside — later slices read it but do not edit
it. The transcript-history rules live with the Diagnostics page, not with the
component file, for exactly that reason. The one exception is Usage: it has no
sheet of its own, and renders its window rows and bar from the shared
`.plan-window*` rules in `src/styles/global.css`, beside the context popover
that draws the same block.

No mock data is left in this surface. The two controls that promised
features this product does not have — "Lock app" and "+ Pair a device" —
were removed rather than left drawn with no handler; pairing now has real
commands behind it.
