import type { Attention, AttentionReason } from "../../types/ipc";
import { surfaceSettingsGet } from "../../lib/tauri";

/**
 * How a session's attention presents to the local user: the words for a
 * reason, the decision of when an OS toast may fire, what the toast says,
 * and the one place that sends it.
 *
 * The daemon already suppresses raises for a session a focused window is
 * looking at (daemon-side policy); nothing here repeats that rule. This
 * module's own gate is the window-level one: a toast is for a user who is
 * not looking at Devboule at all — hidden in the tray, minimized, or
 * unfocused.
 */

/** Human words for why a session wants attention, as the tab pill renders them. */
export function sessionAttentionLabel(reason: AttentionReason): string {
  if (reason === "permission") return "needs approval";
  return reason;
}

/**
 * Whether `next` is a NEW raise of attention. The roster is re-published
 * constantly, so identity is the timestamp: a raise the app has seen (same
 * `atMs`) must not fire again, no matter how many pushes carry it. An older
 * timestamp can only be a stale push, never a new event.
 */
export function attentionRaised(
  previous: Attention | undefined,
  next: Attention | undefined,
): boolean {
  if (next === undefined) return false;
  if (previous === undefined) return true;
  return next.atMs > previous.atMs;
}

/**
 * Whether an OS toast may fire: only when the window is NOT both visible
 * and focused — hidden in the tray, minimized, or behind another window.
 * A user looking at Devboule (any surface) gets no toasts.
 */
export function toastGate(appVisible: boolean, appFocused: boolean): boolean {
  return !(appVisible && appFocused);
}

/** ~220 characters of preview, Paseo's NOTIFICATION_PREVIEW_LIMIT. */
export const PREVIEW_LIMIT = 220;

/**
 * Markdown reduced to the words it would render: links keep their text,
 * emphasis and heading markers go, inline code keeps its content, and runs
 * of whitespace collapse. Plain text passes through untouched.
 */
export function stripMarkdown(text: string): string {
  const withoutLinks = text.replace(/\[([^\]]+)\]\([^)]*\)/g, "$1");
  const withoutInlineCode = withoutLinks.replace(/`([^`]*)`/g, "$1");
  const withoutEmphasis = withoutInlineCode.replace(/(\*\*|__|\*|_)(.*?)\1/g, "$2");
  const withoutHeadings = withoutEmphasis
    .split("\n")
    .map((line) => line.replace(/^\s{0,3}#{1,6}\s+/, "").replace(/^\s*[-*+]\s+/, ""))
    .join("\n");
  return withoutHeadings
    .replace(/[ \t]+/g, " ")
    .replace(/ *\n */g, "\n")
    .replace(/\n+/g, "\n")
    .trim();
}

/** The finished-session preview: markdown-stripped, cut on a boundary. */
export function previewFrom(text: string): string {
  const plain = stripMarkdown(text);
  if (plain.length <= PREVIEW_LIMIT) return plain;
  const cut = plain.slice(0, PREVIEW_LIMIT);
  const boundary = cut.lastIndexOf(" ");
  // A single word longer than the limit is cut hard; otherwise the cut
  // lands after the last whole word.
  return (boundary > 0 ? cut.slice(0, boundary) : cut).trimEnd() + "…";
}

/** Message content the app already holds for a session, if any. */
export interface HeldContent {
  /** The pending permission request's normalized text, when held. */
  permissionText?: string;
  /** The last assistant message's text, when the app holds the transcript. */
  lastAssistantText?: string;
}

/** What an OS toast carries: the session and its reason, always; content only when held. */
export interface ToastContent {
  title: string;
  body: string;
}

/**
 * The toast's words. The title names the session and the reason; the body
 * carries the held permission request for `permission`, or a short preview
 * of the last assistant message for `finished` — and the bare reason when
 * the app holds nothing, never a guess.
 */
export function toastContent(
  sessionTitle: string,
  reason: AttentionReason,
  held: HeldContent | undefined,
): ToastContent {
  const title = `${sessionTitle} — ${sessionAttentionLabel(reason)}`;
  if (reason === "permission") {
    const permissionText = held?.permissionText?.trim();
    return { title, body: permissionText || sessionAttentionLabel(reason) };
  }
  if (reason === "finished" && held?.lastAssistantText) {
    return { title, body: previewFrom(held.lastAssistantText) };
  }
  return { title, body: reason };
}

/**
 * The stored "Play sound" choice, the one notification setting there is.
 * Same surface-settings contract as the close behavior: absent means the
 * default (on), unreadable never overwrites anything.
 */
export type NotificationSoundSettings = { playSound: boolean };

export function playSoundFromStored(value: unknown): boolean {
  if (typeof value === "object" && value !== null && "playSound" in value) {
    const playSound = (value as { playSound: unknown }).playSound;
    if (typeof playSound === "boolean") return playSound;
  }
  return true;
}

export const NOTIFICATIONS_SURFACE_ID = "notifications";

/** Reads the stored sound choice; anything unreadable means the default (on). */
export function loadPlaySoundSetting(): Promise<boolean> {
  return surfaceSettingsGet(NOTIFICATIONS_SURFACE_ID).then(
    (read) => (read.status === "value" ? playSoundFromStored(read.value) : true),
    () => true,
  );
}

/** Everything the OS side of a toast needs, injected for tests. */
export interface ToastDeps {
  send: (content: ToastContent & { silent: boolean }) => void;
  visible: () => boolean;
  focused: () => boolean;
  playSound: () => Promise<boolean>;
}

/** The last raise a toast actually fired for, per session. */
const lastFired = new Map<string, Attention>();

/** Message content the app holds per session, registered by the surface that holds it. */
let heldContentProvider: ((sessionId: string) => HeldContent | undefined) | null = null;

export function setAttentionHeldContentProvider(
  provider: ((sessionId: string) => HeldContent | undefined) | null,
): void {
  heldContentProvider = provider;
}

/**
 * The production toast path, called by the roster controller on every
 * attention transition. Dedupe lives here (per session, by the raise's
 * timestamp) so a push storm can never double-fire; the window gate is
 * this module's own toastGate, never the daemon's suppression rule
 * restated.
 */
export function fireAttentionToast(
  session: { id: string; title: string },
  attention: Attention,
  deps?: Partial<ToastDeps>,
): void {
  if (!attentionRaised(lastFired.get(session.id), attention)) return;
  const visible =
    deps?.visible ??
    (typeof document === "undefined" ? () => false : () => document.visibilityState === "visible");
  const focused =
    deps?.focused ?? (typeof document === "undefined" ? () => false : () => document.hasFocus());
  if (!toastGate(visible(), focused())) {
    // Seen but not raised: remember the raise so a later push of the same
    // event cannot toast after the user has already looked past it.
    lastFired.set(session.id, attention);
    return;
  }
  lastFired.set(session.id, attention);
  const playSound = deps?.playSound ?? loadPlaySoundSetting;
  const held = heldContentProvider?.(session.id);
  const content = toastContent(session.title, attention.reason, held);
  void playSound().then((sound) => {
    (deps?.send ?? defaultSend)({ ...content, silent: !sound });
  });
}

function defaultSend(toast: ToastContent & { silent: boolean }): void {
  void import("@tauri-apps/plugin-notification").then(({ sendNotification }) => {
    void sendNotification({ title: toast.title, body: toast.body, silent: toast.silent });
  });
}

/** Forget fired raises the roster no longer carries (closed sessions). */
export function forgetAttentionFor(sessionIds: ReadonlySet<string>): void {
  for (const id of [...lastFired.keys()]) {
    if (!sessionIds.has(id)) lastFired.delete(id);
  }
}
