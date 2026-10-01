import { invoke } from "@tauri-apps/api/core";
import { memo, useCallback, useEffect, useRef, useState } from "react";
import type {
  AgentActivityState,
  Attention,
  PermissionRequest,
  PermissionResolved,
  SessionState,
} from "../../types/ipc";
import { takeTerminalInput } from "./pendingTerminalInput";
import { TerminalSession, type TerminalBanner } from "./terminalSession";
import { createSessionChannel, type SubscriptionId } from "../../lib/tauri";
import { terminalSessionRegistry } from "./terminalRegistry";
import { PaneHeader } from "../workspace/paneHeader/PaneHeader";
import { headerDisplay } from "../workspace/paneHeader/paneHeaderStatus";
import { headerMenu, type HeaderMenuSeam } from "../workspace/paneHeader/paneHeaderMenu";

// How long the Ctrl+C chip wears the failure label before it goes back to
// itself — the same beat MessageCopyButton's copy chip uses.
const COPY_FAILED_LABEL_MS = 1500;

/** The node the banner's Details control owns; constant so `aria-controls`
 * resolves before the disclosure is opened. */
const BANNER_DETAIL_ID = "terminal-banner-detail";

interface TerminalSurfaceProps {
  workspaceId: string | null;
  sessionId: string;
  observedState?: SessionState | null;
  cwd?: string;
  /** The session's display name. Absent until the workspace passes it. */
  title?: string;
  id?: string;
  onClosed?: () => void;
  onExited?: () => void;
  onPermissionRequest?: (
    sessionId: string,
    subscriptionId: SubscriptionId,
    request: PermissionRequest,
  ) => void;
  onPermissionResolved?: (sessionId: string, resolution: PermissionResolved) => void;
  /**
   * Set only for the tab the "+" menu's Terminal entry just created: take
   * focus once, when the xterm is open. Selecting a tab never sets it.
   */
  autoFocus?: boolean;
  /**
   * Asked at the moment of focus: false means focus has since moved (the
   * user clicked elsewhere) — the request is spent and nothing is focused.
   */
  autoFocusGuard?: () => boolean;
  /** Reports the request spent (focus taken, or declined), so the strip can forget it. */
  onAutoFocusTaken?: () => void;
  /** The kebab's close-group wiring, from the tab-close flow. Absent until the workspace passes it. */
  headerMenuSeam?: HeaderMenuSeam;
  /** The roster's turn status and pending ask, painted by the header. Absent until the workspace passes them. */
  activity?: AgentActivityState;
  attention?: Attention;
  /**
   * Closes this pane's tab — the action the ended banner's sentence names.
   * Present in Workspace; absent only in tests that never show that banner.
   */
  onCloseTab?: () => void;
}

function invokeCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(command, args);
}

function humanSize(bytes: number): string {
  const units = ["B", "KB", "MB", "GB"];
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  const rounded =
    unit === 0
      ? Math.round(value).toString()
      : value >= 10
        ? Math.round(value).toString()
        : value.toFixed(1);
  return `${rounded} ${units[unit]}`;
}

export function bannerText(banner: TerminalBanner): string | null {
  if (banner === null) return null;
  if (banner.kind === "error") return banner.message;
  if (banner.kind === "silent") {
    const minutes = Math.floor(banner.elapsedMs / 60_000);
    return minutes > 0
      ? `The terminal process is still running but has been silent for ${minutes} minute${minutes === 1 ? "" : "s"}.`
      : `The terminal process is still running but has been silent for ${Math.floor(banner.elapsedMs / 1_000)} seconds.`;
  }
  if (banner.kind === "closed") return "The terminal session was closed.";
  if (banner.kind === "ended") return banner.message;
  if (banner.kind === "recovered") {
    if (banner.integrity.trimmedBytes > 0) {
      return banner.integrity.droppedBytes > 0
        ? `The oldest ${humanSize(banner.integrity.trimmedBytes)} was removed by the history limit, and at least ${humanSize(banner.integrity.droppedBytes)} of output was not saved.`
        : `The oldest ${humanSize(banner.integrity.trimmedBytes)} of this transcript was removed by the history limit.`;
    }
    // A zero counter means the amount is unknown, never that nothing was lost;
    // every copy branch therefore tests bytes, not the integrity variant.
    return banner.integrity.droppedBytes > 0
      ? `The previous terminal process is gone. At least ${humanSize(banner.integrity.droppedBytes)} of output was not saved, and the end of the transcript could not be verified either.`
      : "The previous terminal process is gone. The end of the saved transcript could not be verified.";
  }
  if (banner.kind === "journal_degraded") {
    return banner.lost.bytes > 0
      ? `Scrollback history is incomplete: at least ${humanSize(banner.lost.bytes)} of output could not be saved.`
      : "Scrollback history is incomplete because some output could not be saved.";
  }
  const prefix =
    banner.code === null
      ? "The terminal process exited."
      : `The terminal process exited with code ${banner.code}.`;
  if (banner.trimmedBytes > 0) {
    return banner.lost !== null && banner.lost.bytes > 0
      ? `The oldest ${humanSize(banner.trimmedBytes)} was removed by the history limit, and at least ${humanSize(banner.lost.bytes)} of output was not saved.`
      : `The oldest ${humanSize(banner.trimmedBytes)} of this transcript was removed by the history limit.`;
  }
  if (banner.lost === null) return prefix;
  return banner.lost.bytes > 0
    ? `${prefix} At least ${humanSize(banner.lost.bytes)} of output was not saved.`
    : `${prefix} Some output was not saved.`;
}

