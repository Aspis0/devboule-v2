# Terminal

An xterm view over a PTY the daemon owns. The rule that shapes every file here is that
**React never subscribes to terminal output**: frames arrive over a Tauri `Channel` and
are written straight into xterm, and components adopt or detach the view imperatively.
Routing bytes through render state would put a component update between the process and
the screen.

## Ownership

`terminalRegistry.ts` is a module-level map, deliberately not Zustand, holding one record
per workspace for the duration of one app run: the session id and the last sequence number
the view has seen. That cursor is what makes reattaching a resume rather than a restart —
the daemon replays from it instead of from the beginning.

Nothing here survives the app closing. Persistence across runs belongs to the daemon's
journal, not to this module.

## Two places where xterm's defaults are wrong for us

- **`terminalDsr.ts`** — xterm answers cursor-position reports itself, through `onData`.
  The daemon owns that reply now, so a second answer would be an extra, unrequested write
  into the process's input. Only the two CPR forms are consumed; ordinary `onData` is
  untouched.
- **`terminalKeyPolicy.ts`** — plain Ctrl+C does not emit a raw ETX byte. It goes through
  the two-step interrupt guard, so an accidental keystroke cannot kill a long agent run
  without confirmation. Every other chord, including Ctrl+Shift+C and Ctrl+Alt+C, passes
  through unchanged. Keyup is swallowed as well, but cannot re-arm the guard.

## Banners say what was lost

`terminalSession.ts` reports session state as a banner rather than silently rendering a
truncated screen, because the interesting cases are all forms of missing output:

| Banner             | Means                                                                                                       |
| ------------------ | ----------------------------------------------------------------------------------------------------------- |
| `exited`           | The process ended, with the exit code and any frames and bytes measured as lost                             |
| `silent`           | Nothing has arrived for a while; the elapsed time is shown rather than guessed at                           |
| `recovered`        | Reopened from a journal nobody closed orderly — loss counters measured before the daemon died are preserved |
| `journal_degraded` | The journal itself lost frames                                                                              |
| `closed` / `error` | Ended deliberately, or failed with a reason                                                                 |

The scrollback ring is bounded, so a long absence can drop older output. That is reported,
not hidden: a terminal that quietly shows less than happened is worse than one that admits
the gap.

## Where the lifecycle is written down

The tab-level lifecycle — when a session is created, what detaching does, what an explicit
Close calls — is documented in [`../workspace/README.md`](../workspace/README.md), because
the Workspace tab drives it. This module implements it.
