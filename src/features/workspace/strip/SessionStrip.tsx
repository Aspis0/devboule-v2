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
import { ConfirmDialog } from "../../../components/ConfirmDialog";
import { sessionDelegationTakeBack, sessionOriginBadge } from "../workspaceSessions";
import type { useTabCloseFlow } from "./useTabCloseFlow";
import type { useTabSelection } from "./useTabSelection";
import { useSelectedTabVisible } from "./stripScroll";
import { SessionTabMenu } from "./SessionTabMenu";
import { WorkspaceNewTabMenu } from "./WorkspaceNewTabMenu";
import { chipDisplay } from "./stripDisplay";
import { useStripFade } from "./useStripFade";
import { useStripKeyboard } from "./useStripKeyboard";
import { StripChip, ToolStripChip } from "./StripChip";
import type { StripTab } from "./toolTabs";
import "./strip.css";

/** Chips re-render only when their own props change: every other prop the
 * strip passes is stable across unrelated renders (see the callbacks
 * below), so a composer keystroke never walks the strip. */
const MemoStripChip = memo(StripChip);
const MemoToolChip = memo(ToolStripChip);

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
  /** The composed strip Workspace owns — sessions first, tool tabs appended.
   * The same array the selection and the close flow read: never recomposed. */
  tabs: StripTab[];
  /** The ONE active tab id both readers — chips, keyboard, scroll — use. */
  activeTabId: string | null;
  /** Routes a tab id to its owner: the session controller, or the tool list. */
  selectTab: (id: string) => void;
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
  tabs,
  activeTabId,
  selectTab,
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
  const sessions = useMemo(
    () => tabs.flatMap((tab) => (tab.type === "session" ? [tab.session] : [])),
    [tabs],
  );
  const toolTabs = useMemo(
    () => tabs.flatMap((tab) => (tab.type === "tool" ? [tab.tool] : [])),
    [tabs],
  );
  useSelectedTabVisible(scrollportRef, activeTabId, tabs);
  const fade = useStripFade(scrollportRef, tabs);
  const { selection, handleTabClick } = tabSelection;
  const { menu, openMenu, closeSingle } = tabClose;
  const menuAnchorId = menu?.anchorId;
  const keyboard = useStripKeyboard({
    tabs,
    activeTabId,
    selectTab,
    closeTab: closeSingle,
  });
  const { tabIndexFor, onChipKeyDown: keyboardChipKeyDown } = keyboard;

  const handleChipKeyDown = useCallback(
    (id: string, event: ReactKeyboardEvent<HTMLElement>) => {
      if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
        event.preventDefault();
        openMenu(id);
        return;
      }
      keyboardChipKeyDown(id, event);
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
    () => [
      ...chips.map(({ session, display, tooltip, provenanceLines, takeBack }) => (
        <MemoStripChip
          key={session.id}
          session={session}
          selected={activeTabId === session.id}
          multiselected={selection.has(session.id)}
          tabIndex={tabIndexFor(session.id)}
          display={display}
          tooltip={tooltip}
          provenanceLines={provenanceLines}
          menuOpen={menuAnchorId === session.id}
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
          onChipKeyDown={(event) => handleChipKeyDown(session.id, event)}
          onClose={() => closeSingle(session.id)}
        />
      )),
      ...toolTabs.map((tool) => (
        <MemoToolChip
          key={tool.id}
          tool={tool}
          selected={activeTabId === tool.id}
          multiselected={selection.has(tool.id)}
          tabIndex={tabIndexFor(tool.id)}
          tooltip={tool.path}
          menuOpen={menuAnchorId === tool.id}
          onTabClick={(event) => handleTabClick({ id: tool.id }, event)}
          onTabAuxClick={(event) => {
            if (event.button === 1) {
              event.preventDefault();
              closeSingle(tool.id);
            }
          }}
          onRowContextMenu={(event) => {
            event.preventDefault();
            openMenu(tool.id);
          }}
          onChipKeyDown={(event) => handleChipKeyDown(tool.id, event)}
          onClose={() => closeSingle(tool.id)}
        />
      )),
    ],
    [
      chips,
      toolTabs,
      activeTabId,
      selection,
      tabIndexFor,
      menuAnchorId,
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
        aria-label="Tabs"
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
      <ConfirmDialog
        open={tabClose.confirm !== null}
        title={tabClose.confirm?.title ?? ""}
        message={tabClose.confirm?.message ?? ""}
        confirmLabel={tabClose.confirm?.confirmLabel ?? ""}
        tone={tabClose.confirm?.tone ?? "danger"}
        onConfirm={tabClose.confirmClose}
        onCancel={tabClose.cancelClose}
      />
    </div>
  );
}
