import type { ReactNode } from "react";
import "./paneHeader.css";
import type { HeaderDisplay } from "./paneHeaderStatus";
import type { PaneHeaderMenu as PaneHeaderMenuConfig } from "./paneHeaderMenu";
import { PaneHeaderKebab } from "./PaneHeaderKebab";

export interface PaneHeaderProps {
  title: string;
  display: HeaderDisplay;
  /** Null renders no kebab: a menu with nothing actionable is a dead control. */
  menu: PaneHeaderMenuConfig | null;
  /** The terminal's interrupt and close: they share the header's row, not a second one. */
  trailingSlot?: ReactNode;
}

export function PaneHeader({ title, display, menu, trailingSlot }: PaneHeaderProps) {
  const text = display.detail === null ? display.word : `${display.word} · ${display.detail}`;
  return (
    <div className="workspace-terminal-toolbar">
      <span
        className={`workspace-status-dot workspace-dot-${display.tone}${display.pulse ? " dot-pulse" : ""}`}
      />
      <span className="workspace-terminal-title">{title}</span>
      <span className="workspace-terminal-status" role="status" title={display.tooltip}>
        {text}
        {display.srDetail === null ? null : <span className="sr-only"> — {display.srDetail}</span>}
      </span>
      {trailingSlot}
      {menu === null ? null : <PaneHeaderKebab menu={menu} />}
    </div>
  );
}
