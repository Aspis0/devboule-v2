import { sessionPresence, type CommandArgs } from "../../lib/tauri";

/**
 * Presence reporting for the daemon's per-session attention state.
 *
 * The daemon decides which sessions need attention and suppresses the badge
 * for the session the user is actually looking at. This module only carries
 * the truth to it: which session is selected and whether the app window is
 * visible and focused right now. It coalesces — a presence report is sent
 * only when the (focusedSessionId, appVisible) pair actually changes.
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
 * "Visible" honestly means: `document.visibilityState === "visible"` and
 * `document.hasFocus()`. The visibility flag flips for a minimised or hidden
 * window; focus narrows it to the window the user is actually on. WebView
 * runtimes do not report occlusion (another window in front) or another
 * virtual desktop, so a visible-but-behind window still reads as visible —
 * the report then leans on `hasFocus()` for the truth. When the app is not
 * visibly focused there is no session the user is looking at, so
 * `focusedSessionId` is reported as null rather than the selected one.
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

  const emit = (): void => {
    if (disposed) return;
    const appVisible = doc.visibilityState === "visible" && doc.hasFocus();
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

  const onFocusChange = (): void => emit();
  const onVisibilityChange = (): void => emit();

  win.addEventListener("focus", onFocusChange);
  win.addEventListener("blur", onFocusChange);
  doc.addEventListener("visibilitychange", onVisibilityChange);
  emit();

  return {
    onSelectionChanged(nextFocusedSessionId: string | null): void {
      focusedSessionId = nextFocusedSessionId;
      emit();
    },
    dispose(): void {
      disposed = true;
      win.removeEventListener("focus", onFocusChange);
      win.removeEventListener("blur", onFocusChange);
      doc.removeEventListener("visibilitychange", onVisibilityChange);
    },
  };
}
