# Workspace surface

The Workspace surface keeps the agent conversation and side panels mocked while
the terminal tab uses the app-hosted PTY session contract.

## M2 terminal lifecycle

The terminal session is created when the terminal tab first mounts. Its output
is attached through a Tauri `Channel`, and the session is closed when the tab,
Workspace surface, or app surface unmounts. Closing on unmount is intentional
for M2: sessions do not survive surface switches yet. Session persistence and
reattachment belong to M3.
