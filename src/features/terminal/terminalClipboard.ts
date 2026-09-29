import type { Terminal } from "@xterm/xterm";
import { copyToClipboard } from "../../lib/clipboard";

/**
 * Write the terminal's selection to the clipboard through the repo's one
 * clipboard helper. Every attempt clears the selection — a kept one would
 * trap Ctrl+C on a copy that can never succeed — and the result is returned
 * so a refused write can be shown instead of swallowed.
 */
export function copyTerminalSelection(terminal: Terminal): Promise<boolean> {
  const selection = terminal.getSelection();
  if (selection === "") return Promise.resolve(true);
  terminal.clearSelection();
  return copyToClipboard(selection);
}
