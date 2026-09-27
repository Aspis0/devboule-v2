import type { ReactNode } from "react";
import "./paneHeader.css";

export interface PaneHeaderProps {
  kind: "agent" | "terminal";
  title: string;
  statusWord: string;
  dotTone: "green" | "terracotta" | "border";
  pulsing?: boolean;
  cwd?: string;
  subagentSlot?: ReactNode;
  trailingSlot?: ReactNode;
}

export function PaneHeader({
  kind,
  title,
  statusWord,
  dotTone,
  pulsing,
  cwd,
  subagentSlot,
  trailingSlot,
}: PaneHeaderProps) {
  const agent = kind === "agent";
  return (
    <div className={agent ? "workspace-agent-toolbar" : "workspace-terminal-toolbar"}>
      <span
        className={`workspace-status-dot workspace-dot-${dotTone}${pulsing === true ? " dot-pulse" : ""}`}
      />
      <span className={agent ? "workspace-agent-title" : "workspace-terminal-title"}>{title}</span>
      {agent ? subagentSlot : null}
      {agent ? (
        <span className="workspace-agent-status" role="status">
          {statusWord}
        </span>
      ) : (
        <span className="workspace-terminal-status">{statusWord}</span>
      )}
      {cwd ? <span className="workspace-session-cwd">{cwd}</span> : null}
      {agent ? null : trailingSlot}
      <button type="button" className="pane-header-kebab" aria-label="Session actions">
        ⋮
      </button>
    </div>
  );
}
