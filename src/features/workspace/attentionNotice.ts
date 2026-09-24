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

/**
 * The daemon's own escalation order (`AttentionReason::priority`): a raise
 * at an equal timestamp is new only when its reason outranks the one the
 * app already holds.
 */
const REASON_PRIORITY: Record<AttentionReason, number> = {
  finished: 1,
  error: 2,
  permission: 3,
};

/** Human words for why a session wants attention, as the tab pill renders them. */
export function sessionAttentionLabel(reason: AttentionReason): string {
  if (reason === "permission") return "needs approval";
  return reason;
}

/**
 * Whether `next` is a NEW raise of attention. The roster is re-published
 * constantly, so identity is the timestamp plus the reason: the daemon can
 * replace a raise with a higher-priority one in the same millisecond
 * (finished < error < permission, `AttentionReason::priority`), and that
 * escalation must fire. An equal stamp with an equal or lower reason, or
 * an older timestamp, is a duplicate or a stale push — never a new event.
 */
export function attentionRaised(
  previous: Attention | undefined,
  next: Attention | undefined,
): boolean {
  if (next === undefined) return false;
  if (previous === undefined) return true;
  if (next.atMs !== previous.atMs) return next.atMs > previous.atMs;
  // A reason missing from the table is a third state, not "lower": the
  // union does not validate the IPC payload, and an unknown raise is
  // announced rather than silently treated as stale.
  const nextPriority = REASON_PRIORITY[next.reason];
  const previousPriority = REASON_PRIORITY[previous.reason];
  if (nextPriority === undefined || previousPriority === undefined) return true;
  return nextPriority > previousPriority;
}

/**
 * The window's real state, from the OS. Inside a hidden WebView2 the
 * document still reports `visibilityState: "visible"` and `hasFocus: true`,
 * so the document can never answer this question.
 */
export interface WindowState {
  visible: boolean;
  focused: boolean;
  minimized: boolean;
}

/**
 * Whether an OS toast may fire: only when the user is not looking at the
 * window — hidden in the tray, minimized, or behind another window. A user
 * looking at Devboule (any surface) gets no toasts.
 */
