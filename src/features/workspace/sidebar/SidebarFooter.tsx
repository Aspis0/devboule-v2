import type { RefObject } from "react";
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
  onAddProject: () => void;
  addProjectRef: RefObject<HTMLButtonElement | null>;
  onOpenSettings: () => void;
  daemon: DaemonStatus;
  /** The restart-recovery sentence, when a restart attempt failed. */
  note: string | null;
}

/**
 * The sidebar's bottom icon row: add project, settings, and the daemon's
 * status dot. The dot is the only thing drawn at rest; what it stands for
 * rides in the tooltip a pointer reads, in the status text a screen reader
 * reads, and in a tip a keyboard focus reveals — no browser shows `title` on
 * focus, so the tab stop carries its own.
 */
export function SidebarFooter({
  onAddProject,
  addProjectRef,
  onOpenSettings,
  daemon,
  note,
}: SidebarFooterProps) {
  const tooltip = [daemonLabel(daemon), note].filter((part) => part !== null).join(" · ");
  return (
    <div className="workspace-sidebar-footer">
      {/* A group, not a toolbar: the buttons keep their own tab stops, so no
          toolbar keyboard pattern is claimed. */}
      <div className="sidebar-icon-row" role="group" aria-label="Sidebar">
        <button
          type="button"
          className="workspace-icon-button sidebar-icon-button"
          ref={addProjectRef}
          onClick={onAddProject}
          title="New project"
          aria-label="New project"
        >
          <span aria-hidden="true">+</span>
        </button>
        <button
          type="button"
          className="workspace-icon-button sidebar-icon-button"
          onClick={onOpenSettings}
          title="Settings"
          aria-label="Settings"
        >
          <svg
            className="sidebar-icon-glyph"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.75"
            strokeLinecap="round"
            aria-hidden="true"
            focusable="false"
          >
            <path d="M4 7h9M17 7h3M4 17h3M11 17h9" />
            <circle cx="15" cy="7" r="2" />
            <circle cx="9" cy="17" r="2" />
          </svg>
        </button>
        <div
          className="workspace-daemon-status sidebar-foot"
          role="status"
          title={tooltip}
          tabIndex={0}
        >
          <span className={`workspace-status-dot workspace-dot-${daemonDotTone(daemon.state)}`} />
          <span className="sr-only">{tooltip}</span>
          <span className="sidebar-foot-tip" aria-hidden="true">
            {tooltip}
          </span>
        </div>
      </div>
    </div>
  );
}
