# Workspace surface

The Workspace surface keeps the agent conversation and side panels mocked while
the terminal tab uses the app-hosted PTY session contract.

## M2 terminal lifecycle

The terminal session is created when the terminal tab first becomes active. A
runtime-only registry owns one live session per workspace for the duration of
the app run. Leaving the tab or switching away from the Workspace surface
detaches the xterm view and its Tauri `Channel`; it does not close the session.
Returning creates a new view and attaches with a null cursor, replaying the
backend's retained scrollback through that same channel. The 256 KiB ring may
quietly omit older output after a long absence.

An explicit Close action calls `session_close`. A process that exits is removed
and reaped by the Rust reader/cleanup path, and app shutdown drains all live
sessions through `kill_all_on_exit`. Persistence after the app itself closes
(daemon, journal, and resume) remains an M3 concern.