function focusIfIdle(host: HTMLElement): void {
  const active = document.activeElement;
  if (active !== null && active !== document.body) return;
  host.querySelector<HTMLTextAreaElement>(".xterm-helper-textarea")?.focus();
}

export const TerminalSurface = memo(function TerminalSurface({
  workspaceId,
  sessionId,
  observedState,
  cwd,
  title,
  id,
  onClosed,
  onExited,
  onPermissionRequest,
  onPermissionResolved,
  autoFocus,
  autoFocusGuard,
  onAutoFocusTaken,
  onCloseTab,
  headerMenuSeam,
  activity,
  attention,
}: TerminalSurfaceProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const sessionRef = useRef<TerminalSession | null>(null);
  const [banner, setBanner] = useState<TerminalBanner>(null);
  // The disclosure is open for exactly the banner object it was opened on:
  // a new error is a new banner, so it arrives collapsed with no effect to
  // reset it.
  const [openDetailFor, setOpenDetailFor] = useState<TerminalBanner>(null);
  const [ctrlCArmed, setCtrlCArmed] = useState(false);
  const [copyFailed, setCopyFailed] = useState(false);
  const copyFailedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (copyFailedTimer.current !== null) clearTimeout(copyFailedTimer.current);
    },
    [],
  );

  // The recovered flag is read at (re)start only, through this ref, so it
  // is not a dep of the session effect: the row's recovered flip arrives
  // with a new generation, which rebuilds that effect anyway, while a
  // live/silent flip must rebuild nothing. This sync effect is declared
  // before the session effect, so a rebuild always reads the fresh value.
  const sessionRecoveredRef = useRef(observedState?.type === "recovered");
  useEffect(() => {
    sessionRecoveredRef.current = observedState?.type === "recovered";
  }, [observedState]);
  const autoFocusRef = useRef(autoFocus);
  const autoFocusGuardRef = useRef(autoFocusGuard);
  const onAutoFocusTakenRef = useRef(onAutoFocusTaken);
  const focusTakenRef = useRef(false);
  // The request and the open view arrive in either order (the prop can flip
  // true after start() already resolved, or before it), so both the prop
  // change and the start() continuation call this; the refs keep it callable
  // from the session effect without widening that effect's rebuild triggers.
  const takeAutoFocus = useCallback(() => {
    if (focusTakenRef.current || !autoFocusRef.current) return;
    const helper = hostRef.current?.querySelector<HTMLTextAreaElement>(".xterm-helper-textarea");
    if (helper === null || helper === undefined) return;
    // Spent at the first open attempt either way: if the guard declines —
    // the user clicked elsewhere during startup — their focus stays, and a
    // later re-render must not reopen the question.
    focusTakenRef.current = true;
    if (autoFocusGuardRef.current && !autoFocusGuardRef.current()) {
      onAutoFocusTakenRef.current?.();
      return;
    }
    helper.focus();
    onAutoFocusTakenRef.current?.();
  }, []);
  useEffect(() => {
    autoFocusRef.current = autoFocus;
    autoFocusGuardRef.current = autoFocusGuard;
    onAutoFocusTakenRef.current = onAutoFocusTaken;
    if (autoFocus) takeAutoFocus();
  }, [autoFocus, autoFocusGuard, onAutoFocusTaken, takeAutoFocus]);

  // A terminal attachment is valid for exactly one `(sessionId, generation)`
  // pair. Resume keeps the id but increments the generation, so this is the
  // signal that the surface's attachment is dead and must be rebuilt. Generation
  // moves only on resume, so this cannot remount under someone mid-turn.
  useEffect(() => {
    const host = hostRef.current;
    if (host === null) return;

    let mounted = true;
    setBanner(null);
    setCtrlCArmed(false);
    setCopyFailed(false);

    const session = new TerminalSession({
      workspaceId,
      sessionId,
      sessionRecovered: sessionRecoveredRef.current,
      host,
      consumeInitialInput: () => takeTerminalInput(sessionId),
      onInitialInputTaken: () => {
        // The provider-install tab just started typing: take focus only if
        // it is still where navigation left it (body), never steal it.
        if (mounted) focusIfIdle(host);
      },
      createView: async (viewHost, options) => {
        const { createTerminalView } = await import("./createTerminalView");
        return createTerminalView(viewHost, options);
      },
      invoke: invokeCommand,
      createChannel: createSessionChannel,
      registry: terminalSessionRegistry,
      onBanner: (nextBanner) => {
        if (mounted) setBanner(nextBanner);
      },
      onCtrlCArmed: (armed) => {
        if (mounted) setCtrlCArmed(armed);
      },
      onCopyFailed: () => {
        if (!mounted) return;
        // A refused copy must not be silent: the chip says so for a beat,
        // then goes back to naming the interrupt.
        setCopyFailed(true);
        if (copyFailedTimer.current !== null) clearTimeout(copyFailedTimer.current);
        copyFailedTimer.current = setTimeout(() => setCopyFailed(false), COPY_FAILED_LABEL_MS);
      },
      onExited: () => {
        if (mounted) onExited?.();
      },
      onPermissionRequest: (request, subscriptionId) => {
        if (mounted) onPermissionRequest?.(sessionId, subscriptionId, request);
      },
      onPermissionResolved: (resolution) => {
        if (mounted) onPermissionResolved?.(sessionId, resolution);
      },
    });
    sessionRef.current = session;

    void session
      .start()
      .then(() => {
        // start() resolves only after createView opened the xterm, so the
        // helper textarea exists here — take the armed request if any.
        if (mounted) takeAutoFocus();
      })
      .catch(() => {
        if (mounted) setBanner({ kind: "error", message: "Could not start the terminal." });
      });

    return () => {
      mounted = false;
      if (sessionRef.current === session) sessionRef.current = null;
      session.dispose();
    };
  }, [
    workspaceId,
    sessionId,
    onExited,
    onPermissionRequest,
    onPermissionResolved,
    observedState?.generation,
    takeAutoFocus,
  ]);

  useEffect(() => {
    const host = hostRef.current;
    if (host === null || typeof ResizeObserver === "undefined") return;

    // Read the current session inside the callback: the session effect can
    // replace the controller while this observer stays mounted.
    const observer = new ResizeObserver(() => sessionRef.current?.requestResize());
    observer.observe(host);
    return () => observer.disconnect();
  }, [workspaceId, sessionId]);

  const message = bannerText(banner);
  // The ended banner is this failure's ONE surface: its sentence (with the
  // close-tab action below) lives in the pane's bottom banner only, never
  // repeated in the header — which now carries the shared word beside the dot.
  const ended = banner?.kind === "ended";
  // The daemon's own words, for the disclosure beside the sentence — never
  // in the status region, a title or a described-by.
  const bannerDetail =
    banner !== null && (banner.kind === "error" || banner.kind === "ended")
      ? (banner.detail ?? null)
      : null;
  const detailsOpen = banner !== null && openDetailFor === banner;

  return (
    <div id={id} className="workspace-terminal-shell" role="tabpanel" aria-label="Terminal output">
      <PaneHeader
        kind="terminal"
        title={title ?? "Terminal"}
        display={headerDisplay(
          observedState,
          null,
          // The terminal's own failure reads through the shared word: an
          // error banner over a live row must not say Running.
          banner?.kind === "error" ? "error" : null,
          activity,
          attention,
        )}
        menu={headerMenu(cwd, headerMenuSeam, sessionId)}
        trailingSlot={
          <>
            {ended ? null : (
              <button
                type="button"
                className="workspace-terminal-interrupt"
                onClick={() => sessionRef.current?.requestCtrlC()}
                disabled={banner?.kind === "exited" || banner?.kind === "recovered"}
                aria-pressed={ctrlCArmed}
              >
                {copyFailed ? "Copy failed" : ctrlCArmed ? "Press Ctrl+C again" : "Ctrl+C"}
              </button>
            )}
            <button
              type="button"
              className="workspace-terminal-close"
              onClick={() => {
                sessionRef.current?.close();
                setBanner({ kind: "closed" });
                onClosed?.();
              }}
              disabled={banner?.kind === "exited" || banner?.kind === "closed"}
            >
              Close
            </button>
          </>
        }
      />
      {/* The ground wrapper owns the terminal's surface colour: the host's
          margins sit on it, so the terminal reads as one surface in both
          themes and the fit measures a padding-free box. */}
      <div className="workspace-terminal-ground">
        <div ref={hostRef} className="workspace-terminal-host" aria-label="Interactive terminal" />
      </div>
      {message !== null ? (
        <div className="workspace-terminal-banner">
          {/* The live region says the sentence and nothing else; the raw
              words sit outside it, behind the control below. */}
          <span className="workspace-terminal-banner-status" role="status">
            {message}
          </span>
          {ended ? (
            <button
              type="button"
              className="workspace-secondary-action workspace-terminal-banner-action"
              onClick={onCloseTab}
            >
              Close tab
            </button>
          ) : null}
          {bannerDetail !== null ? (
            <>
              <button
                type="button"
                className="workspace-secondary-action workspace-terminal-banner-details"
                aria-expanded={detailsOpen}
                aria-controls={BANNER_DETAIL_ID}
                onClick={() => setOpenDetailFor((current) => (current === banner ? null : banner))}
              >
                Details
              </button>
              <span id={BANNER_DETAIL_ID} className="workspace-terminal-banner-detail">
                {detailsOpen ? bannerDetail : null}
              </span>
            </>
          ) : null}
        </div>
      ) : null}
    </div>
  );
});
