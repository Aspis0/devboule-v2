import type { DaemonStatus } from "../../../types/ipc";

/** Green while the daemon answers, hollow while connecting, red otherwise. */
export function daemonDotTone(state: DaemonStatus["state"]): string {
  if (state === "connected") return "green";
  if (state === "connecting") return "border";
  return "terracotta";
}

/** The foot's tooltip: the whole daemon sentence, not the quiet label. */
export function daemonLabel(status: DaemonStatus): string {
  if (status.state === "connected") {
    const pid = status.pid !== null ? `pid ${status.pid}` : "connected";
    return status.message ? `daemon · ${pid} · ${status.message}` : `daemon · ${pid}`;
  }
  if (status.state === "connecting") return "daemon · connecting";
  if (status.state === "unresponsive") {
    // The supervisor's sentence, verbatim — the tooltip is what keeps the
    // state readable after the user declines the restart dialog.
    return status.message ? `daemon · ${status.message}` : "daemon · not answering";
  }
  if (status.message) return `daemon · ${status.message}`;
  return "daemon · disconnected";
}

export interface SidebarFooterProps {
  historyOpen: boolean;
  onToggleHistory: () => void;
  daemon: DaemonStatus;
  /** The restart-recovery sentence, when a restart attempt failed. */
  note: string | null;
}

/**
 * The sidebar's foot: the quiet History row above the daemon strip — a live
 * dot and the word "Daemon", the pid/message detail in the tooltip.
 */
export function SidebarFooter({ historyOpen, onToggleHistory, daemon, note }: SidebarFooterProps) {
  const tooltip = [daemonLabel(daemon), note].filter((part) => part !== null).join(" · ");
  return (
    <div className="workspace-sidebar-footer">
      <button
        type="button"
        className="workspace-history-button sidebar-quiet-row"
        aria-pressed={historyOpen}
        aria-controls="workspace-history-panel"
        onClick={onToggleHistory}
        title={historyOpen ? "Show workspaces" : "Show history"}
      >
        History
      </button>
      <div className="workspace-daemon-status sidebar-foot" title={tooltip} tabIndex={0}>
        <span className={`workspace-status-dot workspace-dot-${daemonDotTone(daemon.state)}`} />
        <span className="workspace-daemon-status-label">Daemon</span>
        {/* The detail is keyboard- and screen-reader-reachable, not only a
            mouse tooltip. */}
        <span className="sr-only">{tooltip}</span>
      </div>
    </div>
  );
}
