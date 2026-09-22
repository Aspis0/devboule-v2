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

- **The Changes panel is real, and read-only.** `ChangesSurface.tsx` reads
  `workspace_git_status` and the selected row's `workspace_git_diff` through the
  typed commands, driven by `useWorkspaceChanges.ts`: refresh on open, a 5 s poll
  while the panel is open, and a manual Refresh button. No watcher — none exists
  in this codebase — and nothing polls while the panel is closed. Every reply
  state is its own screen: loading, _not a repository_ (a flag, not a failure),
  a clean tree, the wire's own `error` sentence shown verbatim and never
  confused with "not a repository", the row list (a `capped` row carries the `≈`
  mark, never an exact-looking number), and the selected file's diff (loading /
  binary / too large / refused / lines). The badge beside the panel's name is
  the label the open panel last read for that workspace (`changesBadge.ts`);
  closed, it keeps that value, and a workspace never read shows "—". Stage,
  Discard and commit do not exist here — the panel reads, it never writes — and
  the `cargo test · 142 passed` card is gone with them: no source of test
  results exists, and an invented number beside real data is worse than an
  empty space.

- **The Files panel is real, and read-only.** `FilesSurface.tsx` reads
  `workspace_files_list` through the typed command, driven by
  `useWorkspaceFiles.ts`: one directory per request, lazily — the root on
  open, a folder the moment its row expands, and every folder on screen
  again on the manual Refresh button. No poll and no watcher: an
  unattended tree would be a background reader of the checkout for a
  decoration nobody asked about. The daemon resolves the folder from the
  workspace id — never from a path the frontend sends — confines the
  requested path in two layers (component rules, then a walk that refuses
  links and junctions, with the same sentences the diff shows), excludes
  `.git` from every listing and refuses it when named directly — in **any**
  spelling the filesystem resolves to it (NTFS folds case, Win32 drops
  trailing dots and spaces: `.GIT`, `.git.`, `.git ` are the same folder),
  orders the
  entries itself (folders first, then names in byte order, never a locale
  collation), and stops a huge folder at its entry cap with `capped` saying
  the list is partial — never a silent cut. An entry that will not stat or
  is a link is skipped: one bad entry never fails the whole listing, and
  the reply **counts** those skips in `skipped`, which the panel shows — a
  folder with a link inside says so instead of looking complete (`.git` is
  the declared policy exclusion and is not in that count; entries past the
  cap belong to `capped`). Every
  state is its own screen — loading, no workspace, an empty folder, the
  wire's refusal sentence (root and per-folder are separate places), the
  partial-list note and the not-listed note. Folders toggle; files are rows; no rename, delete,
  create or download exists here — the panel reads, it never writes. The
  badge beside the panel's name is `read-only`, a property of the panel:
  no live source for a count exists, and an invented number beside real
  data is the defect the old mock carried.

- **The app and PR panels are still mock, and no longer pretend
  otherwise.** Their bodies in `sidePanels.tsx` render hardcoded examples
  under a note saying so, their rows are non-interactive rather than
  buttons that do nothing, and the controls that named operations this app
  cannot perform — Reindex, Export, Generate, Stage, Discard — stay removed
  instead of left drawn. "Open Design" is real and selects the Design
  surface.

History lives in the left sidebar footer beside the daemon status. It is a
separate journal log view, not terminal screen restore.

## The tab strip

The strip carries live and silent sessions and **recovered** ones as well
(`workspaceSessions.ts`). A recovered session is one whose daemon died: showing
it costs nothing, because attaching to it is reading — replay from the journal,
no process — so it comes back on its own, in a diminished state with the
transcript readable and the composer disabled behind a reason. Resuming it is a
separate click, and only for the families the daemon says are resumable
(`Session.resumable`, never re-derived here). Ended sessions stay in History:
there is nothing for them to come back to.

Swiping a tab reveals the act underneath — archive to the right, delete to the
left — and commits it past `SWIPE_COMMIT_PX`. Neither act calls the daemon.
`pendingSessionActions.ts` records the intent, the tab disappears, and one undo
window of `UNDO_WINDOW_MS` opens; the IPC fires only when it expires. Three
rules in that file are load-bearing:

- Capture happens when the press becomes a **drag**, never on `pointerdown`.
  Capturing on press retargets WebView2's compatibility mouse events and the
  click never reaches the tab button inside.
- The scheduler lives **above** the surface that unmounts. A flush in an
  unmount cleanup would fire the destructive act every time the user navigates
  to Settings; only `beforeunload` is the app closing.
- An intent is keyed by the session's **generation**, not by its id. A row that
  died and came back is not the row the swipe was taken against, so the intent
  is voided rather than applied to a different instance.

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
