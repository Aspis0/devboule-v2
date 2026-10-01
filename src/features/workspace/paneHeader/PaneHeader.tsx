import type { ReactNode } from "react";
import "./paneHeader.css";
import type { HeaderDisplay } from "./paneHeaderStatus";
import type { PaneHeaderMenu as PaneHeaderMenuConfig } from "./paneHeaderMenu";
import { PaneHeaderKebab } from "./PaneHeaderKebab";

export interface PaneHeaderProps {
  kind: "agent" | "terminal";
  title: string;
  display: HeaderDisplay;
  /** Null renders no kebab: a menu with nothing actionable is a dead control. */
  menu: PaneHeaderMenuConfig | null;
  subagentSlot?: ReactNode;
  trailingSlot?: ReactNode;
}

export function PaneHeader({
  kind,
  title,
  display,
  menu,
  subagentSlot,
  trailingSlot,
}: PaneHeaderProps) {
  const agent = kind === "agent";
  const text = display.detail === null ? display.word : `${display.word} · ${display.detail}`;
  return (
    <div className={agent ? "workspace-agent-toolbar" : "workspace-terminal-toolbar"}>
      <span
        className={`workspace-status-dot workspace-dot-${display.tone}${display.pulse ? " dot-pulse" : ""}`}
      />
      <span className={agent ? "workspace-agent-title" : "workspace-terminal-title"}>{title}</span>
      {agent ? subagentSlot : null}
      <span
        className={agent ? "workspace-agent-status" : "workspace-terminal-status"}
        role="status"
        title={display.tooltip}
      >
        {text}
        {display.srDetail === null ? null : <span className="sr-only"> — {display.srDetail}</span>}
      </span>
      {agent ? null : trailingSlot}
      {menu === null ? null : <PaneHeaderKebab menu={menu} />}
    </div>
  );
}
