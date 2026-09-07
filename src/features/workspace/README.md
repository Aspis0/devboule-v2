# Workspace surface

What is wired and what is not:

- **Agent conversation** runs on real daemon sessions. `workspaceSessions.ts`
  lists, creates, and watches sessions through the typed commands
  (`sessions_list`, `session_create`, `sessions_watch`), and daemon liveness
  in the left sidebar footer is the polled `daemonStatus` result
  (`workspaceDaemon.ts`).
- **Projects and workspaces are mock.** `workspaceProjects.ts` builds its
  state from `MOCK_PROJECTS` in `mockData.ts`, `selectedWorkspace` starts as
  the literal `"rust-core"`, and creating a project appends a
  `mock-project-${Date.now()}` row to component state. There is no
  persistence and no Tauri command for projects or workspaces yet.
- **Side panels are mock.** The Changes/Files/app/Design/PR panels in
  `sidePanels.tsx` render hardcoded examples (`MOCK_DIFF_LINES`,
  `MOCK_SHIP_STEPS`); the Changes panel says so in its own mockup note. Real
  git integration is not built.

History lives in the left sidebar footer beside the daemon status. It is a
separate journal log view, not terminal screen restore.

## Terminal lifecycle

The terminal session is created when the terminal view mounts. A runtime-only
registry (`terminalRegistry.ts`) owns one live session per workspace for the
duration of the app run. Leaving the tab or switching away
from the Workspace surface detaches the xterm view and its Tauri `Channel`; it
does not close the session. Returning creates a new view and attaches with a
null cursor, replaying the backend's retained scrollback through that same
channel. The backend drops the unsent suffix beyond its 256 KiB
pending-output budget and resynchronises with a fresh screen snapshot, so
older output may be omitted after a long absence.

An explicit Close action calls `session_close`. A process that exits is
removed and reaped by the Rust reader/cleanup path. Terminal sessions always
use `PersistenceKind::None` on the wire, so nothing restores a terminal after
the app itself closes.
