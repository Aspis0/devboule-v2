import {
  memo,
  useCallback,
  useMemo,
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
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

/** Chips re-render only when their own props change: every other prop the
 * strip passes is stable across unrelated renders (see the callbacks
 * below), so a composer keystroke never walks the strip. */
const MemoStripChip = memo(StripChip);

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
  const fade = useStripFade(scrollportRef, sessions);
  const { selection, handleTabClick } = tabSelection;
  const { menu, openMenu, closeSingle } = tabClose;
  const menuSessionId = menu?.sessionId;
  const keyboard = useStripKeyboard({
    sessions,
    selectedSessionId,
    selectSession,
    closeTab: closeSingle,
  });
  const { tabIndexFor, onChipKeyDown: keyboardChipKeyDown } = keyboard;

  const handleChipKeyDown = useCallback(
    (session: Session, event: ReactKeyboardEvent<HTMLElement>) => {
      if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
        event.preventDefault();
        openMenu(session.id);
        return;
      }
      keyboardChipKeyDown(session.id, event);
    },
    [openMenu, keyboardChipKeyDown],
  );

  // One derivation per row, recomputed only when the roster, the names, or
  // the take-back switch change — a keystroke or daemon tick that leaves
  // them alone reuses the same dot, words and tooltip objects.
  const chips = useMemo(
    () =>
      sessions.map((session) => {
        const display = chipDisplay(session);
        const origin = sessionOriginBadge(session, peerNames) ?? undefined;
        const creator = resolveCreator(session) ?? undefined;
        const tooltip = [display.tooltip, origin, creator]
          .filter((line) => line !== undefined)
          .join("\n");
        return {
          session,
          display,
          tooltip,
          provenanceLines: [...display.detailLines, origin, creator].filter(
            (line) => line !== undefined,
          ),
          takeBack: takeBackAvailable && sessionDelegationTakeBack(session),
        };
      }),
    [sessions, peerNames, resolveCreator, takeBackAvailable],
  );

  // The rows themselves, memoised on the same stable inputs: an
  // unrelated parent render reuses the very same elements, so the
  // memoised chips below never re-render for it.
  const rows = useMemo(
    () =>
      chips.map(({ session, display, tooltip, provenanceLines, takeBack }) => (
        <MemoStripChip
          key={session.id}
          session={session}
          selected={selectedSessionId === session.id}
          multiselected={selection.has(session.id)}
          tabIndex={tabIndexFor(session.id)}
          display={display}
          tooltip={tooltip}
          provenanceLines={provenanceLines}
          menuOpen={menuSessionId === session.id}
          takeBack={takeBack}
          onTakeBack={onTakeBack}
          onTabClick={(event) => handleTabClick(session, event)}
          onTabAuxClick={(event) => {
            if (event.button === 1) {
              event.preventDefault();
              closeSingle(session.id);
            }
          }}
          onRowContextMenu={(event) => {
            event.preventDefault();
            openMenu(session.id);
          }}
          onChipKeyDown={(event) => handleChipKeyDown(session, event)}
          onClose={() => closeSingle(session.id)}
        />
      )),
    [
      chips,
      selectedSessionId,
      selection,
      tabIndexFor,
      menuSessionId,
      onTakeBack,
      handleTabClick,
      closeSingle,
      openMenu,
      handleChipKeyDown,
    ],
  );

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
        {rows}
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
        <WorkspaceNewTabMenu
          open={newTab.open}
          triggerRef={addButtonRef}
          creating={newTab.creating}
          workspaceSelected={newTab.workspaceSelected}
          onAgent={newTab.onAgent}
          onTerminal={newTab.onTerminal}
          onClose={newTab.onCloseMenu}
        />
        {providerMenu}
      </div>
      <span className="workspace-tabs-spacer" />
      <span className="workspace-rate">{statusText}</span>
      <div className="workspace-sr-only" role="status" aria-live="polite">
        {tabSelection.announcement}
      </div>
      <SessionTabMenu
        open={tabClose.menu !== null}
        anchorRef={tabClose.anchorRef}
        entries={tabClose.menu?.entries ?? []}
        onEntry={tabClose.activateEntry}
        onClose={tabClose.closeMenu}
      />
      <CloseConfirm
        open={tabClose.confirm !== null}
        anchorRef={tabClose.anchorRef}
        title={tabClose.confirm?.title ?? ""}
        message={tabClose.confirm?.message ?? ""}
        confirmLabel={tabClose.confirm?.confirmLabel ?? ""}
        onConfirm={tabClose.confirmClose}
        onCancel={tabClose.cancelClose}
      />
    </div>
  );
}
