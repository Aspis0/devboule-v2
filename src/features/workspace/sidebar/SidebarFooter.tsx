import type { DaemonStatus } from "../../../types/ipc";
import { errorSentence } from "../../../lib/errorSentence";

/** Green while the daemon answers, hollow while connecting, red otherwise. */
export function daemonDotTone(state: DaemonStatus["state"]): string {
  if (state === "connected") return "green";
  if (state === "connecting") return "border";
  return "terracotta";
}

/** The foot's tooltip: the whole daemon sentence, not the quiet label.
 * The reported message goes through the mapper, so a known daemon failure
 * reads as advice instead of the supervisor's own diagnosis. */
export function daemonLabel(status: DaemonStatus): string {
  const reason = status.message === null ? null : errorSentence(status.message).sentence;
  if (status.state === "connected") {
    const pid = status.pid !== null ? `pid ${status.pid}` : "connected";
    return reason === null ? `daemon · ${pid}` : `daemon · ${pid} · ${reason}`;
  }
  if (status.state === "connecting") return "daemon · connecting";
  if (status.state === "unresponsive") {
    return reason === null ? "daemon · not answering" : `daemon · ${reason}`;
  }
  return reason === null ? "daemon · disconnected" : `daemon · ${reason}`;
}

export interface SidebarFooterProps {
  historyOpen: boolean;
  onToggleHistory: () => void;
  daemon: DaemonStatus;
  /** The restart-recovery sentence, when a restart attempt failed. */
  note: string | null;
}

/**
 * The sidebar's foot: the quiet History row above the dot. The dot is the only
 * thing drawn; what it stands for rides in the tooltip a pointer reads and in
 * the status text a screen reader reads. Nothing here is focusable: the dot is
 * a report, not an action, and no browser shows a `title` on keyboard focus,
 * so a tab stop here would stop the keyboard on a bare dot.
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
      <div className="workspace-daemon-status sidebar-foot" role="status" title={tooltip}>
        <span className={`workspace-status-dot workspace-dot-${daemonDotTone(daemon.state)}`} />
        <span className="sr-only">{tooltip}</span>
      </div>
    </div>
  );
}
