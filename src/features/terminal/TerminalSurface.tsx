import { Channel, invoke } from "@tauri-apps/api/core";
import { memo, useEffect, useRef, useState } from "react";
import type { PermissionRequest, SessionEvent } from "../../types/ipc";
import { TerminalSession, type TerminalBanner } from "./terminalSession";
import { terminalSessionRegistry } from "./terminalRegistry";

interface TerminalSurfaceProps {
  workspaceId: string | null;
  sessionId: string;
  id?: string;
  onClosed?: () => void;
  onExited?: () => void;
  onPermissionRequest?: (sessionId: string, request: PermissionRequest) => void;
  onPermissionResolved?: (sessionId: string, toolCallId: string) => void;
}

function invokeCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(command, args);
}

function createTerminalChannel(onEvent: (event: SessionEvent) => void): Channel<SessionEvent> {
  return new Channel<SessionEvent>(onEvent);
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

export const TerminalSurface = memo(function TerminalSurface({
  workspaceId,
  sessionId,
  id,
  onClosed,
  onExited,
  onPermissionRequest,
  onPermissionResolved,
}: TerminalSurfaceProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const sessionRef = useRef<TerminalSession | null>(null);
  const [banner, setBanner] = useState<TerminalBanner>(null);
  const [ctrlCArmed, setCtrlCArmed] = useState(false);

  useEffect(() => {
    const host = hostRef.current;
    if (host === null) return;

    let mounted = true;
    setBanner(null);
    setCtrlCArmed(false);

    const session = new TerminalSession({
      workspaceId,
      sessionId,
      host,
      createView: async (viewHost, options) => {
        const { createTerminalView } = await import("./createTerminalView");
        return createTerminalView(viewHost, options);
      },
      invoke: invokeCommand,
      createChannel: createTerminalChannel,
      registry: terminalSessionRegistry,
      onBanner: (nextBanner) => {
        if (mounted) setBanner(nextBanner);
      },
      onCtrlCArmed: (armed) => {
        if (mounted) setCtrlCArmed(armed);
      },
      onExited: () => {
        if (mounted) onExited?.();
      },
      onPermissionRequest: (request) => {
        if (mounted) onPermissionRequest?.(sessionId, request);
      },
      onPermissionResolved: (toolCallId) => {
        if (mounted) onPermissionResolved?.(sessionId, toolCallId);
      },
    });
    sessionRef.current = session;

    void session.start().catch(() => {
      if (mounted) setBanner({ kind: "error", message: "Could not start the terminal." });
    });

    return () => {
      mounted = false;
      if (sessionRef.current === session) sessionRef.current = null;
      session.dispose();
    };
  }, [workspaceId, sessionId, onExited, onPermissionRequest, onPermissionResolved]);

  useEffect(() => {
    const host = hostRef.current;
    const session = sessionRef.current;
    if (host === null || session === null || typeof ResizeObserver === "undefined") return;

    const observer = new ResizeObserver(() => session.requestResize());
    observer.observe(host);
    return () => observer.disconnect();
  }, [workspaceId, sessionId]);

  const message = bannerText(banner);

  return (
    <div id={id} className="workspace-terminal-shell" role="tabpanel" aria-label="Terminal output">
      <div className="workspace-terminal-toolbar">
        <span className="workspace-status-dot workspace-dot-green" />
        <span className="workspace-terminal-title">Terminal</span>
        <span className="workspace-terminal-status">
          {message ?? "Connected to the local shell"}
        </span>
        <button
          type="button"
          className="workspace-terminal-interrupt"
          onClick={() => sessionRef.current?.requestCtrlC()}
          disabled={banner?.kind === "exited" || banner?.kind === "recovered"}
          aria-pressed={ctrlCArmed}
        >
          {ctrlCArmed ? "Press Ctrl+C again" : "Ctrl+C"}
        </button>
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
      </div>
      <div ref={hostRef} className="workspace-terminal-host" aria-label="Interactive terminal" />
      {message !== null ? (
        <div className="workspace-terminal-banner" role="status">
          {message}
        </div>
      ) : null}
    </div>
  );
});