export function toastGate(state: WindowState): boolean {
  return !state.visible || state.minimized || !state.focused;
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
 * session this window can see — a row this window's tab strip renders — and
 * only from what the app holds right now: a pending permission card, and
 * the last assistant message of a transcript on screen. A session the
 * window cannot see gets no content at all, so its toast carries the title
 * and the reason only. (A `Daemon`-role peer's own sessions never appear in
 * any local roster, so they never reach the toast path at all.)
 */
export function heldContentForSession(
  inThisWindow: boolean,
  pending: { title: string; description?: string } | undefined,
  lastAssistantText?: string,
): HeldContent | undefined {
  if (!inThisWindow) return undefined;
  const permissionText = [pending?.title, pending?.description]
    .filter((part) => typeof part === "string" && part.trim().length > 0)
    .join(" — ");
  const assistantText = lastAssistantText?.trim();
  const held: HeldContent = {};
  if (permissionText.length > 0) held.permissionText = permissionText;
  if (assistantText !== undefined && assistantText.length > 0) {
    held.lastAssistantText = assistantText;
  }
  return held.permissionText === undefined && held.lastAssistantText === undefined
    ? undefined
    : held;
}

/**
 * The content provider the Workspace registers. Every input is asked per
 * call — the strip's own rows (never a row an in-flight close has hidden),
 * the permission queue, and the held transcripts — so a toast is never
 * worded from a snapshot older than the raise it announces.
 */
export function workspaceHeldContentProvider(inputs: {
  rendered: (sessionId: string) => boolean;
  pending: (sessionId: string) => { title: string; description?: string } | undefined;
  heldAssistantText: (sessionId: string) => string | undefined;
}): (sessionId: string) => HeldContent | undefined {
  return (sessionId) =>
    heldContentForSession(
      inputs.rendered(sessionId),
      inputs.pending(sessionId),
      inputs.heldAssistantText(sessionId),
    );
}

/**
 * The last assistant message the app holds per session, published by the
 * surface that renders it. It is a cache, not a source: a session whose
 * transcript is not on screen has no entry, and its toast says the reason
 * alone — never a guess.
 */
const heldAssistantText = new Map<string, string>();

export function setHeldAssistantText(sessionId: string, text: string | null): void {
  if (text === null) heldAssistantText.delete(sessionId);
  else heldAssistantText.set(sessionId, text);
}

export function heldAssistantTextFor(sessionId: string): string | undefined {
  return heldAssistantText.get(sessionId);
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

/** The plugin surface `sendWithPermission` needs, narrowed to what is used.
 *  `sendNotification` is `void` on the desktop plugin and async in tests, so
 *  the caller awaits either. The permission answer is the plugin's RUST
 *  side: the plugin's own JS check reads `window.Notification.permission`,
 *  which WebView2 reports as "denied" even while the Rust side says granted
 *  — the web value never stands for the OS permission here. */
export interface NotificationPlugin {
  rustPermissionGranted(): Promise<boolean>;
  sendNotification(options: { title: string; body: string }): void | Promise<void>;
}

/**
 * The permission answer is the plugin's Rust command — one cheap in-process
 * invoke — so it is asked fresh per raise and NO denial is cached: a
 * transient `false` must not become permanent, and a cache would need a
 * module flag whose only saving is that cheap query. A denial (or any
 * refusal) THROWS, so the caller treats the raise as undelivered instead
 * of silently consuming it. On the desktop the OS still decides at toast
 * time (notifications settings, Focus Assist); there is no ask dialog to
 * repeat.
 */
export async function sendWithPermission(
  content: ToastContent,
  plugin: NotificationPlugin,
): Promise<void> {
  const granted = await plugin.rustPermissionGranted();
  if (!granted) {
    throw new Error("the OS permission answer was not granted");
  }
  await plugin.sendNotification({ title: content.title, body: content.body });
}

/** The production sender: the Rust side's permission answer, then the
 *  toast. The invoke goes straight to the plugin command; the JS wrapper
 *  would prefer the web permission, which is the one value that lies. */
async function defaultSend(content: ToastContent): Promise<void> {
  const [{ invoke }, plugin] = await Promise.all([
    import("@tauri-apps/api/core"),
    import("@tauri-apps/plugin-notification"),
  ]);
  await sendWithPermission(content, {
    rustPermissionGranted: () => invoke("plugin:notification|is_permission_granted"),
    sendNotification: (options) => plugin.sendNotification(options),
  });
}

/** Everything the OS side of a toast needs, injected for tests. */
export interface ToastDeps {
  send: (content: ToastContent) => Promise<void>;
  windowState: () => Promise<WindowState>;
}

/** How long a failed toast waits before its one retry. */
export const TOAST_RETRY_DELAY_MS = 1500;

/**
 * The production toast path, called by the roster controller on every
 * attention transition. The window gate marks a raise as seen (the user
 * was looking at the app) and stops there. A send that throws waits once
 * for `TOAST_RETRY_DELAY_MS` and tries again; a second failure is dropped —
 * the raise was announced as far as this app can push it.
 */
/** The OS truth about this window, asked at the moment it is needed. The
 *  document inside a hidden WebView2 keeps claiming visible and focused
 *  forever, so the answer can only come from the window itself. Both the
 *  toast gate and presence report read this one source. */
export async function productionWindowState(): Promise<WindowState> {
  const window = (await import("@tauri-apps/api/window")).getCurrentWindow();
  const [visible, focused, minimized] = await Promise.all([
    window.isVisible(),
    window.isFocused(),
    window.isMinimized(),
  ]);
  return { visible, focused, minimized };
}

/**
 * Subscribes to the window's focus changes for presence: a blur or focus
 * means the seen/hidden answer may have flipped, and presence must report
 * at once — the daemon drops a raise for a session it believes is
 * attended. Returns the unsubscribe.
 */
export function productionOnWindowFocusChange(
  handler: (event: { payload: boolean }) => void,
): Promise<() => void> {
  return (async () => {
    const window = (await import("@tauri-apps/api/window")).getCurrentWindow();
    return window.onFocusChanged((event) => handler({ payload: event.payload }));
  })().catch(() => {
    // No window to subscribe to (or the API is unavailable): the 5 s poll
    // remains the safety net, so the failure only costs immediacy.
    return () => undefined;
  });
}

export function fireAttentionToast(
  sessionId: string,
  title: string,
  attention: Attention,
  deps?: Partial<ToastDeps>,
): void {
  if (!attentionRaised(lastFired.get(sessionId), attention)) return;
  lastFired.set(sessionId, attention);
  const state = deps?.windowState ?? productionWindowState;
  const send = deps?.send ?? defaultSend;
  void (async () => {
    // A rejected read cannot say the user is looking: the window might be
    // in the tray, so behave as if they are not — toast anyway.
    let snapshot: WindowState;
    try {
      snapshot = await state();
    } catch {
      // A rejected read cannot say the user is looking: the window might
      // be in the tray, so behave as if they are not.
      snapshot = { visible: false, focused: false, minimized: false };
    }
    // A raise paused in the read above is stale if a newer one has since
    // taken the slot: the newer toast must not be followed by this one.
    if (lastFired.get(sessionId) !== attention) return;
    if (!toastGate(snapshot)) {
      // Seen but not raised: the user was looking at the window.
      return;
    }
    const held = heldContentProvider?.(sessionId);
    const content = toastContent(title, attention.reason, held);
    try {
      await send(content);
    } catch {
      // A newer raise may have taken the slot while this send was in
      // flight: a stale retry must not land after the newer toast.
      if (lastFired.get(sessionId) !== attention) return;
      setTimeout(() => {
        if (lastFired.get(sessionId) !== attention) return;
        void send(content).catch(() => {});
      }, TOAST_RETRY_DELAY_MS);
    }
  })();
}
