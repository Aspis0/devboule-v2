import type { Attention, AttentionReason } from "../../types/ipc";
import { invokeTyped, type AttentionTarget, type AttentionToast } from "../../lib/tauri";
import { getShowMessagePreviews, getShowNotifications } from "../../lib/notificationPrefs";
import { lookedAtSessionId, onPresenceSettled } from "./presence";
import { traceAttention, withinMs } from "../../lib/boundedWait";
import { windowReaderFor } from "./windowReader";
import { shouldToast, type RaiseContext } from "./attentionToastPolicy";

/** How long the window state may take before the gate decides without it. A
 * window that cannot answer in this time is treated as not seen: the toast
 * goes out rather than waiting on a read that may never settle. */
export const WINDOW_READ_TIMEOUT_MS = 500;

/** How long the OS permission answer may take before the send is treated as
 * failed and retried once. */
export const PERMISSION_READ_TIMEOUT_MS = 1_000;

/**
 * How a session's attention presents to the local user: the words for a
 * reason, the decision of when an OS toast may fire, what the toast says,
 * and the one place that sends it.
 *
 * The controller excludes ended rows before offering raises. The gate
 * announces an eligible raise unless the user is looking at THIS
 * session — the window actively seen AND this session the one this window
 * shows. Which rows the tab strip draws is not the question the gate asks,
 * so nothing here is parked or flushed: a raise for a session in another
 * workspace announces while this window stays focused, and its toast quotes
 * only what this window holds for it (see `heldContentForSession`).
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

function toastAttentionLabel(reason: AttentionReason): string {
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

/** The preview's length in plain-text characters; `previewFrom` cuts after
 * the last whole word under it. */
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
  const title = `${sessionTitle} — ${toastAttentionLabel(reason)}`;
  if (reason === "permission") {
    const permissionText = held?.permissionText?.trim();
    return {
      title,
      body: permissionText ? previewFrom(permissionText) : toastAttentionLabel(reason),
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
 * reason only. (This app holds words for the rows on screen alone, so a
 * raise elsewhere must not be dressed up in words it never had.)
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
 * The last raise a toast announced, per session — recorded only AFTER the
 * gate let the raise out. A raise the gate held back is NOT recorded, so
 * it stays due and a later publication of the same event can still reach
 * the user once they look away from the session.
 */
const lastFired = new Map<string, Attention>();

/**
 * Raises currently inside their own window read. The window answer here
 * awaits the OS, and a roster is
 * re-published constantly, so two publications of one session can interleave —
 * without this marker a duplicate pair of toasts lands, and a delayed older
 * snapshot can claim before the newer raise it should never have outrun.
 *
 * A publication the gate holds back CLEARS its marker on the way out: nothing is left
 * claimed for a raise nobody saw, so the next publication of it is judged afresh.
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
  // Every card the agent raises while one permission state stands is the same
  // state: one toast covers them until the attention settles.
  if (attention.reason === "permission" && lastFired.get(sessionId)?.reason === "permission") {
    return false;
  }
  return attentionRaised(judging.get(sessionId), attention);
}

/**
 * The roster shows these sessions with no attention: their state has settled, so
 * the next raise of any of them is news and may toast again.
 */
export function settleAttentionFor(sessionIds: ReadonlySet<string>): void {
  for (const id of sessionIds) lastFired.delete(id);
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
  for (const map of [lastFired, judging, heldRaises]) {
    for (const id of [...map.keys()]) {
      if (!sessionIds.has(id)) map.delete(id);
    }
  }
}

/** What the toast needs to judge a raise again: the call `fireAttentionToast` got. */
interface HeldRaise {
  title: string;
  target: AttentionTarget;
  attention: Attention;
  deps?: Partial<ToastDeps>;
  context: RaiseContext;
}

/**
 * Raises the gate held back because the user was looking at their session.
 * They stay due, and they are judged again the moment the presence reporter
 * settles — the user looked away, or the window lost focus — rather than at the
 * next roster publication: a quiet session publishes nothing for minutes, and
 * the owner waited ten minutes for a plan-ready toast that way.
 */
const heldRaises = new Map<string, HeldRaise>();

/**
 * Judges every held raise again. Each one is removed first: `fireAttentionToast`
 * puts it back only if the gate still holds it, so a raise the user has looked
 * away from leaves the map and is announced, and one still in view stays due.
 */
export function reconsiderHeldAttention(): void {
  for (const [sessionId, raise] of [...heldRaises]) {
    heldRaises.delete(sessionId);
    fireAttentionToast(raise.target, raise.title, raise.attention, raise.deps, raise.context);
  }
}

onPresenceSettled(reconsiderHeldAttention);

/** The permission surface `sendWithPermission` needs, narrowed to what is
 *  used. `showToast` is `void` where the platform is synchronous and async in
 *  tests, so the caller awaits either. The permission answer is the plugin's
 *  RUST side: the plugin's own JS check reads `window.Notification.permission`,
 *  which WebView2 reports as "denied" even while the Rust side says granted
 *  — the web value never stands for the OS permission here. */
export interface NotificationPlugin {
  rustPermissionGranted(): Promise<boolean>;
  showToast(toast: AttentionToast): void | Promise<void>;
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
  toast: AttentionToast,
  plugin: NotificationPlugin,
): Promise<void> {
  const granted = await withinMs(
    "the OS permission answer",
    PERMISSION_READ_TIMEOUT_MS,
    plugin.rustPermissionGranted(),
  );
  if (!granted) {
    throw new Error("the OS permission answer was not granted");
  }
  await plugin.showToast(toast);
}

/** The production sender: the Rust side's permission answer, then the toast.
 *  The permission invoke goes straight to the plugin command — the JS wrapper
 *  would prefer the web permission, which is the one value that lies — and the
 *  toast goes to this app's own command, because the WinRT toast that can route
 *  a click is the Rust side's to show. */
async function defaultSend(toast: AttentionToast): Promise<void> {
  const { invoke } = await import("@tauri-apps/api/core");
  await sendWithPermission(toast, {
    rustPermissionGranted: () => invoke("plugin:notification|is_permission_granted"),
    showToast: async (request) => {
      await invokeTyped("attention_toast_show", { toast: request });
    },
  });
}

/** Everything the OS side of a toast needs, injected for tests. */
export interface ToastDeps {
  send: (toast: AttentionToast) => Promise<void>;
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
 * publication of a raise. The order: the gate runs
 * FIRST, then the policy (`shouldToast`) decides whether the raise is the
 * person's to answer, and the dedupe record only after both — a raise the gate held back is not
 * consumed, so a later publication announces it once the user looks away. A send
 * that throws waits once for `TOAST_RETRY_DELAY_MS` and tries again; a second
 * failure is dropped — the raise was announced as far as this app can push it.
 */
export function fireAttentionToast(
  target: AttentionTarget,
  title: string,
  attention: Attention,
  deps?: Partial<ToastDeps>,
  context: RaiseContext = { topLevel: true, childBusy: false },
): void {
  const sessionId = target.sessionId;
  // The Notifications page's master switch, read at fire time. A raise
  // that arrives while the switch is off is consumed, not kept due:
  // turning the switch back on announces new raises only, never a burst
  // of old toasts. The record moves only forward, like markAttentionSeen.
  if (!getShowNotifications()) {
    if (attentionRaised(lastFired.get(sessionId), attention)) {
      lastFired.set(sessionId, attention);
    }
    return;
  }
  // Offered, then marked as in flight — the mark is not a claim of having been
  // announced, and a publication the gate holds back gives it back below.
  if (!raiseIsOfferable(sessionId, attention)) return;
  judging.set(sessionId, attention);
  const state = deps?.windowState ?? productionWindowState;
  const send = deps?.send ?? defaultSend;
  traceAttention("offered", {
    session: sessionId,
    raisedAt: attention.atMs,
    daemonToAppMs: Date.now() - attention.atMs,
  });
  void (async () => {
    // The read is bounded and shared (see windowReader): a stale answer is the
    // last one the window gave, and a refused or never-answered read counts as
    // not seen, so the toast is never held on a read that cannot come back.
    const answer = await windowReaderFor(state).read(WINDOW_READ_TIMEOUT_MS, "the window state");
    const snapshot: WindowState = answer.state ?? {
      visible: false,
      focused: false,
      minimized: false,
    };
    // The gate: both halves read at the moment of decision —
    // the user may have reached this session, or left it, while the window was being asked.
    const mayAnnounce = toastGate(snapshot, sessionId, lookedAtSessionId());
    traceAttention("decided", {
      session: sessionId,
      announce: mayAnnounce ? 1 : 0,
      windowSeen: snapshot.visible && snapshot.focused && !snapshot.minimized ? 1 : 0,
      decidedAfterMs: Date.now() - attention.atMs,
    });
    if (judging.get(sessionId) === attention) judging.delete(sessionId);
    if (!mayAnnounce) {
      // Held back, and unrecorded: the daemon still owes this raise to someone,
      // and it is judged again when the presence reporter settles.
      heldRaises.set(sessionId, { title, target, attention, deps, context });
      return;
    }
    // Only a raise the person must answer reaches the OS; the rest stays in the app.
    if (
      !shouldToast({
        ...context,
        reason: attention.reason,
        away: mayAnnounce,
        always: attention.always === true,
      })
    )
      return;
    // The dedupe records here — after
    // the gate, before the send, and with no await between the test and the
    // write, so two publications of one raise cannot both land: the first has
    // covered the second's answer by then, and a raise the session moved past
    // while this one waited fails the same test.
    if (!raiseIsOfferable(sessionId, attention)) return;
    // The master switch again, after the async hops: the user may have
    // switched it off while the window was being asked. Suppressed here
    // means unrecorded — unlike a raise that arrived while off — so the
    // next publication of the same raise still announces.
    if (!getShowNotifications()) return;
    lastFired.set(sessionId, attention);
    // The previews switch, read at the same moment as the wording: off hands
    // the toast no held content, so it names the session and the reason only.
    const held = getShowMessagePreviews() ? heldContentProvider?.(sessionId) : undefined;
    const content = toastContent(title, attention.reason, held);
    try {
      traceAttention("invoking", { session: sessionId, reason: attention.reason });
      await send({ ...content, target });
      traceAttention("accepted", { session: sessionId, reason: attention.reason });
    } catch {
      // A newer raise may have taken the record while this send was in
      // flight: a stale retry must not land after the newer toast.
      if (lastFired.get(sessionId) !== attention) return;
      setTimeout(() => {
        if (lastFired.get(sessionId) !== attention) return;
        // Rebuilt at retry time, never resent frozen: either switch may
        // have moved inside the 1.5 s window, and a retry that lands
        // preview text the user just switched off breaks the privacy the
        // switch promises. A master that is off now drops the retry.
        if (!getShowNotifications()) return;
        const retryHeld = getShowMessagePreviews() ? heldContentProvider?.(sessionId) : undefined;
        void send({ ...toastContent(title, attention.reason, retryHeld), target }).catch(() => {});
      }, TOAST_RETRY_DELAY_MS);
    }
  })();
}
