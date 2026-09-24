import type { Attention, AttentionReason } from "../../types/ipc";

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
 * The toast's words. The title names the session the way the workspace
 * shows it; the body carries the held permission request for `permission`,
 * or a short preview of the last assistant message for `finished` — and
 * the bare reason when the app holds nothing, never a guess.
 */
export function toastContent(
  sessionTitle: string,
  reason: AttentionReason,
  held: HeldContent | undefined,
): ToastContent {
  const title = `${sessionTitle} — ${sessionAttentionLabel(reason)}`;
  if (reason === "permission") {
    const permissionText = held?.permissionText?.trim();
    return {
      title,
      body: permissionText ? previewFrom(permissionText) : sessionAttentionLabel(reason),
    };
  }
  if (reason === "finished" && held?.lastAssistantText) {
    return { title, body: previewFrom(held.lastAssistantText) };
  }
  return { title, body: reason };
}

/**
 * What the window that holds the cards hands the toast: content ONLY for a
 * session this window can see — a row in this window's roster — and only
 * while a permission card for it is actually pending. A session the window
 * cannot see gets no provider content, so its toast carries the title and
 * the reason only. (A `Daemon`-role peer's own sessions never appear in any
 * local roster, so they never reach the toast path at all.)
 */
export function heldContentForSession(
  inThisWindow: boolean,
  pending: { title: string; description?: string } | undefined,
): HeldContent | undefined {
  if (!inThisWindow) return undefined;
  const text = [pending?.title, pending?.description]
    .filter((part) => typeof part === "string" && part.trim().length > 0)
    .join(" — ");
  return text.length > 0 ? { permissionText: text } : undefined;
}

/**
 * Message content the app holds per session, registered by the surface that
 * holds it (the Workspace: its roster is what "this window can see" means,
 * and its permission queue is what "holds" means).
 */
let heldContentProvider: ((sessionId: string) => HeldContent | undefined) | null = null;

export function setAttentionHeldContentProvider(
  provider: ((sessionId: string) => HeldContent | undefined) | null,
): void {
  heldContentProvider = provider;
}

/** The last raise a toast fired (or was gate-blocked) for, per session. */
const lastFired = new Map<string, Attention>();

/** Forget raises of sessions that left the roster, so the map cannot grow forever. */
export function forgetAttentionFor(sessionIds: ReadonlySet<string>): void {
  for (const id of [...lastFired.keys()]) {
    if (!sessionIds.has(id)) lastFired.delete(id);
  }
}

/** The plugin surface `sendWithPermission` needs, narrowed to what is used. */
export interface NotificationPlugin {
  isPermissionGranted(): Promise<boolean>;
  requestPermission(): Promise<NotificationPermission>;
  sendNotification(options: { title: string; body: string }): Promise<void>;
}

/**
 * The plugin's own gate: if permission was never granted, ONE
 * `requestPermission` ask is made — never one per raise — and an ungranted
 * raise is an error, not a silent nothing, so the caller can treat it as
 * undelivered.
 */
export async function sendWithPermission(
  content: ToastContent,
  plugin: NotificationPlugin,
): Promise<void> {
  let granted = await plugin.isPermissionGranted();
  if (!granted) {
    granted = (await plugin.requestPermission()) === "granted";
  }
  if (!granted) {
    throw new Error("notification permission was not granted");
  }
  await plugin.sendNotification({ title: content.title, body: content.body });
}

let permissionAsked = false;

/** The production sender: the plugin's permission flow, then the toast. */
async function defaultSend(content: ToastContent): Promise<void> {
  const plugin = await import("@tauri-apps/plugin-notification");
  let granted = await plugin.isPermissionGranted();
  if (!granted && !permissionAsked) {
    permissionAsked = true;
    granted = (await plugin.requestPermission()) === "granted";
  }
  if (!granted) return;
  await plugin.sendNotification({ title: content.title, body: content.body });
}

/** Everything the OS side of a toast needs, injected for tests. */
export interface ToastDeps {
  send: (content: ToastContent) => Promise<void>;
  visible: () => boolean;
  focused: () => boolean;
}

/**
 * The production toast path, called by the roster controller on every
 * attention transition. The window gate marks a raise as seen (the user
 * was looking at the app); a send marks the raise delivered, and a failed
 * or refused send leaves it unmarked so the next push of the same raise
 * retries.
 */
export function fireAttentionToast(
  sessionId: string,
  title: string,
  attention: Attention,
  deps?: Partial<ToastDeps>,
): void {
  if (!attentionRaised(lastFired.get(sessionId), attention)) return;
  const visible =
    deps?.visible ??
    (typeof document === "undefined" ? () => false : () => document.visibilityState === "visible");
  const focused =
    deps?.focused ?? (typeof document === "undefined" ? () => false : () => document.hasFocus());
  lastFired.set(sessionId, attention);
  if (!toastGate(visible(), focused())) {
    // Seen but not raised: the user was looking at the app.
    return;
  }
  const held = heldContentProvider?.(sessionId);
  const content = toastContent(title, attention.reason, held);
  void (deps?.send ?? defaultSend)(content).catch(() => {
    // Undelivered: forget the raise so the next push of it retries —
    // unless a newer raise has since taken the slot.
    if (lastFired.get(sessionId) === attention) lastFired.delete(sessionId);
  });
}
