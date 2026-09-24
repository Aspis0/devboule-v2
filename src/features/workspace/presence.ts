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
  onWindowFocusChange?: (handler: () => void) => () => void;
}

export interface PresenceReporter {
  /** Call whenever the selected session changes. */
  onSelectionChanged(focusedSessionId: string | null): void;
  dispose(): void;
}

interface Presence {
  focusedSessionId: string | null;
  appVisible: boolean;
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

  let focusedSessionId: string | null = null;
  let lastSent: Presence | null = null;
  let disposed = false;

  const emit = async (): Promise<void> => {
    if (disposed) return;
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
    const appVisible = deps?.windowState
      ? !readFailed && asked !== null && asked.visible && asked.focused && !asked.minimized
      : doc.visibilityState === "visible" && doc.hasFocus();
    const presence: Presence = {
      focusedSessionId: appVisible ? focusedSessionId : null,
      appVisible,
    };
    if (
      lastSent !== null &&
      lastSent.focusedSessionId === presence.focusedSessionId &&
      lastSent.appVisible === presence.appVisible
    ) {
      return;
    }
    lastSent = presence;
    const report = deps?.invoke
      ? deps.invoke("session_presence", {
          focusedSessionId: presence.focusedSessionId,
          appVisible: presence.appVisible,
        })
      : // Production path: the typed bridge wrapper owns the argument keys.
        sessionPresence(presence.focusedSessionId, presence.appVisible);
    void Promise.resolve(report).catch(() => undefined);
  };

  const emitForgotten = (): void => {
    void emit();
  };

  const onFocusChange = (): void => emitForgotten();
  const onVisibilityChange = (): void => emitForgotten();

  win.addEventListener("focus", onFocusChange);
  win.addEventListener("blur", onFocusChange);
  doc.addEventListener("visibilitychange", onVisibilityChange);
  const unsubscribeFocusChange = deps?.onWindowFocusChange?.(emitForgotten);
  const pollTimer =
    deps?.windowState !== undefined
      ? setInterval(emitForgotten, deps?.pollIntervalMs ?? DEFAULT_WINDOW_POLL_INTERVAL_MS)
      : undefined;
  emitForgotten();

  return {
    onSelectionChanged(nextFocusedSessionId: string | null): void {
      focusedSessionId = nextFocusedSessionId;
      emitForgotten();
    },
    dispose(): void {
      disposed = true;
      if (pollTimer !== undefined) clearInterval(pollTimer);
      unsubscribeFocusChange?.();
      win.removeEventListener("focus", onFocusChange);
      win.removeEventListener("blur", onFocusChange);
      doc.removeEventListener("visibilitychange", onVisibilityChange);
    },
  };
}
