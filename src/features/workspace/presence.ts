import { sessionPresence, type CommandArgs } from "../../lib/tauri";
import type { WindowState } from "./attentionNotice";

/** How often production re-asks the window state: hiding the window fires
 *  no DOM event inside WebView2, so a poll is the safety net behind the
 *  focus-change subscription. */
const DEFAULT_WINDOW_POLL_INTERVAL_MS = 5_000;

/**
 * Presence reporting for the daemon's per-session attention state.
 *
 * The daemon decides which sessions need attention and suppresses the badge
 * for the session the user is actually looking at. This module only carries
 * the truth to it: which session is selected and whether the app window is
 * visible and focused right now. It coalesces — a presence report is sent
 * only when the (focusedSessionId, appVisible) pair actually changes — and
 * it reports at once on a focus change, because the daemon DROPS a raise
 * for an attended session: a presence lag on hide would be a lost raise,
 * not a late one.
 *
 * The stored selection is also the app's one answer to "which session is
 * this window looking at": the toast gate reads it through
 * `lookedAtSessionId()`, so what the window holds a toast back for and what
 * the daemon is told can never drift apart.
 */
export interface PresenceEventTargetLike {
  addEventListener(type: string, listener: () => void): void;
  removeEventListener(type: string, listener: () => void): void;
}

export interface PresenceDocumentLike extends PresenceEventTargetLike {
  visibilityState: string;
  hasFocus(): boolean;
}

export interface PresenceDeps {
  /**
   * Test seam. Typed against the bridge's own `CommandArgs` map rather than a
   * loose string/record pair, so a snake_case key here fails to compile — the
   * same class of bug the structural guard exists to prevent. Production (no
   * seam injected) never writes argument keys at all: it calls the typed
   * `sessionPresence` wrapper.
   */
  invoke: (command: "session_presence", args: CommandArgs["session_presence"]) => Promise<unknown>;
  window: PresenceEventTargetLike;
  document: PresenceDocumentLike;
  /**
   * The OS truth about the window. Inside a hidden WebView2 the document
   * keeps claiming `visibilityState: "visible"` and `hasFocus: true`, so
   * production asks the window itself; when absent (tests, plain web) the
   * document answers. A rejected read can never say the user is looking:
   * the window might be in the tray, so a rejection reports not visible.
   */
  windowState?: () => Promise<WindowState>;
  /** How often the window state is re-asked. Only used with `windowState`. */
  pollIntervalMs?: number;
  /**
   * Subscribes to the window's focus changes, returning the unsubscribe. A
   * blur or focus re-asks the state at once — the poll is only the safety
   * net for the gaps, because a raise in those gaps is dropped by the
   * daemon, not delayed.
   */
  onWindowFocusChange?: (handler: (event: { payload: boolean }) => void) => Promise<() => void>;
}

export interface PresenceReporter {
  /** Re-report now: the session this window shows changed. */
  onSelectionChanged(): void;
  dispose(): void;
}

interface Presence {
  focusedSessionId: string | null;
  appVisible: boolean;
}

/**
 * The reporter the app started (App, once per app run), and the ONE record of
 * the session this window shows. The surface that owns the selection writes it
 * through `reportSelection` — it never sees this module's wiring — and both
 * consumers read that one record: the reporter names it in every presence
 * report, and the toast gate asks it what the user is looking at. A reporter
 * starting after its owner mounted finds the value already stored, because
 * React flushes a commit's effects child-first: a Workspace mounting in the
 * same commit as App reports before any reporter exists.
 */
let activeReporter: PresenceReporter | null = null;
let reportedSelection: string | null = null;

export function reportSelection(focusedSessionId: string | null): void {
  reportedSelection = focusedSessionId;
  activeReporter?.onSelectionChanged();
}

/**
 * The session this window shows, or `null` when no surface shows one. The
 * toast gate asks this — not the tab strip — what the user is looking at,
 * the same question the daemon's presence report answers.
 */
export function lookedAtSessionId(): string | null {
  return reportedSelection;
}

/**
 * Starts presence reporting and sends one initial report so the daemon is
 * not guessing before the first selection or event.
 *
 * "Visible" honestly means the window's own state: hidden via the tray or
 * minimized flips it, and neither fires a DOM event inside WebView2 — the
 * document goes on claiming visible and focused forever. So production asks
 * the window itself, reports at once on a focus change, and re-asks on a
 * poll as the safety net; a caller without a window source (plain web)
 * falls back to the document, where the old assumption holds. A rejected
 * window-state read is reported as not visible — never as the document's
 * lie.
 */
