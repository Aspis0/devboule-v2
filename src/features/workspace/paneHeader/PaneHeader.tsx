import type { ReactNode } from "react";
import "./paneHeader.css";
import type { PaneHeaderMenu as PaneHeaderMenuConfig } from "./paneHeaderMenu";
import { PaneHeaderKebab } from "./PaneHeaderKebab";

export interface PaneHeaderProps {
  kind: "agent" | "terminal";
  title: string;
  statusWord: string;
  dotTone: "green" | "terracotta" | "border";
  pulsing?: boolean;
  /** Null renders no kebab: a menu with nothing actionable is a dead control. */
  menu: PaneHeaderMenuConfig | null;
  subagentSlot?: ReactNode;
  trailingSlot?: ReactNode;
}

export function PaneHeader({
  kind,
  title,
  statusWord,
  dotTone,
  pulsing,
  menu,
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
      {agent ? null : trailingSlot}
      {menu === null ? null : <PaneHeaderKebab menu={menu} />}
    </div>
  );
}
