# Workspace surface

What is wired and what is not:

- **Agent conversation** runs on real daemon sessions. `workspaceSessions.ts`
  lists, creates, and watches sessions through the typed commands
  (`sessions_list`, `session_create`, `sessions_watch`), and daemon liveness
  in the left sidebar footer is the polled `daemonStatus` result
  (`workspaceDaemon.ts`).
- **Projects and workspaces are real.** `workspaceProjects.ts` loads them over
  `projects_list` and `workspaces_list`, `NewProjectDialog` registers a folder
  chosen with the native picker through `project_add`, and the daemon persists
  them in the journal. `selectedWorkspace` starts as `null` and settles on real
  data. The id returned by the daemon is authoritative: registering the same
  folder twice updates the existing project rather than adding a second one, so
  nothing here derives an id or a name from the path.

  The selected workspace id is what reaches `session_create` and
  `TerminalSurface`. Only the id travels — the daemon resolves the directory
  from it and echoes back `Session.cwd`, which is display-only and lossy and
  must never be compared, keyed on, or sent back.

  `worktree` isolation is still refused by the daemon, so every workspace is
  the project folder itself until git worktrees land.

- **Side panels are mock, and no longer pretend otherwise.** The
  Changes/Files/app/Design panels in `sidePanels.tsx` render hardcoded examples
  and each carries a note saying so. Their rows are non-interactive rather than
  buttons that do nothing, and the controls that named operations this app
  cannot perform — Stage, Discard, Reindex, Export, Generate — were removed
  instead of left drawn. "Open Design" is real and selects the Design surface.
  There is no git status and no file tree on the wire yet.

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
