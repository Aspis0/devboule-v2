/**
 * What one key event means for the terminal. Plain Ctrl+C never emits an ETX
 * byte itself: on Windows/Linux it copies when text is selected, and on
 * macOS — where Cmd+C is the copy — it is always the interrupt, arming the
 * controller's two-step guard. Copy and interrupt come back for keydown only;
 * the matching keyup is swallowed so it can neither repeat the action nor
 * re-arm the guard. Ctrl+Shift+V passes untouched: the browser's own paste
 * event on xterm's textarea is the one paste road.
 */
export type TerminalKeyAction = "pass" | "swallow" | "copy" | "interrupt";

export interface TerminalKeyboardEvent {
  type: string;
  ctrlKey: boolean;
  /** Command key: macOS's own copy/paste road, deliberately left alone. */
  metaKey?: boolean;
  shiftKey?: boolean;
  altKey?: boolean;
  key: string;
}

export function terminalKeyPolicy(
  event: TerminalKeyboardEvent,
  hasSelection: () => boolean,
  isMac: boolean,
): TerminalKeyAction {
  if (event.metaKey) return "pass";

  const ctrl = event.ctrlKey && !event.altKey;
  const key = event.key.toLowerCase();
  const keydown = event.type === "keydown";

  if (ctrl && event.shiftKey && key === "c") return keydown ? "copy" : "swallow";
  if (ctrl && event.shiftKey && key === "v") return "pass";
  if (ctrl && !event.shiftKey && key === "c") {
    if (!keydown) return "swallow";
    if (isMac) return "interrupt";
    return hasSelection() ? "copy" : "interrupt";
  }

  return "pass";
}
