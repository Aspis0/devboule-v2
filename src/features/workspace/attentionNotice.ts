import type { Attention, AttentionReason } from "../../types/ipc";
import { lookedAtSessionId } from "./presence";

/**
 * How a session's attention presents to the local user: the words for a
 * reason, the decision of when an OS toast may fire, what the toast says,
 * and the one place that sends it.
 *
 * The gate is Paseo's per-agent rule (`session-context.tsx:279-284`): a
 * raise is announced unless the user is looking at THIS session — the window
 * actively seen AND this session the one this window shows. Which rows the
 * tab strip draws is not the question the gate asks, so nothing here is
 * parked or flushed: a raise for a session in another workspace announces
 * while this window stays focused, and its toast quotes only what this window
 * holds for it (see `heldContentForSession`).
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
 * Whether an OS toast may fire for `sessionId`: the user is away from THAT
 * session. Away means the window is not actively seen (hidden in the tray,
 * minimized, or behind another window), or this window shows some other
 * session than the one that raised. Seeing the window alone does not make a
 * raise seen: navigation scopes the strip to one workspace, so a raise in
 * another one is news nobody has looked at.
 */
export function toastGate(state: WindowState, sessionId: string, lookedAt: string | null): boolean {
  const windowSeen = state.visible && state.focused && !state.minimized;
  return !windowSeen || lookedAt !== sessionId;
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
 * only from what the app holds right now: a pending permission card, and the
 * last assistant message of a transcript on screen. A session the window does
 * not render gets no content at all, so its toast carries the title and the
 * reason only. (Paseo builds the body from the raising agent's own stream,
 * which its store always carries; this app holds words for the rows on screen
 * alone, so a raise elsewhere must not be dressed up in words it never had.)
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
 * What a window that renders sessions registers: the words a toast may
 * quote, built from the strip's rows, the permission queue and the held
 * transcripts — every input asked per call, so a toast never speaks from a
 * snapshot older than the raise it announces. Whether the raise ANNOUNCES at
 * all is the gate's answer, never this provider's: it decides wording only.
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
 * What the surface that renders sessions registered (the Workspace: its
 * strip is what "this window can see" means for the wording). Null between
 * registrations and on surfaces without a strip, where a toast quotes nothing.
 */
let heldContentProvider: ((sessionId: string) => HeldContent | undefined) | null = null;

export function setAttentionHeldContentProvider(
  provider: ((sessionId: string) => HeldContent | undefined) | null,
): void {
  heldContentProvider = provider;
}

/**
 * The last raise a toast announced, per session — Paseo's `attentionNotifiedRef`
 * (`session-context.tsx:285-291`), and written in Paseo's place: only AFTER the
 * gate let the raise out. A raise the gate held back is therefore NOT recorded,
 * so it stays due and a later publication of the same event can still reach the
 * user once they look away (the review's P1).
 */
const lastFired = new Map<string, Attention>();

/**
 * Raises currently inside their own window read. Paseo needs no such marker: its
 * window answer is `AppState.currentState`, synchronous, so one event's gate and
 * record are a single step. Ours awaits the OS, and a roster is re-published
 * constantly, so two publications of one session can interleave — without this
 * marker a duplicate pair of toasts lands, and a delayed older snapshot can
 * claim before the newer raise it should never have outrun.
 *
 * A publication the gate holds back CLEARS its marker on the way out. That is
 * the whole of the P1 fix stated the other way round: nothing is left claimed
 * for a raise nobody saw, so the next publication of it is judged afresh.
 */
const judging = new Map<string, Attention>();

/**
 * Whether this publication is worth acting on: nothing announced covers it, and
 * nothing in flight outranks it. Identity is the timestamp plus the daemon's
 * escalation order, so the same event re-published is a duplicate, while a
 * same-millisecond higher-priority raise is news.
 */
function raiseIsOfferable(sessionId: string, attention: Attention): boolean {
  if (!attentionRaised(lastFired.get(sessionId), attention)) return false;
  return attentionRaised(judging.get(sessionId), attention);
}

/**
 * Mark raises as already seen WITHOUT announcing them. The roster controller
 * calls this for the first roster of a run: what already stood when this app
 * came up is old news, and the dedupe has to hold it even though no toast ever
 * fired — otherwise the next application would announce a raise the user
 * watched arrive before this app existed.
 */
export function markAttentionSeen(
  entries: ReadonlyArray<{ sessionId: string; attention: Attention }>,
): void {
  for (const { sessionId, attention } of entries) {
    if (attentionRaised(lastFired.get(sessionId), attention)) lastFired.set(sessionId, attention);
  }
}

/** Forget raises of sessions that left the roster, so the maps cannot grow forever. */
export function forgetAttentionFor(sessionIds: ReadonlySet<string>): void {
  for (const map of [lastFired, judging]) {
    for (const id of [...map.keys()]) {
      if (!sessionIds.has(id)) map.delete(id);
    }
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

/**
 * The production toast path, called by the roster controller with every
 * publication of a raise. Paseo's order (`session-context.tsx:279-291`): the
 * gate FIRST, and the dedupe record only after it — a raise the gate held back
 * is not consumed, so a later publication announces it once the user looks
 * away. A send that throws waits once for `TOAST_RETRY_DELAY_MS` and tries
 * again; a second failure is dropped — the raise was announced as far as this
 * app can push it.
 */
export function fireAttentionToast(
  sessionId: string,
  title: string,
  attention: Attention,
  deps?: Partial<ToastDeps>,
): void {
  // Offered, then marked as in flight — the mark is not a claim of having been
  // announced, and a publication the gate holds back gives it back below.
  if (!raiseIsOfferable(sessionId, attention)) return;
  judging.set(sessionId, attention);
  const state = deps?.windowState ?? productionWindowState;
  const send = deps?.send ?? defaultSend;
  void (async () => {
    let snapshot: WindowState;
    try {
      snapshot = await state();
    } catch {
      // A rejected read cannot say the user is looking: the window might
      // be in the tray, so behave as if they are not.
      snapshot = { visible: false, focused: false, minimized: false };
    }
    // Paseo's gate, both halves read at the moment of decision — the user may
    // have reached this session, or left it, while the window was being asked.
    const mayAnnounce = toastGate(snapshot, sessionId, lookedAtSessionId());
    if (judging.get(sessionId) === attention) judging.delete(sessionId);
    if (!mayAnnounce) {
      // Held back, and unrecorded: the daemon still owes this raise to someone,
      // and the next publication of it is judged from scratch.
      return;
    }
    // Paseo's dedupe does the recording here — after the gate, before the send,
    // and with no await between the test and the write, so two publications of
    // one raise cannot both land: the first has covered the second's answer by
    // then, and a raise the session moved past while this one waited fails the
    // same test.
    if (!raiseIsOfferable(sessionId, attention)) return;
    lastFired.set(sessionId, attention);
    const held = heldContentProvider?.(sessionId);
    const content = toastContent(title, attention.reason, held);
    try {
      await send(content);
    } catch {
      // A newer raise may have taken the record while this send was in
      // flight: a stale retry must not land after the newer toast.
      if (lastFired.get(sessionId) !== attention) return;
      setTimeout(() => {
        if (lastFired.get(sessionId) !== attention) return;
        void send(content).catch(() => {});
      }, TOAST_RETRY_DELAY_MS);
    }
  })();
}
