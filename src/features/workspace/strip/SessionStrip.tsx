import {
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  type RefObject,
} from "react";
import type { Session } from "../../../types/ipc";
import { sessionDelegationTakeBack, sessionOriginBadge } from "../workspaceSessions";
import type { useTabCloseFlow } from "./useTabCloseFlow";
import type { useTabSelection } from "./useTabSelection";
import { useSelectedTabVisible } from "./stripScroll";
import { SessionTabMenu } from "./SessionTabMenu";
import { WorkspaceNewTabMenu } from "./WorkspaceNewTabMenu";
import { CloseConfirm } from "./CloseConfirm";
import { chipDisplay } from "./stripDisplay";
import { useStripFade } from "./useStripFade";
import { useStripKeyboard } from "./useStripKeyboard";
import { StripChip } from "./StripChip";
import "./strip.css";

export interface StripNewTab {
  open: boolean;
  creating: boolean;
  workspaceSelected: boolean;
  onToggle: () => void;
  onAgent: () => void;
  onTerminal: () => void;
  onCloseMenu: () => void;
}

export interface SessionStripProps {
  sessions: Session[];
  selectedSessionId: string | null;
  selectSession: (id: string | null) => void;
  tabSelection: ReturnType<typeof useTabSelection>;
  tabClose: ReturnType<typeof useTabCloseFlow>;
  addButtonRef: RefObject<HTMLButtonElement | null>;
  newTab: StripNewTab;
  /** The provider choice anchored at the strip, or null when it lives elsewhere. */
  providerMenu: ReactNode;
  peerNames: ReadonlyMap<string, string>;
  /** The tooltip's creator line for one session; Workspace binds the roster. */
  resolveCreator: (session: Session) => string | null;
  takeBackAvailable: boolean;
  onTakeBack: () => void;
  statusText: string;
}

/** The tab strip region: the scrolling tablist, the fade on the sides that
 * still hide chips, the "+" pinned after the scrollport, and the count. */
export function SessionStrip({
  sessions,
  selectedSessionId,
  selectSession,
  tabSelection,
  tabClose,
  addButtonRef,
  newTab,
  providerMenu,
  peerNames,
  resolveCreator,
  takeBackAvailable,
  onTakeBack,
  statusText,
}: SessionStripProps) {
  const scrollportRef = useRef<HTMLDivElement>(null);
  useSelectedTabVisible(scrollportRef, selectedSessionId, sessions);
  const fade = useStripFade(scrollportRef);
  const keyboard = useStripKeyboard({
    sessions,
    selectedSessionId,
    selectSession,
    closeTab: tabClose.closeSingle,
  });

  const onChipKeyDown = (session: Session, event: ReactKeyboardEvent<HTMLElement>) => {
    if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      event.preventDefault();
      tabClose.openMenu(session.id);
      return;
    }
    keyboard.onChipKeyDown(session.id, event);
  };

  return (
    <div className="workspace-session-tabs">
      {/* The row of tabs scrolls; the add button below it stays outside
          the scrollport, so a full strip cannot carry it off screen. */}
      <div
        className="workspace-session-tabs-scroll"
        role="tablist"
        aria-label="Sessions"
        ref={scrollportRef}
        data-fade-left={fade.left ? "true" : "false"}
        data-fade-right={fade.right ? "true" : "false"}
      >
        {sessions.map((session) => {
          const display = chipDisplay(session);
          const tooltip = [
            display.tooltip,
            sessionOriginBadge(session, peerNames) ?? undefined,
            resolveCreator(session) ?? undefined,
          ]
            .filter((line) => line !== undefined)
            .join("\n");
          const onRowContextMenu = (event: ReactMouseEvent<HTMLDivElement>) => {
            event.preventDefault();
            tabClose.openMenu(session.id);
          };
          return (
            <StripChip
              key={session.id}
              session={session}
              selected={selectedSessionId === session.id}
              multiselected={tabSelection.selection.has(session.id)}
              tabIndex={keyboard.tabIndexFor(session.id)}
              display={display}
              tooltip={tooltip}
              menuOpen={tabClose.menu?.sessionId === session.id}
              takeBack={takeBackAvailable && sessionDelegationTakeBack(session)}
              onTakeBack={onTakeBack}
              onTabClick={(event) => tabSelection.handleTabClick(session, event)}
              onTabAuxClick={(event) => {
                if (event.button === 1) {
                  event.preventDefault();
                  tabClose.closeSingle(session.id);
                }
              }}
              onRowContextMenu={onRowContextMenu}
              onChipKeyDown={(event) => onChipKeyDown(session, event)}
              onClose={() => tabClose.closeSingle(session.id)}
            />
          );
        })}
      </div>
      <div className="workspace-session-add-wrap">
        <button
          ref={addButtonRef}
          type="button"
          className="workspace-session-add"
          onClick={newTab.onToggle}
          title="New tab"
          aria-label="New tab"
          aria-haspopup="menu"
          aria-expanded={newTab.open}
          disabled={newTab.creating}
        >
          +
        </button>
        {newTab.open ? (
          <WorkspaceNewTabMenu
            triggerRef={addButtonRef}
            creating={newTab.creating}
            workspaceSelected={newTab.workspaceSelected}
            onAgent={newTab.onAgent}
            onTerminal={newTab.onTerminal}
            onClose={newTab.onCloseMenu}
          />
        ) : null}
        {providerMenu}
      </div>
      <span className="workspace-tabs-spacer" />
      <span className="workspace-rate">{statusText}</span>
      <div className="workspace-sr-only" role="status" aria-live="polite">
        {tabSelection.announcement}
      </div>
      {tabClose.menu !== null ? (
        <SessionTabMenu
          anchorRef={tabClose.anchorRef}
          entries={tabClose.menu.entries}
          onEntry={tabClose.activateEntry}
          onClose={tabClose.closeMenu}
        />
      ) : null}
      {tabClose.confirm !== null ? (
        <CloseConfirm
          anchorRef={tabClose.anchorRef}
          title={tabClose.confirm.title}
          message={tabClose.confirm.message}
          confirmLabel={tabClose.confirm.confirmLabel}
          onConfirm={tabClose.confirmClose}
          onCancel={tabClose.cancelClose}
        />
      ) : null}
    </div>
  );
}
