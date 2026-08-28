import { Channel, invoke } from "@tauri-apps/api/core";
import { memo, useEffect, useRef, useState } from "react";
import type { SessionEvent } from "../../types/ipc";
import { TerminalSession, type TerminalBanner } from "./terminalSession";
import { terminalSessionRegistry } from "./terminalRegistry";

interface TerminalSurfaceProps {
  workspaceId: string | null;
  id?: string;
}

function invokeCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(command, args);
}

function createTerminalChannel(onEvent: (event: SessionEvent) => void): Channel<SessionEvent> {
  return new Channel<SessionEvent>(onEvent);
}

function bannerText(banner: TerminalBanner): string | null {
  if (banner === null) return null;
  if (banner.kind === "error") return banner.message;
  if (banner.kind === "closed") return "The terminal session was closed.";
  if (banner.kind === "recovered") {
    // Two different statements, never merged. `truncated` is an OBSERVED
    // loss the previous daemon recorded. Without it the transcript is not
    // certified complete either: the dying process's uncommitted output
    // left no trace, so the end of the transcript simply cannot be
    // verified — say that instead of implying nothing is missing.
    return banner.truncated
      ? "The previous terminal process is gone. Some output was not saved."
      : "The previous terminal process is gone. The end of the saved transcript could not be verified.";
  }
  if (banner.kind === "journal_degraded") {
    return "Scrollback history is incomplete because some output could not be saved.";
  }
  return banner.code === null
    ? "The terminal process exited."
    : `The terminal process exited with code ${banner.code}.`;
}

export const TerminalSurface = memo(function TerminalSurface({
  workspaceId,
  id,
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
  }, [workspaceId]);

  useEffect(() => {
    const host = hostRef.current;
    const session = sessionRef.current;
    if (host === null || session === null || typeof ResizeObserver === "undefined") return;

    const observer = new ResizeObserver(() => session.requestResize());
    observer.observe(host);
    return () => observer.disconnect();
  }, [workspaceId]);

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
