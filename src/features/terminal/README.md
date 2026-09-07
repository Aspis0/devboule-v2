# Terminal surface

App-hosted PTY terminal rendered with xterm.

`terminalSession.ts` owns one session's lifecycle against the daemon: it
creates the session, attaches and detaches the xterm view through a Tauri
`Channel`, replays the backend's retained scrollback on reattach, and maps
exits, silences, and journal-recovery states into the banners the surface
shows. `terminalRegistry.ts` is a runtime-only map that owns one live session
per workspace for the duration of the app run; React never subscribes to
terminal output or session bookkeeping through it — components adopt and
detach imperatively.

`createTerminalView.ts` builds the xterm instance and its addons.
`terminalDsr.ts` suppresses xterm's automatic cursor-position reports, because
the daemon answers those itself. `terminalKeyPolicy.ts` keeps ordinary input
intact with one deliberate exception: plain Ctrl+C is routed through a
two-step interrupt guard instead of emitting a raw ETX byte.
