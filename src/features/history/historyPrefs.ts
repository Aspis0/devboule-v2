const SHOW_ALL_KEY = "devboule.historyShowAll";

/**
 * Whether History lists every saved session (terminals and subagents) or
 * only top-level agents. Off until switched on; one JSON boolean in
 * localStorage, best-effort both ways — the same storage shape as the
 * notification switches. Anything unreadable answers off.
 */
export function getHistoryShowAll(): boolean {
  try {
    const raw = localStorage.getItem(SHOW_ALL_KEY);
    if (raw === null) return false;
    const parsed: unknown = JSON.parse(raw);
    return typeof parsed === "boolean" ? parsed : false;
  } catch {
    return false;
  }
}

export function setHistoryShowAll(value: boolean): void {
  try {
    localStorage.setItem(SHOW_ALL_KEY, JSON.stringify(value));
  } catch {
    // Storage can be full or blocked; a lost preference must not break History.
  }
}
