import type { KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent } from "react";
import type { Session } from "../../../types/ipc";
import { sessionTabElementId } from "./useTabCloseFlow";
import { sessionTitle } from "../workspaceSessions";
import type { ChipDisplay } from "./stripDisplay";
import { StripKindMark } from "./StripKindMark";

const DOT_CLASS: Record<ChipDisplay["dot"], string> = {
  live: "strip-dot-live",
  attention: "strip-dot-attention",
  unattended: "strip-dot-unattended",
  recovered: "strip-dot-recovered",
  idle: "strip-dot-idle",
  ended: "strip-dot-ended",
  unknown: "strip-dot-unknown",
};

export interface StripChipProps {
  session: Session;
  selected: boolean;
  multiselected: boolean;
  tabIndex: 0 | -1;
  display: ChipDisplay;
  tooltip: string;
  /** The tooltip's lines after the state: origin, creator, details. Heard
   * through aria-describedby, never painted; empty means undescribed. */
  provenanceLines: string[];
  menuOpen: boolean;
  takeBack: boolean;
  onTakeBack: () => void;
  onTabClick: (event: ReactMouseEvent<HTMLButtonElement>) => void;
  onTabAuxClick: (event: ReactMouseEvent<HTMLButtonElement>) => void;
  onRowContextMenu: (event: ReactMouseEvent<HTMLDivElement>) => void;
  onChipKeyDown: (event: ReactKeyboardEvent<HTMLElement>) => void;
  onClose: () => void;
}

/** One session's row in the strip: the tab button (dot, kind mark, clipped
 * label), the trailing close overlay, and the take-back where it can act. */
export function StripChip({
  session,
  selected,
  multiselected,
  tabIndex,
  display,
  tooltip,
  provenanceLines,
  menuOpen,
  takeBack,
  onTakeBack,
  onTabClick,
  onTabAuxClick,
  onRowContextMenu,
  onChipKeyDown,
  onClose,
}: StripChipProps) {
  const title = sessionTitle(session);
  const provenanceId = `strip-chip-${session.id}-provenance`;
  const described = provenanceLines.length > 0 ? provenanceId : undefined;
  return (
    <div className="workspace-session-row" onContextMenu={onRowContextMenu}>
      <button
        type="button"
        role="tab"
        id={sessionTabElementId(session.id)}
        aria-selected={selected}
        aria-controls="workspace-panel-terminal"
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        tabIndex={tabIndex}
        title={tooltip}
        aria-keyshortcuts="Delete"
        aria-describedby={described}
        className={`workspace-session-tab${selected ? " workspace-session-tab-selected" : ""}${multiselected ? " workspace-session-tab-multiselected" : ""}`}
        onClick={onTabClick}
        onAuxClick={onTabAuxClick}
        onKeyDown={onChipKeyDown}
      >
        <span
          className={`workspace-status-dot ${DOT_CLASS[display.dot]}${display.pulse ? " dot-pulse" : ""}`}
        />
        <StripKindMark kind={session.kind} />
        <span className="workspace-tab-label">{title}</span>
        {display.words !== null ? (
          <span className="workspace-tab-attention">{display.words}</span>
        ) : (
          // The states that lost their painted words keep them here: heard,
          // never seen, so the accessible name still carries the state.
          // The title keeps the same line for the mouse; aria-describedby
          // below keeps it out of the accessible description, so no
          // reader hears it twice.
          <span className="workspace-sr-only">{display.stateLine}</span>
        )}
        {described !== undefined ? (
          <span id={provenanceId} className="workspace-sr-only">
            {provenanceLines.join(" ")}
          </span>
        ) : null}
      </button>
      <span className="workspace-session-chip">
        <button
          type="button"
          tabIndex={-1}
          className="workspace-session-chip-close"
          aria-label={`Close ${title}`}
          title={`Close ${title}`}
          onClick={onClose}
        >
          <svg width={12} height={12} viewBox="0 0 12 12" aria-hidden="true" focusable="false">
            <path
              d="M2 2l8 8M10 2l-8 8"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              fill="none"
            />
          </svg>
        </button>
      </span>
      {takeBack ? (
        <button
          type="button"
          className="workspace-tab-takeback"
          aria-label="Take back — stops every agent from answering for its children"
          title="Take back — stops every agent from answering for its children"
          onClick={onTakeBack}
        >
          Take back
        </button>
      ) : null}
    </div>
  );
}
