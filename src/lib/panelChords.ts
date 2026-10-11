import { isImeComposition } from "./imeComposition";

/** A side panel's toggle, or both together. */
export type PanelChord = "sidebar" | "panel" | "both";

const PANEL_CHORDS: readonly { chord: PanelChord; key: string }[] = [
  { chord: "sidebar", key: "b" },
  { chord: "panel", key: "e" },
  { chord: "both", key: "." },
];

/**
 * The panel-toggle chords, or null: the command key with B, E or period. Ctrl
 * on Windows and Linux stands down inside a terminal, where tmux's Ctrl+B and
 * readline's Ctrl+E belong to the shell; ⌘ means nothing to a terminal and
 * still fires there. The visible toggle buttons always work.
 */
export function panelChordFor(
  event: Pick<
    KeyboardEvent,
    "key" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey" | "isComposing" | "keyCode"
  > & { target: EventTarget | null },
  mac: boolean,
): PanelChord | null {
  if (isImeComposition(event)) return null;
  if (event.altKey || event.shiftKey) return null;
  const command = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (!command) return null;
  const found = PANEL_CHORDS.find((entry) => entry.key === event.key.toLowerCase());
  if (found === undefined) return null;
  if (!mac && isInTerminal(event.target)) return null;
  return found.chord;
}

function isInTerminal(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && target.closest(".workspace-terminal-shell") !== null;
}
