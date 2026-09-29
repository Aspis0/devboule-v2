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

## Where xterm's defaults are wrong for us

- **`terminalQuerySuppression.ts`** — one responder per query class: every query gets
  exactly one answer. xterm answers protocol queries itself through `onData`, and that
  reply would reach the child as a second answer — typed at the shell prompt once the
  querying app has exited — for every class the daemon also answers. The suppression
  therefore consumes xterm's reply only for the classes the daemon really answers and
  leaves every other class with xterm as its one responder:

  | query                                  | daemon  | xterm client                                                                                              |
  | -------------------------------------- | ------- | --------------------------------------------------------------------------------------------------------- |
  | `CSI c` (DA1), `CSI > c` (DA2)         | answers | suppressed                                                                                                |
  | `CSI 5 n` (DSR 5), `CSI 6 n` (CPR)     | answers | suppressed                                                                                                |
  | `CSI Ps $ p` / `CSI ? Ps $ p` (DECRQM) | answers | suppressed                                                                                                |
  | `CSI 18 t` (text area size)            | answers | suppressed                                                                                                |
  | `CSI ? 6 n` (DECXCPR)                  | —       | answers                                                                                                   |
  | `DCS $ q` (DECRQSS)                    | —       | answers                                                                                                   |
  | `OSC 4/10/11/12 ;?` (colour queries)   | —       | answers                                                                                                   |
  | `CSI 14 t` / `CSI 16 t` (pixel sizes)  | —       | only with `windowOptions`, which this product leaves off — nobody answers today (pre-existing, follow-up) |
  | `CSI = c` (DA3)                        | —       | — (neither answers it, so nothing to suppress)                                                            |

- **`terminalKeyPolicy.ts`** — plain Ctrl+C never emits a raw ETX byte. On Windows and
  Linux it copies when text is selected, and otherwise goes through the two-step
  interrupt guard, so an accidental keystroke cannot kill a long agent run without
  confirmation; on macOS it is always the interrupt, and Cmd+C keeps the native copy.
  Ctrl+Shift+C copies; Ctrl+Shift+V is left to the browser's own paste event on xterm's
  textarea. A refused copy clears the selection anyway, so the interrupt stays
  reachable, and the Ctrl+C chip shows "Copy failed" for a beat. Keyup is swallowed as
  well, but cannot re-arm the guard.

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

`createTerminalView.ts` builds the xterm instance and its addons, so the two policy files
above have one place to attach to rather than a view assembled at each call site.
