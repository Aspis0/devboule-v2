/** The text of the transcript's most recent error item, or null. The picker keeps this
 * under a memo keyed on the items, so a keystroke in the composer never walks the transcript. */
export function lastErrorText(items: readonly { role: string; text?: string }[]): string | null {
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if (item.role === "error") return item.text ?? null;
  }
  return null;
}
