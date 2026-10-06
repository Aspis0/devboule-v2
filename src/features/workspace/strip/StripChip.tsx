import type { KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent } from "react";
import { isAgentKind, type Session } from "../../../types/ipc";
import { sessionTabElementId } from "./useTabCloseFlow";
import { sessionKindLabel, sessionTitle } from "../workspaceSessions";
import type { ChipDisplay } from "./stripDisplay";
import { StripKindMark } from "./StripKindMark";
import { toolTabKindLabel, toolTabLabel, toolTabSubject, type ToolTab } from "./toolTabs";

/** How a browser tab names itself in the strip: the page's own favicon once
 * it has one, and the same globe the rest of the app draws for a page until
 * then. The chip subscribes to the browser tab model itself — the favicon
 * arrives after the tab does, and threading it through the strip would
 * remount every chip for one image. */
function BrowserFavicon({ favicon }: { favicon: string | null }) {
  if (favicon === null) return <StripKindMark kind="browser" />;
  return (
    <span className="strip-kind" aria-hidden="true">
      <img className="strip-browser-favicon" src={favicon} alt="" width={14} height={14} />
    </span>
  );
}

/** The spec's dot tones, one class per state in the strip's vocabulary —
 * shared with the end-of-strip overview, which paints the same dots. */
export const DOT_CLASS: Record<ChipDisplay["dot"], string> = {
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
  const provenanceId = `${sessionTabElementId(session.id)}-provenance`;
  const described = provenanceLines.length > 0 ? provenanceId : undefined;
  const tone = isAgentKind(session.kind) ? "agent" : "quiet";
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
        className={`workspace-session-tab workspace-session-tab-${tone}${selected ? " workspace-session-tab-selected" : ""}${multiselected ? " workspace-session-tab-multiselected" : ""}`}
        onClick={onTabClick}
        onAuxClick={onTabAuxClick}
        onKeyDown={onChipKeyDown}
      >
        <span
          className={`workspace-status-dot ${DOT_CLASS[display.dot]}${display.pulse ? " dot-pulse" : ""}`}
        />
        <StripKindMark kind={session.kind} />
        <span className="workspace-tab-label">{title}</span>
        {/* Heard, never seen: a chip paints a dot tone, never a sentence. */}
        <span className="workspace-sr-only">{`${sessionKindLabel(session.kind)}, ${display.stateLine}`}</span>
      </button>
      {/* The description lives beside the button, never inside it: a
          described-by span inside the button would join the accessible
          name and be announced twice. */}
      {described !== undefined ? (
        <span id={provenanceId} className="workspace-sr-only">
          {provenanceLines.join(" ")}
        </span>
      ) : null}
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
              strokeWidth="2"
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

export interface ToolStripChipProps {
  tool: ToolTab;
  selected: boolean;
  multiselected: boolean;
  tabIndex: 0 | -1;
  /** The browser tab's page as the tab model holds it: absent for a Diff or
   * File tab, whose chip names its path instead. */
  browser?: BrowserTabPage;
  /** The workspace-relative path — the label keeps the basename only. */
  tooltip: string;
  menuOpen: boolean;
  onTabClick: (event: ReactMouseEvent<HTMLButtonElement>) => void;
  onTabAuxClick: (event: ReactMouseEvent<HTMLButtonElement>) => void;
  onRowContextMenu: (event: ReactMouseEvent<HTMLDivElement>) => void;
  onChipKeyDown: (event: ReactKeyboardEvent<HTMLElement>) => void;
  onClose: () => void;
}

/** What a browser tab's chip reads: the page's title or its hostname, and
 * the address the tooltip carries in full. */
export interface BrowserTabPage {
  label: string;
  favicon: string | null;
  url: string;
  /** Whether the page is loading now. False for a tab restored after a
   * restart: it has a record and no page, and nothing is loading. */
  loading: boolean;
}

/** The page is loading: a ring in the favicon's place, so the chip keeps the
 * width it had while the page arrives. */
function BrowserLoading() {
  return (
    <span className="strip-kind" aria-hidden="true">
      <span className="strip-browser-loading" />
    </span>
  );
}

/** No take-back, no rename — a tool tab has no session behind it to act on. */
export function ToolStripChip({
  tool,
  selected,
  multiselected,
  tabIndex,
  browser,
  tooltip,
  menuOpen,
  onTabClick,
  onTabAuxClick,
  onRowContextMenu,
  onChipKeyDown,
  onClose,
}: ToolStripChipProps) {
  const isBrowser = tool.kind === "browser";
  const label = isBrowser ? (browser?.label ?? toolTabSubject(tool)) : toolTabLabel(tool);
  const stateLine = toolTabKindLabel(tool);
  const loading = isBrowser && browser?.loading === true;
  const subject = isBrowser ? (browser?.url ?? toolTabSubject(tool)) : toolTabSubject(tool);
  return (
    <div className="workspace-session-row" onContextMenu={onRowContextMenu}>
      <button
        type="button"
        role="tab"
        id={sessionTabElementId(tool.id)}
        aria-selected={selected}
        aria-controls="workspace-panel-terminal"
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        tabIndex={tabIndex}
        title={tooltip}
        aria-keyshortcuts="Delete"
        className={`workspace-session-tab workspace-session-tab-quiet${selected ? " workspace-session-tab-selected" : ""}${multiselected ? " workspace-session-tab-multiselected" : ""}`}
        onClick={onTabClick}
        onAuxClick={onTabAuxClick}
        onKeyDown={onChipKeyDown}
      >
        {isBrowser ? (
          loading ? (
            <BrowserLoading />
          ) : (
            <BrowserFavicon favicon={browser?.favicon ?? null} />
          )
        ) : (
          <StripKindMark kind={tool.kind} />
        )}
        <span className="workspace-tab-label">{label}</span>
        {/* Heard, never seen: the kind, whether the page is loading, and the
            path — never a session state. */}
        <span className="workspace-sr-only">{`${stateLine}${loading ? " loading" : ""} ${subject}`}</span>
      </button>
      <span className="workspace-session-chip">
        <button
          type="button"
          tabIndex={-1}
          className="workspace-session-chip-close"
          aria-label={`Close ${label}`}
          title={`Close ${label}`}
          onClick={onClose}
        >
          <svg width={12} height={12} viewBox="0 0 12 12" aria-hidden="true" focusable="false">
            <path
              d="M2 2l8 8M10 2l-8 8"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              fill="none"
            />
          </svg>
        </button>
      </span>
    </div>
  );
}
