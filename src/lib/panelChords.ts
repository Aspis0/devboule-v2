import { isImeComposition } from "./imeComposition";

/** A side panel's toggle, or both together. */
export type PanelChord = "sidebar" | "panel" | "both";

const PANEL_CHORDS: readonly { chord: PanelChord; key: string }[] = [
  { chord: "sidebar", key: "b" },
  { chord: "panel", key: "e" },
  { chord: "both", key: "." },
];

/**
 * The panel-toggle chords, or null: the command key with B, E or period, once
 * per press. Inside a terminal every chord stands down, since the shell owns
 * its keys there; the visible toggle buttons always work.
 */
export function panelChordFor(
  event: Pick<
    KeyboardEvent,
    "key" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey" | "isComposing" | "keyCode" | "repeat"
  > & { target: EventTarget | null },
  mac: boolean,
): PanelChord | null {
  if (event.repeat || isImeComposition(event)) return null;
  if (event.altKey || event.shiftKey) return null;
  const command = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (!command) return null;
  const found = PANEL_CHORDS.find((entry) => entry.key === event.key.toLowerCase());
  if (found === undefined) return null;
  if (isInTerminal(event.target)) return null;
  return found.chord;
}

function isInTerminal(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && target.closest(".workspace-terminal-shell") !== null;
}
