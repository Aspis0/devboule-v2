// One sentence for a journal that lost transcript frames, shared by the
// surfaces that render an agent conversation.

/**
 * Bytes the way the terminal banner prints them (1000-based).
 */
export function humanSize(bytes: number): string {
  const units = ["B", "KB", "MB", "GB"];
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  let digits = unit === 0 || value >= 10 ? 0 : 1;
  let rounded = Math.round(value * 10 ** digits) / 10 ** digits;
  // Rounding can cross a unit boundary: 999 999 bytes must not read "1000 KB".
  if (rounded >= 1000 && unit < units.length - 1) {
    rounded = Math.round(rounded / 1000);
    unit += 1;
    digits = 0;
  }
  return `${rounded.toFixed(digits)} ${units[unit]}`;
}

/**
 * The journal-loss notice: pinned for the rest of the session's life, because
 * the transcript on disk stays incomplete no matter what happens next.
 */
export function journalLossCopy(loss: { frames: number; bytes: number }): string {
  const frame = loss.frames === 1 ? "frame" : "frames";
  // Frames and bytes are worst-known per field independently; the sentence
  // must not join them into one measured loss that never occurred.
  return `This conversation is not being saved: at least ${loss.frames} ${frame} and at least ${humanSize(loss.bytes)} of it could not be written to disk.`;
}
