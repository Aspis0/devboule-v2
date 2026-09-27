/**
 * The Notifications page's two switches: whether an attention raise may
 * become an OS toast, and whether that toast may quote message text. One
 * JSON boolean per key in localStorage, best-effort both ways — the same
 * storage shape as the send-behavior store, deliberately not its cache:
 * that store answers from a module variable, while this one must answer
 * from storage on every read (next paragraph). No daemon round-trip is
 * owed for a preference only this app reads.
 *
 * Every read goes back to storage rather than a module cache: the toast
 * path asks at fire time, so a switch flipped between two raises must move
 * the second one with no remount in between. Anything unreadable — a
 * missing key, a foreign value, a blocked store — answers on: today's
 * behaviour is toasts with previews, and a damaged store must not silence
 * them by accident.
 */

const SHOW_KEY = "devboule.showNotifications";
const PREVIEWS_KEY = "devboule.showMessagePreviews";

function readFlag(key: string): boolean {
  try {
    const raw = localStorage.getItem(key);
    if (raw === null) return true;
    const parsed: unknown = JSON.parse(raw);
    return typeof parsed === "boolean" ? parsed : true;
  } catch {
    return true;
  }
}

function writeFlag(key: string, value: boolean): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Storage can be full or blocked; a lost preference must not break the app.
  }
}

/** The subscriber sets, declared above the setters that close over them. */
const showListeners = new Set<(value: boolean) => void>();
const previewsListeners = new Set<(value: boolean) => void>();

/** Whether an attention raise may become an OS toast. On until switched off. */
export function getShowNotifications(): boolean {
  return readFlag(SHOW_KEY);
}

export function setShowNotifications(value: boolean): void {
  writeFlag(SHOW_KEY, value);
  for (const listener of showListeners) listener(value);
}

export function subscribeShowNotifications(listener: (value: boolean) => void): () => void {
  showListeners.add(listener);
  return () => {
    showListeners.delete(listener);
  };
}

/** Whether the toast may quote the last assistant message or the pending
 * request. On until switched off; off names the session and the reason only
 * (the preview reaches the lock screen). */
export function getShowMessagePreviews(): boolean {
  return readFlag(PREVIEWS_KEY);
}

export function setShowMessagePreviews(value: boolean): void {
  writeFlag(PREVIEWS_KEY, value);
  for (const listener of previewsListeners) listener(value);
}

export function subscribeShowMessagePreviews(listener: (value: boolean) => void): () => void {
  previewsListeners.add(listener);
  return () => {
    previewsListeners.delete(listener);
  };
}
