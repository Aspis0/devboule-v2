/**
 * Keep ordinary terminal input intact, with one deliberate exception: plain
 * Ctrl+C is routed through the two-step interrupt guard instead of emitting a
 * raw ETX byte. Keyup is swallowed too, but cannot re-arm the guard.
 */
export interface TerminalKeyboardEvent {
  type: string;
  ctrlKey: boolean;
  shiftKey?: boolean;
  altKey?: boolean;
  key: string;
}

export function terminalKeyPolicy(
  event: TerminalKeyboardEvent,
  onCtrlC?: () => void,
): boolean {
  const isPlainCtrlC =
    event.ctrlKey &&
    !event.shiftKey &&
    !event.altKey &&
    (event.key === 'c' || event.key === 'C');

  if (!isPlainCtrlC) return true;
  if (event.type === 'keydown') onCtrlC?.();
  return false;
}