export function startPresenceReporting(deps?: Partial<PresenceDeps>): PresenceReporter {
  const win = deps?.window ?? (typeof window === "undefined" ? null : window);
  const doc = deps?.document ?? (typeof document === "undefined" ? null : document);
  if (win === null || doc === null) {
    return { onSelectionChanged: () => undefined, dispose: () => undefined };
  }

  // The selection is not held here: `reportedSelection` is the one record, and
  // every report reads it as it stands when the report is built.
  let lastSent: Presence | null = null;
  let disposed = false;
  // Every window-state read takes its sequence number before its await; a
  // read applies only if its number is newer than the last one applied. The
  // poll and the focus subscription overlap, so a read that started earlier
  // and resolves later is dropped instead of overwriting a newer answer.
  let lastRequestedRead = 0;
  let lastAppliedRead = 0;

  const emit = async (): Promise<void> => {
    if (disposed) return;
    const readNumber = ++lastRequestedRead;
    // A rejected read can never be replaced by the document's answer: the
    // live check measured that answer lying for a hidden window. Not seen
    // is the only honest reading when the window cannot be asked.
    let asked: WindowState | null = null;
    let readFailed = false;
    if (deps?.windowState) {
      asked = await deps.windowState().catch(() => {
        readFailed = true;
        return null;
      });
    }
    // `disposed` again: dispose may have landed while the state read was in
    // flight, and a late report must never re-assert an attended session.
    if (disposed) return;
    // A read that started earlier and resolves later is dropped: the newer
    // answer has already been applied.
    if (readNumber <= lastAppliedRead) return;
    lastAppliedRead = readNumber;
    const appVisible = deps?.windowState
      ? !readFailed && asked !== null && asked.visible && asked.focused && !asked.minimized
      : doc.visibilityState === "visible" && doc.hasFocus();
    const presence: Presence = {
      focusedSessionId: appVisible ? reportedSelection : null,
      appVisible,
    };
    if (
      lastSent !== null &&
      lastSent.focusedSessionId === presence.focusedSessionId &&
      lastSent.appVisible === presence.appVisible
    ) {
      return;
    }
    sendPresence(presence);
  };

  let sendInFlight = false;
  let queuedPresence: Presence | null = null;

  /**
   * Sends one applied presence answer. Writes are chained — the next one
   * starts only after the previous settles — so two answers can never
   * reach the daemon reversed, and while a send is in flight only the
   * newest answer is queued behind it. `lastSent` is recorded after the
   * send settles: a rejected send clears it, so the next poll or event
   * resends the same pair instead of deduping it away forever.
   */
  const sendPresence = (presence: Presence): void => {
    if (sendInFlight) {
      queuedPresence = presence;
      return;
    }
    sendInFlight = true;
    void (async () => {
      let current = presence;
      for (;;) {
        try {
          const report = deps?.invoke
            ? deps.invoke("session_presence", {
                focusedSessionId: current.focusedSessionId,
                appVisible: current.appVisible,
              })
            : // Production path: the typed bridge wrapper owns the argument keys.
              sessionPresence(current.focusedSessionId, current.appVisible);
          await Promise.resolve(report);
          lastSent = current;
        } catch {
          lastSent = null;
        }
        if (queuedPresence === null) break;
        current = queuedPresence;
        queuedPresence = null;
        // A dispose that landed while this send was in flight owns the
        // answer's fate: the queued report must never go out after it.
        if (disposed) {
          queuedPresence = null;
          break;
        }
      }
      sendInFlight = false;
    })();
  };

  const emitForgotten = (): void => {
    void emit();
  };

  const onFocusChange = (): void => emitForgotten();
  const onVisibilityChange = (): void => emitForgotten();

  win.addEventListener("focus", onFocusChange);
  win.addEventListener("blur", onFocusChange);
  doc.addEventListener("visibilitychange", onVisibilityChange);
  let unsubscribeFocusChange: (() => void) | undefined;
  if (deps?.onWindowFocusChange) {
    // Setup resolving after dispose (or after a remount replaced this
    // reporter) removes the subscription at once, so two live listeners
    // can never pile up.
    void deps
      .onWindowFocusChange(emitForgotten)
      .then((unlisten) => {
        if (disposed) unlisten();
        else unsubscribeFocusChange = unlisten;
      })
      .catch(() => undefined);
  }
  const pollTimer =
    deps?.windowState !== undefined
      ? setInterval(emitForgotten, deps?.pollIntervalMs ?? DEFAULT_WINDOW_POLL_INTERVAL_MS)
      : undefined;
  emitForgotten();

  const reporter: PresenceReporter = {
    onSelectionChanged(): void {
      emitForgotten();
    },
    dispose(): void {
      disposed = true;
      if (activeReporter === reporter) activeReporter = null;
      if (pollTimer !== undefined) clearInterval(pollTimer);
      unsubscribeFocusChange?.();
      win.removeEventListener("focus", onFocusChange);
      win.removeEventListener("blur", onFocusChange);
      doc.removeEventListener("visibilitychange", onVisibilityChange);
    },
  };
  activeReporter = reporter;
  return reporter;
}
